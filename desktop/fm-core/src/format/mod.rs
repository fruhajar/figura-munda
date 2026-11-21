//! Format handlers.
//!
//! A handler's whole job is to map its own fields onto the ten classes and then
//! honour a class-to-action table. Nothing format-specific escapes this module.

use crate::error::Result;
use crate::model::{Action, Class, FormatId, Inspection};
use crate::profile::Profile;
use serde::{Deserialize, Serialize};

pub mod exif;
pub mod iptc;
pub mod jpeg;
pub mod mp4;
pub mod ooxml;
pub mod pdf;
pub mod png;
pub mod svg;
pub mod xmlfilter;
pub mod xmp;
pub mod zip;

/// One field-level edit, as it will appear in the before/after diff.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    pub class: Class,
    pub locator: String,
    pub label: String,
    pub action: Action,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Whether this is something cleaning did rather than a field the file
    /// disclosed: sample offsets corrected for what was removed ahead of them,
    /// objects pruned because nothing points at them any more.
    ///
    /// Everything else has to be named in the report that was shown before a
    /// byte was written, and [`crate::pipeline::clean_bytes`] refuses the clean
    /// if it is not. This flag is how a handler says "there was no field here
    /// to report", and saying it wrongly is how the guarantee would rot.
    pub derived: bool,
}

impl Change {
    pub fn removed(class: Class, locator: impl Into<String>, label: impl Into<String>, before: Option<String>) -> Self {
        Change {
            class,
            locator: locator.into(),
            label: label.into(),
            action: Action::Remove,
            before,
            after: None,
            derived: false,
        }
    }

    pub fn rewritten(
        class: Class,
        locator: impl Into<String>,
        label: impl Into<String>,
        action: Action,
        before: Option<String>,
        after: Option<String>,
    ) -> Self {
        Change { class, locator: locator.into(), label: label.into(), action, before, after, derived: false }
    }

    /// A consequence of cleaning rather than a field that was in the file, so
    /// no finding named it and none should have.
    pub fn derived(self) -> Self {
        Change { derived: true, ..self }
    }
}

/// The result of cleaning one file's bytes.
pub struct Outcome {
    pub data: Vec<u8>,
    pub changes: Vec<Change>,
}

impl Outcome {
    pub fn unchanged(data: &[u8]) -> Self {
        Outcome { data: data.to_vec(), changes: Vec::new() }
    }
}

pub trait Handler: Send + Sync {
    fn id(&self) -> FormatId;
    /// Recognise by content first, extension only as a tie-breaker. A phone
    /// photo renamed `.txt` still leaks GPS.
    fn sniff(&self, head: &[u8], ext: Option<&str>) -> bool;
    fn inspect(&self, data: &[u8]) -> Result<Inspection>;
    fn clean(&self, data: &[u8], profile: &Profile) -> Result<Outcome>;
}

pub fn handlers() -> &'static [&'static dyn Handler] {
    &[
        &jpeg::Jpeg,
        &png::Png,
        &ooxml::Ooxml,
        &pdf::Pdf,
        &mp4::Mp4,
        // SVG last: it is the only text format, so it must not shadow a
        // container that happens to contain the string "<svg".
        &svg::Svg,
    ]
}

pub fn for_bytes(data: &[u8], ext: Option<&str>) -> Option<&'static dyn Handler> {
    let head = &data[..data.len().min(4096)];
    handlers().iter().copied().find(|h| h.sniff(head, ext))
}

/// Case-folded extension of a path, without the dot.
pub fn extension(path: &std::path::Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_handler_has_a_distinct_id() {
        let mut ids: Vec<FormatId> = handlers().iter().map(|h| h.id()).collect();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn an_empty_file_matches_nothing() {
        assert!(for_bytes(&[], None).is_none());
    }
}
