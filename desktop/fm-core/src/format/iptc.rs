//! The Photoshop image resource block, and the IPTC IIM record inside it.
//!
//! Unlike XMP, an IIM record is rebuilt dataset by dataset rather than dropped
//! whole, because a photo agency's caption and credit are often the reason the
//! file exists. The block only disappears when nothing survives in it.

use super::Change;
use crate::model::{Class, Finding, Impact, Inspection};
use crate::profile::{self, Profile, TimeOutcome};

const RES_IPTC: u16 = 0x0404;
const RES_THUMBNAIL: u16 = 0x040C;
const RES_CAPTION_DIGEST: u16 = 0x0425;
const RES_XMP: u16 = 0x0424;
const RES_URL: u16 = 0x040B;
const RES_PRINT_FLAGS: u16 = 0x2710;

struct Resource {
    id: u16,
    name: Vec<u8>,
    data: Vec<u8>,
}

fn parse_resources(irb: &[u8]) -> Vec<Resource> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 12 <= irb.len() {
        if &irb[at..at + 4] != b"8BIM" {
            break;
        }
        let id = u16::from_be_bytes([irb[at + 4], irb[at + 5]]);
        // Pascal-style name, padded so the whole field is even-length.
        let name_len = irb[at + 6] as usize;
        let name_field = 1 + name_len + usize::from(name_len.is_multiple_of(2));
        let name = irb[at + 7..(at + 7 + name_len).min(irb.len())].to_vec();
        let size_at = at + 6 + name_field;
        if size_at + 4 > irb.len() {
            break;
        }
        let size = u32::from_be_bytes([
            irb[size_at],
            irb[size_at + 1],
            irb[size_at + 2],
            irb[size_at + 3],
        ]) as usize;
        let data_at = size_at + 4;
        let Some(data) = irb.get(data_at..data_at + size) else { break };
        out.push(Resource { id, name, data: data.to_vec() });
        at = data_at + size + (size % 2);
    }
    out
}

fn write_resources(resources: &[Resource]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in resources {
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(&r.id.to_be_bytes());
        out.push(r.name.len() as u8);
        out.extend_from_slice(&r.name);
        if r.name.len() % 2 == 0 {
            out.push(0);
        }
        out.extend_from_slice(&(r.data.len() as u32).to_be_bytes());
        out.extend_from_slice(&r.data);
        if r.data.len() % 2 == 1 {
            out.push(0);
        }
    }
    out
}

struct Dataset {
    record: u8,
    dataset: u8,
    data: Vec<u8>,
}

fn parse_datasets(iim: &[u8]) -> Vec<Dataset> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 5 <= iim.len() {
        if iim[at] != 0x1C {
            at += 1;
            continue;
        }
        let record = iim[at + 1];
        let dataset = iim[at + 2];
        let raw = u16::from_be_bytes([iim[at + 3], iim[at + 4]]);
        // The high bit marks an extended-length field, whose length is itself
        // variable. Those only carry bulk data, never a name or a place.
        if raw & 0x8000 != 0 {
            break;
        }
        let len = raw as usize;
        let Some(data) = iim.get(at + 5..at + 5 + len) else { break };
        out.push(Dataset { record, dataset, data: data.to_vec() });
        at += 5 + len;
    }
    out
}

fn write_datasets(datasets: &[Dataset]) -> Vec<u8> {
    let mut out = Vec::new();
    for d in datasets {
        out.push(0x1C);
        out.push(d.record);
        out.push(d.dataset);
        out.extend_from_slice(&(d.data.len() as u16).to_be_bytes());
        out.extend_from_slice(&d.data);
    }
    out
}

/// Record 2 is the editorial record; the rest describe the picture's encoding.
fn dataset_info(record: u8, dataset: u8) -> Option<(Class, &'static str)> {
    if record != 2 {
        return None;
    }
    let i = |c: Class, l: &'static str| Some((c, l));
    match dataset {
        5 => i(Class::Identity, "object name"),
        25 => i(Class::Identity, "keywords"),
        40 => i(Class::Identity, "special instructions"),
        55 => i(Class::Time, "date created"),
        60 => i(Class::Time, "time created"),
        62 => i(Class::Time, "digital creation date"),
        63 => i(Class::Time, "digital creation time"),
        80 => i(Class::Identity, "by-line, the photographer's name"),
        85 => i(Class::Identity, "by-line title"),
        90 => i(Class::Location, "city"),
        92 => i(Class::Location, "sub-location"),
        95 => i(Class::Location, "province or state"),
        100 => i(Class::Location, "country code"),
        101 => i(Class::Location, "country"),
        103 => i(Class::Correlation, "original transmission reference, an agency job ID"),
        105 => i(Class::Identity, "headline"),
        110 => i(Class::Identity, "credit"),
        115 => i(Class::Identity, "source"),
        116 => i(Class::Identity, "copyright notice"),
        118 => i(Class::Identity, "contact"),
        120 => i(Class::Identity, "caption"),
        122 => i(Class::Identity, "caption writer"),
        _ => None,
    }
}

fn resource_info(id: u16) -> Option<(Class, &'static str, Impact)> {
    match id {
        RES_THUMBNAIL => Some((
            Class::HiddenPayload,
            "Photoshop preview image, which can still show what was cropped away",
            Impact::Content,
        )),
        RES_CAPTION_DIGEST => Some((
            Class::Correlation,
            "caption digest, which links this file to its original",
            Impact::Metadata,
        )),
        RES_URL => Some((Class::Identity, "associated URL", Impact::Metadata)),
        RES_PRINT_FLAGS => Some((
            Class::StructuralFingerprint,
            "Photoshop print flags, which identify the editor",
            Impact::Metadata,
        )),
        RES_XMP => Some((Class::Correlation, "XMP packet inside the resource block", Impact::Metadata)),
        _ => None,
    }
}

/// IPTC is ISO-8859-1 unless 1:90 declares otherwise, so a byte at a time is
/// the specified reading and not a fallback.
fn text(data: &[u8]) -> String {
    crate::text::trimmed(data).0
}

pub fn inspect(irb: &[u8], ins: &mut Inspection) {
    for r in parse_resources(irb) {
        if r.id == RES_IPTC {
            for d in parse_datasets(&r.data) {
                let Some((class, label)) = dataset_info(d.record, d.dataset) else { continue };
                ins.findings.push(
                    Finding::new(class, format!("iptc:{}:{}", d.record, d.dataset), label)
                        .value(text(&d.data)),
                );
            }
            continue;
        }
        if let Some((class, label, impact)) = resource_info(r.id) {
            ins.findings.push(
                Finding::new(class, format!("irb:{:#06x}", r.id), label)
                    .value(format!("{} bytes", r.data.len()))
                    .impact(impact),
            );
        }
    }
}

/// Rebuild the resource block, or `None` if nothing is left to carry.
pub fn clean(irb: &[u8], profile: &Profile, changes: &mut Vec<Change>) -> Option<Vec<u8>> {
    let mut kept_resources = Vec::new();
    for r in parse_resources(irb) {
        if r.id == RES_IPTC {
            let mut kept = Vec::new();
            for d in parse_datasets(&r.data) {
                let locator = format!("iptc:{}:{}", d.record, d.dataset);
                let Some((class, label)) = dataset_info(d.record, d.dataset) else {
                    kept.push(d);
                    continue;
                };
                let shown = text(&d.data);
                if class == Class::Time {
                    match profile.action(Class::Time) {
                        crate::model::Action::Keep => kept.push(d),
                        _ => match profile::retime(profile, &shown) {
                            // IIM splits date and time into separate datasets,
                            // so a coarsened time is zero and a coarsened date
                            // keeps the day. Both are written in IIM's own
                            // spelling: CCYYMMDD and HHMMSS.
                            TimeOutcome::Set(c) => {
                                let replacement = if d.dataset == 55 || d.dataset == 62 {
                                    format!("{:04}{:02}{:02}", c.year, c.month, c.day)
                                } else {
                                    format!("{:02}{:02}{:02}", c.hour, c.minute, c.second)
                                };
                                changes.push(Change::rewritten(
                                    class,
                                    locator,
                                    label,
                                    profile.action(Class::Time),
                                    Some(shown),
                                    Some(replacement.clone()),
                                ));
                                kept.push(Dataset { data: replacement.into_bytes(), ..d });
                            }
                            _ => changes.push(Change::removed(class, locator, label, Some(shown))),
                        },
                    }
                } else if profile.alters(class) {
                    changes.push(Change::removed(class, locator, label, Some(shown)));
                } else {
                    kept.push(d);
                }
            }
            if kept.iter().any(|d| d.record == 2) {
                kept_resources.push(Resource { id: r.id, name: r.name, data: write_datasets(&kept) });
            }
            continue;
        }
        match resource_info(r.id) {
            Some((class, label, _)) if profile.alters(class) => {
                changes.push(Change::removed(
                    class,
                    format!("irb:{:#06x}", r.id),
                    label,
                    Some(format!("{} bytes", r.data.len())),
                ));
            }
            _ => kept_resources.push(r),
        }
    }
    (!kept_resources.is_empty()).then(|| write_resources(&kept_resources))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> Vec<u8> {
        let iim = write_datasets(&[
            Dataset { record: 2, dataset: 80, data: b"Marie Dvorakova".to_vec() },
            Dataset { record: 2, dataset: 90, data: b"Praha".to_vec() },
            Dataset { record: 2, dataset: 55, data: b"20240614".to_vec() },
        ]);
        write_resources(&[
            Resource { id: RES_IPTC, name: Vec::new(), data: iim },
            Resource { id: RES_THUMBNAIL, name: Vec::new(), data: vec![0xAB; 9] },
        ])
    }

    #[test]
    fn round_trips_resources_including_odd_lengths() {
        let parsed = parse_resources(&block());
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].data.len(), 9);
        assert_eq!(write_resources(&parsed), block());
    }

    #[test]
    fn standard_removes_the_name_and_the_place_but_keeps_the_date() {
        let mut changes = Vec::new();
        let out = clean(&block(), &Profile::standard(), &mut changes).unwrap();
        let left = parse_resources(&out);
        assert_eq!(left.len(), 1, "the preview resource goes with hidden payload");
        let datasets = parse_datasets(&left[0].data);
        assert_eq!(datasets.len(), 1);
        assert_eq!(datasets[0].dataset, 55);
        assert!(changes.iter().any(|c| c.class == Class::Location));
    }

    #[test]
    fn strict_coarsens_the_iim_date_in_iim_spelling() {
        let mut changes = Vec::new();
        let out = clean(&block(), &Profile::strict(), &mut changes).unwrap();
        let datasets = parse_datasets(&parse_resources(&out)[0].data);
        assert_eq!(text(&datasets[0].data), "20240614");
    }

    #[test]
    fn uniform_leaves_nothing_and_drops_the_block() {
        let mut p = Profile::uniform();
        p.actions.insert(Class::Time, crate::model::Action::Remove);
        let mut changes = Vec::new();
        assert!(clean(&block(), &p, &mut changes).is_none());
    }

    #[test]
    fn inspect_reports_every_dataset() {
        let mut ins = Inspection::new(crate::model::FormatId::Jpeg, 0);
        inspect(&block(), &mut ins);
        assert_eq!(ins.findings.len(), 4);
    }
}
