//! XMP packets, which turn up embedded in JPEG, PNG, PDF, SVG and OOXML.
//!
//! Packets are reported field by field but cleaned whole: a packet with the
//! names filed off but the structure intact is a rarer shape than no packet at
//! all, and the fields it legitimately carries are duplicated in the host
//! format anyway. So the rule is that an XMP packet survives only if every
//! class it contains is being kept.

use super::Change;
use crate::model::{Class, Finding, Inspection};
use crate::profile::Profile;
use quick_xml::events::Event;
use quick_xml::Reader;

/// What an XMP property name means. Matched on the `prefix:Local` form, which
/// is what files actually carry even though the prefix is nominally arbitrary.
fn classify(qname: &str) -> Option<(Class, &'static str)> {
    let (prefix, local) = qname.split_once(':')?;
    let by_local = |c: Class, l: &'static str| Some((c, l));
    match (prefix, local) {
        ("dc", "creator") => by_local(Class::Identity, "author"),
        ("dc", "rights") => by_local(Class::Identity, "rights statement"),
        ("dc", "title") => by_local(Class::Identity, "title"),
        ("dc", "description") => by_local(Class::Identity, "description"),
        ("dc", "subject") => by_local(Class::Identity, "keywords"),
        ("dc", "publisher" | "contributor") => by_local(Class::Identity, "publisher or contributor"),
        ("dc", "date") => by_local(Class::Time, "date"),
        ("xmp", "CreatorTool") => by_local(Class::Device, "software that wrote the file"),
        ("xmp", "CreateDate") => by_local(Class::Time, "creation date"),
        ("xmp", "ModifyDate") => by_local(Class::Time, "modification date"),
        ("xmp", "MetadataDate") => by_local(Class::Time, "date the metadata was last touched"),
        ("xmpMM", "DocumentID") => by_local(Class::Correlation, "document ID, shared by every copy and edit of this file"),
        ("xmpMM", "InstanceID") => by_local(Class::Correlation, "instance ID, unique to this save"),
        ("xmpMM", "OriginalDocumentID") => by_local(Class::Correlation, "ID of the file this was derived from"),
        ("xmpMM", "DerivedFrom") => by_local(Class::Correlation, "reference to the file this came from"),
        ("xmpMM", "History") => by_local(Class::History, "edit history"),
        ("xmpMM", "Ingredients" | "Pantry") => by_local(Class::History, "list of files that went into this one"),
        ("tiff", "Make") => by_local(Class::Device, "camera maker"),
        ("tiff", "Model") => by_local(Class::Device, "camera model"),
        ("tiff", "Software") => by_local(Class::Device, "software version"),
        ("tiff", "DateTime") => by_local(Class::Time, "file change date"),
        ("tiff", "Artist") => by_local(Class::Identity, "author"),
        ("exif", l) if l.starts_with("GPS") => by_local(Class::Location, "GPS position"),
        ("exif", "DateTimeOriginal" | "DateTimeDigitized") => by_local(Class::Time, "capture date"),
        ("exif", "BodySerialNumber" | "LensSerialNumber") => by_local(Class::Device, "device serial number"),
        ("exifEX", "BodySerialNumber" | "LensSerialNumber") => by_local(Class::Device, "device serial number"),
        ("photoshop", "City") => by_local(Class::Location, "city"),
        ("photoshop", "State" | "Country") => by_local(Class::Location, "region or country"),
        ("photoshop", "Credit" | "Source" | "AuthorsPosition") => by_local(Class::Identity, "credit line"),
        ("photoshop", "DateCreated") => by_local(Class::Time, "creation date"),
        ("Iptc4xmpCore", "Location") => by_local(Class::Location, "named place"),
        ("Iptc4xmpCore", "CreatorContactInfo") => by_local(Class::Identity, "contact details"),
        ("pdf", "Producer") => by_local(Class::Device, "PDF producer"),
        ("pdf", "Keywords") => by_local(Class::Identity, "keywords"),
        ("xmpRights", _) => by_local(Class::Identity, "rights statement"),
        ("stEvt" | "stRef", _) => by_local(Class::History, "edit history entry"),
        ("CreatorTool", _) => by_local(Class::Device, "software that wrote the file"),
        _ => None,
    }
}

/// Every classifiable property in a packet, as (class, label, qname, value).
pub fn properties(xml: &[u8]) -> Vec<(Class, &'static str, String, Option<String>)> {
    let mut found: Vec<(Class, &'static str, String, Option<String>)> = Vec::new();
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut pending: Option<(Class, &'static str, String)> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                for attr in e.attributes().flatten() {
                    let qname = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                    if let Some((class, label)) = classify(&qname) {
                        let value = String::from_utf8_lossy(&attr.value).to_string();
                        push(&mut found, class, label, qname, Some(value));
                    }
                }
                if let Some((class, label)) = classify(&name) {
                    pending = Some((class, label, name));
                } else {
                    // rdf:li and friends carry the value of the element above.
                    if !name.starts_with("rdf:") {
                        pending = None;
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Some((class, label, qname)) = pending.clone() {
                    let value = String::from_utf8_lossy(t.as_ref()).trim().to_string();
                    if !value.is_empty() {
                        push(&mut found, class, label, qname, Some(value));
                        pending = None;
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if let Some((class, label, qname)) = pending.clone() {
                    if qname == name {
                        push(&mut found, class, label, qname, None);
                        pending = None;
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    found
}

fn push(
    found: &mut Vec<(Class, &'static str, String, Option<String>)>,
    class: Class,
    label: &'static str,
    qname: String,
    value: Option<String>,
) {
    // A property appearing in both attribute and element form is one property.
    if let Some(slot) = found.iter_mut().find(|(_, _, q, _)| *q == qname) {
        if slot.3.is_none() {
            slot.3 = value;
        }
        return;
    }
    found.push((class, label, qname, value));
}

pub fn inspect(xml: &[u8], locator: &str, ins: &mut Inspection) {
    ins.mark("xmp:packet");
    for (class, label, qname, value) in properties(xml) {
        let mut f = Finding::new(class, format!("{locator}:{qname}"), label);
        if let Some(v) = value {
            f = f.value(v);
        }
        ins.findings.push(f);
    }
}

/// Whether the packet has to go, recording each lost field in the diff.
pub fn should_drop(xml: &[u8], profile: &Profile, locator: &str, changes: &mut Vec<Change>) -> bool {
    let props = properties(xml);
    let doomed: Vec<_> = props.iter().filter(|(c, ..)| profile.alters(*c)).collect();
    if doomed.is_empty() {
        return false;
    }
    for (class, label, qname, value) in props {
        changes.push(Change::removed(class, format!("{locator}:{qname}"), label, value));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKET: &[u8] = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmpMM:DocumentID="xmp.did:9F2C41A0"
    xmpMM:InstanceID="xmp.iid:55B14E7C"
    xmp:MetadataDate="2024-06-14T19:42:07Z">
   <dc:creator><rdf:Seq><rdf:li>Marie Dvorakova</rdf:li></rdf:Seq></dc:creator>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn finds_both_attribute_and_element_properties() {
        let props = properties(PACKET);
        let qnames: Vec<&str> = props.iter().map(|(_, _, q, _)| q.as_str()).collect();
        assert!(qnames.contains(&"xmpMM:DocumentID"));
        assert!(qnames.contains(&"xmp:MetadataDate"));
        assert!(qnames.contains(&"dc:creator"));
        let creator = props.iter().find(|(_, _, q, _)| q == "dc:creator").unwrap();
        assert_eq!(creator.3.as_deref(), Some("Marie Dvorakova"));
        assert_eq!(creator.0, Class::Identity);
    }

    #[test]
    fn a_packet_with_correlation_ids_always_goes() {
        let mut changes = Vec::new();
        assert!(should_drop(PACKET, &Profile::standard(), "x", &mut changes));
        assert!(changes.iter().any(|c| c.class == Class::Correlation));
        // Every field is reported, not only the ones that forced the drop.
        assert!(changes.iter().any(|c| c.class == Class::Time));
    }

    #[test]
    fn inspect_only_keeps_the_packet() {
        let mut changes = Vec::new();
        assert!(!should_drop(PACKET, &Profile::inspect_only(), "x", &mut changes));
        assert!(changes.is_empty());
    }

    #[test]
    fn malformed_xml_yields_what_it_managed_to_read() {
        let props = properties(b"<rdf:Description dc:creator=\"Someone\"");
        assert!(props.is_empty() || props[0].0 == Class::Identity);
    }
}
