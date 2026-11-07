//! What the runner has already done.
//!
//! A sweep has to be able to tell a file it has cleaned from one it has not,
//! which on a phone cannot rely on a filename: the camera writes `IMG_0001.jpg`
//! and the runner cleans it in place, so the name is the same before and after.
//! The journal records the size and modification time it left behind, which is
//! enough to recognise its own work and to notice when a file has been edited
//! since.
//!
//! Entries are keyed by whatever the host calls a file, as a string. On a
//! desktop that is a path; on Android it is a document URI, because that is
//! what the platform hands out and a path would not survive a reboot.
//!
//! It is written after every file, not at the end of a sweep, because a phone
//! will kill a background task mid-run and a journal that only exists at the
//! end would mean starting over every time.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// What happened to one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum Outcome {
    /// Cleaned, and these are the size and modification time left behind.
    Cleaned { bytes: u64, mtime: i64 },
    /// Already clean under this profile; nothing was written.
    AlreadyClean,
    /// Deliberately not touched, with the reason. A signed PDF is the case.
    Refused { reason: String },
    /// Failed. Recorded so that the same broken file is not retried forever.
    Failed { reason: String, attempts: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// When the runner last acted on this file, as unix seconds.
    pub at: i64,
    /// Which profile was in force. A different profile means a fresh decision.
    pub profile: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// How many times a failing file is retried before the runner leaves it alone.
///
/// Without a cap, one malformed file means one failure in every sweep forever,
/// which on a phone is a battery drain and a notification the user cannot act
/// on.
pub const MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Journal {
    /// When the last sweep finished, as unix seconds.
    pub last_sweep: Option<i64>,
    pub entries: BTreeMap<String, Entry>,
}

impl Journal {
    pub fn load(path: &Path) -> Journal {
        // A missing or corrupt journal is not an error: the worst it costs is
        // one sweep that re-examines files it has already seen, and refusing to
        // run because the bookkeeping is unreadable would be worse.
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write and rename, so a kill mid-write cannot leave an unreadable
        // journal in place of a good one.
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }

    pub fn record(&mut self, id: &str, profile: &str, at: i64, outcome: Outcome) {
        self.entries
            .insert(id.to_string(), Entry { at, profile: profile.to_string(), outcome });
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    /// How many times this file has already failed.
    pub fn attempts(&self, id: &str) -> u32 {
        match self.get(id).map(|e| &e.outcome) {
            Some(Outcome::Failed { attempts, .. }) => *attempts,
            _ => 0,
        }
    }

    /// Forget everything not in `present`, so the journal does not grow without
    /// limit as a camera roll turns over. The host supplies the list, because
    /// only the host can say what still exists.
    pub fn forget_absent(&mut self, present: &[String]) -> usize {
        let before = self.entries.len();
        self.entries.retain(|id, _| present.iter().any(|p| p == id));
        before - self.entries.len()
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for entry in self.entries.values() {
            match entry.outcome {
                Outcome::Cleaned { .. } => c.cleaned += 1,
                Outcome::AlreadyClean => c.already_clean += 1,
                Outcome::Refused { .. } => c.refused += 1,
                Outcome::Failed { .. } => c.failed += 1,
            }
        }
        c
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub cleaned: usize,
    pub already_clean: usize,
    pub refused: usize,
    pub failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fm-runner-journal-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = scratch("roundtrip");
        let path = dir.join("journal.json");
        let mut j = Journal { last_sweep: Some(1_718_394_127), ..Default::default() };
        j.record(
            "content://media/external/images/media/41",
            "standard",
            1_718_394_127,
            Outcome::Cleaned { bytes: 2048, mtime: 1_718_394_127 },
        );
        j.save(&path).unwrap();

        let back = Journal::load(&path);
        assert_eq!(back.last_sweep, j.last_sweep);
        assert_eq!(back.entries, j.entries);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_journal_reads_as_empty_rather_than_failing() {
        let dir = scratch("corrupt");
        let path = dir.join("journal.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        assert!(Journal::load(&path).entries.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_journal_reads_as_empty() {
        assert!(Journal::load(Path::new("/nowhere/at/all.json")).entries.is_empty());
    }

    #[test]
    fn saving_leaves_no_partial_file_behind() {
        let dir = scratch("atomic");
        let path = dir.join("journal.json");
        Journal::default().save(&path).unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["journal.json"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failures_are_counted_so_they_can_be_given_up_on() {
        let mut j = Journal::default();
        let id = "/sdcard/broken.jpg";
        assert_eq!(j.attempts(id), 0);
        j.record(id, "standard", 1, Outcome::Failed { reason: "bad".into(), attempts: 2 });
        assert_eq!(j.attempts(id), 2);
        assert_eq!(j.counts().failed, 1);
    }

    #[test]
    fn entries_the_host_no_longer_lists_are_forgotten() {
        let mut j = Journal::default();
        j.record("content://x/1", "standard", 1, Outcome::AlreadyClean);
        j.record("content://x/gone", "standard", 1, Outcome::AlreadyClean);
        assert_eq!(j.forget_absent(&["content://x/1".to_string()]), 1);
        assert_eq!(j.entries.len(), 1);
    }
}
