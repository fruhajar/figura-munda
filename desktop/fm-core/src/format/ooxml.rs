//! OOXML and OpenDocument: a ZIP of XML parts, several of which exist only to
//! record who worked on the file and for how long.
//!
//! Three things make this format the worst of the set. Tracked changes and
//! comments keep the text that was deleted, together with the name of whoever
//! deleted it. The `w:rsid` save identifiers link two documents to one editing
//! session even when every name is gone. And the container itself is a
//! fingerprint: member order and per-member timestamps identify the writer and
//! survive every edit to the XML inside.

use super::zip::{self, Member};
use super::{xmlfilter, Change, Handler, Outcome};
use crate::error::{Error, Result};
use crate::model::{Action, Blocker, Class, Finding, FormatId, Impact, Inspection};
use crate::profile::{Blending, Profile, TimeOutcome};
use crate::timeutil;

pub struct Ooxml;

/// Parts that exist only to carry metadata or history, and can go whole.
fn disposable_part(name: &str) -> Option<(Class, &'static str, Impact)> {
    let m = |c: Class, l: &'static str| Some((c, l, Impact::Metadata));
    let content = |c: Class, l: &'static str| Some((c, l, Impact::Content));
    match name {
        n if n.starts_with("docProps/thumbnail") => content(
            Class::HiddenPayload,
            "an embedded preview of the first page, which is not updated when the document is",
        ),
        "docProps/custom.xml" => m(Class::Identity, "custom document properties"),
        "word/comments.xml" => content(Class::History, "comments, with the names of whoever left them"),
        "word/commentsExtended.xml" | "word/commentsIds.xml" | "word/commentsExtensible.xml" => {
            m(Class::History, "comment threading data")
        }
        "word/people.xml" => m(Class::Identity, "the list of everyone who edited the document"),
        n if n.starts_with("word/embeddings/") => content(
            Class::HiddenPayload,
            "an embedded file, which carries its own metadata and may hold more than was pasted in",
        ),
        n if n.starts_with("customXml/") => m(Class::Correlation, "custom XML data, often a records-management ID"),
        _ => None,
    }
}

/// Text elements in `docProps`, and what each gives away.
fn property_info(qname: &str) -> Option<(Class, &'static str)> {
    let i = |c: Class, l: &'static str| Some((c, l));
    match qname {
        "dc:creator" => i(Class::Identity, "author"),
        "cp:lastModifiedBy" => i(Class::Identity, "the person who last saved it"),
        "dc:title" => i(Class::Identity, "title"),
        "dc:subject" => i(Class::Identity, "subject"),
        "dc:description" => i(Class::Identity, "description"),
        "cp:keywords" => i(Class::Identity, "keywords"),
        "cp:category" => i(Class::Identity, "category"),
        "cp:contentStatus" => i(Class::Identity, "content status"),
        "Company" => i(Class::Identity, "the company it was written at"),
        "Manager" => i(Class::Identity, "the manager named in the document properties"),
        "Template" => i(Class::Identity, "the template it was made from, which names an internal file"),
        "Application" => i(Class::Device, "the program that wrote it"),
        "AppVersion" => i(Class::Device, "the version of that program"),
        "dcterms:created" => i(Class::Time, "creation date"),
        "dcterms:modified" => i(Class::Time, "last modification date"),
        "cp:lastPrinted" => i(Class::Time, "the date it was last printed"),
        "TotalTime" => i(Class::Time, "how many minutes were spent editing it"),
        "cp:revision" => i(Class::History, "how many times it has been saved"),
        _ => None,
    }
}

/// Elements that carry tracked-change history, and how to resolve each.
///
/// An insertion is accepted by unwrapping it; a deletion is rejected by
/// dropping it, which is what removing the history means. The alternative,
/// keeping the markup and stripping the author name, would leave the deleted
/// text in the file.
const HISTORY_UNWRAP: &[&str] = &["w:ins", "w:moveTo", "w:cellIns", "w:customXmlInsRangeStart"];
const HISTORY_DROP: &[&str] = &[
    "w:del",
    "w:moveFrom",
    "w:rPrChange",
    "w:pPrChange",
    "w:sectPrChange",
    "w:tblPrChange",
    "w:tcPrChange",
    "w:trPrChange",
    "w:tblGridChange",
    "w:cellDel",
    "w:commentRangeStart",
    "w:commentRangeEnd",
    "w:commentReference",
    "w:proofErr",
];

fn is_rels(name: &str) -> bool {
    name.ends_with(".rels")
}

/// Whether this package is OpenDocument rather than OPC.
///
/// ODF declares itself in a `mimetype` member that an OPC package does not
/// have. It shares the ZIP and the XML layers with OOXML and nothing else: the
/// part names differ (`meta.xml`, not `docProps/core.xml`) and so does the
/// whole vocabulary. Reading it with the OOXML part names finds no properties,
/// which is indistinguishable in a report from a document that has none.
fn odf_mimetype(members: &[Member]) -> Option<String> {
    let m = members.iter().find(|m| m.name == "mimetype")?;
    let declared = String::from_utf8_lossy(&m.data).trim().to_string();
    declared.starts_with("application/vnd.oasis.opendocument").then_some(declared)
}

/// What to say instead of a field count. Spelled out because the alternative
/// is a clean report on a file whose every field is still in it.
fn odf_blocker(mimetype: &str) -> Blocker {
    Blocker {
        reason: "this is an OpenDocument file, which is not supported yet".to_string(),
        detail: format!(
            "It declares itself as {mimetype}. The metadata lives in meta.xml and \
             settings.xml, in a vocabulary this tool does not read yet, so it cannot \
             tell you what is in the file and will not touch it. Reporting it as clean \
             is the one thing it must not do. Export to DOCX if it has to be cleaned \
             today."
        ),
    }
}

fn text_of(xml: &[u8], qname: &str) -> Option<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut inside = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) | Err(_) => return None,
            Ok(Event::Start(e)) => inside = e.name().as_ref() == qname.as_bytes(),
            Ok(Event::Text(t)) if inside => {
                let v = String::from_utf8_lossy(t.as_ref()).trim().to_string();
                if !v.is_empty() {
                    return Some(v);
                }
                inside = false;
            }
            Ok(Event::End(_)) => inside = false,
            _ => {}
        }
        buf.clear();
    }
}

/// Every attribute whose value is a per-save or per-paragraph identifier.
fn is_correlation_attr(key: &str) -> bool {
    key.starts_with("w:rsid")
        || key == "w14:paraId"
        || key == "w14:textId"
        || key == "w14:durableId"
        || key == "oleObjectId"
}

/// Elements that are identifiers in their own right. `w:rsids` is the table of
/// every save identifier the document has ever had, and `w15:docId` is a GUID
/// that survives a save-as, so both go whole rather than attribute by
/// attribute.
const CORRELATION_ELEMENTS: &[&str] = &["w:rsids", "w15:docId"];

/// What links this file to the user's other files.
#[derive(Default)]
struct Correlation {
    /// Identifier attributes, as qname and first-seen value.
    attrs: Vec<(String, String)>,
    /// How many distinct `w:rsid*` attributes appear.
    rsid_attrs: usize,
    /// How many entries the save-identifier table holds.
    rsid_table: usize,
    doc_id: Option<String>,
    /// Which of [`CORRELATION_ELEMENTS`] are present.
    elements: Vec<&'static str>,
}

impl Correlation {
    fn is_empty(&self) -> bool {
        self.attrs.is_empty() && self.rsid_table == 0 && self.elements.is_empty()
    }
}

fn correlation_in(xml: &[u8]) -> Correlation {
    use quick_xml::events::Event;
    let mut out = Correlation { attrs: scan_attrs(xml, is_correlation_attr), ..Default::default() };
    out.rsid_attrs = out.attrs.iter().filter(|(k, _)| k.starts_with("w:rsid")).count();

    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let qname = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if qname == "w:rsid" || qname == "w:rsidRoot" {
                    out.rsid_table += 1;
                }
                if qname == "w15:docId" {
                    out.doc_id = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref().ends_with(b"val"))
                        .map(|a| String::from_utf8_lossy(&a.value).to_string());
                }
                if let Some(known) = CORRELATION_ELEMENTS.iter().find(|n| **n == qname) {
                    if !out.elements.contains(known) {
                        out.elements.push(known);
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    out
}

fn scan_attrs(xml: &[u8], pred: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    use quick_xml::events::Event;
    let mut out: Vec<(String, String)> = Vec::new();
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                for a in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(a.key.as_ref()).to_string();
                    if pred(&key) && !out.iter().any(|(k, _)| *k == key) {
                        out.push((key, String::from_utf8_lossy(&a.value).to_string()));
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    out
}

fn count_elements(xml: &[u8], names: &[&str]) -> usize {
    use quick_xml::events::Event;
    let mut n = 0;
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let q = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if names.contains(&q.as_str()) {
                    n += 1;
                }
            }
            _ => {}
        }
        buf.clear();
    }
    n
}

/// Authors named inside tracked changes and comments.
///
/// Collected with its own scan rather than through [`scan_attrs`], which keeps
/// one value per attribute name: here every value matters, because each one is
/// a different person.
fn change_authors(xml: &[u8]) -> Vec<String> {
    use quick_xml::events::Event;
    let mut out: Vec<String> = Vec::new();
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                for a in e.attributes().flatten() {
                    if a.key.as_ref() == b"w:author" {
                        let v = String::from_utf8_lossy(&a.value).to_string();
                        if !v.is_empty() && !out.contains(&v) {
                            out.push(v);
                        }
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    out
}

impl Handler for Ooxml {
    fn id(&self) -> FormatId {
        FormatId::Ooxml
    }

    fn sniff(&self, head: &[u8], ext: Option<&str>) -> bool {
        if !head.starts_with(b"PK\x03\x04") {
            return false;
        }
        // Every OPC package names the content-type map as its first member, and
        // an ODF package names its mimetype. Anything else is a plain ZIP.
        let window = &head[..head.len().min(256)];
        let text = String::from_utf8_lossy(window);
        text.contains("[Content_Types].xml")
            || text.contains("mimetype")
            || matches!(ext, Some("docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp"))
    }

    fn inspect(&self, data: &[u8]) -> Result<Inspection> {
        let members = zip::read(data)?;
        if let Some(mimetype) = odf_mimetype(&members) {
            let mut ins = Inspection::new(FormatId::Opendocument, data.len() as u64);
            ins.blockers.push(odf_blocker(&mimetype));
            return Ok(ins);
        }
        let mut ins = Inspection::new(FormatId::Ooxml, data.len() as u64);

        for m in &members {
            if let Some((class, label, impact)) = disposable_part(&m.name) {
                ins.mark(match class {
                    Class::HiddenPayload if m.name.starts_with("docProps/thumbnail") => "ooxml:thumbnail",
                    Class::History => "ooxml:comments",
                    _ => "ooxml:part",
                });
                ins.findings.push(
                    Finding::new(class, format!("ooxml:part:{}", m.name), label)
                        .value(format!("{} bytes", m.data.len()))
                        .impact(impact),
                );
                continue;
            }
            if m.name.starts_with("docProps/") && m.name.ends_with(".xml") {
                ins.mark("ooxml:core-props");
                for qname in [
                    "dc:creator",
                    "cp:lastModifiedBy",
                    "dc:title",
                    "dc:subject",
                    "dc:description",
                    "cp:keywords",
                    "cp:category",
                    "cp:contentStatus",
                    "Company",
                    "Manager",
                    "Template",
                    "Application",
                    "AppVersion",
                    "dcterms:created",
                    "dcterms:modified",
                    "cp:lastPrinted",
                    "TotalTime",
                    "cp:revision",
                ] {
                    let Some(value) = text_of(&m.data, qname) else { continue };
                    let Some((class, label)) = property_info(qname) else { continue };
                    if matches!(class, Class::Identity | Class::Device) {
                        ins.mark("ooxml:app-props-identity");
                    }
                    ins.findings.push(
                        Finding::new(class, format!("ooxml:prop:{qname}"), label).value(value),
                    );
                }
                continue;
            }

            // Save identifiers, paragraph identifiers and the document GUID.
            let correlation = correlation_in(&m.data);
            if !correlation.is_empty() {
                ins.mark("ooxml:rsid");
                let saves = correlation.rsid_table.max(correlation.rsid_attrs);
                if saves > 0 {
                    ins.findings.push(
                        Finding::new(
                            Class::Correlation,
                            format!("ooxml:rsid:{}", m.name),
                            "save identifiers, which tie this document to your other documents",
                        )
                        .value(format!("{saves} in {}", m.name))
                        .note(
                            "These are not names. They are session identifiers, and two documents \
                             sharing one were edited in the same session by the same install.",
                        ),
                    );
                }
                if let Some(id) = &correlation.doc_id {
                    ins.findings.push(
                        Finding::new(
                            Class::Correlation,
                            "ooxml:attr:w15:docId",
                            "a document GUID that survives every save-as",
                        )
                        .value(id.clone()),
                    );
                }
                for (key, value) in correlation.attrs.iter().filter(|(k, _)| !k.starts_with("w:rsid")) {
                    ins.findings.push(
                        Finding::new(
                            Class::Correlation,
                            format!("ooxml:attr:{key}"),
                            match key.as_str() {
                                "w14:paraId" => "per-paragraph identifiers, stable across edits",
                                _ => "an internal identifier",
                            },
                        )
                        .value(value.clone()),
                    );
                }
            }

            let tracked = count_elements(&m.data, HISTORY_UNWRAP) + count_elements(&m.data, HISTORY_DROP);
            if tracked > 0 {
                ins.mark("ooxml:tracked-changes");
                let authors = change_authors(&m.data);
                let mut f = Finding::new(
                    Class::History,
                    format!("ooxml:tracked:{}", m.name),
                    "tracked changes, which keep the text that was deleted",
                )
                .value(format!("{tracked} marks"))
                .impact(Impact::Content);
                if !authors.is_empty() {
                    f = f.note(format!("Named in them: {}.", authors.join(", ")));
                }
                ins.findings.push(f);
            }

            // Hidden sheets and speaker notes are reported but not touched:
            // deleting a sheet is a decision about the document, not about its
            // metadata, and this tool does not make it for you.
            if m.name == "xl/workbook.xml" {
                let hidden = scan_attrs(&m.data, |k| k == "state")
                    .iter()
                    .filter(|(_, v)| v == "hidden" || v == "veryHidden")
                    .count();
                if hidden > 0 {
                    ins.findings.push(
                        Finding::new(Class::HiddenPayload, "ooxml:hidden-sheet", "hidden worksheets")
                            .value(format!("{hidden}"))
                            .impact(Impact::Content)
                            .advisory()
                            .note("Reported, not removed: deleting a sheet is your decision, not this tool's."),
                    );
                }
            }
            if m.name.starts_with("ppt/notesSlides/") {
                ins.findings.push(
                    Finding::new(Class::HiddenPayload, "ooxml:speaker-notes", "speaker notes")
                        .value(m.name.clone())
                        .impact(Impact::Content)
                        .advisory()
                        .note("Reported, not removed: they are part of the deck, and deleting them is your call."),
                );
            }
            if is_rels(&m.name) {
                for (_, target) in scan_attrs(&m.data, |k| k == "Target") {
                    // A template or data source on a network share names the
                    // organisation, and sometimes the user, in its path.
                    if target.starts_with("file:") || target.starts_with("\\\\") {
                        ins.findings.push(
                            Finding::new(
                                Class::Identity,
                                format!("ooxml:rel:{}", m.name),
                                "a link to a file on your machine or network",
                            )
                            .value(target),
                        );
                    }
                }
            }
        }

        // The container itself, reported only when it deviates from the shape
        // this tool would write, so that a cleaned file reports nothing.
        let stamps: Vec<timeutil::Civil> = members.iter().map(|m| m.timestamp()).collect();
        let uniform_stamp = stamps.iter().all(|s| *s == timeutil::EPOCH_1980);
        if !uniform_stamp {
            ins.mark("ooxml:member-timestamps");
            let distinct: Vec<String> = {
                let mut v: Vec<String> = stamps.iter().map(|s| s.to_iso()).collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            ins.findings.push(
                Finding::new(
                    Class::StructuralFingerprint,
                    "ooxml:zip:timestamps",
                    "per-file timestamps inside the container",
                )
                .value(if distinct.len() == 1 {
                    distinct[0].clone()
                } else {
                    format!("{} distinct, from {} to {}", distinct.len(), distinct[0], distinct[distinct.len() - 1])
                })
                .note("These are not the document's dates. They record when each piece was written, and they identify the program that wrote them."),
            );
        }
        if !zip::is_canonical(&members) {
            ins.mark("ooxml:member-order");
            ins.findings.push(
                Finding::new(
                    Class::StructuralFingerprint,
                    "ooxml:zip:order",
                    "the order the pieces are stored in, which identifies the writing software",
                )
                .value(format!("{} parts, writer's own order", members.len())),
            );
        }
        Ok(ins)
    }

    fn clean(&self, data: &[u8], profile: &Profile) -> Result<Outcome> {
        let mut members = zip::read(data)?;
        if let Some(mimetype) = odf_mimetype(&members) {
            let b = odf_blocker(&mimetype);
            return Err(Error::Refused(format!("{}: {}", b.reason, b.detail)));
        }
        let mut changes = Vec::new();
        if profile.inert() {
            return Ok(Outcome::unchanged(data));
        }

        // Which whole parts are going. Collected first, because the
        // relationships and the content-type map have to stop pointing at them.
        let mut dropped: Vec<String> = Vec::new();
        for m in &members {
            if let Some((class, label, _)) = disposable_part(&m.name) {
                if profile.alters(class) {
                    dropped.push(m.name.clone());
                    changes.push(Change::removed(
                        class,
                        format!("ooxml:part:{}", m.name),
                        label,
                        Some(format!("{} bytes", m.data.len())),
                    ));
                }
            }
        }

        let blank_not_drop = profile.blending == Blending::MatchCommon;
        let mut kept: Vec<Member> = Vec::new();
        for mut m in members.drain(..) {
            if dropped.contains(&m.name) {
                continue;
            }
            let mut rules = xmlfilter::Rules::default();
            let name = m.name.clone();

            if name.starts_with("docProps/") && name.ends_with(".xml") {
                for (qname, class, label) in [
                    "dc:creator",
                    "cp:lastModifiedBy",
                    "dc:title",
                    "dc:subject",
                    "dc:description",
                    "cp:keywords",
                    "cp:category",
                    "cp:contentStatus",
                    "Company",
                    "Manager",
                    "Template",
                    "Application",
                    "AppVersion",
                    "cp:revision",
                ]
                .into_iter()
                .filter_map(|q| property_info(q).map(|(c, l)| (q, c, l)))
                {
                    if !profile.alters(class) {
                        continue;
                    }
                    let before_value = text_of(&m.data, qname);
                    if before_value.is_none() {
                        continue;
                    }
                    // Word's own "remove document properties" leaves the
                    // elements in place and empty, so that is the shape to
                    // match. Minimal blending removes them outright.
                    if blank_not_drop {
                        rules.blank.push(qname);
                    } else {
                        rules.drop.push(qname);
                    }
                    changes.push(Change::rewritten(
                        class,
                        format!("ooxml:prop:{qname}"),
                        label,
                        profile.action(class),
                        before_value,
                        blank_not_drop.then(String::new),
                    ));
                }
                for qname in ["dcterms:created", "dcterms:modified", "cp:lastPrinted", "TotalTime"] {
                    let Some((class, label)) = property_info(qname) else { continue };
                    let Some(original) = text_of(&m.data, qname) else { continue };
                    if qname == "TotalTime" {
                        // Minutes spent editing is not a timestamp, so there is
                        // nothing to coarsen: it goes or it stays.
                        if profile.action(Class::Time) != Action::Keep {
                            rules.drop.push(qname);
                            changes.push(Change::removed(
                                class,
                                format!("ooxml:prop:{qname}"),
                                label,
                                Some(original),
                            ));
                        }
                        continue;
                    }
                    match crate::profile::retime(profile, &original) {
                        TimeOutcome::Unchanged => {}
                        TimeOutcome::Drop => {
                            rules.drop.push(qname);
                            changes.push(Change::removed(
                                class,
                                format!("ooxml:prop:{qname}"),
                                label,
                                Some(original),
                            ));
                        }
                        TimeOutcome::Set(c) => {
                            let v = c.to_iso();
                            rules.set.push((qname, v.clone()));
                            changes.push(Change::rewritten(
                                class,
                                format!("ooxml:prop:{qname}"),
                                label,
                                profile.action(Class::Time),
                                Some(original),
                                Some(v),
                            ));
                        }
                    }
                }
            }

            if profile.alters(Class::Correlation) {
                let correlation = correlation_in(&m.data);
                if !correlation.is_empty() {
                    if !correlation.attrs.is_empty() {
                        rules.drop_attr = Some(Box::new(is_correlation_attr));
                    }
                    rules.drop.extend(correlation.elements.iter().copied());
                    let saves = correlation.rsid_table.max(correlation.rsid_attrs);
                    if saves > 0 {
                        changes.push(Change::removed(
                            Class::Correlation,
                            format!("ooxml:rsid:{name}"),
                            "save identifiers",
                            Some(format!("{saves} in {name}")),
                        ));
                    }
                    if let Some(id) = correlation.doc_id {
                        changes.push(Change::removed(
                            Class::Correlation,
                            "ooxml:attr:w15:docId",
                            "document GUID",
                            Some(id),
                        ));
                    }
                    for (key, value) in correlation.attrs.iter().filter(|(k, _)| !k.starts_with("w:rsid")) {
                        changes.push(Change::removed(
                            Class::Correlation,
                            format!("ooxml:attr:{key}"),
                            "internal identifier",
                            Some(value.clone()),
                        ));
                    }
                }
            }

            if profile.alters(Class::History) {
                let tracked = count_elements(&m.data, HISTORY_UNWRAP) + count_elements(&m.data, HISTORY_DROP);
                if tracked > 0 {
                    rules.unwrap.extend_from_slice(HISTORY_UNWRAP);
                    rules.drop.extend_from_slice(HISTORY_DROP);
                    let authors = change_authors(&m.data);
                    changes.push(Change::removed(
                        Class::History,
                        format!("ooxml:tracked:{name}"),
                        if authors.is_empty() {
                            "tracked changes".to_string()
                        } else {
                            format!("tracked changes naming {}", authors.join(", "))
                        },
                        Some(format!("{tracked} marks")),
                    ));
                }
            }

            // Stop the package pointing at parts that no longer exist.
            if !dropped.is_empty() && (is_rels(&name) || name == "[Content_Types].xml") {
                let gone = dropped.clone();
                let part = name.clone();
                rules.drop_matching = Some(Box::new(move |qname: &str, attrs: &Vec<(String, String)>| {
                    match qname {
                        "Relationship" => attrs.iter().any(|(k, v)| {
                            k == "Target" && gone.contains(&xmlfilter::resolve_target(&part, v))
                        }),
                        "Override" => attrs.iter().any(|(k, v)| {
                            k == "PartName" && gone.contains(&v.trim_start_matches('/').to_string())
                        }),
                        _ => false,
                    }
                }));
            }

            m.data = xmlfilter::filter(&m.data, &rules)?;
            kept.push(m);
        }

        // The container's own shape.
        if profile.action(Class::StructuralFingerprint) == Action::RewriteUniform {
            let before_order = !zip::is_canonical(&kept);
            let stamps: Vec<timeutil::Civil> = kept.iter().map(|m| m.timestamp()).collect();
            for m in kept.iter_mut() {
                m.stamp(timeutil::EPOCH_1980);
                // A member stored rather than deflated is a choice the writer
                // made, so the uniform shape makes the same choice everywhere.
                m.stored = false;
            }
            zip::canonical_order(&mut kept);
            if stamps.iter().any(|s| *s != timeutil::EPOCH_1980) {
                changes.push(Change::rewritten(
                    Class::StructuralFingerprint,
                    "ooxml:zip:timestamps",
                    "per-file timestamps inside the container",
                    Action::RewriteUniform,
                    Some(format!("{} members, writer's own stamps", stamps.len())),
                    Some(timeutil::EPOCH_1980.to_iso()),
                ));
            }
            if before_order {
                changes.push(Change::rewritten(
                    Class::StructuralFingerprint,
                    "ooxml:zip:order",
                    "the order the pieces are stored in",
                    Action::RewriteUniform,
                    Some("the writer's own order".to_string()),
                    Some("a fixed order".to_string()),
                ));
            }
        }

        Ok(Outcome { data: zip::write(&kept), changes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docx() -> Vec<u8> {
        let mut members = vec![
            Member::new(
                "[Content_Types].xml",
                br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/docProps/thumbnail.jpeg" ContentType="image/jpeg"/><Override PartName="/word/comments.xml" ContentType="x"/><Override PartName="/word/document.xml" ContentType="y"/></Types>"#.to_vec(),
            ),
            Member::new(
                "_rels/.rels",
                br#"<Relationships><Relationship Id="rId1" Target="word/document.xml"/><Relationship Id="rId2" Target="docProps/thumbnail.jpeg"/></Relationships>"#.to_vec(),
            ),
            Member::new(
                "word/document.xml",
                br#"<w:document xmlns:w="w"><w:body><w:p w:rsidR="00A14C2E" w14:paraId="7F3B01A2"><w:r><w:t>Settlement of </w:t></w:r><w:del w:id="1" w:author="Petr Novak" w:date="2024-06-15T10:00:00Z"><w:r><w:delText>CZK 1,950,000</w:delText></w:r></w:del><w:ins w:id="2" w:author="Petr Novak" w:date="2024-06-15T10:01:00Z"><w:r><w:t>CZK 1,200,000</w:t></w:r></w:ins><w:commentRangeStart w:id="3"/><w:r><w:t>.</w:t></w:r></w:p></w:body></w:document>"#.to_vec(),
            ),
            Member::new(
                "word/_rels/document.xml.rels",
                br#"<Relationships><Relationship Id="rId9" Target="comments.xml"/></Relationships>"#.to_vec(),
            ),
            Member::new(
                "word/settings.xml",
                br#"<w:settings xmlns:w="w"><w15:docId w15:val="{3F2A9C41-55B1-4E7C-9B30-6D1E2A77C004}"/><w:rsids><w:rsidRoot w:val="00A14C2E"/><w:rsid w:val="00A14C2E"/><w:rsid w:val="00B22D31"/></w:rsids></w:settings>"#.to_vec(),
            ),
            Member::new(
                "word/comments.xml",
                br#"<w:comments xmlns:w="w"><w:comment w:id="3" w:author="Marie Dvorakova" w:initials="MD"><w:p><w:r><w:t>check this with legal</w:t></w:r></w:p></w:comment></w:comments>"#.to_vec(),
            ),
            Member::new(
                "docProps/core.xml",
                br#"<cp:coreProperties xmlns:cp="cp" xmlns:dc="dc" xmlns:dcterms="dcterms"><dc:creator>Marie Dvorakova</dc:creator><cp:lastModifiedBy>Petr Novak</cp:lastModifiedBy><cp:revision>14</cp:revision><dcterms:created>2024-05-02T08:11:00Z</dcterms:created><dcterms:modified>2024-06-15T10:02:00Z</dcterms:modified><cp:lastPrinted>2024-06-13T16:40:00Z</cp:lastPrinted></cp:coreProperties>"#.to_vec(),
            ),
            Member::new(
                "docProps/app.xml",
                br#"<Properties><Application>Microsoft Office Word</Application><AppVersion>16.0000</AppVersion><Company>Severin Holding a.s.</Company><Manager>Jan Prochazka</Manager><Template>sev-internal-memo.dotx</Template><TotalTime>2471</TotalTime></Properties>"#.to_vec(),
            ),
            Member { stored: true, ..Member::new("docProps/thumbnail.jpeg", vec![0xFF, 0xD8, 0xFF, 0xD9]) },
        ];
        for m in members.iter_mut() {
            m.stamp(timeutil::Civil { year: 2019, month: 3, day: 14, hour: 9, minute: 26, second: 52 });
        }
        zip::write(&members)
    }

    /// A minimal ODF package: the `mimetype` member first and stored, then the
    /// parts whose vocabulary this handler does not read.
    fn odt() -> Vec<u8> {
        let members = vec![
            Member {
                stored: true,
                ..Member::new("mimetype", b"application/vnd.oasis.opendocument.text".to_vec())
            },
            Member::new(
                "meta.xml",
                br#"<office:document-meta xmlns:office="o" xmlns:meta="m" xmlns:dc="dc"><office:meta><meta:initial-creator>Marie Dvorakova</meta:initial-creator><dc:creator>Petr Novak</dc:creator><meta:creation-date>2024-05-02T08:11:00</meta:creation-date><meta:generator>LibreOffice/7.6.4.1</meta:generator><meta:editing-cycles>14</meta:editing-cycles><meta:editing-duration>PT41H11M</meta:editing-duration></office:meta></office:document-meta>"#.to_vec(),
            ),
            Member::new("content.xml", b"<office:document-content/>".to_vec()),
        ];
        zip::write(&members)
    }

    /// Read with the OOXML part names an ODF package has no properties at all,
    /// which in a report is indistinguishable from a document that has none.
    /// Until the vocabulary is read, the only honest answer is to say so.
    #[test]
    fn an_opendocument_package_is_refused_rather_than_reported_clean() {
        let data = odt();
        let ins = Ooxml.inspect(&data).unwrap();
        assert_eq!(ins.format, FormatId::Opendocument);
        assert!(ins.refused(), "an ODF package was inspected as an Office document");
        assert!(ins.blockers[0].detail.contains("meta.xml"), "{:?}", ins.blockers[0]);
        assert!(ins.findings.is_empty(), "a field count on ODF overstates what was read");

        match Ooxml.clean(&data, &Profile::standard()) {
            Err(Error::Refused(why)) => assert!(why.contains("OpenDocument"), "{why}"),
            Err(e) => panic!("expected a refusal, got {e}"),
            Ok(_) => panic!("an ODF package was cleaned, leaving every field in it"),
        }

        // And the fields it did not read are still in the package, which is
        // what the refusal is protecting against calling clean.
        assert!(text_of_part(&data, "meta.xml").contains("Marie Dvorakova"));
    }

    /// The OPC path must not be reached through an ODF extension either.
    #[test]
    fn an_office_package_is_still_recognised_and_read() {
        let data = docx();
        assert!(Ooxml.sniff(&data, Some("docx")));
        assert!(Ooxml.sniff(&odt(), Some("odt")), "ODF must still be recognised to be refused");
        let ins = Ooxml.inspect(&data).unwrap();
        assert_eq!(ins.format, FormatId::Ooxml);
        assert!(!ins.refused());
    }

    fn text_of_part(data: &[u8], name: &str) -> String {
        let members = zip::read(data).unwrap();
        let m = members.iter().find(|m| m.name == name).unwrap_or_else(|| panic!("{name} missing"));
        String::from_utf8_lossy(&m.data).to_string()
    }

    fn part_names(data: &[u8]) -> Vec<String> {
        zip::read(data).unwrap().into_iter().map(|m| m.name).collect()
    }

    #[test]
    fn finds_the_authors_the_company_the_history_and_the_save_ids() {
        let ins = Ooxml.inspect(&docx()).unwrap();
        let v = |c: Class| {
            ins.in_class(c).filter_map(|f| f.value.clone()).collect::<Vec<_>>().join(" | ")
        };
        assert!(v(Class::Identity).contains("Marie Dvorakova"));
        assert!(v(Class::Identity).contains("Petr Novak"));
        assert!(v(Class::Identity).contains("Severin Holding a.s."));
        assert!(v(Class::Identity).contains("sev-internal-memo.dotx"));
        assert!(v(Class::Device).contains("Microsoft Office Word"));
        assert!(v(Class::Time).contains("2471"), "editing minutes not found");
        assert!(ins.in_class(Class::Correlation).any(|f| f.locator.contains("rsid")));
        assert!(ins
            .in_class(Class::Correlation)
            .any(|f| f.value.as_deref().unwrap_or_default().contains("3F2A9C41")));
        assert!(ins.in_class(Class::History).any(|f| f.locator.contains("tracked")));
        assert!(ins.in_class(Class::HiddenPayload).any(|f| f.locator.contains("thumbnail")));
    }

    #[test]
    fn names_the_people_hiding_in_the_tracked_changes() {
        let ins = Ooxml.inspect(&docx()).unwrap();
        let tracked = ins.in_class(Class::History).find(|f| f.locator.contains("tracked")).unwrap();
        assert!(tracked.note.as_deref().unwrap().contains("Petr Novak"));
    }

    #[test]
    fn reports_the_container_stamps_as_a_fingerprint_not_as_a_date() {
        let ins = Ooxml.inspect(&docx()).unwrap();
        let f = ins
            .in_class(Class::StructuralFingerprint)
            .find(|f| f.locator == "ooxml:zip:timestamps")
            .unwrap();
        assert!(f.value.as_deref().unwrap().contains("2019-03-14"));
        assert!(f.note.as_deref().unwrap().contains("not the document's dates"));
    }

    #[test]
    fn standard_removes_the_deleted_figure_and_accepts_the_insertion() {
        let out = Ooxml.clean(&docx(), &Profile::standard()).unwrap();
        let doc = text_of_part(&out.data, "word/document.xml");
        assert!(!doc.contains("1,950,000"), "the superseded figure survived: {doc}");
        assert!(doc.contains("CZK 1,200,000"), "the accepted text was lost: {doc}");
        assert!(!doc.contains("Petr Novak"), "{doc}");
        assert!(!doc.contains("w:rsidR"), "{doc}");
        assert!(!doc.contains("commentRangeStart"), "{doc}");
        assert!(doc.contains("Settlement of "), "the real text was damaged: {doc}");
    }

    #[test]
    fn dropping_a_part_takes_its_relationship_and_content_type_with_it() {
        let out = Ooxml.clean(&docx(), &Profile::standard()).unwrap();
        let names = part_names(&out.data);
        assert!(!names.iter().any(|n| n.contains("thumbnail")), "{names:?}");
        assert!(!names.iter().any(|n| n == "word/comments.xml"), "{names:?}");

        let types = text_of_part(&out.data, "[Content_Types].xml");
        assert!(!types.contains("thumbnail"), "{types}");
        assert!(!types.contains("comments.xml"), "{types}");
        assert!(types.contains("document.xml"), "the real part lost its content type: {types}");

        let root_rels = text_of_part(&out.data, "_rels/.rels");
        assert!(!root_rels.contains("thumbnail"), "{root_rels}");
        assert!(root_rels.contains("word/document.xml"), "{root_rels}");

        let doc_rels = text_of_part(&out.data, "word/_rels/document.xml.rels");
        assert!(!doc_rels.contains("comments.xml"), "{doc_rels}");
    }

    #[test]
    fn match_common_tools_empties_the_properties_rather_than_deleting_them() {
        let out = Ooxml.clean(&docx(), &Profile::standard()).unwrap();
        let core = text_of_part(&out.data, "docProps/core.xml");
        assert!(core.contains("<dc:creator></dc:creator>"), "{core}");
        assert!(!core.contains("Marie Dvorakova"), "{core}");
    }

    #[test]
    fn strip_to_the_minimum_deletes_the_properties_outright() {
        let mut p = Profile::standard();
        p.blending = Blending::Minimal;
        let out = Ooxml.clean(&docx(), &p).unwrap();
        let core = text_of_part(&out.data, "docProps/core.xml");
        assert!(!core.contains("dc:creator"), "{core}");
    }

    #[test]
    fn standard_keeps_the_dates_and_strict_coarsens_them() {
        let standard = Ooxml.clean(&docx(), &Profile::standard()).unwrap();
        assert!(text_of_part(&standard.data, "docProps/core.xml").contains("2024-05-02T08:11:00Z"));

        let strict = Ooxml.clean(&docx(), &Profile::strict()).unwrap();
        let core = text_of_part(&strict.data, "docProps/core.xml");
        assert!(core.contains("2024-05-02T00:00:00Z"), "{core}");
        assert!(!core.contains("08:11"), "{core}");
        assert!(!core.contains("2471"), "editing minutes survived: {core}");
    }

    #[test]
    fn uniform_rewrites_the_container_to_a_fixed_shape() {
        let out = Ooxml.clean(&docx(), &Profile::uniform()).unwrap();
        let members = zip::read(&out.data).unwrap();
        assert!(members.iter().all(|m| m.timestamp() == timeutil::EPOCH_1980));
        assert!(zip::is_canonical(&members));
        assert_eq!(members[0].name, "[Content_Types].xml");

        // And the file then reports nothing about its own container.
        let after = Ooxml.inspect(&out.data).unwrap();
        assert!(after.in_class(Class::StructuralFingerprint).next().is_none());
        assert!(crate::report::Verification::of(&after, &Profile::uniform()).clean());
    }

    #[test]
    fn standard_leaves_the_container_fingerprint_alone_and_says_so() {
        let out = Ooxml.clean(&docx(), &Profile::standard()).unwrap();
        let after = Ooxml.inspect(&out.data).unwrap();
        assert!(after.in_class(Class::StructuralFingerprint).next().is_some());
        // Which is not a failure: standard keeps that class on purpose.
        assert!(crate::report::Verification::of(&after, &Profile::standard()).clean());
    }

    #[test]
    fn every_profile_leaves_a_verifiably_clean_package() {
        for profile in Profile::builtins() {
            if profile.inert() {
                continue;
            }
            let out = Ooxml.clean(&docx(), &profile).unwrap();
            let after = Ooxml.inspect(&out.data).unwrap();
            let v = crate::report::Verification::of(&after, &profile);
            assert!(
                v.clean(),
                "{}: {:?}",
                profile.name,
                v.leftovers.iter().map(|f| &f.locator).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn inspect_only_returns_the_package_untouched() {
        let data = docx();
        assert_eq!(Ooxml.clean(&data, &Profile::inspect_only()).unwrap().data, data);
    }

    #[test]
    fn a_plain_zip_is_not_mistaken_for_a_document() {
        let plain = zip::write(&[Member::new("notes.txt", b"hello".to_vec())]);
        assert!(!Ooxml.sniff(&plain, None));
        assert!(Ooxml.sniff(&docx(), None));
    }
}
