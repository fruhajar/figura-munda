//! When a sweep may run, and what to say about the one that just did.
//!
//! The host decides when to call in. Android's `WorkManager` will call on its
//! own schedule and kill the task when it feels like it, so a sweep has to be
//! interruptible at any file boundary and has to pick up where it left off.
//!
//! Everything the decision depends on is passed in: the clock, the battery, the
//! roots. That is what makes the logic testable off a phone, which is the only
//! place it can be tested here.

use crate::journal::Journal;
use fm_core::profile::Profile;

/// What the phone is doing, as the host sees it.
#[derive(Clone, Copy, Debug)]
pub struct Conditions {
    pub charging: bool,
    /// 0 to 100.
    pub battery: u8,
    /// Now, as unix seconds.
    pub now: i64,
}

impl Conditions {
    /// Below this, a sweep waits for the charger. Cleaning a camera roll is
    /// never urgent, and a privacy tool that flattens the battery gets
    /// uninstalled, which protects nobody.
    pub const BATTERY_FLOOR: u8 = 30;

    pub fn may_run(&self) -> bool {
        self.charging || self.battery >= Self::BATTERY_FLOOR
    }
}

/// Why a sweep did not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declined {
    Disabled,
    /// Not long enough since the last one.
    TooSoon { seconds_remaining: i64 },
    LowBattery { battery: u8 },
    NoRoots,
}

impl Declined {
    pub fn sentence(&self) -> String {
        match self {
            Declined::Disabled => "Periodic cleaning is switched off.".to_string(),
            Declined::TooSoon { seconds_remaining } => format!(
                "Next sweep in about {} minutes.",
                (seconds_remaining + 59) / 60
            ),
            Declined::LowBattery { battery } => format!(
                "Waiting for the charger: the battery is at {battery}% and the floor is {}%.",
                Conditions::BATTERY_FLOOR
            ),
            Declined::NoRoots => "No folders have been chosen to watch.".to_string(),
        }
    }
}

/// How long a file must have been still before it is touched.
///
/// A camera, a messaging app or a download in progress is still writing, and
/// cleaning a half-written file would corrupt it. Thirty seconds of quiet is
/// cheap insurance when the sweep itself runs hours apart.
pub const SETTLE_SECONDS: i64 = 30;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Files whose bytes were actually read.
    pub examined: usize,
    pub cleaned: usize,
    pub already_clean: usize,
    pub skipped_recent: usize,
    pub skipped_seen: usize,
    pub refused: usize,
    pub failed: usize,
    pub bytes_saved: i64,
}

impl Report {
    /// The one line a notification shows. Says what happened, not how it works.
    pub fn sentence(&self) -> String {
        if self.cleaned == 0 {
            return match self.examined {
                0 => "Nothing new to clean.".to_string(),
                _ => format!("Checked {} file(s); all already clean.", self.examined),
            };
        }
        let mut text = format!(
            "Cleaned {} file{}.",
            self.cleaned,
            if self.cleaned == 1 { "" } else { "s" }
        );
        if self.refused > 0 {
            text.push_str(&format!(" {} left untouched on purpose.", self.refused));
        }
        if self.failed > 0 {
            text.push_str(&format!(" {} could not be read.", self.failed));
        }
        text
    }
}

/// Should a sweep run at all?
pub fn due(profile: &Profile, journal: &Journal, conditions: Conditions) -> Result<(), Declined> {
    if !profile.runner.enabled {
        return Err(Declined::Disabled);
    }
    if profile.runner.roots.is_empty() {
        return Err(Declined::NoRoots);
    }
    if !conditions.may_run() {
        return Err(Declined::LowBattery { battery: conditions.battery });
    }
    let interval = profile.runner.interval_hours.max(1) as i64 * 3600;
    if let Some(last) = journal.last_sweep {
        let elapsed = conditions.now - last;
        if elapsed < interval {
            return Err(Declined::TooSoon { seconds_remaining: interval - elapsed });
        }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fm-runner-policy-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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

    #[test]
    fn a_disabled_runner_never_sweeps() {
        let j = Journal::default();
        assert_eq!(
            due(&Profile::standard(), &j, conditions(0)),
            Err(Declined::Disabled)
        );
    }


    #[test]
    fn a_runner_with_no_folders_declines_and_says_so() {
        let mut p = Profile::standard();
        p.runner.enabled = true;
        let j = Journal::default();
        assert_eq!(due(&p, &j, conditions(0)), Err(Declined::NoRoots));
        assert!(Declined::NoRoots.sentence().contains("No folders"));
    }


    #[test]
    fn a_low_battery_waits_for_the_charger() {
        let dir = scratch("battery");
        let p = runner_profile(&dir);
        let j = Journal::default();
        let flat = Conditions { charging: false, battery: 12, now: 0 };
        assert_eq!(due(&p, &j, flat), Err(Declined::LowBattery { battery: 12 }));
        // On the charger it goes ahead at any level.
        assert!(due(&p, &j, Conditions { charging: true, ..flat }).is_ok());
        // And above the floor it goes ahead on battery.
        assert!(due(&p, &j, Conditions { battery: 80, ..flat }).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }


    #[test]
    fn the_interval_is_respected_and_the_wait_is_reported_in_minutes() {
        let dir = scratch("interval");
        let p = runner_profile(&dir);
        let j = Journal { last_sweep: Some(1_000_000), ..Default::default() };

        let too_soon = due(&p, &j, conditions(1_000_000 + 3600));
        match too_soon {
            Err(Declined::TooSoon { seconds_remaining }) => {
                assert_eq!(seconds_remaining, 5 * 3600);
                assert!(Declined::TooSoon { seconds_remaining }.sentence().contains("300 minutes"));
            }
            other => panic!("expected a wait, got {other:?}"),
        }
        assert!(due(&p, &j, conditions(1_000_000 + 6 * 3600)).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }


    #[test]
    fn an_interval_of_zero_is_treated_as_an_hour_rather_than_a_busy_loop() {
        let dir = scratch("zero");
        let mut p = runner_profile(&dir);
        p.runner.interval_hours = 0;
        let j = Journal { last_sweep: Some(1_000_000), ..Default::default() };
        assert!(due(&p, &j, conditions(1_000_000 + 60)).is_err());
        assert!(due(&p, &j, conditions(1_000_000 + 3600)).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }


    #[test]
    fn the_notification_line_says_what_happened() {
        let r = Report { cleaned: 1, refused: 1, failed: 2, ..Default::default() };
        let text = r.sentence();
        assert!(text.starts_with("Cleaned 1 file."), "{text}");
        assert!(text.contains("1 left untouched"), "{text}");
        assert!(text.contains("2 could not be read"), "{text}");

        let clean = Report { examined: 4, already_clean: 4, ..Default::default() };
        assert_eq!(clean.sentence(), "Checked 4 file(s); all already clean.");
    }
}
