//! Profiles. The single decision a user makes.
//!
//! A profile is a class-to-action table plus the constants that `normalise`
//! writes. Three are built in; a custom profile overrides any cell. The file
//! format is shared with the mobile runner, so one profile can be carried
//! between a desktop and a phone unchanged.

use crate::model::{Action, Class, ALL_CLASSES};
use crate::timeutil::{self, Civil, EPOCH_1980};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Whether to leave the residue a common pipeline leaves, or the least possible.
///
/// The two are in tension and `MatchCommon` usually wins: a JPEG with no JFIF
/// header at all leaks less in absolute terms but marks you as a user of a
/// metadata stripper, which is a smaller crowd to hide in than "every image
/// that was ever resized by a server".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blending {
    /// Aim for the shape some very common pipeline already emits.
    #[default]
    MatchCommon,
    /// Remove everything removable, accepting a distinctive result.
    Minimal,
}

impl Blending {
    pub fn label(self) -> &'static str {
        match self {
            Blending::MatchCommon => "match common tools",
            Blending::Minimal => "strip to the minimum",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Blending::MatchCommon => {
                "Leave the file looking like one that passed through ordinary software, \
                 so it joins a large crowd instead of marking you as someone who cleans files."
            }
            Blending::Minimal => {
                "Remove everything that can be removed. Fewer bytes leak, but the result \
                 is recognisably the output of a metadata tool."
            }
        }
    }
}

/// What `normalise` writes. Fixed constants, never plausible fakes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Constants {
    /// The instant every normalised timestamp becomes.
    pub timestamp: Civil,
    /// What a field becomes under `replace`, for readers that break on absence.
    pub generic_text: String,
}

impl Default for Constants {
    fn default() -> Self {
        Constants { timestamp: EPOCH_1980, generic_text: String::new() }
    }
}

/// Periodic cleaning, for the mobile background runner and any desktop
/// equivalent. Config only: the engine honours it, the scheduling belongs to
/// whichever host is running.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Runner {
    pub enabled: bool,
    /// How often to sweep. Hours, because a phone will not honour less anyway.
    pub interval_hours: u32,
    /// Folders to sweep, relative to the host's chosen root.
    pub roots: Vec<String>,
    /// Only touch files first seen since the last sweep. Off means re-check
    /// everything, which is slower but catches files edited in place.
    pub new_files_only: bool,
    /// Keep the original alongside the cleaned file. Off by default on mobile:
    /// a copy of the dirty original in the same gallery defeats the point.
    pub keep_original: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub actions: BTreeMap<Class, Action>,
    #[serde(default)]
    pub constants: Constants,
    #[serde(default)]
    pub blending: Blending,
    #[serde(default)]
    pub runner: Runner,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("{class} cannot be set to `{}`", action.label())]
    BadAction { class: Class, action: Action },
    #[error("profile does not say what to do with {0}")]
    Missing(Class),
    #[error("profile is not valid TOML: {0}")]
    Toml(#[from] toml::de::Error),
}

impl Profile {
    /// Fields are kept, nothing is touched. The baseline a report is measured
    /// against, and a useful inspect-only mode.
    pub fn inspect_only() -> Self {
        let mut actions = BTreeMap::new();
        for c in ALL_CLASSES {
            actions.insert(c, if c == Class::RedactionIntegrity { Action::ReportOnly } else { Action::Keep });
        }
        Profile {
            name: "inspect".into(),
            description: "Report what is in the file and change nothing.".into(),
            actions,
            constants: Constants::default(),
            blending: Blending::MatchCommon,
            runner: Runner::default(),
        }
    }

    /// Removes who, what and where. Keeps when, because a document with no date
    /// is conspicuous and the date is usually not the exposure.
    pub fn standard() -> Self {
        let mut p = Self::inspect_only();
        p.name = "standard".into();
        p.description =
            "Remove who made it, what made it and where. Keep the date, and leave the file \
             looking ordinary."
                .into();
        for c in [
            Class::Identity,
            Class::Device,
            Class::Location,
            Class::Correlation,
            Class::History,
            Class::HiddenPayload,
        ] {
            p.actions.insert(c, Action::Remove);
        }
        p
    }

    /// Adds coarsened time, enforced redaction and a filesystem reset.
    pub fn strict() -> Self {
        let mut p = Self::standard();
        p.name = "strict".into();
        p.description =
            "Everything in standard, plus dates coarsened to the day, redactions actually \
             flattened, and filesystem timestamps reset."
                .into();
        p.actions.insert(Class::Time, Action::CoarsenDay);
        p.actions.insert(Class::RedactionIntegrity, Action::Enforce);
        p.actions.insert(Class::Filesystem, Action::Reset);
        p
    }

    /// The one that makes two files from two people indistinguishable, and the
    /// one most likely to look odd to a recipient.
    pub fn uniform() -> Self {
        let mut p = Self::strict();
        p.name = "uniform".into();
        p.description =
            "Make the file indistinguishable from anyone else's: every timestamp a shared \
             constant and the container rewritten to a fixed layout. The most protective and \
             the most unusual-looking."
                .into();
        p.actions.insert(Class::Time, Action::Normalise);
        p.actions.insert(Class::StructuralFingerprint, Action::RewriteUniform);
        p.actions.insert(Class::Filesystem, Action::Normalise);
        p
    }

    pub fn builtins() -> Vec<Profile> {
        vec![Self::inspect_only(), Self::standard(), Self::strict(), Self::uniform()]
    }

    pub fn by_name(name: &str) -> Option<Profile> {
        Self::builtins().into_iter().find(|p| p.name == name)
    }

    pub fn action(&self, class: Class) -> Action {
        self.actions.get(&class).copied().unwrap_or(Action::Keep)
    }

    /// Whether this class's original value will not survive into the output.
    pub fn alters(&self, class: Class) -> bool {
        self.action(class).alters()
    }

    /// What a timestamp of this class becomes, or `None` to drop the field.
    /// `None` is also returned for `Keep`, where the caller must not call this.
    pub fn apply_time(&self, original: Option<Civil>) -> TimeOutcome {
        match self.action(Class::Time) {
            Action::Keep => TimeOutcome::Unchanged,
            Action::Remove => TimeOutcome::Drop,
            Action::CoarsenDay => match original {
                Some(c) => TimeOutcome::Set(c.coarsen_day()),
                None => TimeOutcome::Drop,
            },
            Action::CoarsenMonth => match original {
                Some(c) => TimeOutcome::Set(c.coarsen_month()),
                None => TimeOutcome::Drop,
            },
            Action::Normalise | Action::Replace => TimeOutcome::Set(self.constants.timestamp),
            // Not reachable for Time; Class::allows rejects these at load time.
            _ => TimeOutcome::Drop,
        }
    }

    /// True when no class is being altered, i.e. this is an inspection run and
    /// the engine must not touch a byte.
    pub fn inert(&self) -> bool {
        ALL_CLASSES.iter().all(|c| !self.alters(*c))
    }

    /// Whether a handler may drop a field the profile would keep, because
    /// keeping it would make the file conspicuous.
    ///
    /// `expected` says whether a value of this kind is normal *in this place*,
    /// which is finer than per-format and has to be: every MP4 has a recording
    /// time in its `mvhd` header, and almost none has one in a `udta` box, so
    /// the same date is ordinary in one place and a handset's signature in the
    /// other. Never true for an inert profile: an inspection run changes
    /// nothing, including for this reason.
    pub fn may_blend(&self, expected: bool) -> bool {
        self.blending == Blending::MatchCommon && !self.inert() && !expected
    }

    /// The per-format answer, for formats where a date is unusual anywhere.
    pub fn may_blend_away(&self, format: crate::model::FormatId) -> bool {
        self.may_blend(crate::anonset::time_is_expected(format))
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        for c in ALL_CLASSES {
            let a = self.actions.get(&c).copied().ok_or(ProfileError::Missing(c))?;
            if !c.allows(a) {
                return Err(ProfileError::BadAction { class: c, action: a });
            }
        }
        Ok(())
    }

    pub fn from_toml(s: &str) -> Result<Profile, ProfileError> {
        let p: Profile = toml::from_str(s)?;
        p.validate()?;
        Ok(p)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("a profile always serialises")
    }

    /// One line per class, for the report and the GUI's profile table.
    pub fn table(&self) -> Vec<(Class, Action)> {
        ALL_CLASSES.iter().map(|c| (*c, self.action(*c))).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeOutcome {
    Unchanged,
    Drop,
    Set(Civil),
}

impl TimeOutcome {
    /// Rendered in whichever spelling the field uses, or `None` to delete it.
    pub fn exif(self, original: &str) -> Option<String> {
        match self {
            TimeOutcome::Unchanged => Some(original.to_string()),
            TimeOutcome::Drop => None,
            TimeOutcome::Set(c) => Some(c.to_exif()),
        }
    }

    pub fn iso(self, original: &str) -> Option<String> {
        match self {
            TimeOutcome::Unchanged => Some(original.to_string()),
            TimeOutcome::Drop => None,
            TimeOutcome::Set(c) => Some(c.to_iso()),
        }
    }

    pub fn date(self, original: &str) -> Option<String> {
        match self {
            TimeOutcome::Unchanged => Some(original.to_string()),
            TimeOutcome::Drop => None,
            TimeOutcome::Set(c) => Some(c.to_date()),
        }
    }

    pub fn pdf(self, original: &str) -> Option<String> {
        match self {
            TimeOutcome::Unchanged => Some(original.to_string()),
            TimeOutcome::Drop => None,
            TimeOutcome::Set(c) => Some(c.to_pdf()),
        }
    }
}

/// Coarsen or normalise a timestamp already parsed out of a field.
pub fn retime(profile: &Profile, raw: &str) -> TimeOutcome {
    profile.apply_time(timeutil::parse_loose(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_all_validate() {
        for p in Profile::builtins() {
            p.validate().unwrap_or_else(|e| panic!("{}: {e}", p.name));
        }
    }

    #[test]
    fn the_readme_table_is_what_the_code_says() {
        let s = Profile::standard();
        assert_eq!(s.action(Class::Time), Action::Keep);
        assert_eq!(s.action(Class::RedactionIntegrity), Action::ReportOnly);
        assert_eq!(s.action(Class::StructuralFingerprint), Action::Keep);
        assert_eq!(s.action(Class::Filesystem), Action::Keep);

        let st = Profile::strict();
        assert_eq!(st.action(Class::Time), Action::CoarsenDay);
        assert_eq!(st.action(Class::RedactionIntegrity), Action::Enforce);
        assert_eq!(st.action(Class::Filesystem), Action::Reset);
        assert_eq!(st.action(Class::StructuralFingerprint), Action::Keep);

        let u = Profile::uniform();
        assert_eq!(u.action(Class::Time), Action::Normalise);
        assert_eq!(u.action(Class::StructuralFingerprint), Action::RewriteUniform);
        assert_eq!(u.action(Class::Filesystem), Action::Normalise);
    }

    #[test]
    fn inspect_only_alters_nothing() {
        let p = Profile::inspect_only();
        for c in ALL_CLASSES {
            assert!(!p.alters(c), "{c}");
        }
    }

    #[test]
    fn round_trips_through_toml() {
        let p = Profile::uniform();
        let back = Profile::from_toml(&p.to_toml()).unwrap();
        assert_eq!(back.actions, p.actions);
        assert_eq!(back.constants.timestamp, p.constants.timestamp);
    }

    #[test]
    fn inspect_only_is_inert_and_blends_nothing() {
        let p = Profile::inspect_only();
        assert!(p.inert());
        assert!(!p.may_blend_away(crate::model::FormatId::Png));
    }

    #[test]
    fn blending_away_a_date_applies_only_where_dates_are_unusual() {
        let p = Profile::standard();
        assert!(!p.inert());
        assert!(p.may_blend_away(crate::model::FormatId::Png));
        assert!(!p.may_blend_away(crate::model::FormatId::Jpeg));
        let mut minimal = Profile::standard();
        minimal.blending = Blending::Minimal;
        assert!(!minimal.may_blend_away(crate::model::FormatId::Png));
        // Place, not only format: an MP4 date is expected in a header and not
        // in a user-data box.
        assert!(p.may_blend(false));
        assert!(!p.may_blend(true));
    }

    #[test]
    fn rejects_a_nonsensical_cell() {
        let mut p = Profile::standard();
        p.actions.insert(Class::Identity, Action::CoarsenDay);
        assert!(p.validate().is_err());
    }

    #[test]
    fn coarsening_a_field_with_no_parsable_date_drops_it() {
        let p = Profile::strict();
        assert_eq!(retime(&p, "whenever"), TimeOutcome::Drop);
        assert_eq!(
            retime(&p, "2024:06:14 19:42:07"),
            TimeOutcome::Set(Civil::date_only(2024, 6, 14))
        );
    }

    #[test]
    fn uniform_ignores_the_original_entirely() {
        let p = Profile::uniform();
        assert_eq!(retime(&p, "2024:06:14 19:42:07"), TimeOutcome::Set(EPOCH_1980));
        assert_eq!(retime(&p, "nonsense"), TimeOutcome::Set(EPOCH_1980));
    }
}
