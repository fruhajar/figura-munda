//! A rewriting filter for XML parts, shared by the OOXML and OpenDocument
//! handlers.
//!
//! Four operations cover everything those formats need: drop a subtree, drop a
//! tag but keep its children (which is what accepting a tracked insertion is),
//! empty an element's text, and replace it. Attributes are filtered by
//! predicate, because `w:rsid*` is a family rather than a list.

use crate::error::{Error, Result};
use crate::model::FormatId;
use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

type Attrs = Vec<(String, String)>;
/// Predicate over an attribute's qualified name.
type AttrFilter<'a> = Box<dyn Fn(&str) -> bool + 'a>;
/// Predicate over an element's qualified name and its attributes.
type ElementFilter<'a> = Box<dyn Fn(&str, &Attrs) -> bool + 'a>;

#[derive(Default)]
pub struct Rules<'a> {
    /// Element qnames whose whole subtree goes.
    pub drop: Vec<&'a str>,
    /// Element qnames whose tags go but whose children stay.
    pub unwrap: Vec<&'a str>,
    /// Element qnames kept, with their text emptied.
    pub blank: Vec<&'a str>,
    /// Element qnames kept, with their text replaced.
    pub set: Vec<(&'a str, String)>,
    /// Attribute qnames to strip, by predicate.
    pub drop_attr: Option<AttrFilter<'a>>,
    /// Elements to drop by qname and attributes, for relationship entries and
    /// content-type overrides that point at a part being removed.
    pub drop_matching: Option<ElementFilter<'a>>,
}

impl<'a> Rules<'a> {
    pub fn is_empty(&self) -> bool {
        self.drop.is_empty()
            && self.unwrap.is_empty()
            && self.blank.is_empty()
            && self.set.is_empty()
            && self.drop_attr.is_none()
            && self.drop_matching.is_none()
    }

    fn attrs_of(e: &BytesStart) -> Attrs {
        e.attributes()
            .flatten()
            .map(|a| {
                (
                    String::from_utf8_lossy(a.key.as_ref()).to_string(),
                    String::from_utf8_lossy(&a.value).to_string(),
                )
            })
            .collect()
    }
}

/// Apply the rules. Everything not named is copied through byte for byte.
pub fn filter(xml: &[u8], rules: &Rules) -> Result<Vec<u8>> {
    if rules.is_empty() {
        return Ok(xml.to_vec());
    }
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut buf = Vec::new();

    // Subtrees being dropped, and elements whose text is being suppressed.
    let mut skip: Option<(Vec<u8>, usize)> = None;
    let mut suppress_text: Vec<Vec<u8>> = Vec::new();
    let mut depth = 0usize;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::malformed(FormatId::Ooxml, format!("XML: {e}")))?;

        if let Some((ref name, at)) = skip {
            match &event {
                Event::Start(_) => depth += 1,
                Event::End(e) => {
                    depth = depth.saturating_sub(1);
                    if depth == at && e.name().as_ref() == name.as_slice() {
                        skip = None;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
            continue;
        }

        match event {
            Event::Eof => break,
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(event, Event::Empty(_));
                let name = e.name().as_ref().to_vec();
                let qname = String::from_utf8_lossy(&name).to_string();
                if !is_empty {
                    depth += 1;
                }

                let matched = rules
                    .drop_matching
                    .as_ref()
                    .is_some_and(|f| f(&qname, &Rules::attrs_of(e)));
                if rules.drop.contains(&qname.as_str()) || matched {
                    if !is_empty {
                        skip = Some((name, depth - 1));
                    }
                    buf.clear();
                    continue;
                }
                if rules.unwrap.contains(&qname.as_str()) {
                    // The tag goes, the children stay. Depth bookkeeping has to
                    // stay honest, so the matching End is swallowed below.
                    buf.clear();
                    continue;
                }

                let mut kept = BytesStart::new(qname.clone());
                for (key, value) in Rules::attrs_of(e) {
                    if rules.drop_attr.as_ref().is_some_and(|f| f(&key)) {
                        continue;
                    }
                    kept.push_attribute((key.as_str(), value.as_str()));
                }
                let replacement = rules.set.iter().find(|(n, _)| *n == qname).map(|(_, v)| v.clone());
                let blanking = rules.blank.contains(&qname.as_str());

                if is_empty {
                    match replacement {
                        Some(v) if !v.is_empty() => {
                            writer.write_event(Event::Start(kept)).ok();
                            writer.write_event(Event::Text(BytesText::new(&v))).ok();
                            writer.write_event(Event::End(quick_xml::events::BytesEnd::new(qname))).ok();
                        }
                        _ => {
                            writer.write_event(Event::Empty(kept)).ok();
                        }
                    }
                } else {
                    writer.write_event(Event::Start(kept)).ok();
                    if blanking || replacement.is_some() {
                        suppress_text.push(name.clone());
                        if let Some(v) = replacement.filter(|v| !v.is_empty()) {
                            writer.write_event(Event::Text(BytesText::new(&v))).ok();
                        }
                    }
                }
            }
            Event::End(ref e) => {
                let name = e.name().as_ref().to_vec();
                let qname = String::from_utf8_lossy(&name).to_string();
                depth = depth.saturating_sub(1);
                if rules.unwrap.contains(&qname.as_str()) {
                    buf.clear();
                    continue;
                }
                if suppress_text.last().map(|n| n.as_slice()) == Some(name.as_slice()) {
                    suppress_text.pop();
                }
                writer.write_event(event).ok();
            }
            Event::Text(_) | Event::CData(_) => {
                if suppress_text.is_empty() {
                    writer.write_event(event).ok();
                }
            }
            other => {
                writer.write_event(other).ok();
            }
        }
        buf.clear();
    }
    Ok(writer.into_inner())
}

/// Resolve a relationship target against the part that declared it.
///
/// `word/_rels/document.xml.rels` has base `word/`, so a target of
/// `comments.xml` is the part `word/comments.xml`.
pub fn resolve_target(rels_part: &str, target: &str) -> String {
    if target.starts_with('/') {
        return target.trim_start_matches('/').to_string();
    }
    let base = rels_part
        .rsplit_once("_rels/")
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_default();
    let mut segments: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_a_subtree_and_keeps_its_siblings() {
        let rules = Rules { drop: vec!["w:del"], ..Default::default() };
        let out = filter(
            b"<w:p><w:r>keep</w:r><w:del><w:r><w:delText>gone</w:delText></w:r></w:del><w:r>also keep</w:r></w:p>",
            &rules,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("keep"));
        assert!(text.contains("also keep"));
        assert!(!text.contains("gone"), "{text}");
    }

    #[test]
    fn unwrapping_an_insertion_accepts_it() {
        let rules = Rules { unwrap: vec!["w:ins"], ..Default::default() };
        let out = filter(b"<w:p><w:ins w:author=\"Petr\"><w:r>added text</w:r></w:ins></w:p>", &rules).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "<w:p><w:r>added text</w:r></w:p>");
    }

    #[test]
    fn blanking_keeps_the_element_and_loses_the_name() {
        let rules = Rules { blank: vec!["dc:creator"], ..Default::default() };
        let out = filter(b"<cp:coreProperties><dc:creator>Marie Dvorakova</dc:creator></cp:coreProperties>", &rules).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "<cp:coreProperties><dc:creator></dc:creator></cp:coreProperties>"
        );
    }

    #[test]
    fn setting_replaces_the_text() {
        let rules = Rules {
            set: vec![("dcterms:created", "1980-01-01T00:00:00Z".to_string())],
            ..Default::default()
        };
        let out = filter(b"<x><dcterms:created>2024-05-02T08:11:00Z</dcterms:created></x>", &rules).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("1980-01-01T00:00:00Z"));
    }

    #[test]
    fn attributes_are_filtered_by_family() {
        let rules = Rules {
            drop_attr: Some(Box::new(|k: &str| k.starts_with("w:rsid"))),
            ..Default::default()
        };
        let out = filter(b"<w:p w:rsidR=\"00A1\" w:rsidRDefault=\"00A1\" w14:paraId=\"7F\"/>", &rules).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("rsid"), "{text}");
        assert!(text.contains("w14:paraId"), "{text}");
    }

    #[test]
    fn relationships_can_be_dropped_by_target() {
        let rules = Rules {
            drop_matching: Some(Box::new(|q: &str, attrs: &Attrs| {
                q == "Relationship"
                    && attrs.iter().any(|(k, v)| k == "Target" && v.contains("thumbnail"))
            })),
            ..Default::default()
        };
        let out = filter(
            br#"<Relationships><Relationship Id="rId1" Target="docProps/thumbnail.jpeg"/><Relationship Id="rId2" Target="word/document.xml"/></Relationships>"#,
            &rules,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("thumbnail"), "{text}");
        assert!(text.contains("document.xml"), "{text}");
    }

    #[test]
    fn targets_resolve_against_the_part_that_declared_them() {
        assert_eq!(resolve_target("_rels/.rels", "docProps/core.xml"), "docProps/core.xml");
        assert_eq!(resolve_target("word/_rels/document.xml.rels", "comments.xml"), "word/comments.xml");
        assert_eq!(resolve_target("word/_rels/document.xml.rels", "../docProps/app.xml"), "docProps/app.xml");
        assert_eq!(resolve_target("word/_rels/document.xml.rels", "/word/people.xml"), "word/people.xml");
    }

    #[test]
    fn empty_rules_return_the_input_untouched() {
        let xml = b"<a><b>c</b></a>";
        assert_eq!(filter(xml, &Rules::default()).unwrap(), xml);
    }
}
