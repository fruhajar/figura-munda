//! EXIF, which is a TIFF file living inside another file.
//!
//! Parsed into self-contained entries (every value resolved out of its offset)
//! so that cleaning can be done by rebuilding rather than by patching offsets in
//! place. Rebuilding is always little-endian: two byte orders are two shapes to
//! be fingerprinted by, and little-endian is overwhelmingly the common one.

use super::Change;
use crate::error::{Error, Result};
use crate::model::{Action, Class, Finding, FormatId, Impact, Inspection};
use crate::profile::{retime, Profile, TimeOutcome};

pub const TYPE_BYTE: u16 = 1;
pub const TYPE_ASCII: u16 = 2;
pub const TYPE_SHORT: u16 = 3;
pub const TYPE_LONG: u16 = 4;
pub const TYPE_RATIONAL: u16 = 5;
pub const TYPE_UNDEFINED: u16 = 7;
pub const TYPE_SLONG: u16 = 9;
pub const TYPE_SRATIONAL: u16 = 10;

pub const TAG_ORIENTATION: u16 = 0x0112;
pub const TAG_EXIF_IFD: u16 = 0x8769;
pub const TAG_GPS_IFD: u16 = 0x8825;
pub const TAG_INTEROP_IFD: u16 = 0xA005;
pub const TAG_XMP: u16 = 0x02BC;
pub const TAG_IPTC: u16 = 0x83BB;
/// Smallest byte count a thumbnail could plausibly hold a picture in. Below
/// this it is a placeholder, whatever its header says.
const THUMBNAIL_FLOOR: usize = 256;

pub const TAG_THUMB_OFFSET: u16 = 0x0201;
pub const TAG_THUMB_LENGTH: u16 = 0x0202;

fn opaque(n: usize) -> String {
    format!("{n} opaque byte{}", if n == 1 { "" } else { "s" })
}

fn type_size(typ: u16) -> usize {
    match typ {
        TYPE_BYTE | TYPE_ASCII | TYPE_UNDEFINED | 6 => 1,
        TYPE_SHORT | 8 => 2,
        TYPE_LONG | TYPE_SLONG | 11 => 4,
        TYPE_RATIONAL | TYPE_SRATIONAL | 12 => 8,
        _ => 1,
    }
}

/// Which IFD an entry came from. The same tag number means different things in
/// different IFDs, so the space is part of the identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Ifd0,
    Exif,
    Gps,
    Interop,
    /// The thumbnail IFD. Everything here exists to describe the embedded
    /// preview, which is itself the hidden payload.
    Ifd1,
}

impl Space {
    pub fn label(self) -> &'static str {
        match self {
            Space::Ifd0 => "ifd0",
            Space::Exif => "exif",
            Space::Gps => "gps",
            Space::Interop => "interop",
            Space::Ifd1 => "ifd1",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    /// Value bytes, resolved out of any offset, in the source byte order.
    pub data: Vec<u8>,
}

impl Entry {
    pub fn ascii(tag: u16, s: &str) -> Entry {
        let mut data = s.as_bytes().to_vec();
        data.push(0);
        Entry { tag, typ: TYPE_ASCII, count: data.len() as u32, data }
    }

    pub fn short(tag: u16, v: u16) -> Entry {
        Entry { tag, typ: TYPE_SHORT, count: 1, data: v.to_le_bytes().to_vec() }
    }

    fn u16_at(&self, i: usize, le: bool) -> u16 {
        let b = [self.data[i * 2], self.data[i * 2 + 1]];
        if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    }

    fn u32_at(&self, i: usize, le: bool) -> u32 {
        let b = [self.data[i * 4], self.data[i * 4 + 1], self.data[i * 4 + 2], self.data[i * 4 + 3]];
        if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    }

    fn rational_at(&self, i: usize, le: bool) -> (u32, u32) {
        (self.u32_at(i * 2, le), self.u32_at(i * 2 + 1, le))
    }

    /// Human-readable value, for the report. Not round-trippable, and not
    /// meant to be: the diff shows what a person would recognise.
    pub fn display(&self, le: bool) -> String {
        let fits = |n: usize, w: usize| self.data.len() >= n * w;
        match self.typ {
            TYPE_BYTE | 6 if self.count <= 8 => self
                .data
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            TYPE_ASCII => String::from_utf8_lossy(&self.data)
                .trim_end_matches('\0')
                .trim()
                .to_string(),
            TYPE_SHORT if fits(self.count as usize, 2) => (0..self.count as usize)
                .map(|i| self.u16_at(i, le).to_string())
                .collect::<Vec<_>>()
                .join(", "),
            TYPE_LONG | TYPE_SLONG if fits(self.count as usize, 4) => (0..self.count as usize)
                .map(|i| self.u32_at(i, le).to_string())
                .collect::<Vec<_>>()
                .join(", "),
            TYPE_RATIONAL | TYPE_SRATIONAL if fits(self.count as usize, 8) => (0..self.count as usize)
                .map(|i| {
                    let (n, d) = self.rational_at(i, le);
                    if d == 0 {
                        "0".to_string()
                    } else if n % d == 0 {
                        (n / d).to_string()
                    } else {
                        format!("{:.4}", n as f64 / d as f64)
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
            _ => {
                let printable = self
                    .data
                    .iter()
                    .take(48)
                    .all(|b| b.is_ascii_graphic() || *b == b' ' || *b == 0);
                if printable {
                    let s = String::from_utf8_lossy(&self.data)
                        .trim_end_matches('\0')
                        .trim()
                        .to_string();
                    if s.is_empty() {
                        opaque(self.data.len())
                    } else {
                        s
                    }
                } else {
                    opaque(self.data.len())
                }
            }
        }
    }

    /// GPS coordinates are three rationals; rendered as degrees for the report.
    pub fn display_dms(&self, le: bool, reference: Option<&str>) -> String {
        if self.typ != TYPE_RATIONAL || self.count < 3 || self.data.len() < 24 {
            return self.display(le);
        }
        let v: Vec<f64> = (0..3)
            .map(|i| {
                let (n, d) = self.rational_at(i, le);
                if d == 0 { 0.0 } else { n as f64 / d as f64 }
            })
            .collect();
        let deg = v[0] + v[1] / 60.0 + v[2] / 3600.0;
        match reference {
            Some(r) => format!("{deg:.5}\u{b0} {r}"),
            None => format!("{deg:.5}\u{b0}"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Exif {
    pub le: bool,
    pub ifd0: Vec<Entry>,
    pub exif: Vec<Entry>,
    pub gps: Vec<Entry>,
    pub interop: Vec<Entry>,
    pub ifd1: Vec<Entry>,
    pub thumbnail: Option<Vec<u8>>,
}

impl Exif {
    pub fn space(&self, s: Space) -> &[Entry] {
        match s {
            Space::Ifd0 => &self.ifd0,
            Space::Exif => &self.exif,
            Space::Gps => &self.gps,
            Space::Interop => &self.interop,
            Space::Ifd1 => &self.ifd1,
        }
    }

    pub fn get(&self, s: Space, tag: u16) -> Option<&Entry> {
        self.space(s).iter().find(|e| e.tag == tag)
    }

    pub fn is_empty(&self) -> bool {
        self.ifd0.is_empty()
            && self.exif.is_empty()
            && self.gps.is_empty()
            && self.interop.is_empty()
            && self.ifd1.is_empty()
            && self.thumbnail.is_none()
    }

    /// Parse a TIFF block: `II`/`MM`, magic, offset to IFD0.
    pub fn parse(tiff: &[u8]) -> Result<Exif> {
        let bad = |d: &str| Error::malformed(FormatId::Jpeg, format!("EXIF: {d}"));
        if tiff.len() < 8 {
            return Err(bad("block too short for a TIFF header"));
        }
        let le = match &tiff[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err(bad("no byte-order mark")),
        };
        let rd16 = |at: usize| -> Option<u16> {
            let b = [*tiff.get(at)?, *tiff.get(at + 1)?];
            Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
        };
        let rd32 = |at: usize| -> Option<u32> {
            let b = [*tiff.get(at)?, *tiff.get(at + 1)?, *tiff.get(at + 2)?, *tiff.get(at + 3)?];
            Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
        };
        if rd16(2) != Some(42) {
            return Err(bad("bad TIFF magic"));
        }

        let mut out = Exif { le, ..Default::default() };
        let mut pointers: Vec<(Space, usize)> = Vec::new();
        let mut next = rd32(4).ok_or_else(|| bad("truncated IFD0 pointer"))? as usize;
        let mut space = Space::Ifd0;
        let mut guard = 0;

        // IFD0, then IFD1 via the chain. Deeper IFDs are reached by pointer tag.
        while next != 0 && guard < 8 {
            guard += 1;
            let count = rd16(next).ok_or_else(|| bad("truncated IFD"))? as usize;
            let mut entries = Vec::with_capacity(count);
            for i in 0..count {
                let at = next + 2 + i * 12;
                let (Some(tag), Some(typ), Some(cnt)) = (rd16(at), rd16(at + 2), rd32(at + 4)) else {
                    return Err(bad("truncated IFD entry"));
                };
                let len = type_size(typ).saturating_mul(cnt as usize);
                // A single absurd count must not become a huge allocation.
                if len > tiff.len() {
                    continue;
                }
                let data = if len <= 4 {
                    tiff.get(at + 8..at + 8 + len).unwrap_or_default().to_vec()
                } else {
                    let off = rd32(at + 8).ok_or_else(|| bad("truncated value offset"))? as usize;
                    match tiff.get(off..off + len) {
                        Some(v) => v.to_vec(),
                        None => continue,
                    }
                };
                match (space, tag) {
                    (Space::Ifd0, TAG_EXIF_IFD) | (Space::Ifd0, TAG_GPS_IFD) => {
                        let target = if tag == TAG_EXIF_IFD { Space::Exif } else { Space::Gps };
                        if let Some(off) = rd32(at + 8) {
                            pointers.push((target, off as usize));
                        }
                    }
                    (Space::Exif, TAG_INTEROP_IFD) => {
                        if let Some(off) = rd32(at + 8) {
                            pointers.push((Space::Interop, off as usize));
                        }
                    }
                    _ => entries.push(Entry { tag, typ, count: cnt, data }),
                }
            }
            let chain = rd32(next + 2 + count * 12).unwrap_or(0) as usize;
            match space {
                Space::Ifd0 => {
                    out.ifd0 = entries;
                    // Walk the pointed-to IFDs before following the chain.
                    while let Some((target, off)) = pointers.pop() {
                        let sub = read_ifd(tiff, off, le, &mut pointers, target)?;
                        match target {
                            Space::Exif => out.exif = sub,
                            Space::Gps => out.gps = sub,
                            Space::Interop => out.interop = sub,
                            _ => {}
                        }
                    }
                    space = Space::Ifd1;
                    next = chain;
                }
                _ => {
                    out.ifd1 = entries;
                    next = 0;
                }
            }
        }

        // The thumbnail bytes live outside any IFD, pointed at from IFD1.
        let thumb_off = out.get(Space::Ifd1, TAG_THUMB_OFFSET).and_then(|e| {
            (e.data.len() >= 4).then(|| {
                let b = [e.data[0], e.data[1], e.data[2], e.data[3]];
                (if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }) as usize
            })
        });
        let thumb_len = out.get(Space::Ifd1, TAG_THUMB_LENGTH).and_then(|e| {
            (e.data.len() >= 4).then(|| {
                let b = [e.data[0], e.data[1], e.data[2], e.data[3]];
                (if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }) as usize
            })
        });
        if let (Some(off), Some(len)) = (thumb_off, thumb_len) {
            if let Some(bytes) = tiff.get(off..off + len) {
                out.thumbnail = Some(bytes.to_vec());
            }
        }
        Ok(out)
    }

    /// Serialise to a little-endian TIFF block.
    ///
    /// Entries are emitted in ascending tag order, which the TIFF specification
    /// requires and which also removes the writer's own ordering as a signal.
    pub fn to_tiff(&self) -> Vec<u8> {
        let mut ifd0 = self.ifd0.clone();
        let exif = sorted(&self.exif);
        let gps = sorted(&self.gps);
        let mut ifd1 = self.ifd1.clone();

        // Thumbnail pointers are rewritten below, so drop any inherited values.
        ifd1.retain(|e| e.tag != TAG_THUMB_OFFSET && e.tag != TAG_THUMB_LENGTH);
        let thumb = self.thumbnail.clone();
        if thumb.is_some() {
            ifd1.push(Entry { tag: TAG_THUMB_OFFSET, typ: TYPE_LONG, count: 1, data: vec![0; 4] });
            ifd1.push(Entry { tag: TAG_THUMB_LENGTH, typ: TYPE_LONG, count: 1, data: vec![0; 4] });
        }
        if !exif.is_empty() {
            ifd0.push(Entry { tag: TAG_EXIF_IFD, typ: TYPE_LONG, count: 1, data: vec![0; 4] });
        }
        if !gps.is_empty() {
            ifd0.push(Entry { tag: TAG_GPS_IFD, typ: TYPE_LONG, count: 1, data: vec![0; 4] });
        }
        let ifd0 = sorted(&ifd0);
        let ifd1 = sorted(&ifd1);

        // Layout: header, IFD0, [ExifIFD], [GpsIFD], [IFD1], heap, [thumbnail].
        let block = |n: usize| 2 + n * 12 + 4;
        let ifd0_at = 8;
        let exif_at = ifd0_at + block(ifd0.len());
        let gps_at = exif_at + if exif.is_empty() { 0 } else { block(exif.len()) };
        let ifd1_at = gps_at + if gps.is_empty() { 0 } else { block(gps.len()) };
        let heap_at = ifd1_at + if ifd1.is_empty() { 0 } else { block(ifd1.len()) };

        let mut heap: Vec<u8> = Vec::new();
        let mut out = Vec::with_capacity(heap_at + 64);
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&(ifd0_at as u32).to_le_bytes());

        // Pass one with a provisional thumbnail offset, then patch it: the
        // thumbnail sits after the heap, whose size is only known once written.
        let mut thumb_patch: Option<usize> = None;
        let write_ifd = |out: &mut Vec<u8>, heap: &mut Vec<u8>, entries: &[Entry], next: u32, thumb_patch: &mut Option<usize>| {
            out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for e in entries {
                out.extend_from_slice(&e.tag.to_le_bytes());
                out.extend_from_slice(&e.typ.to_le_bytes());
                out.extend_from_slice(&e.count.to_le_bytes());
                let resolved: Option<u32> = match e.tag {
                    TAG_EXIF_IFD => Some(exif_at as u32),
                    TAG_GPS_IFD => Some(gps_at as u32),
                    TAG_THUMB_LENGTH => Some(thumb.as_ref().map_or(0, |t| t.len() as u32)),
                    _ => None,
                };
                if e.tag == TAG_THUMB_OFFSET {
                    *thumb_patch = Some(out.len());
                    out.extend_from_slice(&0u32.to_le_bytes());
                } else if let Some(v) = resolved {
                    out.extend_from_slice(&v.to_le_bytes());
                } else if e.data.len() <= 4 {
                    let mut v = e.data.clone();
                    v.resize(4, 0);
                    out.extend_from_slice(&v);
                } else {
                    // Heap values are 2-aligned, as every TIFF writer emits.
                    if heap.len() % 2 == 1 {
                        heap.push(0);
                    }
                    out.extend_from_slice(&((heap_at + heap.len()) as u32).to_le_bytes());
                    heap.extend_from_slice(&e.data);
                }
            }
            out.extend_from_slice(&next.to_le_bytes());
        };

        write_ifd(&mut out, &mut heap, &ifd0, if ifd1.is_empty() { 0 } else { ifd1_at as u32 }, &mut thumb_patch);
        if !exif.is_empty() {
            write_ifd(&mut out, &mut heap, &exif, 0, &mut thumb_patch);
        }
        if !gps.is_empty() {
            write_ifd(&mut out, &mut heap, &gps, 0, &mut thumb_patch);
        }
        if !ifd1.is_empty() {
            write_ifd(&mut out, &mut heap, &ifd1, 0, &mut thumb_patch);
        }
        debug_assert_eq!(out.len(), heap_at, "IFD block sizes must match the layout");
        out.extend_from_slice(&heap);
        if let (Some(at), Some(t)) = (thumb_patch, thumb.as_ref()) {
            let off = out.len() as u32;
            out[at..at + 4].copy_from_slice(&off.to_le_bytes());
            out.extend_from_slice(t);
        }
        out
    }
}

fn sorted(entries: &[Entry]) -> Vec<Entry> {
    let mut v = entries.to_vec();
    v.sort_by_key(|e| e.tag);
    v.dedup_by_key(|e| e.tag);
    v
}

fn read_ifd(
    tiff: &[u8],
    at: usize,
    le: bool,
    pointers: &mut Vec<(Space, usize)>,
    space: Space,
) -> Result<Vec<Entry>> {
    let bad = |d: &str| Error::malformed(FormatId::Jpeg, format!("EXIF: {d}"));
    let rd16 = |at: usize| -> Option<u16> {
        let b = [*tiff.get(at)?, *tiff.get(at + 1)?];
        Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let rd32 = |at: usize| -> Option<u32> {
        let b = [*tiff.get(at)?, *tiff.get(at + 1)?, *tiff.get(at + 2)?, *tiff.get(at + 3)?];
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let count = rd16(at).ok_or_else(|| bad("truncated sub-IFD"))? as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let e_at = at + 2 + i * 12;
        let (Some(tag), Some(typ), Some(cnt)) = (rd16(e_at), rd16(e_at + 2), rd32(e_at + 4)) else {
            break;
        };
        let len = type_size(typ).saturating_mul(cnt as usize);
        if len > tiff.len() {
            continue;
        }
        if space == Space::Exif && tag == TAG_INTEROP_IFD {
            if let Some(off) = rd32(e_at + 8) {
                pointers.push((Space::Interop, off as usize));
            }
            continue;
        }
        let data = if len <= 4 {
            tiff.get(e_at + 8..e_at + 8 + len).unwrap_or_default().to_vec()
        } else {
            let off = rd32(e_at + 8).unwrap_or(0) as usize;
            match tiff.get(off..off + len) {
                Some(v) => v.to_vec(),
                None => continue,
            }
        };
        entries.push(Entry { tag, typ, count: cnt, data });
    }
    Ok(entries)
}

/// What a tag is, in the user's terms.
pub struct TagInfo {
    pub class: Class,
    pub label: &'static str,
    /// Stripping it would change how the file renders.
    pub protected: bool,
}

/// Tags that describe how to render the image rather than who made it. Copied
/// through untouched and never reported, except [`TAG_ORIENTATION`], which is
/// reported as protected so the report can say why it stayed.
pub const RENDER_TAGS: &[u16] = &[
    0x0100, // ImageWidth
    0x0101, // ImageLength
    0x0102, // BitsPerSample
    0x0103, // Compression
    0x0106, // PhotometricInterpretation
    0x0112, // Orientation
    0x011A, // XResolution
    0x011B, // YResolution
    0x0128, // ResolutionUnit
    0x0213, // YCbCrPositioning
    0xA001, // ColorSpace
    0xA002, // PixelXDimension
    0xA003, // PixelYDimension
];

pub fn tag_info(space: Space, tag: u16) -> Option<TagInfo> {
    let info = |class: Class, label: &'static str| Some(TagInfo { class, label, protected: false });
    if space == Space::Gps && tag == 0x0000 {
        // Present in files that never had a fix, so it says something about the
        // writer rather than about a place. Calling it location would make a
        // report claim a position the file does not contain.
        return Some(TagInfo {
            class: Class::Device,
            label: "GPS tag version, which is written even when no position was recorded",
            protected: false,
        });
    }
    if space == Space::Gps {
        // Every GPS tag is location, including the ones that only describe the
        // fix, so the mapping is by IFD rather than by tag. The reference tags
        // are named for what they are: on their own a bare "N" is not a
        // coordinate, and labelling it "GPS latitude" would read as a duplicate
        // of the coordinate it belongs to.
        return Some(TagInfo {
            class: Class::Location,
            label: match tag {
                0x0001 => "hemisphere for the latitude",
                0x0002 => "GPS latitude",
                0x0003 => "hemisphere for the longitude",
                0x0004 => "GPS longitude",
                0x0005 => "whether the altitude is above or below sea level",
                0x0006 => "GPS altitude",
                0x0007 => "time of the GPS fix",
                0x0008 => "GPS satellites used",
                0x0009 => "GPS receiver status",
                0x000A => "GPS measurement mode",
                0x000B => "GPS accuracy",
                0x000C | 0x000D => "speed over ground",
                0x000E | 0x000F => "direction of travel",
                0x0010 | 0x0011 => "direction the camera was pointing",
                0x0012 => "map datum",
                0x001D => "date of the GPS fix",
                _ => "GPS data",
            },
            protected: false,
        });
    }
    if space == Space::Ifd1 {
        return Some(TagInfo {
            class: Class::HiddenPayload,
            label: "embedded preview image",
            protected: false,
        });
    }
    if tag == TAG_ORIENTATION {
        return Some(TagInfo {
            class: Class::Device,
            label: "orientation, which decides which way up the image is shown",
            protected: true,
        });
    }
    if RENDER_TAGS.contains(&tag) {
        return None;
    }
    match (space, tag) {
        (Space::Ifd0, 0x010F) => info(Class::Device, "camera or scanner maker"),
        (Space::Ifd0, 0x0110) => info(Class::Device, "camera or scanner model"),
        (Space::Ifd0, 0x0131) => info(Class::Device, "software and OS version"),
        (Space::Ifd0, 0x013B) => info(Class::Identity, "author"),
        (Space::Ifd0, 0x8298) => info(Class::Identity, "copyright notice"),
        (Space::Ifd0, 0x010E) => info(Class::Identity, "image description"),
        (Space::Ifd0, 0x0132) => info(Class::Time, "file change date"),
        (Space::Ifd0, TAG_XMP) => info(Class::Correlation, "embedded XMP metadata packet"),
        (Space::Ifd0, TAG_IPTC) => info(Class::Identity, "embedded IPTC record"),
        (Space::Ifd0, 0x8773) => Some(TagInfo {
            class: Class::Device,
            label: "ICC colour profile",
            protected: true,
        }),
        (Space::Exif, 0x9003) => info(Class::Time, "date and time the picture was taken"),
        (Space::Exif, 0x9004) => info(Class::Time, "date and time it was digitised"),
        (Space::Exif, 0x9290..=0x9292) => info(Class::Time, "sub-second precision on the capture time"),
        (Space::Exif, 0x9010..=0x9012) => {
            info(Class::Location, "time zone offset, which narrows where you were")
        }
        (Space::Exif, 0x927C) => info(Class::Device, "maker notes, an opaque block that usually repeats the serial number"),
        (Space::Exif, 0xA420) => info(Class::Correlation, "unique image ID, which links copies of this picture"),
        (Space::Exif, 0xA430) => info(Class::Identity, "camera owner"),
        (Space::Exif, 0xA431) => info(Class::Device, "camera body serial number"),
        (Space::Exif, 0xA433) => info(Class::Device, "lens maker"),
        (Space::Exif, 0xA434) => info(Class::Device, "lens model"),
        (Space::Exif, 0xA435) => info(Class::Device, "lens serial number"),
        (Space::Exif, 0x9286) => info(Class::Identity, "user comment"),
        (Space::Exif, 0xA004) => info(Class::Correlation, "related sound file"),
        // Exposure settings are not identifying on their own, but together they
        // narrow the model, so they belong to the device class.
        (Space::Exif, 0x829A | 0x829D | 0x8822 | 0x8827 | 0x9201 | 0x9202 | 0x9204 | 0x9207 | 0x9209 | 0x920A | 0xA402 | 0xA403 | 0xA406) => {
            info(Class::Device, "camera settings, which narrow down the model")
        }
        (Space::Interop, _) => info(Class::Device, "interoperability declaration"),
        _ => None,
    }
}

/// How one entry should read in a report.
pub fn display_for(exif: &Exif, space: Space, e: &Entry) -> String {
    match (space, e.tag) {
        (Space::Gps, 0x0002 | 0x0004) => e.display_dms(exif.le, gps_ref(exif, e.tag).as_deref()),
        (Space::Gps, 0x0007) if e.count >= 3 => {
            let parts: Vec<String> = e.display(exif.le).split(", ").map(|s| s.to_string()).collect();
            if parts.len() >= 3 {
                format!(
                    "{:0>2}:{:0>2}:{:0>2} UTC",
                    parts[0], parts[1], parts[2]
                )
            } else {
                e.display(exif.le)
            }
        }
        _ => e.display(exif.le),
    }
}

/// The reference letter that goes with a GPS coordinate, for display.
pub fn gps_ref(exif: &Exif, tag: u16) -> Option<String> {
    let r = match tag {
        0x0002 => 0x0001,
        0x0004 => 0x0003,
        _ => return None,
    };
    exif.get(Space::Gps, r).map(|e| e.display(exif.le))
}

/// Impact of removing an entry: the thumbnail is content, everything else is
/// metadata.
pub fn impact(space: Space) -> Impact {
    if space == Space::Ifd1 {
        Impact::Content
    } else {
        Impact::Metadata
    }
}

/// Report every EXIF entry worth naming, in every IFD.
pub fn inspect_entries(exif: &Exif, ins: &mut Inspection) {
    if !exif.gps.is_empty() {
        ins.mark("exif:gps");
    }
    if exif.get(Space::Exif, 0x927C).is_some() {
        ins.mark("exif:maker-note");
    }
    if exif.thumbnail.is_some() {
        ins.mark("exif:thumbnail");
    }
    // Two shapes worth naming: a block holding nothing but rendering hints,
    // and one holding nothing but those and a date. Both are common; anything
    // more is not.
    let mut has_time = false;
    let mut has_other = !exif.gps.is_empty() || !exif.interop.is_empty() || exif.thumbnail.is_some();
    for space in [Space::Ifd0, Space::Exif] {
        for e in exif.space(space) {
            if RENDER_TAGS.contains(&e.tag) {
                continue;
            }
            match tag_info(space, e.tag) {
                Some(i) if i.protected => {}
                Some(i) if i.class == Class::Time => has_time = true,
                _ => has_other = true,
            }
        }
    }
    if !has_other {
        ins.mark(if has_time { "exif:datetime-only" } else { "exif:render-only" });
    }

    for space in [Space::Ifd0, Space::Exif, Space::Gps, Space::Interop] {
        for e in exif.space(space) {
            let Some(info) = tag_info(space, e.tag) else { continue };
            let value = display_for(exif, space, e);
            let mut f = Finding::new(
                info.class,
                format!("exif:{}:{:#06x}", space.label(), e.tag),
                info.label,
            )
            .impact(impact(space));
            if !value.is_empty() {
                f = f.value(value);
            }
            if info.protected {
                f = f.protected().note("kept on purpose: removing it would change how the file looks");
            }
            ins.findings.push(f);
        }
    }
    if let Some(t) = &exif.thumbnail {
        // Scale the language to the payload. A preview big enough to hold a
        // picture is the serious finding, because it is not regenerated when the
        // picture is cropped. A four-byte stub is just a field to remove, and
        // calling it a recoverable original would be alarming and wrong.
        let real = t.starts_with(&[0xFF, 0xD8]) && t.len() >= THUMBNAIL_FLOOR;
        let mut f = Finding::new(
            Class::HiddenPayload,
            "exif:ifd1:thumbnail",
            if real { "embedded preview image" } else { "an empty preview entry" },
        )
        .value(format!("{} bytes", t.len()))
        .impact(if real { Impact::Content } else { Impact::Metadata });
        if real {
            f = f.note(
                "A preview is stored separately from the picture and is not updated when the \
                 picture is cropped or edited, so it can still show what was removed.",
            );
        }
        ins.findings.push(f);
    }
}

/// Decide the fate of one EXIF entry under a profile.
enum Fate {
    Keep,
    Drop,
    Retime(TimeOutcome),
}

fn fate(space: Space, e: &Entry, profile: &Profile) -> Fate {
    if RENDER_TAGS.contains(&e.tag) && space != Space::Ifd1 {
        return Fate::Keep;
    }
    let Some(info) = tag_info(space, e.tag) else {
        // Unrecognised tags are treated as device fingerprint: an unknown field
        // in a camera's EXIF is far more likely to identify the camera than to
        // be needed by a reader.
        return if profile.alters(Class::Device) { Fate::Drop } else { Fate::Keep };
    };
    if info.protected {
        return Fate::Keep;
    }
    if info.class == Class::Time {
        return match profile.action(Class::Time) {
            Action::Keep => Fate::Keep,
            _ => Fate::Retime(retime(profile, &e.display(true))),
        };
    }
    if profile.alters(info.class) {
        Fate::Drop
    } else {
        Fate::Keep
    }
}

pub fn clean_entries(exif: &Exif, profile: &Profile, changes: &mut Vec<Change>) -> Option<Exif> {
    let mut out = Exif { le: true, ..Default::default() };
    for space in [Space::Ifd0, Space::Exif, Space::Gps, Space::Interop] {
        for e in exif.space(space) {
            let locator = format!("exif:{}:{:#06x}", space.label(), e.tag);
            let info = tag_info(space, e.tag);
            let label = info.as_ref().map_or("unrecognised field", |i| i.label).to_string();
            let class = info.as_ref().map_or(Class::Device, |i| i.class);
            let shown = display_for(exif, space, e);
            let mut keep = |entry: Entry| match space {
                Space::Ifd0 => out.ifd0.push(entry),
                Space::Exif => out.exif.push(entry),
                Space::Gps => out.gps.push(entry),
                Space::Interop => out.interop.push(entry),
                Space::Ifd1 => {}
            };
            match fate(space, e, profile) {
                Fate::Keep => {
                    // Entries are re-emitted little-endian, so big-endian
                    // multi-byte values have to be byte-swapped on the way out.
                    keep(if exif.le { e.clone() } else { swap_endian(e) });
                }
                Fate::Drop => changes.push(Change::removed(class, locator, label, Some(shown))),
                Fate::Retime(outcome) => match outcome.exif(&shown) {
                    Some(v) if v != shown => {
                        keep(Entry::ascii(e.tag, &v));
                        changes.push(Change::rewritten(
                            Class::Time,
                            locator,
                            label,
                            profile.action(Class::Time),
                            Some(shown),
                            Some(v),
                        ));
                    }
                    Some(_) => keep(if exif.le { e.clone() } else { swap_endian(e) }),
                    None => changes.push(Change::removed(Class::Time, locator, label, Some(shown))),
                },
            }
        }
    }
    if exif.thumbnail.is_some() {
        if profile.alters(Class::HiddenPayload) {
            let t = exif.thumbnail.as_ref();
            let real = t.is_some_and(|t| t.starts_with(&[0xFF, 0xD8]) && t.len() >= THUMBNAIL_FLOOR);
            changes.push(Change::removed(
                Class::HiddenPayload,
                "exif:ifd1:thumbnail",
                if real { "embedded preview image" } else { "an empty preview entry" },
                t.map(|t| format!("{} bytes", t.len())),
            ));
        } else {
            out.thumbnail = exif.thumbnail.clone();
            out.ifd1 = exif.ifd1.clone();
        }
    }
    (!out.is_empty()).then_some(out)
}

fn swap_endian(e: &Entry) -> Entry {
    let width = match e.typ {
        TYPE_SHORT => 2,
        TYPE_LONG | TYPE_SLONG => 4,
        TYPE_RATIONAL | TYPE_SRATIONAL => 4,
        _ => return e.clone(),
    };
    let mut data = e.data.clone();
    for chunk in data.chunks_mut(width) {
        chunk.reverse();
    }
    Entry { data, ..e.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_a_block_it_can_reparse() {
        let mut e = Exif { le: true, ..Default::default() };
        e.ifd0.push(Entry::ascii(0x010F, "Apple"));
        e.ifd0.push(Entry::short(TAG_ORIENTATION, 6));
        e.exif.push(Entry::ascii(0x9003, "2024:06:14 19:42:07"));
        e.gps.push(Entry::ascii(0x001D, "2024:06:14"));
        e.thumbnail = Some(vec![0xAA; 16]);
        e.ifd1.push(Entry::short(0x0103, 6));

        let back = Exif::parse(&e.to_tiff()).unwrap();
        assert_eq!(back.get(Space::Ifd0, 0x010F).unwrap().display(true), "Apple");
        assert_eq!(back.get(Space::Exif, 0x9003).unwrap().display(true), "2024:06:14 19:42:07");
        assert_eq!(back.get(Space::Gps, 0x001D).unwrap().display(true), "2024:06:14");
        assert_eq!(back.thumbnail.as_deref(), Some(&[0xAA; 16][..]));
    }

    #[test]
    fn an_empty_exif_block_still_parses() {
        let e = Exif { le: true, ..Default::default() };
        assert!(Exif::parse(&e.to_tiff()).unwrap().is_empty());
    }

    #[test]
    fn rejects_a_block_with_no_byte_order_mark() {
        assert!(Exif::parse(b"not a tiff at all").is_err());
    }

    #[test]
    fn render_tags_are_not_reported() {
        assert!(tag_info(Space::Ifd0, 0xA002).is_none());
        assert!(tag_info(Space::Ifd0, TAG_ORIENTATION).unwrap().protected);
    }

    #[test]
    fn every_gps_tag_maps_to_location() {
        for tag in [0x0001u16, 0x0005, 0x001D, 0x00FF] {
            assert_eq!(tag_info(Space::Gps, tag).unwrap().class, Class::Location);
        }
    }
}
