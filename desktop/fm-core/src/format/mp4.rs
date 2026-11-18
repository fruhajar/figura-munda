//! MP4 and MOV: a tree of boxes, with the revealing ones gathered in `udta`.
//!
//! A handset writes its make, model, capture date and GPS position into
//! `moov/udta`, and anything that has been through an editor usually also has
//! an iTunes-style `ilst`. Both are removable without touching a frame.
//!
//! The one real hazard is that the sample tables in `moov` hold absolute file
//! offsets into `mdat`. Shrinking `moov` is therefore only safe if `moov` comes
//! after the media; when it comes first, every offset in every `stco` and
//! `co64` has to be corrected by the number of bytes removed. Both cases are
//! handled, and the second is the one that silently corrupts a file if it is
//! not.

use super::{Change, Handler, Outcome};
use crate::error::{Error, Result};
use crate::model::{Action, Class, Finding, FormatId, Inspection};
use crate::profile::{Profile, TimeOutcome};
use crate::timeutil::{Civil, MP4_EPOCH_OFFSET};

pub struct Mp4;

/// One box: type, the range it occupies, and its children.
#[derive(Clone, Debug)]
struct Box_ {
    kind: [u8; 4],
    /// Offset of the box's own header in the file.
    at: usize,
    /// Total size including the header.
    size: usize,
    /// Offset of the first payload byte.
    body: usize,
    children: Vec<Box_>,
}

impl Box_ {
    fn name(&self) -> String {
        String::from_utf8_lossy(&self.kind).to_string()
    }

    fn end(&self) -> usize {
        self.at + self.size
    }

    #[cfg(test)]
    fn find(&self, path: &[&[u8; 4]]) -> Option<&Box_> {
        let (first, rest) = path.split_first()?;
        let child = self.children.iter().find(|c| &c.kind == *first)?;
        if rest.is_empty() {
            Some(child)
        } else {
            child.find(rest)
        }
    }

    fn descendants(&self) -> Vec<&Box_> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.descendants());
        }
        out
    }
}

/// Boxes whose children are boxes. Everything else is a leaf.
const CONTAINERS: &[&[u8; 4]] = &[
    b"moov", b"trak", b"mdia", b"minf", b"stbl", b"udta", b"meta", b"ilst", b"edts", b"dinf",
    b"mvex", b"moof", b"traf", b"\xa9too",
];

fn is_container(kind: &[u8; 4]) -> bool {
    CONTAINERS.contains(&kind)
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(d.get(at..at + 8)?.try_into().ok()?))
}

fn parse_boxes(data: &[u8], from: usize, to: usize, depth: usize) -> Vec<Box_> {
    let mut out = Vec::new();
    let mut at = from;
    while at + 8 <= to && depth < 12 {
        let Some(declared) = u32_at(data, at) else { break };
        let Some(kind) = data.get(at + 4..at + 8) else { break };
        let kind: [u8; 4] = kind.try_into().unwrap();
        let (size, body) = match declared {
            // 0 means "to the end of the file"; 1 means a 64-bit size follows.
            0 => (to - at, at + 8),
            1 => match u64_at(data, at + 8) {
                Some(big) if big as usize >= 16 => (big as usize, at + 16),
                _ => break,
            },
            n if n >= 8 => (n as usize, at + 8),
            _ => break,
        };
        if at + size > to {
            break;
        }
        let child_start =
            if &kind == b"meta" { meta_child_start(data, body, at + size) } else { body };
        let children = if is_container(&kind) {
            parse_boxes(data, child_start, at + size, depth + 1)
        } else {
            Vec::new()
        };
        out.push(Box_ { kind, at, size, body, children });
        at += size;
    }
    out
}

fn root(data: &[u8]) -> Box_ {
    Box_ {
        kind: *b"root",
        at: 0,
        size: data.len(),
        body: 0,
        children: parse_boxes(data, 0, data.len(), 0),
    }
}

/// Whether the bytes at `at` read as a box header that fits inside `end`.
fn looks_like_box(data: &[u8], at: usize, end: usize) -> bool {
    let Some(size) = u32_at(data, at).map(|n| n as usize) else { return false };
    let Some(kind) = data.get(at + 4..at + 8) else { return false };
    size >= 8 && at + size <= end && kind.iter().all(|b| matches!(b, 0x20..=0x7e | 0xa9))
}

/// ISO base media's `meta` is a FullBox: a version and flags word sits ahead
/// of its children. QuickTime's `meta`, which is what a `.mov` carries, has no
/// such word, and assuming it does puts every child four bytes out, so the box
/// reads as empty and nothing inside it ever reaches a report.
fn meta_child_start(data: &[u8], body: usize, end: usize) -> usize {
    if looks_like_box(data, body, end) {
        body
    } else {
        body + 4
    }
}

/// Where the top-level box chain stops covering the file, if it does.
///
/// Parsing is forgiving everywhere else, and here it must not be. An
/// interrupted copy leaves the last box declaring more bytes than are present,
/// and `moov` is usually last, so the box that goes missing is exactly the one
/// holding the metadata: the file would inspect as near-empty, clean to
/// "nothing changed" and verify as clean while the original still holds every
/// field.
fn truncated_at(data: &[u8]) -> Option<usize> {
    let mut at = 0usize;
    while at < data.len() {
        if at + 8 > data.len() {
            return Some(at);
        }
        let Some(declared) = u32_at(data, at) else { return Some(at) };
        let size = match declared {
            0 => data.len() - at,
            1 => match u64_at(data, at + 8) {
                Some(big) if big >= 16 => big as usize,
                _ => return Some(at),
            },
            n if n >= 8 => n as usize,
            _ => return Some(at),
        };
        if at + size > data.len() {
            return Some(at);
        }
        at += size;
    }
    None
}

/// The box tree, refusing a file whose boxes do not account for every byte.
fn checked_tree(data: &[u8]) -> Result<Box_> {
    let tree = root(data);
    if tree.children.is_empty() {
        return Err(Error::malformed(FormatId::Mp4, "no boxes"));
    }
    if let Some(at) = truncated_at(data) {
        return Err(Error::malformed(
            FormatId::Mp4,
            format!("the box tree stops covering the file at byte {at}; the copy is incomplete"),
        ));
    }
    Ok(tree)
}

/// What a `udta` or `ilst` leaf means. The `\xa9` prefix is QuickTime's.
fn leaf_info(kind: &[u8; 4]) -> Option<(Class, &'static str)> {
    let i = |c: Class, l: &'static str| Some((c, l));
    match kind {
        b"\xa9xyz" => i(Class::Location, "the place the video was taken, as coordinates"),
        b"loci" => i(Class::Location, "the place the video was taken, with a name"),
        b"\xa9mak" | b"make" => i(Class::Device, "the make of the device that recorded it"),
        b"\xa9mod" | b"modl" => i(Class::Device, "the model of the device that recorded it"),
        b"\xa9swr" => i(Class::Device, "the software that wrote it"),
        b"\xa9too" => i(Class::Device, "the encoder that wrote it"),
        b"\xa9dat" | b"\xa9day" | b"date" => i(Class::Time, "the date it was recorded"),
        b"\xa9ART" | b"auth" => i(Class::Identity, "the artist or author"),
        b"\xa9nam" | b"titl" => i(Class::Identity, "the title"),
        b"\xa9cmt" | b"desc" => i(Class::Identity, "a comment or description"),
        b"\xa9cpy" => i(Class::Identity, "a copyright notice"),
        b"\xa9alb" => i(Class::Identity, "the album"),
        b"cprt" => i(Class::Identity, "a copyright notice"),
        b"gps " | b"gpsc" => i(Class::Location, "GPS data"),
        b"uuid" => i(Class::Correlation, "an opaque identifier box"),
        b"\xa9enc" => i(Class::Device, "the encoding tool"),
        _ => None,
    }
}

/// The `keys`/`ilst` pair iOS writes into `moov/meta`: the `ilst` children are
/// numbered rather than named, and `keys` is the dictionary saying what each
/// number means. A handler that only knows four-byte types sees nothing there,
/// and that is most of what a recent iPhone records.
struct KeyTable {
    /// Entry *i* names key id *i + 1*.
    names: Vec<String>,
    keys: (usize, usize),
    ilst: (usize, usize),
}

fn key_tables(data: &[u8], tree: &Box_) -> Vec<KeyTable> {
    let mut out = Vec::new();
    for meta in tree.descendants() {
        if &meta.kind != b"meta" {
            continue;
        }
        let child = |k: &[u8; 4]| meta.children.iter().find(|c| &c.kind == k);
        if let (Some(keys), Some(ilst)) = (child(b"keys"), child(b"ilst")) {
            out.push(KeyTable {
                names: key_names(data, keys),
                keys: (keys.at, keys.end()),
                ilst: (ilst.at, ilst.end()),
            });
        }
    }
    out
}

/// `keys` is a version and flags word, a count, then one entry per key: a
/// size, a four-byte namespace, and the name.
fn key_names(data: &[u8], keys: &Box_) -> Vec<String> {
    let count = u32_at(data, keys.body + 4).unwrap_or(0) as usize;
    let mut names = Vec::new();
    let mut at = keys.body + 8;
    for _ in 0..count.min(1024) {
        let Some(size) = u32_at(data, at).map(|n| n as usize) else { break };
        if size < 8 || at + size > keys.end() {
            break;
        }
        let name = data.get(at + 8..at + size).unwrap_or(&[]);
        names.push(String::from_utf8_lossy(name).trim().to_string());
        at += size;
    }
    names
}

fn key_name(keys: &[KeyTable], b: &Box_) -> Option<String> {
    keys.iter().find_map(|t| {
        if b.at < t.ilst.0 || b.end() > t.ilst.1 {
            return None;
        }
        t.names.get(u32::from_be_bytes(b.kind).checked_sub(1)? as usize).cloned()
    })
}

/// What one named key reveals. The namespace prefix varies by writer, so the
/// match is on what the name contains, and a name nothing here recognises is
/// still a per-device tag worth reporting rather than passing over in silence.
fn apple_key_info(name: &str) -> (Class, String) {
    let n = name.to_ascii_lowercase();
    let (class, label): (Class, &str) = if n.contains("location") {
        (Class::Location, "the place the video was taken")
    } else if n.contains("make") {
        (Class::Device, "the make of the device that recorded it")
    } else if n.contains("model") {
        (Class::Device, "the model of the device that recorded it")
    } else if n.contains("software") || n.contains("encoder") || n.contains("camera") {
        (Class::Device, "the software or camera that wrote it")
    } else if n.contains("date") {
        (Class::Time, "the date it was recorded")
    } else if n.contains("signature") || n.contains("identifier") || n.contains("uuid") {
        (Class::Correlation, "an identifier tying this file to others from the same device")
    } else if n.contains("author") || n.contains("artist") || n.contains("title")
        || n.contains("description") || n.contains("comment") || n.contains("copyright")
        || n.contains("album")
    {
        (Class::Identity, "a name or description the recorder stored")
    } else {
        (Class::Correlation, "a tagged value the recording device wrote")
    };
    (class, format!("{label} ({name})"))
}

/// What a leaf means: either its four-byte type, or, inside a `keys`-indexed
/// `ilst`, whatever `keys` says its number stands for.
fn resolve_leaf(keys: &[KeyTable], b: &Box_) -> Option<(Class, String, String)> {
    if let Some(name) = key_name(keys, b) {
        let (class, label) = apple_key_info(&name);
        return Some((class, format!("mp4:key:{name}"), label));
    }
    let (class, label) = leaf_info(&b.kind)?;
    Some((class, format!("mp4:{}", b.name()), label.to_string()))
}

/// Text in a `udta` leaf, past its language or `data` wrapper.
fn leaf_text(data: &[u8], b: &Box_) -> Option<String> {
    let body = data.get(b.body..b.end())?;
    // An iTunes `ilst` entry wraps its value in a `data` box; QuickTime's
    // `udta` entries put a two-byte length and a language code first.
    let text = if body.len() > 16 && &body[4..8] == b"data" {
        String::from_utf8_lossy(&body[16..]).to_string()
    } else if body.len() > 4 {
        String::from_utf8_lossy(&body[4..]).to_string()
    } else {
        String::from_utf8_lossy(body).to_string()
    };
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .collect::<String>()
        .trim()
        .to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The creation and modification times in `mvhd`, `tkhd` and `mdhd`.
///
/// These are seconds since 1904-01-01, in two 32-bit fields for version 0 and
/// two 64-bit fields for version 1.
struct TimeFields {
    created_at: usize,
    modified_at: usize,
    wide: bool,
}

fn time_fields(data: &[u8], b: &Box_) -> Option<TimeFields> {
    if !matches!(&b.kind, b"mvhd" | b"tkhd" | b"mdhd") {
        return None;
    }
    let version = *data.get(b.body)?;
    let base = b.body + 4;
    Some(TimeFields {
        created_at: base,
        modified_at: base + if version == 1 { 8 } else { 4 },
        wide: version == 1,
    })
}

fn read_time(data: &[u8], at: usize, wide: bool) -> Option<Civil> {
    let secs = if wide {
        u64_at(data, at)? as i64
    } else {
        u32_at(data, at)? as i64
    };
    if secs == 0 {
        return None;
    }
    Some(Civil::from_unix_seconds(secs - MP4_EPOCH_OFFSET))
}

fn write_time(out: &mut [u8], at: usize, wide: bool, when: Civil) {
    let secs = (when.unix_seconds() + MP4_EPOCH_OFFSET).max(0);
    if wide {
        out[at..at + 8].copy_from_slice(&(secs as u64).to_be_bytes());
    } else {
        out[at..at + 4].copy_from_slice(&(secs as u32).to_be_bytes());
    }
}

impl Handler for Mp4 {
    fn id(&self) -> FormatId {
        FormatId::Mp4
    }

    fn sniff(&self, head: &[u8], _ext: Option<&str>) -> bool {
        head.len() > 12 && &head[4..8] == b"ftyp"
    }

    fn inspect(&self, data: &[u8]) -> Result<Inspection> {
        let tree = checked_tree(data)?;
        let keys = key_tables(data, &tree);
        let mut ins = Inspection::new(FormatId::Mp4, data.len() as u64);

        for b in tree.descendants() {
            if &b.kind == b"udta" {
                ins.mark("mp4:udta");
            }
            if &b.kind == b"ilst" {
                ins.mark("mp4:ilst");
            }
            if &b.kind == b"loci" {
                ins.mark("mp4:loci");
            }
            if &b.kind == b"\xa9xyz" {
                ins.mark("mp4:xyz");
            }
            if &b.kind == b"keys" {
                ins.findings.push(
                    Finding::new(
                        Class::Correlation,
                        "mp4:keys",
                        "the dictionary naming every field the recorder wrote",
                    )
                    .value(format!("{} keys", key_names(data, b).len())),
                );
            }
            if let Some((class, locator, label)) = resolve_leaf(&keys, b) {
                let mut f = Finding::new(class, locator, label);
                if let Some(text) = leaf_text(data, b) {
                    f = f.value(text);
                } else {
                    f = f.value(format!("{} bytes", b.size));
                }
                ins.findings.push(f);
            }
            if let Some(fields) = time_fields(data, b) {
                for (at, what) in [
                    (fields.created_at, "recording time"),
                    (fields.modified_at, "last modification time"),
                ] {
                    let Some(when) = read_time(data, at, fields.wide) else { continue };
                    ins.mark("mp4:header-time");
                    ins.findings.push(
                        Finding::new(
                            Class::Time,
                            format!("mp4:{}:{what}", b.name()),
                            format!("{what} in the {} header", b.name()),
                        )
                        .value(when.to_iso()),
                    );
                }
            }
        }
        Ok(ins)
    }

    fn clean(&self, data: &[u8], profile: &Profile) -> Result<Outcome> {
        let tree = checked_tree(data)?;
        let keys = key_tables(data, &tree);
        let mut changes = Vec::new();
        if profile.inert() {
            return Ok(Outcome::unchanged(data));
        }

        // Which boxes go. A `udta` or `ilst` whose every named child is being
        // removed goes whole, which is tidier and matches what a transcode
        // leaves behind.
        let mut doomed: Vec<(usize, usize)> = Vec::new();
        for b in tree.descendants() {
            // Anything already going with its parent is neither reported twice
            // nor removed twice; `meta` holding an `ilst` is the common case.
            if inside_any(&doomed, b.at) {
                continue;
            }
            if matches!(&b.kind, b"udta" | b"ilst" | b"meta") {
                let named: Vec<&Box_> = b
                    .descendants()
                    .into_iter()
                    .filter(|c| resolve_leaf(&keys, c).is_some())
                    .collect();
                let all_doomed = !named.is_empty()
                    && named.iter().all(|c| {
                        blend_leaf(&keys, c, profile)
                            || resolve_leaf(&keys, c).is_some_and(|(class, ..)| profile.alters(class))
                    });
                if all_doomed {
                    for c in named {
                        let (class, locator, label) = resolve_leaf(&keys, c).unwrap();
                        // A field the profile would have kept still has to say
                        // why it went, even when its whole container went too.
                        let label = if blend_leaf(&keys, c, profile) && !profile.alters(class) {
                            format!("{label} (removed to match common tools: {WHY_UDTA_DATE})")
                        } else {
                            label
                        };
                        changes.push(Change::removed(class, locator, label, leaf_text(data, c)));
                    }
                    doomed.push((b.at, b.end()));
                    continue;
                }
            }
            if let Some((class, locator, label)) = resolve_leaf(&keys, b) {
                if blend_leaf(&keys, b, profile) {
                    changes.push(Change::rewritten(
                        class,
                        locator,
                        format!("{label} (removed to match common tools: {WHY_UDTA_DATE})"),
                        Action::Remove,
                        leaf_text(data, b),
                        None,
                    ));
                    doomed.push((b.at, b.end()));
                    continue;
                }
                if profile.alters(class) {
                    changes.push(Change::removed(class, locator, label, leaf_text(data, b)));
                    doomed.push((b.at, b.end()));
                }
            }
        }
        // A `keys` dictionary left on its own still names every field the
        // recorder wrote, so it goes when the values it indexes go.
        for t in &keys {
            if !t.names.is_empty() && inside_any(&doomed, t.ilst.0) && !inside_any(&doomed, t.keys.0)
            {
                changes.push(Change::removed(
                    Class::Correlation,
                    "mp4:keys",
                    "the dictionary naming every field the recorder wrote",
                    Some(format!("{} keys", t.names.len())),
                ));
                doomed.push(t.keys);
            }
        }
        doomed.sort_unstable();
        doomed.dedup();
        // Keep only the outermost ranges, so nothing is counted twice.
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for (start, end) in doomed {
            if ranges.last().is_some_and(|(_, e)| *e >= end) {
                continue;
            }
            ranges.push((start, end));
        }

        let mut out = remove_ranges(data, &ranges);
        shrink_parents(&mut out, data, &tree, &ranges);
        fix_chunk_offsets(&mut out, &ranges, &mut changes)?;

        // Header timestamps are fixed-width, so they are rewritten in place
        // rather than removed: there is no way to omit them.
        let after_tree = root(&out);
        if profile.action(Class::Time) != Action::Keep {
            let mut edits: Vec<(usize, bool, Civil, Civil)> = Vec::new();
            for b in after_tree.descendants() {
                let Some(fields) = time_fields(&out, b) else { continue };
                for at in [fields.created_at, fields.modified_at] {
                    let Some(original) = read_time(&out, at, fields.wide) else { continue };
                    match profile.apply_time(Some(original)) {
                        TimeOutcome::Unchanged => {}
                        // A zero means "unknown", which is what every box that
                        // never had a time carries, so removal is expressible.
                        TimeOutcome::Drop => edits.push((at, fields.wide, original, Civil::from_unix_seconds(-MP4_EPOCH_OFFSET))),
                        TimeOutcome::Set(c) => edits.push((at, fields.wide, original, c)),
                    }
                }
            }
            for (at, wide, original, replacement) in edits {
                write_time(&mut out, at, wide, replacement);
                changes.push(Change::rewritten(
                    Class::Time,
                    "mp4:header-time",
                    "the recording time in the file's own header",
                    profile.action(Class::Time),
                    Some(original.to_iso()),
                    Some(replacement.to_iso()),
                ));
            }
        }

        Ok(Outcome { data: out, changes })
    }
}

const WHY_UDTA_DATE: &str = "a date in a user-data box is written by handsets, not by encoders";

/// A date in `udta` goes even where the profile keeps the time class, because
/// its presence rather than its value is what identifies the recorder. The
/// header times, which every file has, are left to the profile.
fn blend_leaf(keys: &[KeyTable], b: &Box_, profile: &Profile) -> bool {
    let date = matches!(&b.kind, b"\xa9dat" | b"\xa9day" | b"date")
        || key_name(keys, b).is_some_and(|n| n.to_ascii_lowercase().contains("creationdate"));
    date && profile.may_blend(false)
}

fn inside_any(ranges: &[(usize, usize)], at: usize) -> bool {
    ranges.iter().any(|(s, e)| at >= *s && at < *e)
}

fn remove_ranges(data: &[u8], ranges: &[(usize, usize)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    for (start, end) in ranges {
        out.extend_from_slice(&data[at..*start]);
        at = *end;
    }
    out.extend_from_slice(&data[at..]);
    out
}

/// Every box that contained a removed range has to declare its new size.
fn shrink_parents(out: &mut [u8], data: &[u8], tree: &Box_, ranges: &[(usize, usize)]) {
    for b in tree.descendants() {
        if b.kind == *b"root" {
            continue;
        }
        let lost: usize = ranges
            .iter()
            .filter(|(s, e)| *s >= b.at && *e <= b.end())
            .map(|(s, e)| e - s)
            .sum();
        if lost == 0 {
            continue;
        }
        let before: usize = ranges.iter().filter(|(_, e)| *e <= b.at).map(|(s, e)| e - s).sum();
        let new_at = b.at - before;
        let new_size = b.size - lost;
        // A box whose own header was removed with it needs no update.
        if new_at + 4 > out.len() || inside_any(ranges, b.at) {
            continue;
        }
        let declared = u32_at(data, b.at).unwrap_or(0);
        if declared == 1 {
            // 64-bit size field, eight bytes after the header.
            if new_at + 16 <= out.len() {
                out[new_at + 8..new_at + 16].copy_from_slice(&(new_size as u64).to_be_bytes());
            }
        } else if declared != 0 {
            out[new_at..new_at + 4].copy_from_slice(&(new_size as u32).to_be_bytes());
        }
    }
}

/// Correct the sample tables for the bytes that were removed before them.
///
/// `stco` and `co64` hold absolute offsets into the file. If anything was
/// removed ahead of the media, every one of them is now wrong, and the file
/// plays as silence or garbage rather than failing loudly.
fn fix_chunk_offsets(out: &mut [u8], ranges: &[(usize, usize)], changes: &mut Vec<Change>) -> Result<()> {
    if ranges.is_empty() {
        return Ok(());
    }
    let tree = root(out);
    let mut corrected = 0usize;
    let mut edits: Vec<(usize, u64, bool)> = Vec::new();
    for b in tree.descendants() {
        let wide = match &b.kind {
            b"stco" => false,
            b"co64" => true,
            _ => continue,
        };
        let count = u32_at(out, b.body + 4).unwrap_or(0) as usize;
        let width = if wide { 8 } else { 4 };
        for i in 0..count {
            let at = b.body + 8 + i * width;
            let Some(offset) = (if wide { u64_at(out, at) } else { u32_at(out, at).map(u64::from) })
            else {
                break;
            };
            // Offsets are into the original file, so what matters is how many
            // removed bytes came before this chunk in the original layout.
            let shift: u64 = ranges
                .iter()
                .filter(|(s, _)| (*s as u64) < offset)
                .map(|(s, e)| (e - s) as u64)
                .sum();
            if shift > 0 {
                edits.push((at, offset.saturating_sub(shift), wide));
                corrected += 1;
            }
        }
    }
    for (at, value, wide) in edits {
        if wide {
            out[at..at + 8].copy_from_slice(&value.to_be_bytes());
        } else {
            out[at..at + 4].copy_from_slice(&(value as u32).to_be_bytes());
        }
    }
    if corrected > 0 {
        changes.push(Change::rewritten(
            Class::StructuralFingerprint,
            "mp4:stco",
            "sample offsets corrected for the bytes removed ahead of the media",
            Action::RewriteUniform,
            None,
            Some(format!("{corrected} offsets")),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_bytes(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn udta_leaf(kind: &[u8; 4], text: &str) -> Vec<u8> {
        // QuickTime form: two-byte length, two-byte language, then the text.
        let mut body = (text.len() as u16).to_be_bytes().to_vec();
        body.extend_from_slice(&[0x15, 0xC7]);
        body.extend_from_slice(text.as_bytes());
        box_bytes(kind, &body)
    }

    fn mvhd(created: Civil) -> Vec<u8> {
        let mut body = vec![0u8; 100];
        let secs = (created.unix_seconds() + MP4_EPOCH_OFFSET) as u32;
        body[4..8].copy_from_slice(&secs.to_be_bytes());
        body[8..12].copy_from_slice(&secs.to_be_bytes());
        box_bytes(b"mvhd", &body)
    }

    /// `moov` before `mdat`, which is the layout that makes removal dangerous:
    /// shrinking `moov` moves the media, so every sample offset has to follow.
    fn clip_moov_first() -> Vec<u8> {
        let created = Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 };
        let mut udta = Vec::new();
        udta.extend_from_slice(&udta_leaf(b"\xa9xyz", "+50.0755+014.4378/"));
        udta.extend_from_slice(&udta_leaf(b"\xa9mak", "Apple"));
        udta.extend_from_slice(&udta_leaf(b"\xa9mod", "iPhone 14 Pro"));
        udta.extend_from_slice(&udta_leaf(b"\xa9ART", "Marie Dvorakova"));

        // One chunk offset, filled in once the layout is known.
        let mut stco_body = vec![0u8; 4];
        stco_body.extend_from_slice(&1u32.to_be_bytes());
        stco_body.extend_from_slice(&0u32.to_be_bytes());

        let mut moov_body = mvhd(created);
        moov_body.extend_from_slice(&box_bytes(b"udta", &udta));
        moov_body.extend_from_slice(&box_bytes(b"stbl", &box_bytes(b"stco", &stco_body)));

        let ftyp = box_bytes(b"ftyp", b"isom\0\0\x02\0isomiso2");
        let moov = box_bytes(b"moov", &moov_body);
        let mdat = box_bytes(b"mdat", b"\x01\x02\x03\x04frames go here");
        let mdat_payload_at = ftyp.len() + moov.len() + 8;

        let mut out = ftyp;
        out.extend_from_slice(&moov);
        out.extend_from_slice(&mdat);

        let stco_at = {
            let tree = root(&out);
            tree.descendants().into_iter().find(|b| &b.kind == b"stco").unwrap().body
        };
        out[stco_at + 8..stco_at + 12].copy_from_slice(&(mdat_payload_at as u32).to_be_bytes());
        debug_assert_eq!(&out[mdat_payload_at..mdat_payload_at + 4], &[1, 2, 3, 4]);
        out
    }

    fn stco_first_offset(data: &[u8]) -> u32 {
        let tree = root(data);
        let stco = tree.descendants().into_iter().find(|b| &b.kind == b"stco").unwrap();
        u32_at(data, stco.body + 8).unwrap()
    }

    #[test]
    fn finds_the_place_the_device_and_the_author() {
        let ins = Mp4.inspect(&clip_moov_first()).unwrap();
        let v = |c: Class| {
            ins.in_class(c).filter_map(|f| f.value.clone()).collect::<Vec<_>>().join(" | ")
        };
        assert!(v(Class::Location).contains("+50.0755+014.4378"), "{}", v(Class::Location));
        assert!(v(Class::Device).contains("iPhone 14 Pro"));
        assert!(v(Class::Identity).contains("Marie Dvorakova"));
        assert!(v(Class::Time).contains("2024-06-14T19:42:07Z"), "{}", v(Class::Time));
        assert!(ins.shape.contains(&"mp4:udta".to_string()));
    }

    #[test]
    fn standard_removes_the_user_data_and_leaves_the_media_playable() {
        let data = clip_moov_first();
        let original_offset = stco_first_offset(&data);
        let out = Mp4.clean(&data, &Profile::standard()).unwrap();

        let after = Mp4.inspect(&out.data).unwrap();
        assert!(after.in_class(Class::Location).next().is_none());
        assert!(after.in_class(Class::Device).next().is_none());
        assert!(after.in_class(Class::Identity).next().is_none());
        assert!(!after.shape.contains(&"mp4:udta".to_string()));

        // The media is where the sample table now says it is.
        let removed = data.len() - out.data.len();
        assert!(removed > 0);
        let new_offset = stco_first_offset(&out.data);
        assert_eq!(new_offset, original_offset - removed as u32);
        assert_eq!(&out.data[new_offset as usize..new_offset as usize + 4], &[1, 2, 3, 4]);
    }

    #[test]
    fn every_container_declares_its_new_size() {
        let out = Mp4.clean(&clip_moov_first(), &Profile::standard()).unwrap();
        // If a parent's size were stale, the tree would not reparse to the end.
        let tree = root(&out.data);
        let last = tree.children.last().unwrap();
        assert_eq!(last.end(), out.data.len(), "the box tree does not cover the file");
        assert!(tree.find(&[b"moov", b"mvhd"]).is_some(), "mvhd was lost");
        assert!(tree.find(&[b"moov", b"udta"]).is_none(), "udta survived");
    }

    /// Every MP4 has a recording time in its header, so that one follows the
    /// profile. Almost none has one in a user-data box, so that one goes even
    /// when the profile keeps the time class.
    #[test]
    fn a_handsets_user_data_date_goes_but_the_header_time_stays() {
        let mut udta = udta_leaf(b"\xa9dat", "2024-06-14T19:42:07Z");
        udta.extend_from_slice(&udta_leaf(b"\xa9nam", "clip"));
        let mut moov_body = mvhd(Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 });
        moov_body.extend_from_slice(&box_bytes(b"udta", &udta));
        let mut data = box_bytes(b"ftyp", b"isom\0\0\x02\0isomiso2");
        data.extend_from_slice(&box_bytes(b"moov", &moov_body));

        let out = Mp4.clean(&data, &Profile::standard()).unwrap();
        let after = Mp4.inspect(&out.data).unwrap();
        assert!(!after.shape.contains(&"mp4:udta".to_string()), "{:?}", after.shape);
        assert!(after.shape.contains(&"mp4:header-time".to_string()));
        assert!(out.changes.iter().any(|c| c.label.contains("written by handsets")));

        // And with blending off, the kept date stays where it was.
        let mut minimal = Profile::standard();
        minimal.blending = crate::profile::Blending::Minimal;
        let out = Mp4.clean(&data, &minimal).unwrap();
        assert!(Mp4.inspect(&out.data).unwrap().shape.contains(&"mp4:udta".to_string()));
    }

    #[test]
    fn strict_coarsens_the_recording_time_in_place() {
        let out = Mp4.clean(&clip_moov_first(), &Profile::strict()).unwrap();
        let after = Mp4.inspect(&out.data).unwrap();
        let times: Vec<String> = after.in_class(Class::Time).filter_map(|f| f.value.clone()).collect();
        assert!(times.iter().all(|t| t == "2024-06-14T00:00:00Z"), "{times:?}");
    }

    #[test]
    fn uniform_normalises_the_recording_time() {
        let out = Mp4.clean(&clip_moov_first(), &Profile::uniform()).unwrap();
        let after = Mp4.inspect(&out.data).unwrap();
        let times: Vec<String> = after.in_class(Class::Time).filter_map(|f| f.value.clone()).collect();
        assert!(times.iter().all(|t| t == "1980-01-01T00:00:00Z"), "{times:?}");
    }

    #[test]
    fn a_cleaned_clip_joins_the_large_crowd() {
        let out = Mp4.clean(&clip_moov_first(), &Profile::standard()).unwrap();
        let after = Mp4.inspect(&out.data).unwrap();
        let sig = crate::anonset::classify(FormatId::Mp4, &after.shape).unwrap();
        assert_eq!(sig.id, "mp4-plain");
    }

    #[test]
    fn every_profile_leaves_a_verifiably_clean_clip() {
        let data = clip_moov_first();
        for profile in Profile::builtins() {
            if profile.inert() {
                continue;
            }
            let out = Mp4.clean(&data, &profile).unwrap();
            let after = Mp4.inspect(&out.data).unwrap();
            let v = crate::report::Verification::after(&after, &profile, &out.changes);
            assert!(
                v.clean(),
                "{}: {:?}",
                profile.name,
                v.leftovers.iter().map(|f| &f.locator).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn inspect_only_returns_the_bytes_untouched() {
        let data = clip_moov_first();
        assert_eq!(Mp4.clean(&data, &Profile::inspect_only()).unwrap().data, data);
    }

    /// `moov` last is what a phone writes, and what an interrupted copy loses
    /// first: the file still parses, with the metadata-bearing box simply
    /// absent from the tree.
    fn clip_moov_last() -> Vec<u8> {
        let mut udta = udta_leaf(b"\xa9mak", "Apple");
        udta.extend_from_slice(&udta_leaf(b"\xa9mod", "iPhone 14 Pro"));
        udta.extend_from_slice(&udta_leaf(b"\xa9xyz", "+50.0755+014.4378/"));
        let mut moov_body =
            mvhd(Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 });
        moov_body.extend_from_slice(&box_bytes(b"udta", &udta));

        let mut out = box_bytes(b"ftyp", b"isom\0\0\x02\0isomiso2");
        out.extend_from_slice(&box_bytes(b"mdat", b"\x01\x02\x03\x04frames go here"));
        out.extend_from_slice(&box_bytes(b"moov", &moov_body));
        out
    }

    /// An `ilst` entry wraps its value in a `data` box: a well-known type, a
    /// locale, then the bytes.
    fn itunes_leaf(kind: &[u8; 4], text: &str) -> Vec<u8> {
        let mut body = 1u32.to_be_bytes().to_vec();
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(text.as_bytes());
        box_bytes(kind, &box_bytes(b"data", &body))
    }

    fn keys_box(names: &[&str]) -> Vec<u8> {
        let mut body = vec![0u8; 4];
        body.extend_from_slice(&(names.len() as u32).to_be_bytes());
        for n in names {
            body.extend_from_slice(&((n.len() + 8) as u32).to_be_bytes());
            body.extend_from_slice(b"mdta");
            body.extend_from_slice(n.as_bytes());
        }
        box_bytes(b"keys", &body)
    }

    /// A `.mov`'s `moov/meta`, which has no version and flags word.
    fn quicktime_meta_clip(meta_children: &[u8]) -> Vec<u8> {
        let mut moov_body =
            mvhd(Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 });
        moov_body.extend_from_slice(&box_bytes(b"meta", meta_children));
        let mut data = box_bytes(b"ftyp", b"qt  \0\0\x02\0qt  qt  ");
        data.extend_from_slice(&box_bytes(b"moov", &moov_body));
        data
    }

    fn values(ins: &Inspection, class: Class) -> Vec<String> {
        ins.in_class(class).filter_map(|f| f.value.clone()).collect()
    }

    /// A copy that stopped a few bytes short used to inspect as a two-field
    /// file, clean to "nothing changed" and verify as clean, while the
    /// original still held the make, the model and the place.
    #[test]
    fn a_clip_that_stops_short_is_refused_rather_than_reported_clean() {
        let whole = clip_moov_last();
        assert_eq!(values(&Mp4.inspect(&whole).unwrap(), Class::Device).len(), 2);

        let short = &whole[..whole.len() - 15];
        let err = Mp4.inspect(short).unwrap_err().to_string();
        assert!(err.contains("incomplete"), "{err}");
        assert!(Mp4.clean(short, &Profile::standard()).is_err());
    }

    /// QuickTime's `meta` has no version word. Assuming it does shifts every
    /// child four bytes, and the whole box reads as empty.
    #[test]
    fn a_quicktime_meta_is_read_even_without_a_version_word() {
        let mut children = box_bytes(b"hdlr", b"\0\0\0\0mdirappl\0\0\0\0\0\0\0\0\0");
        let mut ilst = itunes_leaf(b"\xa9mak", "Apple");
        ilst.extend_from_slice(&itunes_leaf(b"\xa9mod", "iPhone 14 Pro"));
        ilst.extend_from_slice(&itunes_leaf(b"\xa9day", "2024-06-14T19:42:07+0200"));
        children.extend_from_slice(&box_bytes(b"ilst", &ilst));
        let data = quicktime_meta_clip(&children);

        let ins = Mp4.inspect(&data).unwrap();
        assert!(values(&ins, Class::Device).iter().any(|v| v == "iPhone 14 Pro"), "{ins:?}");
        assert!(values(&ins, Class::Time).iter().any(|v| v.starts_with("2024-06-14T19:42:07+")));

        let after = Mp4.inspect(&Mp4.clean(&data, &Profile::standard()).unwrap().data).unwrap();
        assert!(after.in_class(Class::Device).next().is_none());
    }

    /// What a recent iPhone writes: `ilst` entries numbered rather than named,
    /// with `keys` holding the name each number stands for.
    #[test]
    fn a_keys_indexed_ilst_is_read_and_removed_with_its_dictionary() {
        let names = [
            "com.apple.quicktime.location.ISO6709",
            "com.apple.quicktime.make",
            "com.apple.quicktime.model",
            "com.apple.quicktime.creationdate",
        ];
        let values_in = ["+50.0755+014.4378/", "Apple", "iPhone 14 Pro", "2024-06-14T19:42:07+0200"];
        let mut ilst = Vec::new();
        for (i, v) in values_in.iter().enumerate() {
            let kind = (i as u32 + 1).to_be_bytes();
            ilst.extend_from_slice(&itunes_leaf(&kind, v));
        }
        let mut children = keys_box(&names);
        children.extend_from_slice(&box_bytes(b"ilst", &ilst));
        let data = quicktime_meta_clip(&children);

        let ins = Mp4.inspect(&data).unwrap();
        assert!(values(&ins, Class::Location).iter().any(|v| v.contains("+50.0755")), "{ins:?}");
        assert!(values(&ins, Class::Device).iter().any(|v| v == "iPhone 14 Pro"));

        let out = Mp4.clean(&data, &Profile::standard()).unwrap();
        let after = Mp4.inspect(&out.data).unwrap();
        assert!(after.in_class(Class::Location).next().is_none());
        assert!(after.in_class(Class::Device).next().is_none());
        assert!(after.in_class(Class::Time).next().is_some(), "the header time is not a key");

        // Each change names the key it removed, so the report can say what went.
        let locators: Vec<&str> = out.changes.iter().map(|c| c.locator.as_str()).collect();
        assert!(locators.iter().any(|l| l.ends_with("quicktime.model")), "{locators:?}");
        assert_eq!(locators.iter().filter(|l| l.contains("quicktime.model")).count(), 1);
        assert!(!out.data.windows(4).any(|w| w == b"keys"), "the dictionary survived");
    }

    #[test]
    fn a_file_with_no_boxes_is_rejected() {
        assert!(Mp4.inspect(b"not an mp4 at all, no boxes here").is_err());
    }

    #[test]
    fn sniffing_looks_for_ftyp_where_it_belongs() {
        assert!(Mp4.sniff(&clip_moov_first(), None));
        assert!(!Mp4.sniff(b"\xff\xd8\xff\xe0 ftyp later on", None));
    }
}
