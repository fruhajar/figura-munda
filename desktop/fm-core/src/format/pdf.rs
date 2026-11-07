//! PDF, the format with the most ways to keep what you thought you removed.
//!
//! Four of them matter. The document information dictionary and the XMP packet
//! are the easy part. The trailer `/ID` pair links every copy and every revision
//! of a document to one original. An incremental update keeps the whole previous
//! revision in the same file, recoverable in full, which is how a redlined draft
//! reaches a counterparty intact. And a redaction drawn as a filled rectangle
//! over live text hides nothing at all: the text is still there, in order, and
//! any reader can copy it out.
//!
//! Cleaning rewrites the file from the resolved object set rather than appending
//! to it, which is what makes the superseded revisions actually go away. That
//! also means the output has this tool's structural shape instead of the
//! original writer's, which is reported rather than hidden.

use super::{xmp, Change, Handler, Outcome};
use crate::error::{Error, Result};
use crate::model::{Action, Blocker, Class, Finding, FormatId, Impact, Inspection};
use crate::profile::{Blending, Profile, TimeOutcome};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId};

pub struct Pdf;

mod redaction;
use redaction::{analyse, Covered};

/// Info-dictionary keys worth naming, and what each gives away.
fn info_info(key: &str) -> Option<(Class, &'static str)> {
    let i = |c: Class, l: &'static str| Some((c, l));
    match key {
        "Author" => i(Class::Identity, "author"),
        "Title" => i(Class::Identity, "title"),
        "Subject" => i(Class::Identity, "subject"),
        "Keywords" => i(Class::Identity, "keywords"),
        "Creator" => i(Class::Device, "the program the document was written in"),
        "Producer" => i(Class::Device, "the program that turned it into a PDF"),
        "CreationDate" => i(Class::Time, "creation date"),
        "ModDate" => i(Class::Time, "last modification date"),
        "Trapped" => None,
        _ => i(Class::Identity, "a custom document property"),
    }
}

fn as_text(o: &Object) -> Option<String> {
    match o {
        Object::String(bytes, _) => {
            let s = String::from_utf8_lossy(bytes).trim().to_string();
            (!s.is_empty()).then_some(s)
        }
        Object::Name(n) => Some(String::from_utf8_lossy(n).to_string()),
        Object::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Does this file contain a signature? If so nothing may be touched: any edit
/// invalidates it, and silently breaking a signature is worse than leaving the
/// metadata in place.
fn signature_in(doc: &Document) -> Option<String> {
    // A signature is spread over two dictionaries: the form field, which gives
    // it a name in the form, and the signature itself, which gives the signer's
    // name. Both say `/Sig`, so the named one has to be preferred rather than
    // whichever comes first by object number.
    let mut unnamed = false;
    for (_, object) in doc.objects.iter() {
        let Object::Dictionary(d) = object else { continue };
        let is_sig = matches!(d.get(b"Type").and_then(|o| o.as_name()), Ok(b"Sig"))
            || matches!(d.get(b"FT").and_then(|o| o.as_name()), Ok(b"Sig"));
        if !is_sig {
            continue;
        }
        match d.get(b"Name").ok().and_then(as_text) {
            Some(who) => return Some(who),
            None => unnamed = true,
        }
    }
    if unnamed {
        return Some("an unnamed signer".to_string());
    }
    if let Ok(catalog) = doc.catalog() {
        if let Ok(Object::Dictionary(form)) = catalog.get(b"AcroForm") {
            if form.get(b"SigFlags").is_ok() {
                return Some("a signature field".to_string());
            }
        }
    }
    None
}

/// How many revisions the file holds. More than one means the earlier ones are
/// still in the bytes.
fn revision_count(data: &[u8]) -> usize {
    data.windows(9).filter(|w| *w == b"startxref").count().max(1)
}

/// What a reader can recover from the superseded revision.
///
/// The earlier revision ends at the first `%%EOF`, so loading the prefix is
/// enough to read it, which is also exactly what an adversary would do.
fn recover_previous(data: &[u8]) -> Option<String> {
    let end = data.windows(5).position(|w| w == b"%%EOF")? + 5;
    let doc = Document::load_mem(&data[..end]).ok()?;
    let mut found: Vec<String> = Vec::new();
    for page_id in doc.get_pages().values() {
        if let Ok(text) = doc.extract_text(&[page_number_of(&doc, *page_id)]) {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if !text.is_empty() {
                found.push(text);
            }
        }
    }
    (!found.is_empty()).then(|| found.join(" / "))
}

fn page_number_of(doc: &Document, page_id: ObjectId) -> u32 {
    doc.get_pages()
        .iter()
        .find(|(_, id)| **id == page_id)
        .map(|(n, _)| *n)
        .unwrap_or(1)
}

/// JavaScript hiding in an action dictionary.
fn is_javascript(doc: &Document, object: &Object) -> bool {
    let dict = match object {
        Object::Dictionary(d) => d.clone(),
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => return false,
        },
        _ => return false,
    };
    matches!(dict.get(b"S").and_then(|o| o.as_name()), Ok(b"JavaScript")) || dict.get(b"JS").is_ok()
}

fn info_dict(doc: &Document) -> Option<(ObjectId, Dictionary)> {
    let id = doc.trailer.get(b"Info").ok()?.as_reference().ok()?;
    match doc.get_object(id) {
        Ok(Object::Dictionary(d)) => Some((id, d.clone())),
        _ => None,
    }
}

fn metadata_stream(doc: &Document) -> Option<(ObjectId, Vec<u8>)> {
    let id = doc.catalog().ok()?.get(b"Metadata").ok()?.as_reference().ok()?;
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    let content = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
    Some((id, content))
}

impl Handler for Pdf {
    fn id(&self) -> FormatId {
        FormatId::Pdf
    }

    fn sniff(&self, head: &[u8], _ext: Option<&str>) -> bool {
        head[..head.len().min(1024)].windows(5).any(|w| w == b"%PDF-")
    }

    fn inspect(&self, data: &[u8]) -> Result<Inspection> {
        let doc = Document::load_mem(data)
            .map_err(|e| Error::malformed(FormatId::Pdf, format!("{e}")))?;
        let mut ins = Inspection::new(FormatId::Pdf, data.len() as u64);

        if let Some(signer) = signature_in(&doc) {
            ins.blockers.push(Blocker {
                reason: "the document is signed".to_string(),
                detail: format!(
                    "It carries a signature by {signer}. Any change to the metadata invalidates \
                     it, so nothing here will be touched. Remove the signature first if the file \
                     has to be cleaned."
                ),
            });
            // Still worth reporting what it exposes, so the user can decide.
            ins.findings.push(
                Finding::new(Class::Identity, "pdf:sig:name", "the name on the signature")
                    .value(signer)
                    .protected()
                    .note("inside the signature, so it cannot be removed without breaking it"),
            );
        }

        if let Some((_, info)) = info_dict(&doc) {
            for (key, value) in info.iter() {
                let key = String::from_utf8_lossy(key).to_string();
                let Some(text) = as_text(value) else { continue };
                let Some((class, label)) = info_info(&key) else { continue };
                match key.as_str() {
                    "Producer" => ins.mark("pdf:producer"),
                    "Author" => ins.mark("pdf:author"),
                    "CreationDate" => ins.mark("pdf:creation-date"),
                    _ => {}
                }
                ins.findings
                    .push(Finding::new(class, format!("pdf:info:{key}"), label).value(text));
            }
        }

        if let Ok(Object::Array(id)) = doc.trailer.get(b"ID") {
            let shown: Vec<String> = id
                .iter()
                .filter_map(|o| match o {
                    Object::String(b, _) => Some(hex(b)),
                    _ => None,
                })
                .collect();
            if !shown.is_empty() {
                ins.mark("pdf:id");
                ins.findings.push(
                    Finding::new(
                        Class::Correlation,
                        "pdf:trailer:ID",
                        "the file identifier, which is the same in every copy and every revision",
                    )
                    .value(shown.join(" / "))
                    .note(
                        "Two documents sharing the first half of this pair came from the same \
                         original, whatever their names and contents say.",
                    ),
                );
            }
        }

        if let Some((_, packet)) = metadata_stream(&doc) {
            ins.mark("pdf:xmp");
            xmp::inspect(&packet, "pdf:xmp", &mut ins);
        }

        if let Ok(catalog) = doc.catalog() {
            if let Ok(names) = catalog.get(b"Names") {
                let names = match names {
                    Object::Reference(id) => doc.get_object(*id).ok().cloned(),
                    other => Some(other.clone()),
                };
                if let Some(Object::Dictionary(d)) = names {
                    if d.get(b"EmbeddedFiles").is_ok() {
                        ins.mark("pdf:embedded-files");
                        ins.findings.push(
                            Finding::new(
                                Class::HiddenPayload,
                                "pdf:names:EmbeddedFiles",
                                "files attached to the document, each with metadata of its own",
                            )
                            .impact(Impact::Content),
                        );
                    }
                    if d.get(b"JavaScript").is_ok() {
                        ins.mark("pdf:openaction-js");
                        ins.findings.push(
                            Finding::new(
                                Class::HiddenPayload,
                                "pdf:names:JavaScript",
                                "JavaScript that runs when the document is opened",
                            )
                            .impact(Impact::Content),
                        );
                    }
                }
            }
            if let Ok(open) = catalog.get(b"OpenAction") {
                if is_javascript(&doc, open) {
                    ins.mark("pdf:openaction-js");
                    ins.findings.push(
                        Finding::new(
                            Class::HiddenPayload,
                            "pdf:catalog:OpenAction",
                            "an action that runs when the document is opened",
                        )
                        .impact(Impact::Content),
                    );
                }
            }
            if catalog.get(b"StructTreeRoot").is_ok() {
                ins.findings.push(
                    Finding::new(
                        Class::StructuralFingerprint,
                        "pdf:catalog:StructTreeRoot",
                        "the accessibility structure that lets a screen reader follow the document",
                    )
                    .protected()
                    .note("kept on purpose: removing it makes the document unreadable to a screen reader"),
                );
            }
        }

        let revisions = revision_count(data);
        if revisions > 1 {
            ins.mark("pdf:incremental");
            let mut f = Finding::new(
                Class::History,
                "pdf:incremental",
                "an earlier version of the document, still present in full",
            )
            .value(format!("{revisions} revisions"))
            .impact(Impact::Structural)
            .note(
                "A PDF saved this way keeps every previous revision in the same file. Anyone \
                 can read them with a text editor.",
            );
            if let Some(text) = recover_previous(data) {
                f = f.value(format!("{revisions} revisions; recoverable: {text}"));
            }
            ins.findings.push(f);
        }

        // Text that a filled rectangle only appears to hide.
        for (number, page_id) in doc.get_pages() {
            for covered in analyse(&doc, page_id) {
                match covered {
                    Covered::ByFill { text, .. } => {
                        ins.mark("pdf:covered-text");
                        ins.findings.push(
                            Finding::new(
                                Class::RedactionIntegrity,
                                format!("pdf:page{number}:covered"),
                                "text hidden behind a filled rectangle, still readable",
                            )
                            .value(text)
                            .impact(Impact::Content)
                            .note(
                                "The rectangle is drawn over the text, not instead of it. Select \
                                 the area in any reader and the words come out.",
                            ),
                        );
                    }
                    Covered::ByImage { text, .. } => ins.findings.push(
                        Finding::new(
                            Class::RedactionIntegrity,
                            format!("pdf:page{number}:under-image"),
                            "text underneath a placed image",
                        )
                        .value(text)
                        .impact(Impact::Content)
                        .advisory()
                        .note(
                            "This is what a scanned page with an OCR layer looks like, and also \
                             what a redaction pasted over as a picture looks like. Reported \
                             rather than removed, because the two cannot be told apart from here.",
                        ),
                    ),
                }
            }
        }
        Ok(ins)
    }

    fn clean(&self, data: &[u8], profile: &Profile) -> Result<Outcome> {
        let mut doc = Document::load_mem(data)
            .map_err(|e| Error::malformed(FormatId::Pdf, format!("{e}")))?;
        if let Some(signer) = signature_in(&doc) {
            return Err(Error::Refused(format!(
                "the document is signed by {signer}; any edit would invalidate the signature"
            )));
        }
        if profile.inert() {
            return Ok(Outcome::unchanged(data));
        }
        let before = self.inspect(data)?;
        let mut changes = Vec::new();

        // The information dictionary.
        if let Some((info_id, info)) = info_dict(&doc) {
            let mut kept = Dictionary::new();
            for (key, value) in info.iter() {
                let name = String::from_utf8_lossy(key).to_string();
                let shown = as_text(value);
                let Some((class, label)) = info_info(&name) else {
                    kept.set(key.to_vec(), value.clone());
                    continue;
                };
                let locator = format!("pdf:info:{name}");
                if class == Class::Time {
                    match crate::profile::retime(profile, shown.as_deref().unwrap_or_default()) {
                        TimeOutcome::Unchanged => kept.set(key.to_vec(), value.clone()),
                        TimeOutcome::Drop => {
                            changes.push(Change::removed(class, locator, label, shown))
                        }
                        TimeOutcome::Set(c) => {
                            let v = c.to_pdf();
                            kept.set(key.to_vec(), Object::string_literal(v.clone()));
                            changes.push(Change::rewritten(
                                class,
                                locator,
                                label,
                                profile.action(Class::Time),
                                shown,
                                Some(v),
                            ));
                        }
                    }
                    continue;
                }
                match profile.action(class) {
                    Action::Keep => kept.set(key.to_vec(), value.clone()),
                    Action::Replace => {
                        let v = profile.constants.generic_text.clone();
                        kept.set(key.to_vec(), Object::string_literal(v.clone()));
                        changes.push(Change::rewritten(
                            class,
                            locator,
                            label,
                            Action::Replace,
                            shown,
                            Some(v),
                        ));
                    }
                    _ => changes.push(Change::removed(class, locator, label, shown)),
                }
            }
            if kept.is_empty() {
                doc.trailer.remove(b"Info");
                doc.objects.remove(&info_id);
            } else {
                doc.set_object(info_id, Object::Dictionary(kept));
            }
        }

        // The XMP packet goes whole or stays whole, as everywhere else.
        if let Some((meta_id, packet)) = metadata_stream(&doc) {
            if xmp::should_drop(&packet, profile, "pdf:xmp", &mut changes) {
                doc.objects.remove(&meta_id);
                if let Ok(catalog) = doc.catalog_mut() {
                    catalog.remove(b"Metadata");
                }
            }
        }

        // Attachments and anything that runs on open.
        if profile.alters(Class::HiddenPayload) {
            let mut removed: Vec<&str> = Vec::new();
            let names_id = doc
                .catalog()
                .ok()
                .and_then(|c| c.get(b"Names").ok())
                .and_then(|o| o.as_reference().ok());
            if let Some(id) = names_id {
                if let Ok(Object::Dictionary(d)) = doc.get_object_mut(id) {
                    if d.remove(b"EmbeddedFiles").is_some() {
                        removed.push("pdf:names:EmbeddedFiles");
                    }
                    if d.remove(b"JavaScript").is_some() {
                        removed.push("pdf:names:JavaScript");
                    }
                }
            }
            if let Ok(catalog) = doc.catalog_mut() {
                if let Ok(Object::Dictionary(d)) = catalog.get_mut(b"Names") {
                    if d.remove(b"EmbeddedFiles").is_some() {
                        removed.push("pdf:names:EmbeddedFiles");
                    }
                    if d.remove(b"JavaScript").is_some() {
                        removed.push("pdf:names:JavaScript");
                    }
                }
            }
            let open_is_js = doc
                .catalog()
                .ok()
                .and_then(|c| c.get(b"OpenAction").ok().cloned())
                .map(|o| is_javascript(&doc, &o))
                .unwrap_or(false);
            if let Ok(catalog) = doc.catalog_mut() {
                if open_is_js && catalog.remove(b"OpenAction").is_some() {
                    removed.push("pdf:catalog:OpenAction");
                }
                if catalog.remove(b"AA").is_some() {
                    removed.push("pdf:catalog:AA");
                }
            }
            for locator in removed {
                let found = before.findings.iter().find(|f| f.locator == locator);
                changes.push(Change::removed(
                    Class::HiddenPayload,
                    locator,
                    found.map(|f| f.label.clone()).unwrap_or_else(|| "embedded payload".into()),
                    found.and_then(|f| f.value.clone()),
                ));
            }
        }

        // Text that was only covered up, not removed.
        if profile.action(Class::RedactionIntegrity) == Action::Enforce {
            for (number, page_id) in doc.get_pages() {
                let covered = analyse(&doc, page_id);
                let doomed: Vec<usize> = covered
                    .iter()
                    .filter_map(|c| match c {
                        Covered::ByFill { op_index, .. } => Some(*op_index),
                        Covered::ByImage { .. } => None,
                    })
                    .collect();
                if doomed.is_empty() {
                    continue;
                }
                let Ok(content) = doc.get_and_decode_page_content(page_id) else { continue };
                let operations: Vec<Operation> = content
                    .operations
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| !doomed.contains(i))
                    .map(|(_, op)| op)
                    .collect();
                let encoded = Content { operations }
                    .encode()
                    .map_err(|e| Error::malformed(FormatId::Pdf, format!("content stream: {e}")))?;
                // Replacing the page's content is not enough on its own: the
                // old stream object would still be written out, text and all.
                for old in doc.get_page_contents(page_id) {
                    doc.objects.remove(&old);
                }
                let stream = lopdf::Stream::new(Dictionary::new(), encoded);
                let new_id = doc.add_object(Object::Stream(stream));
                if let Ok(Object::Dictionary(page)) = doc.get_object_mut(page_id) {
                    page.set("Contents", Object::Reference(new_id));
                }
                for c in covered {
                    if let Covered::ByFill { text, .. } = c {
                        changes.push(Change::removed(
                            Class::RedactionIntegrity,
                            format!("pdf:page{number}:covered"),
                            "text hidden behind a filled rectangle",
                            Some(text),
                        ));
                    }
                }
            }
        }

        // The file identifier. Removing it is distinctive, because almost every
        // PDF has one, so the default is to replace it with a value derived
        // from the cleaned document itself: unique per document, and linked to
        // nothing that came before.
        if profile.alters(Class::Correlation) {
            let before_id = doc.trailer.get(b"ID").ok().map(|o| format!("{o:?}"));
            if before_id.is_some() {
                let shown = before
                    .findings
                    .iter()
                    .find(|f| f.locator == "pdf:trailer:ID")
                    .and_then(|f| f.value.clone());
                if profile.blending == Blending::Minimal {
                    doc.trailer.remove(b"ID");
                    changes.push(Change::removed(
                        Class::Correlation,
                        "pdf:trailer:ID",
                        "the file identifier",
                        shown,
                    ));
                } else {
                    let digest = derive_id(&doc);
                    doc.trailer.set(
                        "ID",
                        Object::Array(vec![
                            Object::String(digest.clone(), lopdf::StringFormat::Hexadecimal),
                            Object::String(digest.clone(), lopdf::StringFormat::Hexadecimal),
                        ]),
                    );
                    changes.push(Change::rewritten(
                        Class::Correlation,
                        "pdf:trailer:ID",
                        "the file identifier, replaced with one derived from this file's own contents",
                        Action::Replace,
                        shown,
                        Some(hex(&digest)),
                    ));
                }
            }
        }

        // Unlinking an attachment or an action only removes the pointer. The
        // bytes it pointed at are still objects in the file and would be
        // written out, so everything now unreachable from the trailer goes.
        let orphans = doc.prune_objects();
        if !orphans.is_empty() && profile.alters(Class::HiddenPayload) {
            changes.push(Change::removed(
                Class::HiddenPayload,
                "pdf:orphans",
                "the contents of what was unlinked above, which would otherwise stay in the file",
                Some(format!("{} objects", orphans.len())),
            ));
        }

        let mut out = Vec::new();
        doc.save_to(&mut out)
            .map_err(|e| Error::malformed(FormatId::Pdf, format!("writing: {e}")))?;

        if revision_count(data) > 1 {
            let found = before.findings.iter().find(|f| f.locator == "pdf:incremental");
            changes.push(Change::removed(
                Class::History,
                "pdf:incremental",
                "the earlier revisions kept inside the file",
                found.and_then(|f| f.value.clone()),
            ));
        }
        Ok(Outcome { data: out, changes })
    }
}

/// A file identifier derived from the document's own objects.
///
/// Deterministic, so cleaning the same file twice gives the same result, and
/// unrelated to whatever the original identifier was, so it links nothing.
fn derive_id(doc: &Document) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (id, object) in doc.objects.iter() {
        hasher.update(id.0.to_le_bytes());
        hasher.update(format!("{object:?}").as_bytes());
    }
    hasher.finalize()[..16].to_vec()
}

#[cfg(test)]
mod tests;
