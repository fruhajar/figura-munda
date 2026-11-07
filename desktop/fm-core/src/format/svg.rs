//! SVG: a document whose metadata is part of its markup.
//!
//! Three things leak here. The `<metadata>` block carries the same Dublin Core
//! and RDF an XMP packet does. The editor namespaces (`inkscape:`, `sodipodi:`
//! and friends) carry a version and, worse, the filename the drawing was saved
//! under. And anything parked outside the `viewBox` is invisible on screen but
//! fully present in the file, which is the SVG form of a hidden payload.

use super::{xmp, Change, Handler, Outcome};
use crate::error::{Error, Result};
use crate::model::{Class, Finding, FormatId, Impact, Inspection};
use crate::profile::Profile;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

pub struct Svg;

/// Namespace prefixes that exist to record which program wrote the file.
const EDITOR_PREFIXES: &[&str] = &[
    "inkscape", "sodipodi", "ai", "i", "serif", "krita", "figma", "sketch", "vectornator", "graph",
];

/// Prefixes that only ever appear inside a metadata block.
const METADATA_PREFIXES: &[&str] = &["dc", "cc", "rdf", "xmp", "xmpMM", "pdf", "photoshop", "tiff", "exif"];

fn prefix_of(qname: &[u8]) -> Option<&str> {
    let s = std::str::from_utf8(qname).ok()?;
    s.split_once(':').map(|(p, _)| p)
}

fn local_of(qname: &[u8]) -> &str {
    std::str::from_utf8(qname)
        .unwrap_or_default()
        .rsplit(':')
        .next()
        .unwrap_or_default()
}

/// What an editor-namespace attribute gives away.
fn editor_attr_info(qname: &str) -> (Class, &'static str) {
    match qname {
        "sodipodi:docname" => (
            Class::Correlation,
            "the filename this was saved under, which often names a project or a client",
        ),
        "inkscape:version" | "ai:version" => (Class::Device, "editor version"),
        q if q.ends_with(":label") => (Class::Identity, "layer or object label"),
        q if q.contains("window") => (Class::Device, "editor window geometry, which reveals the screen size"),
        _ => (Class::Device, "editor state"),
    }
}

#[derive(Clone, Copy, Debug)]
struct ViewBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl ViewBox {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x - 1.0 && y >= self.y - 1.0 && x <= self.x + self.w + 1.0 && y <= self.y + self.h + 1.0
    }
}

fn parse_viewbox(e: &BytesStart) -> Option<ViewBox> {
    let attr = e.attributes().flatten().find(|a| a.key.as_ref() == b"viewBox")?;
    let text = String::from_utf8_lossy(&attr.value).to_string();
    let nums: Vec<f64> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    (nums.len() == 4).then(|| ViewBox { x: nums[0], y: nums[1], w: nums[2], h: nums[3] })
}

/// A position for an element, if it states one plainly enough to judge.
///
/// Only elements that place themselves with literal coordinates and carry no
/// transform are considered: a transform could put the content back on screen,
/// and reporting visible content as hidden would be worse than missing some.
fn plain_position(e: &BytesStart) -> Option<(f64, f64)> {
    let mut x = None;
    let mut y = None;
    for a in e.attributes().flatten() {
        let key = a.key.as_ref();
        if key == b"transform" {
            return None;
        }
        let value: Option<f64> = String::from_utf8_lossy(&a.value).trim().parse().ok();
        match key {
            b"x" | b"cx" => x = value,
            b"y" | b"cy" => y = value,
            _ => {}
        }
    }
    match (x, y) {
        (Some(x), Some(y)) => Some((x, y)),
        (None, Some(y)) => Some((0.0, y)),
        (Some(x), None) => Some((x, 0.0)),
        _ => None,
    }
}

const DRAWABLE: &[&str] = &[
    "text", "rect", "circle", "ellipse", "image", "use", "foreignObject", "tspan",
];

impl Handler for Svg {
    fn id(&self) -> FormatId {
        FormatId::Svg
    }

    fn sniff(&self, head: &[u8], ext: Option<&str>) -> bool {
        if ext == Some("svg") {
            return true;
        }
        let text = String::from_utf8_lossy(&head[..head.len().min(1024)]);
        let trimmed = text.trim_start();
        (trimmed.starts_with("<?xml") || trimmed.starts_with("<svg") || trimmed.starts_with("<!DOCTYPE svg"))
            && text.contains("<svg")
    }

    fn inspect(&self, data: &[u8]) -> Result<Inspection> {
        let mut ins = Inspection::new(FormatId::Svg, data.len() as u64);
        let mut reader = Reader::from_reader(data);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        let mut viewbox: Option<ViewBox> = None;
        let mut in_metadata = 0usize;
        let mut metadata_xml: Vec<u8> = Vec::new();
        let mut transform_depth = 0usize;
        let mut depth = 0usize;
        let mut saw_root = false;

        loop {
            let event = reader.read_event_into(&mut buf);
            match event {
                Err(e) => {
                    return Err(Error::malformed(FormatId::Svg, format!("XML: {e}")));
                }
                Ok(Event::Eof) => break,
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let name = e.name().as_ref().to_vec();
                    let local = local_of(&name).to_string();
                    if !saw_root && local == "svg" {
                        saw_root = true;
                        viewbox = parse_viewbox(&e);
                    }
                    if local == "metadata" {
                        in_metadata += 1;
                        ins.mark("svg:metadata");
                    }
                    if in_metadata > 0 {
                        metadata_xml.extend_from_slice(b"<");
                        metadata_xml.extend_from_slice(&e);
                        metadata_xml.extend_from_slice(b">");
                    }
                    if local == "script" {
                        ins.findings.push(
                            Finding::new(Class::HiddenPayload, "svg:script", "embedded script")
                                .impact(Impact::Content)
                                .note("A script in a drawing runs when the drawing is opened in a browser."),
                        );
                    }
                    if local == "title" || local == "desc" {
                        ins.findings.push(
                            Finding::new(
                                Class::Identity,
                                format!("svg:{local}"),
                                format!("{local} element, which may name its author"),
                            )
                            .protected()
                            .note("kept on purpose: screen readers read this out"),
                        );
                    }
                    // Editor-namespace attributes, including the declarations.
                    for a in e.attributes().flatten() {
                        let qname = String::from_utf8_lossy(a.key.as_ref()).to_string();
                        let Some(prefix) = qname.split_once(':').map(|(p, _)| p) else { continue };
                        if prefix == "xmlns" {
                            continue;
                        }
                        if EDITOR_PREFIXES.contains(&prefix) {
                            ins.mark("svg:editor-namespace");
                            let (class, label) = editor_attr_info(&qname);
                            ins.findings.push(
                                Finding::new(class, format!("svg:attr:{qname}"), label)
                                    .value(String::from_utf8_lossy(&a.value).to_string()),
                            );
                        }
                    }
                    // Content parked off the canvas.
                    if let (Some(vb), true) = (viewbox, DRAWABLE.contains(&local.as_str())) {
                        if transform_depth == 0 {
                            if let Some((x, y)) = plain_position(&e) {
                                if !vb.contains(x, y) {
                                    ins.mark("svg:offcanvas");
                                    ins.findings.push(
                                        Finding::new(
                                            Class::HiddenPayload,
                                            format!("svg:offcanvas:{local}@{x},{y}"),
                                            "content placed outside the visible area",
                                        )
                                        .value(format!("<{local}> at {x}, {y}"))
                                        .impact(Impact::Content)
                                        .note(
                                            "It does not appear when the drawing is viewed, but it \
                                             is in the file and any text in it can be read out.",
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    if e.attributes().flatten().any(|a| a.key.as_ref() == b"transform") {
                        transform_depth += 1;
                    }
                    depth += 1;
                }
                Ok(Event::End(e)) => {
                    let name = e.name().as_ref().to_vec();
                    if in_metadata > 0 {
                        metadata_xml.extend_from_slice(b"</");
                        metadata_xml.extend_from_slice(&name);
                        metadata_xml.extend_from_slice(b">");
                    }
                    if local_of(&name) == "metadata" {
                        in_metadata = in_metadata.saturating_sub(1);
                    }
                    depth = depth.saturating_sub(1);
                    transform_depth = transform_depth.min(depth);
                }
                Ok(Event::Text(t)) => {
                    if in_metadata > 0 {
                        metadata_xml.extend_from_slice(t.as_ref());
                    }
                }
                Ok(Event::Comment(c)) => {
                    let body = String::from_utf8_lossy(c.as_ref()).trim().to_string();
                    if !body.is_empty() {
                        ins.findings.push(
                            Finding::new(Class::Identity, "svg:comment", "comment left in the markup")
                                .value(body),
                        );
                    }
                }
                _ => {}
            }
            buf.clear();
        }

        if !metadata_xml.is_empty() {
            xmp::inspect(&metadata_xml, "svg:metadata", &mut ins);
        }
        Ok(ins)
    }

    fn clean(&self, data: &[u8], profile: &Profile) -> Result<Outcome> {
        let before = self.inspect(data)?;
        let mut changes = Vec::new();
        if profile.inert() {
            return Ok(Outcome::unchanged(data));
        }

        let drop_metadata = before
            .findings
            .iter()
            .filter(|f| f.locator.starts_with("svg:metadata"))
            .any(|f| profile.alters(f.class));
        let drop_editor = profile.alters(Class::Device) || profile.alters(Class::Correlation);
        let drop_offcanvas = profile.alters(Class::HiddenPayload);
        let drop_comments = profile.alters(Class::Identity);

        let mut reader = Reader::from_reader(data);
        reader.config_mut().trim_text(false);
        let mut writer = Writer::new(Vec::new());
        let mut buf = Vec::new();
        let mut skip_until: Option<(Vec<u8>, usize)> = None;
        let mut depth = 0usize;
        let mut viewbox: Option<ViewBox> = None;
        let mut transform_depth = 0usize;
        let mut saw_root = false;

        loop {
            let event = reader
                .read_event_into(&mut buf)
                .map_err(|e| Error::malformed(FormatId::Svg, format!("XML: {e}")))?;
            // Inside a subtree being dropped, write nothing until it closes.
            if let Some((ref name, at)) = skip_until {
                match &event {
                    Event::Start(_) => depth += 1,
                    Event::End(e) => {
                        depth = depth.saturating_sub(1);
                        if depth == at && e.name().as_ref() == name.as_slice() {
                            skip_until = None;
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
                Event::Comment(ref c) => {
                    if drop_comments {
                        let body = String::from_utf8_lossy(c.as_ref()).trim().to_string();
                        if !body.is_empty() {
                            changes.push(Change::removed(
                                Class::Identity,
                                "svg:comment",
                                "comment left in the markup",
                                Some(body),
                            ));
                            buf.clear();
                            continue;
                        }
                    }
                    writer.write_event(event).ok();
                }
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let is_empty = matches!(event, Event::Empty(_));
                    let name = e.name().as_ref().to_vec();
                    let local = local_of(&name).to_string();
                    if !saw_root && local == "svg" {
                        saw_root = true;
                        viewbox = parse_viewbox(e);
                    }
                    if !is_empty {
                        depth += 1;
                    }

                    if local == "metadata" && drop_metadata {
                        for f in before.findings.iter().filter(|f| f.locator.starts_with("svg:metadata")) {
                            changes.push(Change::removed(
                                f.class,
                                f.locator.clone(),
                                f.label.clone(),
                                f.value.clone(),
                            ));
                        }
                        if !is_empty {
                            skip_until = Some((name, depth - 1));
                        }
                        buf.clear();
                        continue;
                    }
                    if local == "script" && drop_offcanvas {
                        changes.push(Change::removed(
                            Class::HiddenPayload,
                            "svg:script",
                            "embedded script",
                            None,
                        ));
                        if !is_empty {
                            skip_until = Some((name, depth - 1));
                        }
                        buf.clear();
                        continue;
                    }
                    if drop_offcanvas && transform_depth == 0 && DRAWABLE.contains(&local.as_str()) {
                        if let (Some(vb), Some((x, y))) = (viewbox, plain_position(e)) {
                            if !vb.contains(x, y) {
                                changes.push(Change::removed(
                                    Class::HiddenPayload,
                                    format!("svg:offcanvas:{local}@{x},{y}"),
                                    "content placed outside the visible area",
                                    Some(format!("<{local}> at {x}, {y}")),
                                ));
                                if !is_empty {
                                    skip_until = Some((name, depth - 1));
                                }
                                buf.clear();
                                continue;
                            }
                        }
                    }

                    // Rebuild the tag without the editor-namespace attributes.
                    let mut kept = BytesStart::new(String::from_utf8_lossy(&name).to_string());
                    for a in e.attributes().flatten() {
                        let qname = String::from_utf8_lossy(a.key.as_ref()).to_string();
                        let doomed = match qname.split_once(':') {
                            Some(("xmlns", ns)) => {
                                drop_editor && EDITOR_PREFIXES.contains(&ns)
                                    || drop_metadata && METADATA_PREFIXES.contains(&ns)
                            }
                            Some((prefix, _)) => drop_editor && EDITOR_PREFIXES.contains(&prefix),
                            None => false,
                        };
                        if doomed {
                            if EDITOR_PREFIXES.iter().any(|p| qname.starts_with(&format!("{p}:"))) {
                                let (class, label) = editor_attr_info(&qname);
                                changes.push(Change::removed(
                                    class,
                                    format!("svg:attr:{qname}"),
                                    label,
                                    Some(String::from_utf8_lossy(&a.value).to_string()),
                                ));
                            }
                            continue;
                        }
                        kept.push_attribute((a.key.as_ref(), a.value.as_ref()));
                    }
                    let out = if is_empty { Event::Empty(kept) } else { Event::Start(kept) };
                    writer.write_event(out).ok();

                    if !is_empty && e.attributes().flatten().any(|a| a.key.as_ref() == b"transform") {
                        transform_depth += 1;
                    }
                }
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    transform_depth = transform_depth.min(depth);
                    writer.write_event(event).ok();
                }
                other => {
                    writer.write_event(other).ok();
                }
            }
            buf.clear();
        }

        Ok(Outcome { data: writer.into_inner(), changes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAWING: &[u8] = br#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg"
     xmlns:dc="http://purl.org/dc/elements/1.1/"
     xmlns:cc="http://creativecommons.org/ns#"
     xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
     xmlns:sodipodi="http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd"
     xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape"
     viewBox="0 0 200 100"
     inkscape:version="1.3.2"
     sodipodi:docname="org-chart-INTERNAL-v4.svg">
  <metadata><rdf:RDF><cc:Work><dc:creator><cc:Agent><dc:title>Marie Dvorakova</dc:title></cc:Agent></dc:creator><dc:date>2024-06-14</dc:date></cc:Work></rdf:RDF></metadata>
  <!-- draft for the board, do not circulate -->
  <rect x="10" y="10" width="80" height="40"/>
  <text x="10" y="400">superseded headcount plan: -18 FTE Q3</text>
</svg>"#;

    #[test]
    fn finds_the_editor_the_filename_and_the_parked_text() {
        let ins = Svg.inspect(DRAWING).unwrap();
        let locators: Vec<&str> = ins.findings.iter().map(|f| f.locator.as_str()).collect();
        assert!(locators.contains(&"svg:attr:inkscape:version"));
        assert!(locators.contains(&"svg:attr:sodipodi:docname"));
        assert!(locators.contains(&"svg:comment"));
        let hidden = ins.in_class(Class::HiddenPayload).next().expect("off-canvas text not found");
        assert_eq!(hidden.impact, Impact::Content);
        assert!(ins.shape.contains(&"svg:offcanvas".to_string()));
        assert!(ins.shape.contains(&"svg:metadata".to_string()));
    }

    #[test]
    fn the_filename_is_a_correlation_handle_not_a_device_detail() {
        let ins = Svg.inspect(DRAWING).unwrap();
        let f = ins.findings.iter().find(|f| f.locator == "svg:attr:sodipodi:docname").unwrap();
        assert_eq!(f.class, Class::Correlation);
        assert_eq!(f.value.as_deref(), Some("org-chart-INTERNAL-v4.svg"));
    }

    #[test]
    fn standard_leaves_a_drawing_that_still_draws() {
        let out = Svg.clean(DRAWING, &Profile::standard()).unwrap();
        let text = String::from_utf8(out.data.clone()).unwrap();
        assert!(text.contains(r#"<rect x="10" y="10""#), "{text}");
        assert!(text.contains("viewBox"), "{text}");
        assert!(!text.contains("inkscape:version"), "{text}");
        assert!(!text.contains("sodipodi:docname"), "{text}");
        assert!(!text.contains("Marie Dvorakova"), "{text}");
        assert!(!text.contains("-18 FTE"), "{text}");
        assert!(!text.contains("do not circulate"), "{text}");

        let after = Svg.inspect(&out.data).unwrap();
        assert!(crate::report::Verification::of(&after, &Profile::standard()).clean());
    }

    #[test]
    fn the_visible_rectangle_is_never_mistaken_for_hidden_payload() {
        let out = Svg.clean(DRAWING, &Profile::uniform()).unwrap();
        assert!(out
            .changes
            .iter()
            .all(|c| !c.before.as_deref().unwrap_or_default().contains("rect")));
    }

    #[test]
    fn inspect_only_returns_the_bytes_untouched() {
        let out = Svg.clean(DRAWING, &Profile::inspect_only()).unwrap();
        assert_eq!(out.data, DRAWING);
    }

    #[test]
    fn a_cleaned_drawing_joins_the_large_crowd() {
        let out = Svg.clean(DRAWING, &Profile::standard()).unwrap();
        let after = Svg.inspect(&out.data).unwrap();
        let sig = crate::anonset::classify(FormatId::Svg, &after.shape).unwrap();
        assert_eq!(sig.id, "svg-plain");
    }

    #[test]
    fn transformed_content_is_left_alone() {
        let drawing = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
  <g transform="translate(0,-350)"><text x="10" y="400">visible after all</text></g>
</svg>"#;
        let ins = Svg.inspect(drawing).unwrap();
        assert!(ins.in_class(Class::HiddenPayload).next().is_none());
    }

    #[test]
    fn sniffing_does_not_depend_on_the_extension() {
        assert!(Svg.sniff(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"x\">", None));
        assert!(!Svg.sniff(b"<html><body><svg></svg>", None));
    }
}
