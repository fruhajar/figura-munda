//! The option model: what is in a file, and what can be done to each kind of it.
//!
//! The ten classes and the five actions are the vocabulary the whole tool is
//! built on. A format handler's only job is to map its own fields onto them.

use serde::{Deserialize, Serialize};
use std::fmt;

/// What a file can reveal. The ten classes from the root README.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Identity,
    Device,
    Location,
    Time,
    /// `DocumentID`, PDF `/ID`, OOXML `rsid`. Links two files to one author or
    /// session even with every name stripped.
    Correlation,
    History,
    HiddenPayload,
    RedactionIntegrity,
    StructuralFingerprint,
    Filesystem,
}

pub const ALL_CLASSES: [Class; 10] = [
    Class::Identity,
    Class::Device,
    Class::Location,
    Class::Time,
    Class::Correlation,
    Class::History,
    Class::HiddenPayload,
    Class::RedactionIntegrity,
    Class::StructuralFingerprint,
    Class::Filesystem,
];

impl Class {
    pub fn title(self) -> &'static str {
        match self {
            Class::Identity => "Identity",
            Class::Device => "Device",
            Class::Location => "Location",
            Class::Time => "Time",
            Class::Correlation => "Correlation handles",
            Class::History => "History",
            Class::HiddenPayload => "Hidden payload",
            Class::RedactionIntegrity => "Redaction integrity",
            Class::StructuralFingerprint => "Structural fingerprint",
            Class::Filesystem => "Filesystem",
        }
    }

    /// The plain-language sentence the report leads with. Not a field dump.
    pub fn sentence(self) -> &'static str {
        match self {
            Class::Identity => "names the person or organisation behind it",
            Class::Device => "identifies what took or wrote it",
            Class::Location => "records where it was made",
            Class::Time => "notes when it was made, down to the second",
            Class::Correlation => {
                "carries identifiers that tie it to your other files, even once the names are gone"
            }
            Class::History => "still contains earlier versions, edits or comments",
            Class::HiddenPayload => "contains more than it shows",
            Class::RedactionIntegrity => "has blacked-out content that can still be read",
            Class::StructuralFingerprint => "is laid out in a way that identifies the software that wrote it",
            Class::Filesystem => "travels with metadata the filesystem keeps about it",
        }
    }

    /// Which actions make sense for this class. Guards custom profiles.
    pub fn allows(self, action: Action) -> bool {
        match self {
            Class::Time => matches!(
                action,
                Action::Keep | Action::Remove | Action::CoarsenDay | Action::CoarsenMonth | Action::Normalise
            ),
            Class::RedactionIntegrity => matches!(action, Action::ReportOnly | Action::Enforce),
            Class::StructuralFingerprint => matches!(action, Action::Keep | Action::RewriteUniform),
            Class::Filesystem => matches!(action, Action::Keep | Action::Reset | Action::Normalise),
            _ => matches!(action, Action::Keep | Action::Remove | Action::Replace),
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

/// What can be done to a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Keep,
    Remove,
    /// Overwrite with a generic constant rather than deleting the field, for the
    /// cases where a reader breaks on absence.
    Replace,
    CoarsenDay,
    CoarsenMonth,
    /// Set to a fixed constant shared across every file and every user. Makes no
    /// false claim, unlike a plausible fake.
    Normalise,
    /// Redaction integrity only: say so in the report, change nothing.
    ReportOnly,
    /// Redaction integrity only: destroy the covered content.
    Enforce,
    /// Structural fingerprint only: rewrite container layout to a fixed shape.
    RewriteUniform,
    /// Filesystem only: reset to the time of cleaning rather than a constant.
    Reset,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Keep => "keep",
            Action::Remove => "remove",
            Action::Replace => "replace",
            Action::CoarsenDay => "coarsen to day",
            Action::CoarsenMonth => "coarsen to month",
            Action::Normalise => "normalise",
            Action::ReportOnly => "report only",
            Action::Enforce => "enforce",
            Action::RewriteUniform => "rewrite uniformly",
            Action::Reset => "reset",
        }
    }

    /// Whether the original value stops being recoverable from the output.
    pub fn alters(self) -> bool {
        !matches!(self, Action::Keep | Action::ReportOnly)
    }
}

/// How much a change costs beyond the metadata itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Impact {
    /// Metadata only. The rendered document is byte-identical in appearance.
    Metadata,
    /// Visible content disappears: off-canvas text, hidden slides, covered text
    /// under a redaction. Correct, but the user has to be told.
    Content,
    /// Cannot be done without rewriting the file wholesale.
    Structural,
}

/// One thing found in one file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub class: Class,
    /// Machine-readable address, e.g. `exif:ifd0:Artist`, `zip:docProps/core.xml:dc:creator`.
    pub locator: String,
    /// Plain-language description of what this field is, in the user's terms.
    pub label: String,
    /// The revealing value itself, where showing it is the point.
    pub value: Option<String>,
    /// Stripping this would be a regression: ICC profiles, tagged-PDF structure,
    /// anything inside a signed PDF. Never altered regardless of profile.
    pub protected: bool,
    /// Present in essentially every file of this format, so its presence in a
    /// cleaned file does not prove the original value survived: a tool that
    /// replaced it leaves a field that looks exactly like one never touched.
    /// The PDF trailer identifier is the case this exists for. Verify mode
    /// reports these separately rather than calling them failures.
    pub replaceable: bool,
    /// Worth reporting but outside what this tool can change, so verify mode
    /// must not hold it against a handler. A file's creation time on the
    /// filesystem is the example: it becomes the time of cleaning no matter
    /// what, because writing a new file is what cleaning is.
    pub advisory: bool,
    pub impact: Impact,
    pub note: Option<String>,
}

impl Finding {
    pub fn new(class: Class, locator: impl Into<String>, label: impl Into<String>) -> Self {
        Finding {
            class,
            locator: locator.into(),
            label: label.into(),
            value: None,
            protected: false,
            replaceable: false,
            advisory: false,
            impact: Impact::Metadata,
            note: None,
        }
    }

    pub fn advisory(mut self) -> Self {
        self.advisory = true;
        self
    }

    pub fn replaceable(mut self) -> Self {
        self.replaceable = true;
        self
    }

    pub fn value(mut self, v: impl Into<String>) -> Self {
        self.value = Some(v.into());
        self
    }

    pub fn protected(mut self) -> Self {
        self.protected = true;
        self
    }

    pub fn impact(mut self, i: Impact) -> Self {
        self.impact = i;
        self
    }

    pub fn note(mut self, n: impl Into<String>) -> Self {
        self.note = Some(n.into());
        self
    }
}

/// A reason to refuse rather than proceed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blocker {
    pub reason: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatId {
    Jpeg,
    Png,
    Svg,
    Ooxml,
    Opendocument,
    Pdf,
    Mp4,
    Unknown,
}

impl FormatId {
    pub fn label(self) -> &'static str {
        match self {
            FormatId::Jpeg => "JPEG image",
            FormatId::Png => "PNG image",
            FormatId::Svg => "SVG drawing",
            FormatId::Ooxml => "Office document",
            FormatId::Opendocument => "OpenDocument file",
            FormatId::Pdf => "PDF document",
            FormatId::Mp4 => "MP4 video",
            FormatId::Unknown => "unrecognised file",
        }
    }

    /// The noun the report uses: "This photo", "This document".
    pub fn noun(self) -> &'static str {
        match self {
            FormatId::Jpeg => "photo",
            FormatId::Png => "image",
            FormatId::Svg => "drawing",
            FormatId::Ooxml | FormatId::Opendocument => "document",
            FormatId::Pdf => "PDF",
            FormatId::Mp4 => "video",
            FormatId::Unknown => "file",
        }
    }
}

/// Everything the inspect phase learned about one file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inspection {
    pub format: FormatId,
    pub bytes: u64,
    pub findings: Vec<Finding>,
    pub blockers: Vec<Blocker>,
    /// Structural markers present in the file, as `format:marker` keys. Drives
    /// the anonymity-set check: it is the shape of the output, not its field
    /// values, that says which crowd a cleaned file joins.
    pub shape: Vec<String>,
}

impl Inspection {
    pub fn new(format: FormatId, bytes: u64) -> Self {
        Inspection {
            format,
            bytes,
            findings: Vec::new(),
            blockers: Vec::new(),
            shape: Vec::new(),
        }
    }

    pub fn mark(&mut self, marker: impl Into<String>) {
        let m = marker.into();
        if !self.shape.contains(&m) {
            self.shape.push(m);
        }
    }

    pub fn classes(&self) -> Vec<Class> {
        let mut seen: Vec<Class> = ALL_CLASSES
            .iter()
            .copied()
            .filter(|c| self.findings.iter().any(|f| f.class == *c && !f.protected))
            .collect();
        seen.dedup();
        seen
    }

    pub fn in_class(&self, c: Class) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.class == c)
    }

    pub fn refused(&self) -> bool {
        !self.blockers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_constrain_their_actions() {
        assert!(Class::Time.allows(Action::CoarsenDay));
        assert!(!Class::Identity.allows(Action::CoarsenDay));
        assert!(Class::RedactionIntegrity.allows(Action::Enforce));
        assert!(!Class::RedactionIntegrity.allows(Action::Remove));
        assert!(!Class::Location.allows(Action::RewriteUniform));
    }

    #[test]
    fn every_class_has_exactly_one_valid_default() {
        // A class with no permitted action would silently drop out of a profile.
        for c in ALL_CLASSES {
            assert!(
                [Action::Keep, Action::ReportOnly].iter().any(|a| c.allows(*a)),
                "{c} has no inert action"
            );
        }
    }

    #[test]
    fn protected_findings_do_not_raise_a_class() {
        let mut ins = Inspection::new(FormatId::Jpeg, 100);
        ins.findings.push(Finding::new(Class::Device, "icc", "colour profile").protected());
        assert!(ins.classes().is_empty());
    }
}
