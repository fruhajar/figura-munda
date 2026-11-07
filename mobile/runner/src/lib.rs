//! Periodic cleaning, for the phone.
//!
//! The runner is the mobile product: a photograph is geotagged the moment it is
//! taken, and a user who has to remember to clean it before sending it will
//! forget. So the phone sweeps the folders it was pointed at, on a schedule,
//! under one profile, and says what it did.
//!
//! Nothing here talks to a platform. The host supplies the clock, the battery
//! state and the folder list, calls [`sweep::due`] to ask whether a sweep is
//! owed, and then drives the files one at a time through [`decide`]. The engine
//! underneath is the one the desktop uses, so a file cleaned on the phone and
//! the same file cleaned on the desktop come out identical.
//!
//! [`fsdrive`] is the driver for hosts that have real paths. Android does not:
//! since scoped storage the camera roll is reached through document URIs, so
//! the Android app lists, reads and writes through the platform and shares
//! every decision with [`decide`]. That split is the whole reason this crate is
//! shaped as it is.
//!
//! The profile file is the one the desktop writes, unchanged, so a profile can
//! be carried between the two.

#[cfg(target_os = "android")]
pub mod android;
pub mod decide;
pub mod fsdrive;
pub mod journal;
pub mod sweep;

pub use decide::{plan, record, resolve, Item, Plan, Resolution, Skip};
pub use fsdrive::sweep;
pub use journal::{Journal, Outcome};
pub use sweep::{due, Conditions, Declined, Report};

/// Load a profile from the host's config directory, falling back to `standard`
/// with the runner switched off.
///
/// A phone that cannot read its own settings must not guess at them: an
/// unreadable profile means no sweep, not a sweep under assumptions the user
/// never agreed to.
pub fn load_profile(path: &std::path::Path) -> fm_core::Profile {
    match std::fs::read_to_string(path).ok().and_then(|text| fm_core::Profile::from_toml(&text).ok())
    {
        Some(profile) => profile,
        None => {
            let mut fallback = fm_core::Profile::standard();
            fallback.runner.enabled = false;
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_profile_leaves_the_runner_switched_off() {
        let profile = load_profile(std::path::Path::new("/nowhere/profile.toml"));
        assert!(!profile.runner.enabled);
        assert_eq!(profile.name, "standard");
    }

    #[test]
    fn a_profile_written_by_the_desktop_is_read_by_the_phone() {
        let dir = std::env::temp_dir().join("fm-runner-profile");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");

        let mut desktop = fm_core::Profile::strict();
        desktop.runner.enabled = true;
        desktop.runner.interval_hours = 6;
        desktop.runner.roots = vec!["DCIM/Camera".to_string()];
        std::fs::write(&path, desktop.to_toml()).unwrap();

        let phone = load_profile(&path);
        assert_eq!(phone.name, "strict");
        assert!(phone.runner.enabled);
        assert_eq!(phone.runner.interval_hours, 6);
        assert_eq!(phone.runner.roots, vec!["DCIM/Camera".to_string()]);
        assert_eq!(phone.actions, desktop.actions);
        let _ = std::fs::remove_dir_all(dir);
    }
}
