//! JPEG: a marker-segment list, most of which is metadata.
//!
//! Cleaning is done by rebuilding the segment list and, where EXIF survives at
//! all, rebuilding the EXIF block from the entries that were kept. Nothing is
//! patched in place, so there is no way for a stale offset to point at a value
//! that was supposed to be gone.

use super::exif::{self, Exif};
use super::{iptc, xmp, Change, Handler, Outcome};
use crate::error::{Error, Result};
use crate::model::{Class, Finding, FormatId, Impact, Inspection};
use crate::profile::{Blending, Profile};

pub struct Jpeg;

const APP0: u8 = 0xE0;
const APP1: u8 = 0xE1;
const APP2: u8 = 0xE2;
const APP13: u8 = 0xED;
const APP14: u8 = 0xEE;
const COM: u8 = 0xFE;
const SOS: u8 = 0xDA;

const EXIF_SIG: &[u8] = b"Exif\0\0";
const XMP_SIG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXT_SIG: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const ICC_SIG: &[u8] = b"ICC_PROFILE\0";
const PS_SIG: &[u8] = b"Photoshop 3.0\0";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Jfif,
    Exif,
    Xmp,
    XmpExtension,
    Icc,
    Photoshop,
    Adobe,
    Comment,
    Other,
}

struct Segment {
    marker: u8,
    payload: Vec<u8>,
}

impl Segment {
    fn kind(&self) -> Kind {
        let starts = |sig: &[u8]| self.payload.starts_with(sig);
        match self.marker {
            APP0 if starts(b"JFIF\0") || starts(b"JFXX\0") => Kind::Jfif,
            APP1 if starts(EXIF_SIG) => Kind::Exif,
            APP1 if starts(XMP_SIG) => Kind::Xmp,
            APP1 if starts(XMP_EXT_SIG) => Kind::XmpExtension,
            APP2 if starts(ICC_SIG) => Kind::Icc,
            APP13 if starts(PS_SIG) => Kind::Photoshop,
            APP14 if starts(b"Adobe") => Kind::Adobe,
            COM => Kind::Comment,
            _ => Kind::Other,
        }
    }
}

/// Split into segments plus the entropy-coded tail, which begins at SOS and is
/// never touched: the pixels are not the tool's business.
fn split(data: &[u8]) -> Result<(Vec<Segment>, &[u8])> {
    let bad = |d: &str| Error::malformed(FormatId::Jpeg, d.to_string());
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(bad("no start-of-image marker"));
    }
    let mut segments = Vec::new();
    let mut at = 2;
    loop {
        // Fill bytes between segments are legal; skip them.
        while data.get(at) == Some(&0xFF) && data.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        match (data.get(at), data.get(at + 1)) {
            (Some(0xFF), Some(&marker)) => {
                if marker == SOS {
                    return Ok((segments, &data[at..]));
                }
                if marker == 0xD9 {
                    return Ok((segments, &data[at..]));
                }
                // Standalone markers carry no length field.
                if (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
                    at += 2;
                    continue;
                }
                let len = match (data.get(at + 2), data.get(at + 3)) {
                    (Some(&hi), Some(&lo)) => u16::from_be_bytes([hi, lo]) as usize,
                    _ => return Err(bad("truncated segment length")),
                };
                if len < 2 {
                    return Err(bad("segment length below its own header"));
                }
                let end = at + 2 + len;
                let payload = data.get(at + 4..end).ok_or_else(|| bad("segment runs past end of file"))?;
                segments.push(Segment { marker, payload: payload.to_vec() });
                at = end;
            }
            _ => return Err(bad("lost marker alignment")),
        }
    }
}

fn assemble(segments: &[Segment], tail: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tail.len() + 1024);
    out.extend_from_slice(&[0xFF, 0xD8]);
    for s in segments {
        out.extend_from_slice(&[0xFF, s.marker]);
        out.extend_from_slice(&((s.payload.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&s.payload);
    }
    out.extend_from_slice(tail);
    out
}

impl Handler for Jpeg {
    fn id(&self) -> FormatId {
        FormatId::Jpeg
    }

    fn sniff(&self, head: &[u8], _ext: Option<&str>) -> bool {
        head.starts_with(&[0xFF, 0xD8, 0xFF])
    }

    fn inspect(&self, data: &[u8]) -> Result<Inspection> {
        let (segments, _) = split(data)?;
        let mut ins = Inspection::new(FormatId::Jpeg, data.len() as u64);
        for s in &segments {
            match s.kind() {
                Kind::Jfif => ins.mark("jpeg:app0-jfif"),
                Kind::Icc => {
                    ins.mark("jpeg:app2-icc");
                    ins.findings.push(
                        Finding::new(Class::Device, "jpeg:app2:icc", "ICC colour profile")
                            .value(format!("{} bytes", s.payload.len()))
                            .protected()
                            .note("kept on purpose: without it the colours shift"),
                    );
                }
                Kind::Adobe => {
                    ins.mark("jpeg:app14-adobe");
                    ins.findings.push(
                        Finding::new(
                            Class::StructuralFingerprint,
                            "jpeg:app14:adobe",
                            "Adobe colour-transform marker, which identifies the encoder",
                        )
                        .impact(Impact::Metadata),
                    );
                }
                Kind::Exif => match Exif::parse(&s.payload[EXIF_SIG.len()..]) {
                    Ok(exif) => {
                        ins.mark("jpeg:app1-exif");
                        exif::inspect_entries(&exif, &mut ins);
                    }
                    Err(e) => ins.findings.push(
                        Finding::new(Class::Device, "jpeg:app1:exif", "EXIF block that could not be read")
                            .value(e.to_string())
                            .note("it will be removed wholesale rather than rebuilt"),
                    ),
                },
                Kind::Xmp | Kind::XmpExtension => {
                    ins.mark("jpeg:app1-xmp");
                    let start = if s.kind() == Kind::Xmp { XMP_SIG.len() } else { XMP_EXT_SIG.len() };
                    xmp::inspect(&s.payload[start..], "jpeg:app1:xmp", &mut ins);
                }
                Kind::Photoshop => {
                    ins.mark("jpeg:app13-iptc");
                    iptc::inspect(&s.payload[PS_SIG.len()..], &mut ins);
                }
                Kind::Comment => {
                    ins.mark("jpeg:com");
                    ins.findings.push(
                        Finding::new(Class::Identity, "jpeg:com", "free-text comment")
                            .value(String::from_utf8_lossy(&s.payload).trim().to_string()),
                    );
                }
                Kind::Other => {}
            }
        }
        Ok(ins)
    }

    fn clean(&self, data: &[u8], profile: &Profile) -> Result<Outcome> {
        let (segments, tail) = split(data)?;
        let mut changes = Vec::new();
        let mut out: Vec<Segment> = Vec::new();

        for s in segments {
            match s.kind() {
                // The JFIF header carries no metadata and its absence is itself
                // distinctive, so it stays unless the profile asks to strip to
                // the minimum.
                Kind::Jfif => {
                    if profile.blending == Blending::Minimal {
                        changes.push(Change::removed(
                            Class::StructuralFingerprint,
                            "jpeg:app0:jfif",
                            "JFIF header",
                            None,
                        ));
                    } else {
                        out.push(s);
                    }
                }
                Kind::Icc => out.push(s),
                Kind::Adobe => {
                    if profile.alters(Class::StructuralFingerprint) {
                        changes.push(Change::removed(
                            Class::StructuralFingerprint,
                            "jpeg:app14:adobe",
                            "Adobe encoder marker",
                            None,
                        ));
                    } else {
                        out.push(s);
                    }
                }
                Kind::Exif => {
                    let parsed = Exif::parse(&s.payload[EXIF_SIG.len()..]);
                    match parsed {
                        Ok(exif) => {
                            if let Some(kept) = exif::clean_entries(&exif, profile, &mut changes) {
                                let mut payload = EXIF_SIG.to_vec();
                                payload.extend_from_slice(&kept.to_tiff());
                                out.push(Segment { marker: APP1, payload });
                            }
                        }
                        Err(e) => changes.push(Change::removed(
                            Class::Device,
                            "jpeg:app1:exif",
                            "unreadable EXIF block",
                            Some(e.to_string()),
                        )),
                    }
                }
                // A partly-stripped XMP packet is a rarer shape than no packet
                // at all, so XMP goes whole or stays whole.
                Kind::Xmp | Kind::XmpExtension => {
                    let start = if s.kind() == Kind::Xmp { XMP_SIG.len() } else { XMP_EXT_SIG.len() };
                    if xmp::should_drop(&s.payload[start..], profile, "jpeg:app1:xmp", &mut changes) {
                        continue;
                    }
                    out.push(s);
                }
                Kind::Photoshop => if let Some(rest) = iptc::clean(&s.payload[PS_SIG.len()..], profile, &mut changes) {
                    let mut payload = PS_SIG.to_vec();
                    payload.extend_from_slice(&rest);
                    out.push(Segment { marker: APP13, payload });
                },
                Kind::Comment => {
                    if profile.alters(Class::Identity) {
                        changes.push(Change::removed(
                            Class::Identity,
                            "jpeg:com",
                            "free-text comment",
                            Some(String::from_utf8_lossy(&s.payload).trim().to_string()),
                        ));
                    } else {
                        out.push(s);
                    }
                }
                Kind::Other => out.push(s),
            }
        }

        Ok(Outcome { data: assemble(&out, tail), changes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smallest structurally valid JPEG: SOI, a JFIF APP0, SOS, EOI.
    fn minimal() -> Vec<u8> {
        let jfif = Segment { marker: APP0, payload: b"JFIF\0\x01\x02\x00\x00\x01\x00\x01\x00\x00".to_vec() };
        assemble(&[jfif], &[0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9])
    }

    #[test]
    fn splits_and_reassembles_byte_for_byte() {
        let data = minimal();
        let (segs, tail) = split(&data).unwrap();
        assert_eq!(assemble(&segs, tail), data);
    }

    #[test]
    fn rejects_a_file_that_is_not_a_jpeg() {
        assert!(split(b"\x89PNG\r\n\x1a\n").is_err());
    }

    #[test]
    fn keeps_the_jfif_header_when_matching_common_tools() {
        let out = Jpeg.clean(&minimal(), &Profile::uniform()).unwrap();
        let ins = Jpeg.inspect(&out.data).unwrap();
        assert!(ins.shape.contains(&"jpeg:app0-jfif".to_string()));
    }

    #[test]
    fn strips_the_jfif_header_only_under_minimal_blending() {
        let mut p = Profile::uniform();
        p.blending = Blending::Minimal;
        let out = Jpeg.clean(&minimal(), &p).unwrap();
        let ins = Jpeg.inspect(&out.data).unwrap();
        assert!(ins.shape.is_empty());
    }
}
