//! The decision, with no file system in it.
//!
//! This exists because of Android. Since scoped storage an app does not reach
//! the camera roll through file paths: the user grants a folder once and the app
//! gets document URIs, which are opened through the platform's own content
//! resolver. A runner built on `std::fs` would work on a desktop and be unable
//! to touch a photograph on any phone sold in the last five years.
//!
//! So the listing, the reading and the writing belong to the host, and what
//! lives here is the part worth testing: given one file's bytes and what the
//! journal remembers, should it be cleaned, and what does cleaning it produce.
//! The filesystem driver in [`crate::fsdrive`] is one host; the Android app is
//! another, and neither duplicates this.

use crate::journal::{Journal, Outcome, MAX_ATTEMPTS};
use crate::sweep::{Conditions, SETTLE_SECONDS};
use fm_core::profile::Profile;

/// One candidate, as the host describes it.
#[derive(Clone, Debug)]
pub struct Item {
    /// Stable identity in whatever the host's namespace is: a path, or a
    /// document URI. Used as the journal key, so it has to survive a reboot.
    pub id: String,
    /// What to call it in a notification.
    pub name: String,
    pub bytes: u64,
    /// Last modification, unix seconds.
    pub mtime: i64,
    /// Lower-case extension without the dot, where the host knows one.
    pub ext: Option<String>,
}

/// Why a file is being passed over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Written too recently to be sure something is not still writing it.
    StillSettling,
    /// Dealt with under this profile, and unchanged since.
    AlreadyDone,
    /// Failed too many times to keep trying.
    GivenUp,
}

impl Skip {
    pub fn sentence(self) -> &'static str {
        match self {
            Skip::StillSettling => "something may still be writing it",
            Skip::AlreadyDone => "already done, and unchanged since",
            Skip::GivenUp => "failed too often to keep trying",
        }
    }
}

/// What the host should do with one file.
#[derive(Debug)]
pub enum Plan {
    Pass(Skip),
    /// Read it and call [`resolve`].
    Examine,
}

/// Whether this file needs looking at, from its metadata alone.
///
/// Deciding this before reading matters on a phone: reading a thousand
/// photographs to discover that none of them changed is the difference between
/// a sweep that costs nothing and one the user notices.
pub fn plan(item: &Item, profile: &Profile, journal: &Journal, conditions: Conditions) -> Plan {
    if conditions.now - item.mtime < SETTLE_SECONDS {
        return Plan::Pass(Skip::StillSettling);
    }
    if journal.attempts(&item.id) >= MAX_ATTEMPTS {
        return Plan::Pass(Skip::GivenUp);
    }
    let Some(entry) = journal.get(&item.id) else { return Plan::Examine };
    // A different profile means the previous decision does not apply.
    if entry.profile != profile.name {
        return Plan::Examine;
    }
    match &entry.outcome {
        Outcome::Cleaned { bytes, mtime } => {
            // Still as the runner left it. If anything has edited it since, the
            // size or the time differs and it is examined again.
            if *bytes == item.bytes && *mtime == item.mtime {
                Plan::Pass(Skip::AlreadyDone)
            } else {
                Plan::Examine
            }
        }
        Outcome::AlreadyClean => Plan::Pass(Skip::AlreadyDone),
        Outcome::Refused { .. } => Plan::Pass(Skip::AlreadyDone),
        Outcome::Failed { .. } => Plan::Examine,
    }
}

/// What came of examining one file.
pub enum Resolution {
    /// Write these bytes back, then call [`record`] with what the host wrote.
    Write { data: Vec<u8>, changes: usize, verified: bool },
    /// Nothing to do; the file is already as the profile wants it.
    AlreadyClean,
    /// Deliberately not touched.
    Refused(String),
    Failed(String),
}

/// Clean one file's bytes.
///
/// The host does not write anything until this returns `Write`, and a file whose
/// verification failed is never written: on a phone there is nothing to compare
/// against afterwards and nobody watching, so a run that cannot prove it worked
/// must not touch the only copy.
pub fn resolve(item: &Item, data: &[u8], profile: &Profile) -> Resolution {
    match fm_core::clean_bytes(data, item.ext.as_deref(), profile) {
        Ok(cleaned) => {
            if cleaned.changes.is_empty() {
                return Resolution::AlreadyClean;
            }
            if !cleaned.verification.clean() {
                return Resolution::Failed(format!(
                    "the check did not pass: {} field(s) the profile promised to remove are still there",
                    cleaned.verification.leftovers.len()
                ));
            }
            Resolution::Write {
                data: cleaned.data,
                changes: cleaned.changes.len(),
                verified: true,
            }
        }
        Err(fm_core::Error::Refused(why)) => Resolution::Refused(why),
        Err(e) => Resolution::Failed(e.to_string()),
    }
}

/// Write down what the host did. `wrote` is the size and modification time the
/// file ended up with, which only the host can know.
pub fn record(
    journal: &mut Journal,
    item: &Item,
    profile: &Profile,
    conditions: Conditions,
    resolution: &Resolution,
    wrote: Option<(u64, i64)>,
) {
    let outcome = match resolution {
        Resolution::Write { .. } => match wrote {
            Some((bytes, mtime)) => Outcome::Cleaned { bytes, mtime },
            // The host said it wrote nothing after all, so nothing is claimed.
            None => Outcome::Failed {
                reason: "the host did not write the cleaned file".to_string(),
                attempts: journal.attempts(&item.id) + 1,
            },
        },
        Resolution::AlreadyClean => Outcome::AlreadyClean,
        Resolution::Refused(why) => Outcome::Refused { reason: why.clone() },
        Resolution::Failed(why) => Outcome::Failed {
            reason: why.clone(),
            attempts: journal.attempts(&item.id) + 1,
        },
    };
    journal.record(&item.id, &profile.name, conditions.now, outcome);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, bytes: u64, mtime: i64) -> Item {
        Item {
            id: id.to_string(),
            name: id.rsplit('/').next().unwrap_or(id).to_string(),
            bytes,
            mtime,
            ext: Some("jpg".to_string()),
        }
    }

    fn conditions(now: i64) -> Conditions {
        Conditions { charging: true, battery: 100, now }
    }

    fn runner_profile() -> Profile {
        let mut p = Profile::standard();
        p.runner.enabled = true;
        p.runner.roots = vec!["DCIM/Camera".to_string()];
        p
    }

    #[test]
    fn a_file_written_a_moment_ago_is_left_to_settle() {
        let j = Journal::default();
        let it = item("content://x/1", 100, 1_000_000);
        match plan(&it, &runner_profile(), &j, conditions(1_000_010)) {
            Plan::Pass(Skip::StillSettling) => {}
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            plan(&it, &runner_profile(), &j, conditions(1_000_000 + SETTLE_SECONDS)),
            Plan::Examine
        ));
    }

    #[test]
    fn an_unseen_file_is_examined() {
        let j = Journal::default();
        assert!(matches!(
            plan(&item("content://x/1", 100, 0), &runner_profile(), &j, conditions(1_000_000)),
            Plan::Examine
        ));
    }

    #[test]
    fn a_file_still_as_the_runner_left_it_is_passed_over() {
        let mut j = Journal::default();
        let p = runner_profile();
        j.record("content://x/1", &p.name, 1, Outcome::Cleaned { bytes: 80, mtime: 500 });
        let unchanged = item("content://x/1", 80, 500);
        assert!(matches!(
            plan(&unchanged, &p, &j, conditions(1_000_000)),
            Plan::Pass(Skip::AlreadyDone)
        ));
        // Edited since: the size moved, so it is examined again.
        let edited = item("content://x/1", 8467, 500);
        assert!(matches!(plan(&edited, &p, &j, conditions(1_000_000)), Plan::Examine));
        // Or the time moved.
        let retouched = item("content://x/1", 80, 900);
        assert!(matches!(plan(&retouched, &p, &j, conditions(1_000_000)), Plan::Examine));
    }

    #[test]
    fn changing_the_profile_makes_the_runner_reconsider() {
        let mut j = Journal::default();
        j.record("content://x/1", "standard", 1, Outcome::AlreadyClean);
        let mut strict = Profile::strict();
        strict.runner = runner_profile().runner;
        assert!(matches!(
            plan(&item("content://x/1", 80, 500), &strict, &j, conditions(1_000_000)),
            Plan::Examine
        ));
    }

    #[test]
    fn a_refusal_is_not_retried_every_sweep() {
        let mut j = Journal::default();
        let p = runner_profile();
        j.record("content://x/1", &p.name, 1, Outcome::Refused { reason: "signed".into() });
        assert!(matches!(
            plan(&item("content://x/1", 80, 500), &p, &j, conditions(1_000_000)),
            Plan::Pass(Skip::AlreadyDone)
        ));
    }

    #[test]
    fn a_file_that_keeps_failing_is_eventually_given_up_on() {
        let mut j = Journal::default();
        let p = runner_profile();
        let it = item("content://x/1", 80, 500);
        for attempt in 1..=MAX_ATTEMPTS {
            j.record(
                &it.id,
                &p.name,
                1,
                Outcome::Failed { reason: "malformed".into(), attempts: attempt },
            );
            let expected_examine = attempt < MAX_ATTEMPTS;
            assert_eq!(
                matches!(plan(&it, &p, &j, conditions(1_000_000)), Plan::Examine),
                expected_examine,
                "attempt {attempt}"
            );
        }
    }

    #[test]
    fn a_file_that_will_not_verify_is_never_offered_for_writing() {
        // An unreadable file cannot be cleaned, and the phone must not write
        // anything it could not check.
        let it = item("content://x/1", 9, 0);
        match resolve(&it, b"not a jpeg", &runner_profile()) {
            Resolution::Failed(why) => assert!(!why.is_empty()),
            other => panic!("expected a failure, got {}", describe(&other)),
        }
    }

    fn describe(r: &Resolution) -> String {
        match r {
            Resolution::Write { changes, .. } => format!("write with {changes} changes"),
            Resolution::AlreadyClean => "already clean".to_string(),
            Resolution::Refused(w) => format!("refused: {w}"),
            Resolution::Failed(w) => format!("failed: {w}"),
        }
    }

    #[test]
    fn a_host_that_writes_nothing_is_recorded_as_a_failure_not_a_success() {
        let mut j = Journal::default();
        let p = runner_profile();
        let it = item("content://x/1", 80, 500);
        let resolution = Resolution::Write { data: vec![1, 2, 3], changes: 4, verified: true };
        record(&mut j, &it, &p, conditions(10), &resolution, None);
        match &j.get(&it.id).unwrap().outcome {
            Outcome::Failed { reason, attempts } => {
                assert!(reason.contains("did not write"));
                assert_eq!(*attempts, 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn recording_a_write_stores_what_the_host_actually_produced() {
        let mut j = Journal::default();
        let p = runner_profile();
        let it = item("content://x/1", 8467, 500);
        let resolution = Resolution::Write { data: vec![0; 2561], changes: 35, verified: true };
        record(&mut j, &it, &p, conditions(10), &resolution, Some((2561, 777)));
        assert_eq!(
            j.get(&it.id).unwrap().outcome,
            Outcome::Cleaned { bytes: 2561, mtime: 777 }
        );
    }
}
