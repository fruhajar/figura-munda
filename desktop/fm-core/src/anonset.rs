//! Anonymity sets: which crowd a cleaned file joins.
//!
//! Stripping is itself a fingerprint. A file cleaned in exactly one tool's
//! pattern identifies you as a user of that tool, which for a small tool is a
//! small crowd. So where there is a choice, the output should take the shape
//! that some very common pipeline already emits.
//!
//! Every signature here carries its [`Provenance`], because the honest answer
//! about most of them is "assumed, not measured". The leakage survey in the root
//! README is what turns the assumptions into measurements, and this table is the
//! slot its results land in. Nothing in the engine depends on a signature being
//! right: a wrong one produces a wrong line in a report, never a leak.

use crate::model::FormatId;

/// How a signature was established. Reports print this, so a guess cannot be
/// mistaken for a measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Follows from the format specification, not from observing any tool.
    Structural,
    /// Measured from a real file produced by the named pipeline, on this machine.
    Measured,
    /// Believed, not checked. Needs the leakage survey.
    Assumed,
}

impl Provenance {
    pub fn label(self) -> &'static str {
        match self {
            Provenance::Structural => "follows from the format spec",
            Provenance::Measured => "measured locally",
            Provenance::Assumed => "assumed, not yet measured",
        }
    }
}

/// How many other files look like this. The whole point of the exercise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Crowd {
    /// Billions of files. Indistinguishable is the goal and this reaches it.
    Large,
    /// Common but identifiable as a category.
    Medium,
    /// Identifies the tool. Avoid unless the user asked for it.
    Small,
}

impl Crowd {
    pub fn label(self) -> &'static str {
        match self {
            Crowd::Large => "very common",
            Crowd::Medium => "common",
            Crowd::Small => "distinctive",
        }
    }
}

pub struct Signature {
    pub id: &'static str,
    pub name: &'static str,
    pub format: FormatId,
    /// Markers that must be present.
    pub requires: &'static [&'static str],
    /// Markers that must be absent.
    pub forbids: &'static [&'static str],
    pub crowd: Crowd,
    pub provenance: Provenance,
    pub note: &'static str,
}

impl Signature {
    pub fn matches(&self, shape: &[String]) -> bool {
        let has = |m: &str| shape.iter().any(|s| s == m);
        self.requires.iter().all(|m| has(m)) && !self.forbids.iter().any(|m| has(m))
    }
}

/// The table.
///
/// Ordered most-common-first within a format, so the first match is the best
/// answer to "what does this now look like".
pub const SIGNATURES: &[Signature] = &[
    Signature {
        id: "jpeg-plain-baseline",
        name: "a plain baseline JPEG with no application segments",
        format: FormatId::Jpeg,
        requires: &["jpeg:app0-jfif"],
        forbids: &["jpeg:app1-exif", "jpeg:app1-xmp", "jpeg:app13-iptc", "jpeg:com", "jpeg:app14-adobe"],
        crowd: Crowd::Large,
        provenance: Provenance::Structural,
        // A JFIF header and nothing else is what a re-encode through a stock
        // imaging library leaves behind, which is most JPEGs that have passed
        // through any server-side resize.
        note: "the shape left by any re-encode through a stock imaging library",
    },
    Signature {
        id: "jpeg-icc-only",
        name: "a baseline JPEG carrying only a colour profile",
        format: FormatId::Jpeg,
        requires: &["jpeg:app2-icc"],
        forbids: &["jpeg:app1-exif", "jpeg:app1-xmp", "jpeg:app13-iptc", "jpeg:com"],
        crowd: Crowd::Large,
        provenance: Provenance::Structural,
        note: "what a colour-managed re-encode leaves; the profile must stay, so this is the target when one is present",
    },
    Signature {
        id: "jpeg-orientation-only",
        name: "a JPEG keeping only the rendering hints it needs",
        format: FormatId::Jpeg,
        requires: &["jpeg:app1-exif", "exif:render-only"],
        forbids: &["jpeg:app1-xmp", "jpeg:app13-iptc", "jpeg:com"],
        crowd: Crowd::Large,
        provenance: Provenance::Structural,
        note: "an EXIF block holding orientation and nothing else is what most resize pipelines leave, because dropping orientation would turn the picture on its side",
    },
    Signature {
        id: "jpeg-date-only",
        name: "a JPEG keeping only its capture date",
        format: FormatId::Jpeg,
        requires: &["jpeg:app1-exif", "exif:datetime-only"],
        forbids: &["jpeg:app1-xmp", "jpeg:app13-iptc", "exif:gps", "exif:maker-note", "exif:thumbnail"],
        crowd: Crowd::Medium,
        provenance: Provenance::Structural,
        note: "a date-bearing EXIF block with nothing else is uncommon enough to notice, which is the cost of keeping the date",
    },
    Signature {
        id: "jpeg-total-strip",
        name: "a JPEG with every marker segment removed",
        format: FormatId::Jpeg,
        requires: &[],
        forbids: &["jpeg:app0-jfif", "jpeg:app1-exif", "jpeg:app2-icc", "jpeg:app13-iptc", "jpeg:com"],
        crowd: Crowd::Small,
        provenance: Provenance::Assumed,
        note: "removing even the JFIF header is the pattern of a dedicated metadata stripper, and identifies you as a user of one",
    },
    Signature {
        id: "png-plain",
        name: "a PNG with no textual chunks",
        format: FormatId::Png,
        requires: &[],
        forbids: &["png:text", "png:time", "png:exif", "png:exif-protected-only", "png:xmp"],
        crowd: Crowd::Large,
        provenance: Provenance::Structural,
        note: "most PNGs written by libraries and browsers carry no text chunks at all",
    },
    Signature {
        id: "png-orientation-only",
        name: "a PNG keeping only what it must",
        format: FormatId::Png,
        requires: &["png:exif-protected-only"],
        forbids: &["png:text", "png:time", "png:exif", "png:xmp"],
        crowd: Crowd::Small,
        provenance: Provenance::Assumed,
        note: "an EXIF block holding nothing but an orientation is what is left when a PNG that had one is cleaned, and few PNGs have one at all; it stays because removing it turns the picture on its side",
    },
    Signature {
        id: "png-time-only",
        name: "a PNG keeping only its modification time",
        format: FormatId::Png,
        requires: &["png:time"],
        forbids: &["png:text", "png:exif", "png:xmp"],
        crowd: Crowd::Medium,
        provenance: Provenance::Structural,
        note: "a lone tIME chunk is written by few encoders, so keeping the date costs the large crowd",
    },
    Signature {
        id: "pdf-producer-only",
        name: "a PDF declaring only a producer",
        format: FormatId::Pdf,
        requires: &["pdf:producer"],
        forbids: &["pdf:author", "pdf:xmp", "pdf:embedded-files", "pdf:openaction-js", "pdf:incremental"],
        crowd: Crowd::Large,
        provenance: Provenance::Structural,
        note: "almost every PDF in circulation names a producer and nothing else; an empty Info dict is rarer than a generic one",
    },
    Signature {
        id: "pdf-dates-only",
        name: "a PDF keeping only its dates",
        format: FormatId::Pdf,
        requires: &["pdf:creation-date"],
        forbids: &["pdf:author", "pdf:producer", "pdf:xmp", "pdf:embedded-files", "pdf:openaction-js", "pdf:incremental"],
        crowd: Crowd::Medium,
        provenance: Provenance::Structural,
        note: "dated but with no producer named is an unusual combination; a profile that replaces the producer with a generic string instead of removing it reaches a larger crowd, at the cost of writing a string that is not strictly true",
    },
    Signature {
        id: "pdf-no-info",
        name: "a PDF with no document information at all",
        format: FormatId::Pdf,
        requires: &[],
        forbids: &["pdf:producer", "pdf:author", "pdf:xmp", "pdf:creation-date"],
        crowd: Crowd::Small,
        provenance: Provenance::Assumed,
        note: "a completely empty Info dict is the pattern of a stripper",
    },
    Signature {
        id: "ooxml-minimal-props",
        name: "an Office document with empty core properties",
        format: FormatId::Ooxml,
        requires: &["ooxml:core-props"],
        forbids: &["ooxml:app-props-identity", "ooxml:thumbnail", "ooxml:rsid", "ooxml:tracked-changes", "ooxml:comments"],
        crowd: Crowd::Medium,
        provenance: Provenance::Structural,
        note: "core.xml present but blank is what a document authored and never saved by a named user looks like",
    },
    Signature {
        id: "svg-plain",
        name: "an SVG with no editor metadata",
        format: FormatId::Svg,
        requires: &[],
        forbids: &["svg:metadata", "svg:editor-namespace", "svg:offcanvas"],
        crowd: Crowd::Large,
        provenance: Provenance::Structural,
        note: "hand-written and exported SVGs commonly carry nothing; editor namespaces are the outlier",
    },
    Signature {
        id: "mp4-plain",
        name: "an MP4 with no user-data boxes",
        format: FormatId::Mp4,
        requires: &[],
        forbids: &["mp4:udta", "mp4:ilst", "mp4:loci", "mp4:xyz"],
        crowd: Crowd::Large,
        provenance: Provenance::Structural,
        note: "transcoded and streamed MP4s rarely carry udta; handset originals almost always do",
    },
];

/// Whether files of this format normally carry a timestamp at all.
///
/// Almost every PDF and every camera JPEG has a date in it, so keeping one
/// costs nothing. A PNG with a `tIME` chunk or an SVG with a `dc:date` is the
/// outlier, so there the date is the whole fingerprint. Under
/// [`crate::profile::Blending::MatchCommon`] a handler may drop a timestamp the
/// profile would have kept, for exactly this reason, and say so in the diff.
pub fn time_is_expected(format: FormatId) -> bool {
    match format {
        FormatId::Jpeg | FormatId::Pdf | FormatId::Ooxml | FormatId::Opendocument | FormatId::Mp4 => true,
        FormatId::Png | FormatId::Svg | FormatId::Unknown => false,
    }
}

/// Best match for a shape, most-common-first.
pub fn classify(format: FormatId, shape: &[String]) -> Option<&'static Signature> {
    SIGNATURES.iter().find(|s| s.format == format && s.matches(shape))
}

pub fn by_id(id: &str) -> Option<&'static Signature> {
    SIGNATURES.iter().find(|s| s.id == id)
}

/// Every signature for a format, for the GUI's profile editor.
pub fn for_format(format: FormatId) -> impl Iterator<Item = &'static Signature> {
    SIGNATURES.iter().filter(move |s| s.format == format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_stripped_jpeg_with_jfif_joins_the_large_crowd() {
        let s = classify(FormatId::Jpeg, &shape(&["jpeg:app0-jfif"])).unwrap();
        assert_eq!(s.id, "jpeg-plain-baseline");
        assert_eq!(s.crowd, Crowd::Large);
    }

    #[test]
    fn dropping_the_jfif_header_too_shrinks_the_crowd() {
        let s = classify(FormatId::Jpeg, &shape(&[])).unwrap();
        assert_eq!(s.id, "jpeg-total-strip");
        assert_eq!(s.crowd, Crowd::Small);
    }

    #[test]
    fn an_uncleaned_phone_photo_matches_nothing() {
        let dirty = shape(&["jpeg:app0-jfif", "jpeg:app1-exif", "exif:gps", "jpeg:app13-iptc"]);
        assert!(classify(FormatId::Jpeg, &dirty).is_none());
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = SIGNATURES.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }
}
