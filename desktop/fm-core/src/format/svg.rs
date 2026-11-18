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
    /// Whether any part of `ext` lands on the canvas. The one-unit slack keeps
    /// a shape sitting exactly on the edge visible.
    fn shows(&self, ext: &Extent) -> bool {
        ext.x1 >= self.x - 1.0
            && ext.y1 >= self.y - 1.0
            && ext.x0 <= self.x + self.w + 1.0
            && ext.y0 <= self.y + self.h + 1.0
    }
}

/// The box an element occupies, in the coordinates it states.
#[derive(Clone, Copy, Debug)]
struct Extent {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

fn parse_viewbox(e: &BytesStart) -> Option<ViewBox> {
    let attr = e.attributes().flatten().find(|a| a.key.as_ref() == b"viewBox")?;
    let text = crate::text::text(&attr.value);
    let nums: Vec<f64> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    (nums.len() == 4).then(|| ViewBox { x: nums[0], y: nums[1], w: nums[2], h: nums[3] })
}

/// The box an element occupies, if it states one plainly enough to judge.
///
/// Only elements that place themselves with literal coordinates and carry no
/// transform are considered: a transform could put the content back on screen,
/// and reporting visible content as hidden would be worse than missing some.
///
/// The extent is what matters, not the anchor. A bleed rectangle starts
/// outside the canvas and covers all of it, so judging it by its anchor called
/// the background hidden payload and `uniform` then deleted the artwork. Where
/// no extent is stated (`text`, `tspan`, a `use` without a size) the anchor is
/// all there is. A number that is not plain, a percentage or a unit or a
/// calc, makes the extent unknown, which is not the same as off the canvas:
/// neither is judged.
fn plain_extent(e: &BytesStart, local: &str) -> Option<Extent> {
    let mut x = None;
    let mut y = None;
    let mut w = None;
    let mut h = None;
    let mut r = None;
    let mut rx = None;
    let mut ry = None;
    for a in e.attributes().flatten() {
        let key = a.key.as_ref();
        if key == b"transform" {
            return None;
        }
        if !matches!(
            key,
            b"x" | b"y" | b"cx" | b"cy" | b"width" | b"height" | b"r" | b"rx" | b"ry"
        ) {
            continue;
        }
        let value: f64 = String::from_utf8_lossy(&a.value).trim().parse().ok()?;
        match key {
            b"x" | b"cx" => x = Some(value),
            b"y" | b"cy" => y = Some(value),
            b"width" => w = Some(value),
            b"height" => h = Some(value),
            b"r" => r = Some(value),
            b"rx" => rx = Some(value),
            b"ry" => ry = Some(value),
            _ => {}
        }
    }
    if x.is_none() && y.is_none() {
        return None;
    }
    let (x, y) = (x.unwrap_or(0.0), y.unwrap_or(0.0));
    // A circle or an ellipse states a centre and radii; everything else states
    // a corner and, when it states a size at all, a width and a height.
    let (dx0, dy0, dx1, dy1) = match local {
        "circle" => {
            let r = r.unwrap_or(0.0).abs();
            (-r, -r, r, r)
        }
        "ellipse" => {
            let rx = rx.or(r).unwrap_or(0.0).abs();
            let ry = ry.or(r).unwrap_or(0.0).abs();
            (-rx, -ry, rx, ry)
        }
        _ => (0.0, 0.0, w.unwrap_or(0.0).max(0.0), h.unwrap_or(0.0).max(0.0)),
    };
    Some(Extent { x0: x + dx0, y0: y + dy0, x1: x + dx1, y1: y + dy1 })
}

/// Whether an open ancestor carries a `transform`.
///
/// Counting transforms and clamping the count to the element depth looked
/// equivalent to this and was not: the count never came back down, so one
/// `<g transform>` anywhere above switched the off-canvas check off for the
/// rest of the file, in the report and in the clean alike.
#[derive(Default)]
struct TransformStack(Vec<bool>);

impl TransformStack {
    fn open(&mut self, e: &BytesStart) {
        self.0.push(e.attributes().flatten().any(|a| a.key.as_ref() == b"transform"));
    }

    fn close(&mut self) {
        self.0.pop();
    }

    fn inside(&self) -> bool {
        self.0.iter().any(|&t| t)
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
        let mut transform = TransformStack::default();
        let mut saw_root = false;

        loop {
            let event = reader.read_event_into(&mut buf);
            match event {
                Err(e) => {
                    return Err(Error::malformed(FormatId::Svg, format!("XML: {e}")));
                }
                Ok(Event::Eof) => break,
                Ok(ev @ (Event::Start(_) | Event::Empty(_))) => {
                    let (e, is_empty) = match &ev {
                        Event::Start(e) => (e, false),
                        Event::Empty(e) => (e, true),
                        _ => unreachable!(),
                    };
                    let name = e.name().as_ref().to_vec();
                    let local = local_of(&name).to_string();
                    if !saw_root && local == "svg" {
                        saw_root = true;
                        viewbox = parse_viewbox(e);
                    }
                    if local == "metadata" {
                        in_metadata += 1;
                        ins.mark("svg:metadata");
                    }
                    if in_metadata > 0 {
                        metadata_xml.extend_from_slice(b"<");
                        metadata_xml.extend_from_slice(&e[..]);
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
                                    .value(crate::text::text(&a.value)),
                            );
                        }
                    }
                    // Content parked off the canvas.
                    if let (Some(vb), true) = (viewbox, DRAWABLE.contains(&local.as_str())) {
                        if !transform.inside() {
                            if let Some(ext) = plain_extent(e, &local) {
                                if !vb.shows(&ext) {
                                    let (x, y) = (ext.x0, ext.y0);
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
                    if !is_empty {
                        transform.open(e);
                    }
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
                    transform.close();
                }
                Ok(Event::Text(t)) => {
                    if in_metadata > 0 {
                        metadata_xml.extend_from_slice(t.as_ref());
                    }
                }
                Ok(Event::Comment(c)) => {
                    let body = crate::text::text(c.as_ref()).trim().to_string();
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
        let mut transform = TransformStack::default();
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
                    if drop_offcanvas && !transform.inside() && DRAWABLE.contains(&local.as_str()) {
                        if let (Some(vb), Some(ext)) = (viewbox, plain_extent(e, &local)) {
                            let (x, y) = (ext.x0, ext.y0);
                            if !vb.shows(&ext) {
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
                                    Some(crate::text::text(&a.value)),
                                ));
                            }
                            continue;
                        }
                        kept.push_attribute((a.key.as_ref(), a.value.as_ref()));
                    }
                    let out = if is_empty { Event::Empty(kept) } else { Event::Start(kept) };
                    writer.write_event(out).ok();

                    if !is_empty {
                        transform.open(e);
                    }
                }
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    transform.close();
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

    /// One closed `<g transform>` used to switch the check off for the rest of
    /// the file: the transform count was clamped to the element depth and never
    /// came back down. The parked text after the group went unreported and,
    /// under `uniform`, unremoved.
    #[test]
    fn a_closed_transform_group_does_not_switch_the_check_off() {
        let drawing = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
  <g transform="translate(10,10)"><rect x="0" y="0" width="20" height="20"/></g>
  <g transform="rotate(4)"><circle cx="50" cy="50" r="8"/></g>
  <text x="10" y="400">superseded headcount plan: -18 FTE Q3</text>
</svg>"#;
        let ins = Svg.inspect(drawing).unwrap();
        let hidden = ins.in_class(Class::HiddenPayload).next().expect("parked text not found");
        assert!(hidden.locator.contains("text@10,400"), "{}", hidden.locator);

        let out = Svg.clean(drawing, &Profile::uniform()).unwrap();
        let text = String::from_utf8_lossy(&out.data);
        assert!(!text.contains("-18 FTE Q3"), "the parked text survived uniform");
        // The visible drawing is all still there.
        assert!(text.contains("<circle") && text.contains("<rect"), "{text}");
    }

    /// A bleed rectangle starts outside the canvas and covers all of it. Judged
    /// by its anchor it looked like hidden payload, and `uniform` deleted the
    /// artwork and reported it as a removal.
    #[test]
    fn a_bleed_rectangle_that_covers_the_canvas_is_not_hidden_payload() {
        let drawing = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
  <rect x="-10" y="-10" width="220" height="120" fill="#6b7a8f"/>
  <circle cx="-4" cy="20" r="30"/>
  <image x="-20" y="-20" width="40" height="40" href="a.png"/>
  <rect x="-300" y="-300" width="10" height="10"/>
</svg>"##;
        let ins = Svg.inspect(drawing).unwrap();
        let hidden: Vec<&str> = ins.in_class(Class::HiddenPayload).map(|f| f.locator.as_str()).collect();
        assert_eq!(hidden, vec!["svg:offcanvas:rect@-300,-300"], "{hidden:?}");

        let out = Svg.clean(drawing, &Profile::uniform()).unwrap();
        let text = String::from_utf8_lossy(&out.data);
        assert!(text.contains(r#"width="220""#), "the bleed rectangle was deleted: {text}");
        assert!(text.contains("<circle") && text.contains("<image"), "{text}");
        assert!(!text.contains(r#"x="-300""#), "the parked rectangle survived");
        assert_eq!(out.changes.iter().filter(|c| c.locator.contains("offcanvas")).count(), 1);
    }

    /// A size that is not a plain number leaves the extent unknown, which is
    /// not the same as off the canvas.
    #[test]
    fn an_extent_in_percent_is_not_judged_either_way() {
        let drawing = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100">
  <rect x="-10" y="-10" width="100%" height="100%"/>
</svg>"#;
        let ins = Svg.inspect(drawing).unwrap();
        assert!(ins.in_class(Class::HiddenPayload).next().is_none());
        let out = Svg.clean(drawing, &Profile::uniform()).unwrap();
        assert!(String::from_utf8_lossy(&out.data).contains("100%"));
    }

    #[test]
    fn sniffing_does_not_depend_on_the_extension() {
        assert!(Svg.sniff(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"x\">", None));
        assert!(!Svg.sniff(b"<html><body><svg></svg>", None));
    }
}
