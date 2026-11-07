//! The filesystem host.
//!
//! One driver for [`crate::decide`], for every place a sweep does work on real
//! paths: a desktop, a server, Termux on a rooted phone, and the tests. The
//! Android app is the other driver and shares every decision with this one.

use crate::decide::{self, Item, Plan, Resolution, Skip};
use crate::journal::Journal;
use crate::sweep::{Conditions, Report};
use fm_core::format;
use fm_core::profile::Profile;
use fm_core::scan::{self, ScanOptions};
use std::path::{Path, PathBuf};

fn mtime_of(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn item_of(path: &Path, bytes: u64) -> Item {
    Item {
        id: path.display().to_string(),
        name: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
        bytes,
        mtime: mtime_of(path),
        ext: format::extension(path),
    }
}

/// Run one sweep over the profile's folders.
///
/// The journal is written after every file rather than at the end, because the
/// host will kill this partway and a journal that only exists at the end would
/// mean starting over every time.
pub fn sweep(
    profile: &Profile,
    journal: &mut Journal,
    journal_path: &Path,
    conditions: Conditions,
) -> Report {
    let mut report = Report::default();
    let options = ScanOptions { recursive: true, ..Default::default() };
    let mut present: Vec<String> = Vec::new();

    for root in profile.runner.roots.iter().map(PathBuf::from) {
        for candidate in scan::scan(&root, &options).candidates {
            let item = item_of(&candidate.path, candidate.bytes);
            present.push(item.id.clone());

            match decide::plan(&item, profile, journal, conditions) {
                Plan::Pass(Skip::StillSettling) => {
                    report.skipped_recent += 1;
                    continue;
                }
                Plan::Pass(_) => {
                    report.skipped_seen += 1;
                    continue;
                }
                Plan::Examine => {}
            }

            let Ok(data) = std::fs::read(&candidate.path) else {
                report.failed += 1;
                let resolution = Resolution::Failed("could not be read".to_string());
                decide::record(journal, &item, profile, conditions, &resolution, None);
                let _ = journal.save(journal_path);
                continue;
            };
            report.examined += 1;

            let resolution = decide::resolve(&item, &data, profile);
            let wrote = match &resolution {
                Resolution::Write { data, .. } => {
                    report.cleaned += 1;
                    report.bytes_saved += candidate.bytes as i64 - data.len() as i64;
                    write_back(&candidate.path, data, profile).ok()
                }
                Resolution::AlreadyClean => {
                    report.already_clean += 1;
                    None
                }
                Resolution::Refused(_) => {
                    report.refused += 1;
                    None
                }
                Resolution::Failed(_) => {
                    report.failed += 1;
                    None
                }
            };
            decide::record(journal, &item, profile, conditions, &resolution, wrote);
            let _ = journal.save(journal_path);
        }
    }

    journal.last_sweep = Some(conditions.now);
    journal.forget_absent(&present);
    let _ = journal.save(journal_path);
    report
}

/// Write the cleaned bytes, and return the size and time they ended up with.
///
/// In place by default: a cleaned copy sitting next to the dirty original in the
/// same gallery folder protects nobody and leaves the user with two of
/// everything. Writing through a temporary file and renaming means a kill
/// mid-write cannot destroy the only copy.
fn write_back(path: &Path, data: &[u8], profile: &Profile) -> std::io::Result<(u64, i64)> {
    let destination = if profile.runner.keep_original { beside(path) } else { path.to_path_buf() };
    let tmp = destination.with_extension(match destination.extension() {
        Some(e) => format!("{}.fm-part", e.to_string_lossy()),
        None => "fm-part".to_string(),
    });
    std::fs::write(&tmp, data)?;
    let _ = fm_core::fsmeta::apply(&tmp, profile);
    std::fs::rename(&tmp, &destination)?;
    let _ = fm_core::fsmeta::apply(&destination, profile);
    Ok((std::fs::metadata(&destination)?.len(), mtime_of(&destination)))
}

fn beside(source: &Path) -> PathBuf {
    let stem = source.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let parent = source.parent().unwrap_or(Path::new("."));
    match source.extension() {
        Some(ext) => parent.join(format!("{stem}-clean.{}", ext.to_string_lossy())),
        None => parent.join(format!("{stem}-clean")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Outcome;
    use fm_core::model::Class;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fm-runner-sweep-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(name: &str) -> Option<PathBuf> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/out")
            .join(name);
        path.exists().then_some(path)
    }

    /// A profile with the runner switched on, watching one folder.
    fn runner_profile(root: &Path) -> Profile {
        let mut p = Profile::standard();
        p.runner.enabled = true;
        p.runner.interval_hours = 6;
        p.runner.roots = vec![root.display().to_string()];
        p
    }

    fn conditions(now: i64) -> Conditions {
        Conditions { charging: true, battery: 100, now }
    }

    /// Make the file look as though it was written long enough ago to settle.
    fn backdate(path: &Path, seconds: i64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds as u64);
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(when).set_accessed(when)).unwrap();
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn a_sweep_cleans_in_place_and_records_what_it_left() {
        let Some(source) = fixture("photo-phone.jpg") else {
            eprintln!("skipping: fixtures/out absent");
            return;
        };
        let dir = scratch("inplace");
        let photo = dir.join("IMG_0001.jpg");
        std::fs::copy(&source, &photo).unwrap();
        backdate(&photo, 600);

        let profile = runner_profile(&dir);
        let mut journal = Journal::default();
        let journal_path = dir.join("journal.json");
        let report = sweep(&profile, &mut journal, &journal_path, conditions(now()));

        assert_eq!(report.cleaned, 1, "{report:?}");
        assert!(report.bytes_saved > 0);
        assert!(photo.exists(), "the file was renamed or lost");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "a stray copy was left");

        let after = fm_core::inspect_bytes(&std::fs::read(&photo).unwrap(), Some("jpg")).unwrap();
        assert!(after.in_class(Class::Location).next().is_none(), "the GPS survived");

        match &journal.get(&photo.display().to_string()).unwrap().outcome {
            Outcome::Cleaned { bytes, .. } => {
                assert_eq!(*bytes, std::fs::metadata(&photo).unwrap().len())
            }
            other => panic!("not recorded as cleaned: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_second_sweep_does_not_touch_what_the_first_cleaned() {
        let Some(source) = fixture("photo-phone.jpg") else { return };
        let dir = scratch("idempotent");
        let photo = dir.join("IMG_0002.jpg");
        std::fs::copy(&source, &photo).unwrap();
        backdate(&photo, 600);

        let profile = runner_profile(&dir);
        let mut journal = Journal::default();
        let journal_path = dir.join("journal.json");
        sweep(&profile, &mut journal, &journal_path, conditions(now()));
        let after_first = std::fs::read(&photo).unwrap();

        let second = sweep(&profile, &mut journal, &journal_path, conditions(now() + 7 * 3600));
        assert_eq!(second.examined, 0, "{second:?}");
        assert_eq!(second.skipped_seen, 1);
        assert_eq!(std::fs::read(&photo).unwrap(), after_first, "the file was rewritten");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_edited_since_the_last_sweep_is_looked_at_again() {
        let Some(source) = fixture("photo-phone.jpg") else { return };
        let dir = scratch("edited");
        let photo = dir.join("IMG_0003.jpg");
        std::fs::copy(&source, &photo).unwrap();
        backdate(&photo, 600);

        let profile = runner_profile(&dir);
        let mut journal = Journal::default();
        let journal_path = dir.join("journal.json");
        sweep(&profile, &mut journal, &journal_path, conditions(now()));

        // Something put the metadata back.
        std::fs::copy(&source, &photo).unwrap();
        backdate(&photo, 600);
        let second = sweep(&profile, &mut journal, &journal_path, conditions(now() + 7 * 3600));
        assert_eq!(second.cleaned, 1, "an edited file was passed over: {second:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_still_being_written_is_left_alone() {
        let Some(source) = fixture("photo-phone.jpg") else { return };
        let dir = scratch("settle");
        let photo = dir.join("IMG_0004.jpg");
        std::fs::copy(&source, &photo).unwrap();
        // Just written, so it may still be growing.

        let profile = runner_profile(&dir);
        let mut journal = Journal::default();
        let report = sweep(&profile, &mut journal, &dir.join("journal.json"), conditions(now()));
        assert_eq!(report.examined, 0, "{report:?}");
        assert_eq!(report.skipped_recent, 1);
        assert!(journal.get(&photo.display().to_string()).is_none(), "an untouched file should not be recorded");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn changing_the_profile_makes_the_runner_reconsider_every_file() {
        let Some(source) = fixture("photo-phone.jpg") else { return };
        let dir = scratch("profile-change");
        let photo = dir.join("IMG_0005.jpg");
        std::fs::copy(&source, &photo).unwrap();
        backdate(&photo, 600);

        let standard = runner_profile(&dir);
        let mut journal = Journal::default();
        let journal_path = dir.join("journal.json");
        sweep(&standard, &mut journal, &journal_path, conditions(now()));

        // standard keeps the capture date; strict coarsens it, so there is more
        // to do and the file has to be looked at again.
        let mut strict = Profile::strict();
        strict.runner = standard.runner.clone();
        let second = sweep(&strict, &mut journal, &journal_path, conditions(now() + 7 * 3600));
        assert_eq!(second.examined, 1, "{second:?}");
        assert_eq!(second.cleaned, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_signed_document_is_recorded_as_refused_and_not_retried() {
        let Some(source) = fixture("signed.pdf") else { return };
        let dir = scratch("refused");
        let pdf = dir.join("mandate.pdf");
        std::fs::copy(&source, &pdf).unwrap();
        backdate(&pdf, 600);
        let original = std::fs::read(&pdf).unwrap();

        let profile = runner_profile(&dir);
        let mut journal = Journal::default();
        let journal_path = dir.join("journal.json");
        let first = sweep(&profile, &mut journal, &journal_path, conditions(now()));
        assert_eq!(first.refused, 1, "{first:?}");
        assert_eq!(std::fs::read(&pdf).unwrap(), original, "a signed file was modified");

        let second = sweep(&profile, &mut journal, &journal_path, conditions(now() + 7 * 3600));
        assert_eq!(second.examined, 0, "a refusal should not be retried every sweep");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keeping_the_original_writes_beside_it() {
        let Some(source) = fixture("screenshot.png") else { return };
        let dir = scratch("keep");
        let png = dir.join("Screenshot.png");
        std::fs::copy(&source, &png).unwrap();
        backdate(&png, 600);

        let mut profile = runner_profile(&dir);
        profile.runner.keep_original = true;
        let mut journal = Journal::default();
        sweep(&profile, &mut journal, &dir.join("journal.json"), conditions(now()));

        assert!(dir.join("Screenshot-clean.png").exists());
        assert_eq!(std::fs::read(&png).unwrap(), std::fs::read(&source).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_sweep_of_an_empty_folder_says_nothing_new() {
        let dir = scratch("empty");
        let profile = runner_profile(&dir);
        let mut journal = Journal::default();
        let report = sweep(&profile, &mut journal, &dir.join("journal.json"), conditions(now()));
        assert_eq!(report, Report { examined: 0, ..Default::default() });
        assert_eq!(report.sentence(), "Nothing new to clean.");
        assert!(journal.last_sweep.is_some(), "the sweep still happened");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_journal_is_on_disk_before_the_sweep_ends() {
        let Some(source) = fixture("photo-phone.jpg") else { return };
        let dir = scratch("crashsafe");
        for n in 0..3 {
            let photo = dir.join(format!("IMG_{n}.jpg"));
            std::fs::copy(&source, &photo).unwrap();
            backdate(&photo, 600);
        }
        let profile = runner_profile(&dir);
        let journal_path = dir.join("journal.json");
        let mut journal = Journal::default();
        sweep(&profile, &mut journal, &journal_path, conditions(now()));

        // Reloading from disk has to show the same work, because a phone will
        // kill the task and the next sweep starts from the file.
        let reloaded = Journal::load(&journal_path);
        assert_eq!(reloaded.counts().cleaned, 3);
        assert_eq!(reloaded.last_sweep, journal.last_sweep);
        let _ = std::fs::remove_dir_all(dir);
    }

}
