//! Just enough ZIP to rebuild an OPC package deliberately.
//!
//! A general-purpose ZIP crate would do the reading, but not the writing: the
//! point here is to control member order, per-member timestamps and the
//! compression choice, because those three are the structural fingerprint that
//! identifies the writing software. So the container is rebuilt by hand.

use crate::error::{Error, Result};
use crate::model::FormatId;
use crate::timeutil::Civil;
use std::io::{Read, Write};

const LOCAL_SIG: u32 = 0x0403_4B50;
const CENTRAL_SIG: u32 = 0x0201_4B50;
const EOCD_SIG: u32 = 0x0605_4B50;

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    /// Uncompressed contents.
    pub data: Vec<u8>,
    pub date: u16,
    pub time: u16,
    /// Stored rather than deflated. Preserved on round-trip because switching
    /// a member's method is itself a change of shape.
    pub stored: bool,
    pub external_attrs: u32,
}

impl Member {
    pub fn new(name: impl Into<String>, data: Vec<u8>) -> Member {
        Member {
            name: name.into(),
            data,
            date: 0x0021,
            time: 0,
            stored: false,
            external_attrs: 0,
        }
    }

    pub fn stamp(&mut self, when: Civil) {
        let (date, time) = when.to_dos();
        self.date = date;
        self.time = time;
    }

    pub fn timestamp(&self) -> Civil {
        Civil::from_dos(self.date, self.time)
    }
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(at)?, *d.get(at + 1)?]))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *d.get(at)?,
        *d.get(at + 1)?,
        *d.get(at + 2)?,
        *d.get(at + 3)?,
    ]))
}

/// Read a package, keeping the central directory's order: that order is the
/// one a reader sees, and the one that identifies the writer.
pub fn read(data: &[u8]) -> Result<Vec<Member>> {
    let bad = |d: &str| Error::malformed(FormatId::Ooxml, format!("ZIP: {d}"));
    // The end-of-central-directory record is at the end, possibly behind a
    // comment, so it has to be searched for backwards.
    let eocd = (0..data.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(data, i) == Some(EOCD_SIG))
        .ok_or_else(|| bad("no end-of-central-directory record"))?;
    let count = u16_at(data, eocd + 10).ok_or_else(|| bad("truncated record count"))? as usize;
    let mut at = u32_at(data, eocd + 16).ok_or_else(|| bad("truncated directory offset"))? as usize;

    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(data, at) != Some(CENTRAL_SIG) {
            return Err(bad("central directory entry out of place"));
        }
        let method = u16_at(data, at + 10).ok_or_else(|| bad("truncated method"))?;
        let time = u16_at(data, at + 12).unwrap_or(0);
        let date = u16_at(data, at + 14).unwrap_or(0);
        let compressed = u32_at(data, at + 20).ok_or_else(|| bad("truncated size"))? as usize;
        let name_len = u16_at(data, at + 28).unwrap_or(0) as usize;
        let extra_len = u16_at(data, at + 30).unwrap_or(0) as usize;
        let comment_len = u16_at(data, at + 32).unwrap_or(0) as usize;
        let external_attrs = u32_at(data, at + 38).unwrap_or(0);
        let local_at = u32_at(data, at + 42).ok_or_else(|| bad("truncated local offset"))? as usize;
        let name = String::from_utf8_lossy(
            data.get(at + 46..at + 46 + name_len).ok_or_else(|| bad("truncated name"))?,
        )
        .to_string();

        if u32_at(data, local_at) != Some(LOCAL_SIG) {
            return Err(bad("local header out of place"));
        }
        let l_name = u16_at(data, local_at + 26).unwrap_or(0) as usize;
        let l_extra = u16_at(data, local_at + 28).unwrap_or(0) as usize;
        let body_at = local_at + 30 + l_name + l_extra;
        let body = data
            .get(body_at..body_at + compressed)
            .ok_or_else(|| bad("member body runs past end of file"))?;

        let plain = match method {
            0 => body.to_vec(),
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(body)
                    .read_to_end(&mut v)
                    .map_err(|e| bad(&format!("member {name} will not inflate: {e}")))?;
                v
            }
            m => return Err(bad(&format!("member {name} uses compression method {m}"))),
        };
        out.push(Member {
            name,
            data: plain,
            date,
            time,
            stored: method == 0,
            external_attrs,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// Write a package in exactly the order given.
///
/// No extra fields and no data descriptors, so the output's shape is decided
/// entirely by the member list the caller built.
pub fn write(members: &[Member]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();

    for m in members {
        let (method, body) = if m.stored {
            (0u16, m.data.clone())
        } else {
            let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
            let _ = enc.write_all(&m.data);
            (8u16, enc.finish().unwrap_or_else(|_| m.data.clone()))
        };
        let crc = crc32(&m.data);
        let local_at = out.len() as u32;

        out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&m.time.to_le_bytes());
        out.extend_from_slice(&m.date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(m.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(m.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(m.name.as_bytes());
        out.extend_from_slice(&body);

        central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&m.time.to_le_bytes());
        central.extend_from_slice(&m.date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(body.len() as u32).to_le_bytes());
        central.extend_from_slice(&(m.data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(m.name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&m.external_attrs.to_le_bytes());
        central.extend_from_slice(&local_at.to_le_bytes());
        central.extend_from_slice(m.name.as_bytes());
    }

    let central_at = out.len() as u32;
    let central_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&EOCD_SIG.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with directory
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_len.to_le_bytes());
    out.extend_from_slice(&central_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

/// The ZIP CRC-32, the same polynomial PNG uses.
fn crc32(bytes: &[u8]) -> u32 {
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

/// The canonical member order: the content-type map first, because a streaming
/// OPC reader needs it before anything else, then everything else by name.
///
/// Any fixed order would do for anonymity. This one is also valid, which a
/// purely alphabetical order would not reliably be.
pub fn canonical_order(members: &mut [Member]) {
    members.sort_by(|a, b| {
        let rank = |m: &Member| if m.name == "[Content_Types].xml" { 0 } else { 1 };
        rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name))
    });
}

pub fn is_canonical(members: &[Member]) -> bool {
    let mut sorted = members.to_vec();
    canonical_order(&mut sorted);
    sorted.iter().map(|m| &m.name).eq(members.iter().map(|m| &m.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeutil::EPOCH_1980;

    fn package() -> Vec<Member> {
        vec![
            Member::new("[Content_Types].xml", b"<Types/>".to_vec()),
            Member::new("word/document.xml", b"<w:document>hello</w:document>".to_vec()),
            Member {
                stored: true,
                ..Member::new("docProps/thumbnail.jpeg", vec![0xFF, 0xD8, 0xFF, 0xD9])
            },
        ]
    }

    #[test]
    fn round_trips_names_data_and_methods() {
        let written = write(&package());
        let back = read(&written).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[1].name, "word/document.xml");
        assert_eq!(back[1].data, b"<w:document>hello</w:document>");
        assert!(back[2].stored, "a stored member must not come back deflated");
    }

    #[test]
    fn preserves_the_order_it_was_given() {
        let mut members = package();
        members.reverse();
        let back = read(&write(&members)).unwrap();
        assert_eq!(back[0].name, "docProps/thumbnail.jpeg");
        assert!(!is_canonical(&back));
    }

    #[test]
    fn canonical_order_puts_the_content_types_first() {
        let mut members = package();
        members.reverse();
        canonical_order(&mut members);
        assert_eq!(members[0].name, "[Content_Types].xml");
        assert_eq!(members[1].name, "docProps/thumbnail.jpeg");
        assert!(is_canonical(&members));
    }

    #[test]
    fn timestamps_survive_a_round_trip_at_dos_resolution() {
        let mut m = Member::new("a.xml", b"x".to_vec());
        m.stamp(Civil { year: 2019, month: 3, day: 14, hour: 9, minute: 26, second: 53 });
        let back = read(&write(&[m])).unwrap();
        // DOS time halves seconds, so an odd second is unrepresentable.
        assert_eq!(back[0].timestamp().second, 52);
        assert_eq!(back[0].timestamp().day, 14);
    }

    #[test]
    fn the_1980_constant_is_what_an_unstamped_member_gets() {
        let back = read(&write(&[Member::new("a.xml", b"x".to_vec())])).unwrap();
        assert_eq!(back[0].timestamp(), EPOCH_1980);
    }

    #[test]
    fn rejects_something_that_is_not_a_zip() {
        assert!(read(b"not a zip, not even close").is_err());
    }
}
