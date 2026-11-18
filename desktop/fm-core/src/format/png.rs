//! PNG: a chunk list, where everything revealing is optional and ancillary.
//!
//! Cleaning is a filter over the chunk list. The critical chunks and the colour
//! chunks are copied untouched, so the decoded image is bit-identical.

use super::exif::{self, Exif};
use super::{xmp, Change, Handler, Outcome};
use crate::error::{Error, Result};
use crate::model::{Action, Class, Finding, FormatId, Impact, Inspection};
use crate::profile::{self, Profile, TimeOutcome};
use crate::timeutil::Civil;

pub struct Png;

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

struct Chunk {
    kind: [u8; 4],
    data: Vec<u8>,
}

impl Chunk {
    fn name(&self) -> String {
        String::from_utf8_lossy(&self.kind).to_string()
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    // PNG's CRC-32, computed directly rather than pulled in as a dependency.
    let mut crc = 0xFFFF_FFFFu32;
    for b in bytes {
        crc ^= *b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn split(data: &[u8]) -> Result<Vec<Chunk>> {
    let bad = |d: &str| Error::malformed(FormatId::Png, d.to_string());
    if !data.starts_with(SIGNATURE) {
        return Err(bad("no PNG signature"));
    }
    let mut out = Vec::new();
    let mut at = SIGNATURE.len();
    while at + 12 <= data.len() {
        let len = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        let body = data.get(at + 8..at + 8 + len).ok_or_else(|| bad("chunk runs past end of file"))?;
        out.push(Chunk { kind, data: body.to_vec() });
        at += 12 + len;
        if &kind == b"IEND" {
            break;
        }
    }
    if out.is_empty() {
        return Err(bad("no chunks"));
    }
    Ok(out)
}

fn assemble(chunks: &[Chunk]) -> Vec<u8> {
    let mut out = Vec::from(SIGNATURE);
    for c in chunks {
        out.extend_from_slice(&(c.data.len() as u32).to_be_bytes());
        let mut crc_input = Vec::with_capacity(4 + c.data.len());
        crc_input.extend_from_slice(&c.kind);
        crc_input.extend_from_slice(&c.data);
        out.extend_from_slice(&c.kind);
        out.extend_from_slice(&c.data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    out
}

/// A text chunk's keyword and its text, whatever the three spellings.
fn text_pair(c: &Chunk) -> Option<(String, String)> {
    let split_at = c.data.iter().position(|b| *b == 0)?;
    let keyword = crate::text::text(&c.data[..split_at]);
    let rest = &c.data[split_at + 1..];
    let text = match &c.kind {
        // tEXt and zTXt are Latin-1 by specification; iTXt is UTF-8.
        b"tEXt" => crate::text::text(rest),
        b"zTXt" => {
            // One compression-method byte, then a zlib stream.
            let body = rest.get(1..).unwrap_or_default();
            inflate(body).map(|v| crate::text::text(&v)).unwrap_or_default()
        }
        b"iTXt" => {
            let compressed = rest.first().copied().unwrap_or(0) == 1;
            let body = rest.get(2..)?;
            // language tag, then translated keyword, both NUL-terminated.
            let mut skipped = body;
            for _ in 0..2 {
                let at = skipped.iter().position(|b| *b == 0)?;
                skipped = &skipped[at + 1..];
            }
            if compressed {
                inflate(skipped).map(|v| crate::text::text(&v)).unwrap_or_default()
            } else {
                crate::text::text(skipped)
            }
        }
        _ => return None,
    };
    Some((keyword, text))
}

fn inflate(body: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(body).read_to_end(&mut out).ok()?;
    Some(out)
}

/// The registered and de-facto keywords, mapped onto the classes.
fn keyword_info(keyword: &str) -> (Class, &'static str) {
    match keyword {
        "Author" => (Class::Identity, "author"),
        "Copyright" => (Class::Identity, "copyright notice"),
        "Title" => (Class::Identity, "title"),
        "Description" => (Class::Identity, "description"),
        "Comment" => (Class::Identity, "comment, which often names the machine"),
        "Disclaimer" | "Warning" => (Class::Identity, "disclaimer"),
        "Software" => (Class::Device, "software that wrote the file"),
        "Source" => (Class::Device, "device that produced the image"),
        "Creation Time" => (Class::Time, "creation time"),
        k if k.starts_with("XML:com.adobe.xmp") => (Class::Correlation, "embedded XMP packet"),
        _ => (Class::Identity, "text annotation"),
    }
}

/// A field removed not because the profile asked but because keeping it would
/// have left the file in a crowd of almost nobody.
///
/// The diff has to say which it was and why, so the class is the field's own
/// and the reason is the one that actually applies: a lone date chunk is one
/// case, a whole EXIF block inside a PNG is another.
fn blended_away(
    class: Class,
    locator: impl Into<String>,
    label: impl Into<String>,
    before: Option<String>,
    reason: &str,
) -> Change {
    let mut c = Change::removed(class, locator, label, before);
    c.label = format!("{} (removed to match common tools: {reason})", c.label);
    c
}

const WHY_DATE: &str = "a date in a PNG is unusual";
const WHY_EXIF: &str = "an EXIF block in a PNG is unusual whatever is in it";

fn parse_time(data: &[u8]) -> Option<Civil> {
    if data.len() < 7 {
        return None;
    }
    Some(Civil {
        year: u16::from_be_bytes([data[0], data[1]]) as i32,
        month: data[2],
        day: data[3],
        hour: data[4],
        minute: data[5],
        second: data[6],
    })
}

fn write_time(c: Civil) -> Vec<u8> {
    let mut v = Vec::with_capacity(7);
    v.extend_from_slice(&(c.year as u16).to_be_bytes());
    v.extend_from_slice(&[c.month, c.day, c.hour, c.minute, c.second]);
    v
}

impl Handler for Png {
    fn id(&self) -> FormatId {
        FormatId::Png
    }

    fn sniff(&self, head: &[u8], _ext: Option<&str>) -> bool {
        head.starts_with(SIGNATURE)
    }

    fn inspect(&self, data: &[u8]) -> Result<Inspection> {
        let chunks = split(data)?;
        let mut ins = Inspection::new(FormatId::Png, data.len() as u64);
        for c in &chunks {
            match &c.kind {
                b"tEXt" | b"zTXt" | b"iTXt" => {
                    ins.mark("png:text");
                    let Some((keyword, text)) = text_pair(c) else { continue };
                    if keyword.starts_with("XML:com.adobe.xmp") {
                        ins.mark("png:xmp");
                        xmp::inspect(text.as_bytes(), "png:itxt:xmp", &mut ins);
                        continue;
                    }
                    let (class, label) = keyword_info(&keyword);
                    ins.findings.push(
                        Finding::new(class, format!("png:{}:{keyword}", c.name()), label)
                            .value(text.trim().to_string()),
                    );
                }
                b"tIME" => {
                    ins.mark("png:time");
                    let shown = parse_time(&c.data).map(|t| t.to_iso()).unwrap_or_default();
                    ins.findings.push(
                        Finding::new(Class::Time, "png:tIME", "last modification time").value(shown),
                    );
                }
                b"eXIf" => {
                    ins.mark("png:exif");
                    match Exif::parse(&c.data) {
                        Ok(exif) => exif::inspect_entries(&exif, &mut ins),
                        Err(e) => ins.findings.push(
                            Finding::new(Class::Device, "png:eXIf", "EXIF block that could not be read")
                                .value(e.to_string()),
                        ),
                    }
                }
                b"iCCP" => {
                    ins.mark("png:iccp");
                    ins.findings.push(
                        Finding::new(Class::Device, "png:iCCP", "ICC colour profile")
                            .value(format!("{} bytes", c.data.len()))
                            .protected()
                            .note("kept on purpose: without it the colours shift"),
                    );
                }
                b"dSIG" => ins.findings.push(
                    Finding::new(
                        Class::StructuralFingerprint,
                        "png:dSIG",
                        "embedded digital signature",
                    )
                    .impact(Impact::Structural),
                ),
                _ => {}
            }
        }
        Ok(ins)
    }

    fn clean(&self, data: &[u8], profile: &Profile) -> Result<Outcome> {
        let chunks = split(data)?;
        let mut changes = Vec::new();
        let mut out: Vec<Chunk> = Vec::new();

        for c in chunks {
            match &c.kind {
                b"tEXt" | b"zTXt" | b"iTXt" => {
                    let Some((keyword, text)) = text_pair(&c) else {
                        out.push(c);
                        continue;
                    };
                    let locator = format!("png:{}:{keyword}", c.name());
                    if keyword.starts_with("XML:com.adobe.xmp") {
                        if xmp::should_drop(text.as_bytes(), profile, "png:itxt:xmp", &mut changes) {
                            continue;
                        }
                        out.push(c);
                        continue;
                    }
                    let (class, label) = keyword_info(&keyword);
                    if class == Class::Time && profile.action(Class::Time) != Action::Keep {
                        match profile::retime(profile, &text).iso(&text) {
                            Some(v) => {
                                let mut body = keyword.clone().into_bytes();
                                body.push(0);
                                body.extend_from_slice(v.as_bytes());
                                changes.push(Change::rewritten(
                                    class,
                                    locator,
                                    label,
                                    profile.action(Class::Time),
                                    Some(text),
                                    Some(v),
                                ));
                                out.push(Chunk { kind: *b"tEXt", data: body });
                            }
                            None => changes.push(Change::removed(class, locator, label, Some(text))),
                        }
                        continue;
                    }
                    if profile.alters(class) {
                        changes.push(Change::removed(class, locator, label, Some(text.trim().to_string())));
                    } else {
                        out.push(c);
                    }
                }
                b"tIME" => {
                    let original = parse_time(&c.data);
                    let shown = original.map(|t| t.to_iso()).unwrap_or_default();
                    if profile.may_blend_away(FormatId::Png) {
                        changes.push(blended_away(
                            Class::Time,
                            "png:tIME",
                            "last modification time",
                            Some(shown),
                            WHY_DATE,
                        ));
                        continue;
                    }
                    match profile.apply_time(original) {
                        TimeOutcome::Unchanged => out.push(c),
                        TimeOutcome::Drop => changes.push(Change::removed(
                            Class::Time,
                            "png:tIME",
                            "last modification time",
                            Some(shown),
                        )),
                        TimeOutcome::Set(t) => {
                            changes.push(Change::rewritten(
                                Class::Time,
                                "png:tIME",
                                "last modification time",
                                profile.action(Class::Time),
                                Some(shown),
                                Some(t.to_iso()),
                            ));
                            out.push(Chunk { kind: *b"tIME", data: write_time(t) });
                        }
                    }
                }
                b"eXIf" if profile.may_blend_away(FormatId::Png) => {
                    // An EXIF block in a PNG is rare enough on its own to be a
                    // fingerprint, so there is nothing in it worth keeping.
                    let mut inner = Inspection::new(FormatId::Png, 0);
                    if let Ok(exif) = Exif::parse(&c.data) {
                        exif::inspect_entries(&exif, &mut inner);
                    }
                    for f in inner.findings.into_iter().filter(|f| !f.protected) {
                        changes.push(blended_away(f.class, f.locator, f.label, f.value, WHY_EXIF));
                    }
                }
                b"eXIf" => match Exif::parse(&c.data) {
                    Ok(exif) => {
                        if let Some(kept) = exif::clean_entries(&exif, profile, &mut changes) {
                            out.push(Chunk { kind: *b"eXIf", data: kept.to_tiff() });
                        }
                    }
                    Err(e) => changes.push(Change::removed(
                        Class::Device,
                        "png:eXIf",
                        "unreadable EXIF block",
                        Some(e.to_string()),
                    )),
                },
                // iCCP and the colour chunks are never touched; the critical
                // chunks obviously stay.
                _ => out.push(c),
            }
        }

        Ok(Outcome { data: assemble(&out), changes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_with(extra: Vec<Chunk>) -> Vec<u8> {
        let mut chunks = vec![Chunk {
            kind: *b"IHDR",
            data: vec![0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0],
        }];
        chunks.extend(extra);
        chunks.push(Chunk { kind: *b"IEND", data: Vec::new() });
        assemble(&chunks)
    }

    fn text_chunk(keyword: &str, text: &str) -> Chunk {
        let mut data = keyword.as_bytes().to_vec();
        data.push(0);
        data.extend_from_slice(text.as_bytes());
        Chunk { kind: *b"tEXt", data }
    }

    #[test]
    fn crc_matches_the_known_iend_value() {
        // IEND's CRC is a fixed, well-known constant.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn round_trips_a_file_it_wrote() {
        let data = png_with(vec![text_chunk("Author", "Marie Dvorakova")]);
        assert_eq!(assemble(&split(&data).unwrap()), data);
    }

    #[test]
    fn standard_removes_the_author_and_keeps_the_date() {
        let data = png_with(vec![
            text_chunk("Author", "Marie Dvorakova"),
            Chunk { kind: *b"tIME", data: write_time(Civil { year: 2024, month: 6, day: 14, hour: 17, minute: 42, second: 7 }) },
        ]);
        let mut p = Profile::standard();
        p.blending = crate::profile::Blending::Minimal;
        let out = Png.clean(&data, &p).unwrap();
        let ins = Png.inspect(&out.data).unwrap();
        assert!(ins.findings.iter().all(|f| f.class != Class::Identity));
        assert_eq!(ins.in_class(Class::Time).count(), 1);
        assert!(out.changes.iter().any(|c| c.before.as_deref() == Some("Marie Dvorakova")));
    }

    #[test]
    fn uniform_normalises_the_modification_time_when_told_not_to_blend() {
        let mut p = Profile::uniform();
        p.blending = crate::profile::Blending::Minimal;
        let data = png_with(vec![Chunk {
            kind: *b"tIME",
            data: write_time(Civil { year: 2024, month: 6, day: 14, hour: 17, minute: 42, second: 7 }),
        }]);
        let out = Png.clean(&data, &p).unwrap();
        let ins = Png.inspect(&out.data).unwrap();
        let t = ins.in_class(Class::Time).next().unwrap();
        assert_eq!(t.value.as_deref(), Some("1980-01-01T00:00:00Z"));
    }

    /// The whole EXIF block goes when a PNG carries one, and each field keeps
    /// its own class in the diff rather than all of them being filed as dates.
    #[test]
    fn a_blended_away_exif_block_reports_each_field_under_its_own_class() {
        let mut exif = crate::format::exif::Exif { le: true, ..Default::default() };
        exif.ifd0.push(crate::format::exif::Entry::ascii(0x0131, "GNOME Screenshot 45.1"));
        exif.exif.push(crate::format::exif::Entry::ascii(0x9003, "2024:06:14 19:42:07"));
        let data = png_with(vec![Chunk { kind: *b"eXIf", data: exif.to_tiff() }]);

        let out = Png.clean(&data, &Profile::standard()).unwrap();
        assert!(Png.inspect(&out.data).unwrap().findings.is_empty());
        let device = out.changes.iter().find(|c| c.class == Class::Device).expect("no device change");
        assert!(device.label.contains("EXIF block in a PNG is unusual"), "{}", device.label);
        assert!(out.changes.iter().any(|c| c.class == Class::Time));
    }

    #[test]
    fn the_colour_profile_survives_every_profile() {
        let data = png_with(vec![Chunk { kind: *b"iCCP", data: b"p\0\0junk".to_vec() }]);
        let out = Png.clean(&data, &Profile::uniform()).unwrap();
        assert!(split(&out.data).unwrap().iter().any(|c| &c.kind == b"iCCP"));
    }

    #[test]
    fn standard_drops_a_png_date_because_a_png_date_is_unusual() {
        let data = png_with(vec![Chunk {
            kind: *b"tIME",
            data: write_time(Civil { year: 2024, month: 6, day: 14, hour: 17, minute: 42, second: 7 }),
        }]);
        let out = Png.clean(&data, &Profile::standard()).unwrap();
        let ins = Png.inspect(&out.data).unwrap();
        assert!(ins.in_class(Class::Time).next().is_none());
        assert!(out.changes[0].label.contains("a date in a PNG is unusual"));
        assert_eq!(out.changes[0].class, Class::Time);
    }

    #[test]
    fn strip_to_the_minimum_still_honours_a_kept_date() {
        let mut p = Profile::standard();
        p.blending = crate::profile::Blending::Minimal;
        let data = png_with(vec![Chunk {
            kind: *b"tIME",
            data: write_time(Civil { year: 2024, month: 6, day: 14, hour: 17, minute: 42, second: 7 }),
        }]);
        let out = Png.clean(&data, &p).unwrap();
        assert_eq!(Png.inspect(&out.data).unwrap().in_class(Class::Time).count(), 1);
    }

    #[test]
    fn inspect_only_changes_nothing_at_all() {
        let data = png_with(vec![
            text_chunk("Author", "Marie Dvorakova"),
            Chunk { kind: *b"tIME", data: write_time(Civil { year: 2024, month: 6, day: 14, hour: 17, minute: 42, second: 7 }) },
        ]);
        let out = Png.clean(&data, &Profile::inspect_only()).unwrap();
        assert_eq!(out.data, data);
        assert!(out.changes.is_empty());
    }

    #[test]
    fn a_cleaned_png_joins_the_large_crowd() {
        let data = png_with(vec![text_chunk("Author", "Marie Dvorakova")]);
        let out = Png.clean(&data, &Profile::uniform()).unwrap();
        let ins = Png.inspect(&out.data).unwrap();
        let sig = crate::anonset::classify(FormatId::Png, &ins.shape).unwrap();
        assert_eq!(sig.crowd, crate::anonset::Crowd::Large);
    }
}
