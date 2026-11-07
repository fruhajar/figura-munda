//! inspect, then plan, then apply, then verify.
//!
//! The order is the product. A file is never written before it has been
//! inspected, and never reported as cleaned before the output has been
//! re-inspected and found clean.

use crate::error::{Error, Result};
use crate::format::{self, Change, Handler};
use crate::fsmeta;
use crate::model::{Class, Inspection};
use crate::profile::Profile;
use crate::report::{Counts, Verification};
use std::fs;
use std::path::Path;

pub struct Cleaned {
    /// What was in the file before.
    pub before: Inspection,
    /// What is in the file afterwards.
    pub after: Inspection,
    pub changes: Vec<Change>,
    pub verification: Verification,
    pub counts: Counts,
    pub data: Vec<u8>,
}

impl Cleaned {
    pub fn bytes_saved(&self) -> i64 {
        self.before.bytes as i64 - self.after.bytes as i64
    }
}

fn handler_for(data: &[u8], ext: Option<&str>) -> Result<&'static dyn Handler> {
    format::for_bytes(data, ext).ok_or(Error::Unsupported)
}

pub fn inspect_bytes(data: &[u8], ext: Option<&str>) -> Result<Inspection> {
    handler_for(data, ext)?.inspect(data)
}

/// Inspect a file on disk, including what the filesystem keeps about it.
pub fn inspect_file(path: &Path) -> Result<Inspection> {
    let data = fs::read(path)?;
    let ext = format::extension(path);
    let mut ins = inspect_bytes(&data, ext.as_deref())?;
    ins.findings.extend(fsmeta::inspect(path));
    Ok(ins)
}

/// Clean bytes in memory. Does not touch the filesystem class, which has no
/// meaning without a path.
pub fn clean_bytes(data: &[u8], ext: Option<&str>, profile: &Profile) -> Result<Cleaned> {
    let handler = handler_for(data, ext)?;
    let before = handler.inspect(data)?;
    if let Some(b) = before.blockers.first() {
        return Err(Error::Refused(format!("{}: {}", b.reason, b.detail)));
    }
    let outcome = handler.clean(data, profile)?;
    let after = handler.inspect(&outcome.data)?;
    let verification = Verification::after(&after, profile, &outcome.changes);
    let counts = Counts::of(&before, &outcome.changes);
    Ok(Cleaned {
        before,
        after,
        changes: outcome.changes,
        verification,
        counts,
        data: outcome.data,
    })
}

/// How to treat the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Write the cleaned file somewhere else and leave the original alone.
    Copy,
    /// Replace the original, via a temporary file in the same directory so a
    /// crash cannot leave a half-written file in place of the only copy.
    InPlace,
}

/// Clean a file on disk. The destination is written only if verification passes.
pub fn clean_file(
    source: &Path,
    dest: &Path,
    profile: &Profile,
    disposition: Disposition,
) -> Result<Cleaned> {
    let data = fs::read(source)?;
    let ext = format::extension(source);
    let mut cleaned = clean_bytes(&data, ext.as_deref(), profile)?;
    cleaned.before.findings.extend(fsmeta::inspect(source));

    if !cleaned.verification.clean() {
        let names: Vec<String> = cleaned
            .verification
            .leftovers
            .iter()
            .map(|f| f.locator.clone())
            .collect();
        return Err(Error::VerifyFailed(names.join(", ")));
    }

    // Write beside the destination and rename, so an interrupted run never
    // leaves a partially cleaned file where a whole one was.
    let tmp = dest.with_extension(match dest.extension() {
        Some(e) => format!("{}.fm-part", e.to_string_lossy()),
        None => "fm-part".to_string(),
    });
    fs::write(&tmp, &cleaned.data)?;

    if profile.action(Class::Filesystem) == crate::model::Action::Keep {
        // Keeping the filesystem class means keeping the original's times,
        // not acquiring today's.
        let _ = fsmeta::carry_times(source, &tmp);
    }
    for note in fsmeta::apply(&tmp, profile)? {
        cleaned.changes.push(Change::rewritten(
            Class::Filesystem,
            "fs",
            note,
            profile.action(Class::Filesystem),
            None,
            None,
        ));
    }

    fs::rename(&tmp, dest)?;
    if disposition == Disposition::InPlace && source != dest {
        fs::remove_file(source)?;
    }
    // Zone marks live beside the file, so they have to be cleared at the
    // destination path after the rename as well as on the temporary.
    for note in fsmeta::apply(dest, profile)? {
        let _ = note;
    }
    cleaned.after.findings.extend(fsmeta::inspect(dest));
    Ok(cleaned)
}

/// Re-inspect bytes and check them against a profile. Verify mode, standalone.
pub fn verify_bytes(data: &[u8], ext: Option<&str>, profile: &Profile) -> Result<Verification> {
    let ins = inspect_bytes(data, ext)?;
    Ok(Verification::of(&ins, profile))
}

pub fn verify_file(path: &Path, profile: &Profile) -> Result<Verification> {
    let ins = inspect_file(path)?;
    Ok(Verification::of(&ins, profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecognised_file_is_reported_as_unsupported() {
        assert!(matches!(
            inspect_bytes(b"just some text, honestly", None),
            Err(Error::Unsupported)
        ));
    }
}
