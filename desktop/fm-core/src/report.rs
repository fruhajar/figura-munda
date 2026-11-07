//! The four visible artefacts: the report, the diff, verify mode and the
//! recipient view.
//!
//! Cleaning a file is invisible, so these are what the user actually gets. The
//! structures here are rendered by the GUI directly and by the CLI through the
//! `text_*` helpers; both read from the same values so they cannot disagree.

use crate::anonset::{self, Signature};
use crate::format::Change;
use crate::model::{Action, Class, Finding, Impact, Inspection, ALL_CLASSES};
use crate::profile::Profile;

/// "47 fields across 6 classes, 41 removed, 6 preserved."
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub fields: usize,
    pub classes: usize,
    pub removed: usize,
    pub preserved: usize,
    pub protected: usize,
}

impl Counts {
    pub fn of(inspection: &Inspection, changes: &[Change]) -> Counts {
        let removed = changes.iter().filter(|c| c.action.alters()).count();
        let protected = inspection.findings.iter().filter(|f| f.protected).count();
        Counts {
            fields: inspection.findings.len(),
            classes: inspection.classes().len(),
            removed,
            preserved: inspection.findings.len().saturating_sub(removed),
            protected,
        }
    }

    pub fn sentence(&self) -> String {
        format!(
            "{} field{} across {} class{}, {} removed, {} preserved.",
            self.fields,
            plural(self.fields),
            self.classes,
            if self.classes == 1 { "" } else { "es" },
            self.removed,
            self.preserved
        )
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The report: plain language first, fields second.
pub struct Report<'a> {
    pub inspection: &'a Inspection,
    pub counts: Counts,
}

impl<'a> Report<'a> {
    pub fn new(inspection: &'a Inspection) -> Self {
        Report { inspection, counts: Counts::of(inspection, &[]) }
    }

    /// "This photo records where it was taken, which phone took it, and still
    /// contains the uncropped original."
    pub fn headline(&self) -> String {
        let classes = self.inspection.classes();
        if classes.is_empty() {
            return format!(
                "This {} reveals nothing this tool knows how to look for.",
                self.inspection.format.noun()
            );
        }
        let mut parts: Vec<String> = classes.iter().map(|c| c.sentence().to_string()).collect();
        let last = parts.pop().unwrap();
        let joined = if parts.is_empty() {
            last
        } else {
            format!("{}, and {}", parts.join(", "), last)
        };
        format!("This {} {}.", self.inspection.format.noun(), joined)
    }

    /// Findings grouped by class, in the README's order, protected ones last.
    pub fn grouped(&self) -> Vec<(Class, Vec<&Finding>)> {
        ALL_CLASSES
            .iter()
            .filter_map(|c| {
                let mut v: Vec<&Finding> = self.inspection.in_class(*c).collect();
                v.sort_by_key(|f| f.protected);
                (!v.is_empty()).then_some((*c, v))
            })
            .collect()
    }

    /// The single most alarming thing in the file, for a one-line summary in a
    /// list of many files.
    pub fn worst(&self) -> Option<&Finding> {
        let rank = |f: &Finding| match f.class {
            Class::RedactionIntegrity => 0,
            Class::History => 1,
            Class::HiddenPayload => 2,
            Class::Location => 3,
            Class::Identity => 4,
            Class::Correlation => 5,
            Class::Device => 6,
            Class::Time => 7,
            Class::StructuralFingerprint => 8,
            Class::Filesystem => 9,
        };
        self.inspection
            .findings
            .iter()
            .filter(|f| !f.protected)
            .min_by_key(|f| rank(f))
    }
}

/// Verify mode: re-inspect the output and assert it is clean.
///
/// This is the regression suite for free. A leftover is a finding in a class the
/// profile said it would alter, which means the handler did not do what the
/// profile promised.
pub struct Verification {
    pub leftovers: Vec<Finding>,
    pub signature: Option<&'static Signature>,
    pub remaining: Vec<Finding>,
}

/// Whether one surviving finding means the profile was not honoured.
///
/// Absence is only the right test for `remove`. A coarsened timestamp is
/// supposed to still be there, so the check is on its value: zero time of day
/// for a day, the first of the month for a month, the shared constant for
/// `normalise`. That makes verify mode able to fail a handler that wrote the
/// original value back out, which is the mistake worth catching.
fn is_leftover(f: &Finding, profile: &Profile) -> bool {
    if f.protected || f.advisory {
        return false;
    }
    let parsed = f.value.as_deref().and_then(crate::timeutil::parse_loose);
    match profile.action(f.class) {
        Action::Keep | Action::ReportOnly => false,
        // The covered content, the superseded revision, the field itself: all
        // of these should simply be gone.
        Action::Remove | Action::Enforce | Action::RewriteUniform => true,
        Action::Replace => f.value.as_deref() != Some(profile.constants.generic_text.as_str()),
        Action::CoarsenDay => match parsed {
            Some(c) => (c.hour, c.minute, c.second) != (0, 0, 0),
            None => true,
        },
        Action::CoarsenMonth => match parsed {
            Some(c) => (c.day, c.hour, c.minute, c.second) != (1, 0, 0, 0),
            None => true,
        },
        Action::Normalise => parsed != Some(profile.constants.timestamp),
        // `reset` writes the time of cleaning, which there is nothing to assert
        // about beyond its presence.
        Action::Reset => false,
    }
}

impl Verification {
    pub fn of(output: &Inspection, profile: &Profile) -> Verification {
        let leftovers = output
            .findings
            .iter()
            .filter(|f| is_leftover(f, profile))
            .cloned()
            .collect();
        Verification {
            leftovers,
            signature: anonset::classify(output.format, &output.shape),
            remaining: output
                .findings
                .iter()
                .filter(|f| !f.protected && !f.advisory)
                .cloned()
                .collect(),
        }
    }

    pub fn clean(&self) -> bool {
        self.leftovers.is_empty()
    }

    /// What the crowd check says, including how trustworthy the claim is.
    pub fn anonymity_sentence(&self) -> String {
        match self.signature {
            Some(s) => format!(
                "The result looks like {} ({}, {}).",
                s.name,
                s.crowd.label(),
                s.provenance.label()
            ),
            None => {
                "The result does not match any shape this tool has on record, which means it \
                 may stand out. Nothing is leaking; it is the pattern of what remains that is \
                 unusual."
                    .to_string()
            }
        }
    }
}

/// The recipient view: only what survives, said in the second person about them.
pub struct RecipientView<'a> {
    pub visible: Vec<(Class, Vec<&'a Finding>)>,
}

impl<'a> RecipientView<'a> {
    pub fn of(output: &'a Inspection) -> Self {
        let visible = ALL_CLASSES
            .iter()
            .filter_map(|c| {
                let v: Vec<&Finding> =
                    output.in_class(*c).filter(|f| !f.protected && !f.advisory).collect();
                (!v.is_empty()).then_some((*c, v))
            })
            .collect();
        RecipientView { visible }
    }

    pub fn sentence(&self) -> String {
        if self.visible.is_empty() {
            return "Whoever receives this file learns nothing from it beyond its contents.".into();
        }
        let classes: Vec<&str> = self.visible.iter().map(|(c, _)| c.sentence()).collect();
        format!("Whoever receives this file can still see that it {}.", classes.join(", and that it "))
    }
}

/// Plain-text rendering of a report, for the CLI and for pasting into a bug
/// report. Deliberately the same wording the GUI shows.
pub fn text_report(inspection: &Inspection, name: &str) -> String {
    let report = Report::new(inspection);
    let mut out = String::new();
    out.push_str(&format!("{name}  ({}, {} bytes)\n", inspection.format.label(), inspection.bytes));
    out.push_str(&format!("\n{}\n", report.headline()));
    out.push_str(&format!("{}\n", report.counts.sentence()));
    for b in &inspection.blockers {
        out.push_str(&format!("\nREFUSED: {} - {}\n", b.reason, b.detail));
    }
    for (class, findings) in report.grouped() {
        out.push_str(&format!("\n  {}\n", class.title()));
        for f in findings {
            let value = f.value.as_deref().unwrap_or("present");
            let mark = if f.protected { " [kept on purpose]" } else { "" };
            out.push_str(&format!("    {:<26} {}{}\n", f.label, truncate(value, 64), mark));
            if let Some(n) = &f.note {
                out.push_str(&format!("      {n}\n"));
            }
        }
    }
    out
}

/// The diff: field by field, what was kept and why.
pub fn text_diff(changes: &[Change], profile: &Profile) -> String {
    let mut out = String::new();
    if changes.is_empty() {
        return "Nothing changed.\n".to_string();
    }
    for class in ALL_CLASSES {
        let mine: Vec<&Change> = changes.iter().filter(|c| c.class == class).collect();
        if mine.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n  {} - {}\n",
            class.title(),
            profile.action(class).label()
        ));
        for c in mine {
            match (&c.before, &c.after) {
                (Some(b), Some(a)) => out.push_str(&format!(
                    "    {:<26} {} -> {}\n",
                    c.label,
                    truncate(b, 40),
                    truncate(a, 24)
                )),
                (Some(b), None) => {
                    out.push_str(&format!("    {:<26} {} -> gone\n", c.label, truncate(b, 40)))
                }
                (None, Some(a)) => out.push_str(&format!("    {:<26} added {}\n", c.label, truncate(a, 40))),
                (None, None) => out.push_str(&format!("    {:<26} removed\n", c.label)),
            }
        }
    }
    out
}

pub fn text_verify(v: &Verification) -> String {
    let mut out = String::new();
    if v.clean() {
        out.push_str("VERIFIED: nothing the profile promised to remove is still there.\n");
    } else {
        out.push_str("FAILED: the output still contains\n");
        for f in &v.leftovers {
            out.push_str(&format!("    {} ({}) {}\n", f.label, f.class.title(), f.locator));
        }
    }
    out.push_str(&format!("{}\n", v.anonymity_sentence()));
    out
}

/// The whole profile table, so the user can see what the single decision meant.
pub fn text_profile(profile: &Profile) -> String {
    let mut out = format!("{}: {}\n\n", profile.name, profile.description);
    for (class, action) in profile.table() {
        out.push_str(&format!("  {:<24} {}\n", class.title(), action.label()));
    }
    out.push_str(&format!("\n  {:<24} {}\n", "blending", profile.blending.label()));
    out
}

/// Changes that destroy visible content, which the GUI must confirm before it
/// writes anything.
pub fn destructive<'a>(inspection: &'a Inspection, profile: &Profile) -> Vec<&'a Finding> {
    inspection
        .findings
        .iter()
        .filter(|f| {
            !f.protected
                && matches!(f.impact, Impact::Content)
                && profile.alters(f.class)
                && profile.action(f.class) != Action::Keep
        })
        .collect()
}

fn truncate(s: &str, n: usize) -> String {
    let flat = s.replace(['\n', '\r'], " ");
    if flat.chars().count() <= n {
        return flat;
    }
    let kept: String = flat.chars().take(n.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Finding, FormatId};

    fn photo() -> Inspection {
        let mut ins = Inspection::new(FormatId::Jpeg, 8467);
        ins.findings.push(Finding::new(Class::Location, "exif:gps:0x0002", "GPS latitude").value("50.07550"));
        ins.findings.push(Finding::new(Class::Device, "exif:ifd0:0x0110", "camera model").value("iPhone 14 Pro"));
        ins.findings.push(
            Finding::new(Class::HiddenPayload, "exif:ifd1:thumbnail", "embedded preview image")
                .impact(Impact::Content),
        );
        ins.findings
            .push(Finding::new(Class::Device, "jpeg:app2:icc", "ICC colour profile").protected());
        ins
    }

    #[test]
    fn the_headline_reads_as_a_sentence() {
        let h = Report::new(&photo()).headline();
        assert!(h.starts_with("This photo identifies what took or wrote it,"), "{h}");
        assert!(h.contains("records where it was made"), "{h}");
        assert!(h.ends_with("contains more than it shows."), "{h}");
    }

    #[test]
    fn a_clean_file_says_so() {
        let ins = Inspection::new(FormatId::Pdf, 10);
        assert!(Report::new(&ins).headline().contains("reveals nothing"));
    }

    #[test]
    fn counts_do_not_credit_protected_fields_to_a_class() {
        let c = Counts::of(&photo(), &[]);
        assert_eq!(c.fields, 4);
        assert_eq!(c.classes, 3);
        assert_eq!(c.protected, 1);
    }

    #[test]
    fn the_worst_finding_is_the_one_a_user_would_name_first() {
        assert_eq!(Report::new(&photo()).worst().unwrap().class, Class::HiddenPayload);
    }

    #[test]
    fn verification_ignores_protected_leftovers() {
        let mut out = Inspection::new(FormatId::Jpeg, 10);
        out.findings
            .push(Finding::new(Class::Device, "jpeg:app2:icc", "ICC colour profile").protected());
        let v = Verification::of(&out, &Profile::uniform());
        assert!(v.clean());
    }

    #[test]
    fn verification_catches_a_handler_that_did_not_deliver() {
        let mut out = Inspection::new(FormatId::Jpeg, 10);
        out.findings.push(Finding::new(Class::Location, "exif:gps:0x0002", "GPS latitude"));
        let v = Verification::of(&out, &Profile::standard());
        assert!(!v.clean());
        assert_eq!(v.leftovers.len(), 1);
    }

    #[test]
    fn a_correctly_coarsened_timestamp_is_not_a_leftover() {
        let mut out = Inspection::new(FormatId::Jpeg, 10);
        out.findings.push(
            Finding::new(Class::Time, "exif:exif:0x9003", "capture date").value("2024:06:14 00:00:00"),
        );
        assert!(Verification::of(&out, &Profile::strict()).clean());
    }

    #[test]
    fn a_timestamp_that_kept_its_time_of_day_is_a_leftover() {
        let mut out = Inspection::new(FormatId::Jpeg, 10);
        out.findings.push(
            Finding::new(Class::Time, "exif:exif:0x9003", "capture date").value("2024:06:14 19:42:07"),
        );
        assert!(!Verification::of(&out, &Profile::strict()).clean());
    }

    #[test]
    fn normalise_demands_the_exact_constant() {
        let mut out = Inspection::new(FormatId::Jpeg, 10);
        out.findings
            .push(Finding::new(Class::Time, "exif:exif:0x9003", "capture date").value("1980:01:01 00:00:00"));
        assert!(Verification::of(&out, &Profile::uniform()).clean());
        out.findings[0].value = Some("1970:01:01 00:00:00".into());
        assert!(!Verification::of(&out, &Profile::uniform()).clean());
    }

    #[test]
    fn advisory_findings_never_fail_verification() {
        let mut out = Inspection::new(FormatId::Jpeg, 10);
        out.findings.push(
            Finding::new(Class::Filesystem, "fs:birthtime", "creation time")
                .value("2026-11-07T10:00:00Z")
                .advisory(),
        );
        assert!(Verification::of(&out, &Profile::uniform()).clean());
    }

    #[test]
    fn destructive_changes_are_flagged_for_confirmation() {
        let ins = photo();
        let d = destructive(&ins, &Profile::standard());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].class, Class::HiddenPayload);
        assert!(destructive(&ins, &Profile::inspect_only()).is_empty());
    }

    #[test]
    fn the_recipient_view_speaks_about_the_recipient() {
        let ins = photo();
        let v = RecipientView::of(&ins);
        assert!(v.sentence().starts_with("Whoever receives this file can still see"));
        assert_eq!(v.visible.len(), 3);
    }
}
