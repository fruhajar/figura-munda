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
    /// Write the cleaned file somewhere else and leave the original alone,
    /// refusing to put it where something else already is.
    Copy,
    /// The same, but replacing whatever is at the destination, because the
    /// caller has said so in as many words.
    Overwrite,
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

    guard_destination(source, dest, disposition, &cleaned.data)?;

    // Write beside the destination and rename, so an interrupted run never
    // leaves a partially cleaned file where a whole one was.
    let tmp = unique_temp(dest);
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

/// Everything that has to be true before a byte is written.
///
/// Each of these was a way to lose a file. `--out` pointed at the folder the
/// originals are in replaced them with no warning and no mention in the
/// report. A destination that already held an unrelated file of the same name
/// was overwritten. And cleaning a symlink or one name of a hardlinked file in
/// place replaced the link or that one name, leaving the content it pointed at
/// as dirty as before, which reads as a clean that happened when it did not.
fn guard_destination(
    source: &Path,
    dest: &Path,
    disposition: Disposition,
    result: &[u8],
) -> Result<()> {
    let same_file = same_path(source, dest);
    if disposition != Disposition::InPlace && same_file {
        return Err(Error::Refused(format!(
            "the destination is the original, {}. Writing there replaces it, so say that is \
             what you mean with the in-place option, or choose another folder.",
            source.display()
        )));
    }
    if disposition == Disposition::Copy && !same_file && dest.exists() {
        // Cleaning is idempotent, so a destination already holding exactly
        // this result is this tool's own earlier output, and rewriting it
        // changes nothing. Anything else is somebody's file.
        let ours = fs::read(dest).is_ok_and(|existing| existing == result);
        if !ours {
            return Err(Error::Refused(format!(
                "{} already exists and holds something other than this file cleaned. Nothing \
                 here overwrites a file it did not write; move it, or ask for it to be \
                 replaced.",
                dest.display()
            )));
        }
    }
    if disposition == Disposition::InPlace {
        let link = fs::symlink_metadata(source)?;
        if link.file_type().is_symlink() {
            let target = fs::read_link(source).unwrap_or_default();
            return Err(Error::Refused(format!(
                "{} is a link to {}. Cleaning it in place would leave a cleaned file where the \
                 link was and the original untouched where it points, so point this at {} \
                 instead.",
                source.display(),
                target.display(),
                target.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let names = link.nlink();
            if names > 1 {
                return Err(Error::Refused(format!(
                    "{} is one of {names} names for the same file. Cleaning it in place would \
                     give this name a cleaned copy and leave the others on the original, \
                     which no report would mention.",
                    source.display()
                )));
            }
        }
    }
    Ok(())
}

/// Whether two paths are the same file, links followed, falling back to
/// comparing the paths when one of them does not exist yet.
fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// A temporary path nothing else is using.
///
/// A folder run cleans in parallel, and two sources with the same name once
/// shared one `.fm-part` path, so each wrote over the other's bytes and
/// whichever renamed last won with whatever it happened to hold.
fn unique_temp(dest: &Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    dest.with_extension(match dest.extension() {
        Some(e) => format!("{}.{pid}-{n}.fm-part", e.to_string_lossy()),
        None => format!("{pid}-{n}.fm-part"),
    })
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

    /// The reason a refusal gave, or a panic saying what happened instead.
    fn refusal(result: Result<Cleaned>) -> String {
        match result {
            Err(Error::Refused(why)) => why,
            Err(e) => panic!("expected a refusal, got the error {e}"),
            Ok(_) => panic!("expected a refusal, the file was written"),
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fm-core-dest-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A one-pixel PNG with a text chunk, so there is something to remove and
    /// the cleaned bytes differ from the original.
    fn dirty_png() -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |kind: &[u8; 4], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            let mut crc = kind.to_vec();
            crc.extend_from_slice(data);
            c.extend_from_slice(&crc32fast(&crc).to_be_bytes());
            c
        };
        out.extend_from_slice(&chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        out.extend_from_slice(&chunk(b"tEXt", b"Author\0Marie Dvorakova"));
        out.extend_from_slice(&chunk(b"IEND", b""));
        out
    }

    fn crc32fast(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    /// `--out` pointed at the folder the originals are in replaced them, with
    /// no warning and nothing in the report to say the original was gone.
    #[test]
    fn a_destination_that_is_the_original_is_refused_unless_it_is_meant() {
        let dir = scratch("same");
        let file = dir.join("photo.png");
        fs::write(&file, dirty_png()).unwrap();

        let why = refusal(clean_file(&file, &file, &Profile::standard(), Disposition::Copy));
        assert!(why.contains("the original"), "{why}");
        assert_eq!(fs::read(&file).unwrap(), dirty_png(), "the original was touched");

        // In place is the same path on purpose, and that is allowed.
        clean_file(&file, &file, &Profile::standard(), Disposition::InPlace).unwrap();
        assert_ne!(fs::read(&file).unwrap(), dirty_png(), "in place changed nothing");
    }

    /// An unrelated file of the same name at the destination was overwritten.
    #[test]
    fn a_destination_holding_someone_elses_file_is_refused() {
        let dir = scratch("occupied");
        let source = dir.join("photo.png");
        let dest = dir.join("out").join("photo.png");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&source, dirty_png()).unwrap();
        fs::write(&dest, b"a letter from my accountant").unwrap();

        let why = refusal(clean_file(&source, &dest, &Profile::standard(), Disposition::Copy));
        assert!(why.contains("already exists"), "{why}");
        assert_eq!(fs::read(&dest).unwrap(), b"a letter from my accountant");

        // Asked for in as many words, it is replaced.
        clean_file(&source, &dest, &Profile::standard(), Disposition::Overwrite).unwrap();
        assert_ne!(fs::read(&dest).unwrap(), b"a letter from my accountant");
    }

    /// Cleaning is idempotent, so a destination already holding exactly this
    /// result is this tool's own earlier output. Refusing that would make a
    /// second run of the same job fail for no reason.
    #[test]
    fn a_destination_holding_this_files_own_clean_is_written_again() {
        let dir = scratch("again");
        let source = dir.join("photo.png");
        let dest = dir.join("photo-clean.png");
        fs::write(&source, dirty_png()).unwrap();

        let first = clean_file(&source, &dest, &Profile::standard(), Disposition::Copy).unwrap();
        let written = fs::read(&dest).unwrap();
        assert_eq!(written, first.data);
        clean_file(&source, &dest, &Profile::standard(), Disposition::Copy)
            .expect("a second run of the same job was refused");
        assert_eq!(fs::read(&dest).unwrap(), written);
    }

    /// Cleaning a link in place replaced the link with a file and left what it
    /// pointed at as dirty as before, which reads as a clean that happened.
    #[cfg(unix)]
    #[test]
    fn a_link_is_not_cleaned_in_place() {
        let dir = scratch("link");
        let real = dir.join("original.png");
        let link = dir.join("link.png");
        fs::write(&real, dirty_png()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let why = refusal(clean_file(&link, &link, &Profile::standard(), Disposition::InPlace));
        assert!(why.contains("is a link to"), "{why}");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link went");
        assert_eq!(fs::read(&real).unwrap(), dirty_png(), "the target was touched");
    }

    /// One name of a hardlinked file cleaned in place leaves every other name
    /// on the original content, and no report mentions them.
    #[cfg(unix)]
    #[test]
    fn a_file_with_more_than_one_name_is_not_cleaned_in_place() {
        let dir = scratch("hardlink");
        let first = dir.join("photo.png");
        let second = dir.join("same-photo.png");
        fs::write(&first, dirty_png()).unwrap();
        fs::hard_link(&first, &second).unwrap();

        let why =
            refusal(clean_file(&first, &first, &Profile::standard(), Disposition::InPlace));
        assert!(why.contains("names for the same file"), "{why}");
        assert_eq!(fs::read(&first).unwrap(), dirty_png());
        assert_eq!(fs::read(&second).unwrap(), dirty_png());
    }

    /// A folder run cleans in parallel, and two sources with the same name
    /// shared one `.fm-part` path, so each wrote over the other's bytes.
    #[test]
    fn two_runs_for_one_destination_do_not_share_a_temporary_path() {
        let dest = std::path::Path::new("/tmp/out/photo.png");
        let a = unique_temp(dest);
        let b = unique_temp(dest);
        assert_ne!(a, b);
        for t in [a, b] {
            let name = t.file_name().unwrap().to_string_lossy().to_string();
            assert!(name.ends_with(".fm-part"), "{name}");
            assert!(name.starts_with("photo."), "{name}");
        }
    }

    #[test]
    fn an_unrecognised_file_is_reported_as_unsupported() {
        assert!(matches!(
            inspect_bytes(b"just some text, honestly", None),
            Err(Error::Unsupported)
        ));
    }
}
