//! The state the window draws, with none of the drawing.
//!
//! Everything a user can do is a method here, so the whole flow can be tested
//! without a display: pick files, pick a profile, review what would change,
//! acknowledge content loss, run, read the results. The drawing code in
//! `app.rs` only renders this and calls these.

use fm_core::anonset::Signature;
use fm_core::format::Change;
use fm_core::model::{Class, FormatId, Inspection};
use fm_core::profile::Profile;
use fm_core::report::{self, Counts};
use fm_core::scan::{self, ScanOptions};
use std::path::{Path, PathBuf};

/// The two-phase flow: look, then act. A file is never written in the first
/// phase, and the second phase cannot be reached without passing through the
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Choosing files and a profile.
    Choose,
    /// Inspecting, in the background.
    Reviewing,
    /// The report is ready and nothing has been written.
    Review,
    /// Writing.
    Running,
    /// Written, with results to read.
    Done,
}

impl Stage {
    pub fn title(self) -> &'static str {
        match self {
            Stage::Choose => "Choose what to look at",
            Stage::Reviewing => "Looking",
            Stage::Review => "What these files reveal",
            Stage::Running => "Cleaning",
            Stage::Done => "Done",
        }
    }
}

/// Where the cleaned file goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// Beside the original, with `-clean` added to the name. The original stays
    /// available to compare against, which is the point of the diff.
    Beside,
    /// A folder of the user's choosing, keeping the original names.
    Folder(PathBuf),
    /// Replace the originals.
    InPlace,
}

impl Destination {
    pub fn label(&self) -> String {
        match self {
            Destination::Beside => "beside the originals, renamed".to_string(),
            Destination::Folder(p) => format!("into {}", p.display()),
            Destination::InPlace => "replacing the originals".to_string(),
        }
    }

    pub fn resolve(&self, source: &Path) -> PathBuf {
        match self {
            Destination::InPlace => source.to_path_buf(),
            Destination::Folder(dir) => {
                dir.join(source.file_name().unwrap_or(source.as_os_str()))
            }
            Destination::Beside => {
                let stem = source.file_stem().unwrap_or_default().to_string_lossy().to_string();
                let parent = source.parent().unwrap_or(Path::new("."));
                match source.extension() {
                    Some(ext) => parent.join(format!("{stem}-clean.{}", ext.to_string_lossy())),
                    None => parent.join(format!("{stem}-clean")),
                }
            }
        }
    }
}

/// What the review phase learned about one file: everything except the cleaned
/// bytes, which are not kept because a folder of photographs would not fit.
pub struct Preview {
    pub inspection: Inspection,
    pub changes: Vec<Change>,
    pub counts: Counts,
    pub verified: bool,
    pub signature: Option<&'static Signature>,
    /// Labels of the changes that would remove something a reader can see.
    pub content_loss: Vec<String>,
}

impl Preview {
    pub fn of(data: &[u8], ext: Option<&str>, profile: &Profile) -> fm_core::Result<Preview> {
        let cleaned = fm_core::clean_bytes(data, ext, profile)?;
        let content_loss = report::destructive(&cleaned.before, profile)
            .into_iter()
            .map(|f| f.label.clone())
            .collect();
        Ok(Preview {
            inspection: cleaned.before,
            changes: cleaned.changes,
            counts: cleaned.counts,
            verified: cleaned.verification.clean(),
            signature: cleaned.verification.signature,
            content_loss,
        })
    }
}

pub enum ItemState {
    Waiting,
    Previewed(Box<Preview>),
    /// Cleaned and written, with the size it ended up and whether it verified.
    Written { to: PathBuf, bytes: u64, verified: bool },
    /// Deliberately not touched, with the reason. A signed PDF is the case.
    Refused(String),
    Failed(String),
}

impl ItemState {
    pub fn summary(&self) -> String {
        match self {
            ItemState::Waiting => "waiting".to_string(),
            ItemState::Previewed(p) => p.counts.sentence(),
            ItemState::Written { bytes, verified, .. } => {
                if *verified {
                    format!("cleaned, {bytes} bytes")
                } else {
                    format!("written but NOT verified, {bytes} bytes")
                }
            }
            ItemState::Refused(why) => format!("not touched: {why}"),
            ItemState::Failed(why) => format!("failed: {why}"),
        }
    }
}

pub struct Item {
    pub path: PathBuf,
    pub format: FormatId,
    pub bytes: u64,
    pub state: ItemState,
    /// Whether this file is included in the run. Refused files cannot be.
    pub include: bool,
}

impl Item {
    /// The one line shown in the file list: the worst thing in the file.
    pub fn headline(&self) -> String {
        match &self.state {
            ItemState::Previewed(p) => {
                let report = report::Report::new(&p.inspection);
                match report.worst() {
                    Some(f) => f.label.clone(),
                    None => "nothing found".to_string(),
                }
            }
            other => other.summary(),
        }
    }

    pub fn runnable(&self) -> bool {
        self.include && matches!(self.state, ItemState::Previewed(_))
    }

    /// "JPEG image, 8.3 kB", for the second line of the list entry.
    pub fn kind_and_size(&self) -> String {
        format!("{}, {}", self.format.label(), human_bytes(self.bytes))
    }
}

/// Which of the review tabs is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailTab {
    /// What is in the file, in plain language.
    Report,
    /// Field by field, before and after.
    Diff,
    /// Only what survives, as the recipient sees it.
    Recipient,
    /// The profile table, so the single decision is visible.
    Profile,
}

pub struct Session {
    pub stage: Stage,
    pub profile: Profile,
    pub items: Vec<Item>,
    pub destination: Destination,
    pub recursive: bool,
    /// Set by the user after reading the list of content that would be lost.
    pub acknowledged_content_loss: bool,
    pub selected: Option<usize>,
    pub tab: DetailTab,
    /// Files that were looked at and are not a format this tool handles. Shown,
    /// because a folder run that silently skipped a RAW file would be a lie.
    pub skipped: Vec<PathBuf>,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            stage: Stage::Choose,
            profile: Profile::standard(),
            items: Vec::new(),
            destination: Destination::Beside,
            recursive: true,
            acknowledged_content_loss: false,
            selected: None,
            tab: DetailTab::Report,
            skipped: Vec::new(),
        }
    }
}

impl Session {
    /// Add files, expanding folders. Duplicates are ignored.
    pub fn add_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if path.is_dir() {
                let found = scan::scan(path, &ScanOptions { recursive: self.recursive, ..Default::default() });
                self.skipped.extend(found.skipped);
                for c in found.candidates {
                    self.push(c.path, c.format, c.bytes);
                }
            } else if let Some(c) = scan::candidate(path) {
                self.push(c.path, c.format, c.bytes);
            } else {
                self.skipped.push(path.clone());
            }
        }
        // Anything already looked at has to be looked at again: the profile or
        // the file set may have changed since.
        self.invalidate();
    }

    fn push(&mut self, path: PathBuf, format: FormatId, bytes: u64) {
        if self.items.iter().any(|i| i.path == path) {
            return;
        }
        self.items.push(Item { path, format, bytes, state: ItemState::Waiting, include: true });
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
            self.selected = None;
            self.invalidate();
        }
    }

    pub fn clear(&mut self) {
        *self = Session { profile: self.profile.clone(), ..Default::default() };
    }

    pub fn set_profile(&mut self, profile: Profile) {
        self.profile = profile;
        self.invalidate();
    }

    /// Any previous look is stale, so go back to the first phase.
    fn invalidate(&mut self) {
        for item in self.items.iter_mut() {
            if matches!(item.state, ItemState::Previewed(_)) {
                item.state = ItemState::Waiting;
            }
        }
        self.acknowledged_content_loss = false;
        if matches!(self.stage, Stage::Review | Stage::Done) {
            self.stage = Stage::Choose;
        }
    }

    pub fn can_review(&self) -> bool {
        !self.items.is_empty() && !self.profile.inert() && self.stage == Stage::Choose
    }

    /// Everything a run would destroy that a reader can see, by file.
    pub fn content_loss(&self) -> Vec<(&Path, &str)> {
        self.items
            .iter()
            .filter(|i| i.include)
            .flat_map(|i| match &i.state {
                ItemState::Previewed(p) => p
                    .content_loss
                    .iter()
                    .map(|label| (i.path.as_path(), label.as_str()))
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect()
    }

    /// Whether the run button does anything.
    ///
    /// Content loss has to be acknowledged first: the review phase exists so
    /// that removing a comment thread or an off-canvas paragraph is a decision
    /// and not a surprise.
    pub fn can_run(&self) -> bool {
        self.stage == Stage::Review
            && self.items.iter().any(|i| i.runnable())
            && (self.content_loss().is_empty() || self.acknowledged_content_loss)
    }

    pub fn why_not_run(&self) -> Option<String> {
        if self.stage != Stage::Review {
            return Some("Look at the files first.".to_string());
        }
        if !self.items.iter().any(|i| i.runnable()) {
            return Some("Nothing here can be cleaned.".to_string());
        }
        let loss = self.content_loss();
        if !loss.is_empty() && !self.acknowledged_content_loss {
            return Some(format!(
                "{} change{} would remove something a reader can see. Read the list and tick the box.",
                loss.len(),
                if loss.len() == 1 { "" } else { "s" }
            ));
        }
        None
    }

    /// Inspect every waiting file. Synchronous; the GUI calls this on a worker
    /// thread and feeds the results back one at a time.
    pub fn review_blocking(&mut self) {
        self.stage = Stage::Reviewing;
        for index in 0..self.items.len() {
            let path = self.items[index].path.clone();
            let state = match std::fs::read(&path) {
                Ok(data) => {
                    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
                    match Preview::of(&data, ext.as_deref(), &self.profile) {
                        Ok(mut preview) => {
                            preview.inspection.findings.extend(fm_core::fsmeta::inspect(&path));
                            ItemState::Previewed(Box::new(preview))
                        }
                        Err(fm_core::Error::Refused(why)) => ItemState::Refused(why),
                        Err(e) => ItemState::Failed(e.to_string()),
                    }
                }
                Err(e) => ItemState::Failed(e.to_string()),
            };
            if matches!(state, ItemState::Refused(_)) {
                self.items[index].include = false;
            }
            self.items[index].state = state;
        }
        self.stage = Stage::Review;
        if self.selected.is_none() {
            self.selected = self.worst_item();
        }
    }

    /// Clean every included file. Synchronous, for the same reason.
    pub fn run_blocking(&mut self) {
        self.stage = Stage::Running;
        let disposition = if self.destination == Destination::InPlace {
            fm_core::pipeline::Disposition::InPlace
        } else {
            fm_core::pipeline::Disposition::Copy
        };
        for index in 0..self.items.len() {
            if !self.items[index].runnable() {
                continue;
            }
            let path = self.items[index].path.clone();
            let dest = self.destination.resolve(&path);
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            self.items[index].state =
                match fm_core::pipeline::clean_file(&path, &dest, &self.profile, disposition) {
                    Ok(cleaned) => ItemState::Written {
                        to: dest,
                        bytes: cleaned.after.bytes,
                        verified: cleaned.verification.clean(),
                    },
                    Err(fm_core::Error::Refused(why)) => ItemState::Refused(why),
                    Err(e) => ItemState::Failed(e.to_string()),
                };
        }
        self.stage = Stage::Done;
    }

    /// The file with the worst thing in it, which is the one to open first.
    pub fn worst_item(&self) -> Option<usize> {
        let rank = |item: &Item| match &item.state {
            ItemState::Previewed(p) => {
                report::Report::new(&p.inspection).worst().map(|f| severity(f.class)).unwrap_or(99)
            }
            _ => 98,
        };
        self.items
            .iter()
            .enumerate()
            .min_by_key(|(_, item)| rank(item))
            .map(|(i, _)| i)
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.selected.and_then(|i| self.items.get(i))
    }

    /// The headline for the whole set, which is what a user reads first.
    pub fn headline(&self) -> String {
        match self.stage {
            Stage::Choose if self.items.is_empty() => {
                "Drop files or a folder here, or use the buttons above.".to_string()
            }
            Stage::Choose => format!(
                "{} ready to look at. Nothing has been read yet.",
                files(self.items.len())
            ),
            Stage::Reviewing => "Reading the files. Nothing is being written.".to_string(),
            Stage::Review => {
                let classes: Vec<Class> = fm_core::model::ALL_CLASSES
                    .iter()
                    .copied()
                    .filter(|c| {
                        self.items.iter().any(|i| match &i.state {
                            ItemState::Previewed(p) => p.inspection.in_class(*c).any(|f| !f.protected),
                            _ => false,
                        })
                    })
                    .collect();
                if classes.is_empty() {
                    return format!("{} and nothing to remove in any of them.", files(self.items.len()));
                }
                format!(
                    "{} between them reveal {} different kinds of thing about you. Nothing has been written.",
                    files(self.items.len()),
                    classes.len()
                )
            }
            Stage::Running => "Writing.".to_string(),
            Stage::Done => {
                let written = self
                    .items
                    .iter()
                    .filter(|i| matches!(i.state, ItemState::Written { .. }))
                    .count();
                let unverified = self
                    .items
                    .iter()
                    .filter(|i| matches!(i.state, ItemState::Written { verified: false, .. }))
                    .count();
                let failed = self
                    .items
                    .iter()
                    .filter(|i| matches!(i.state, ItemState::Failed(_)))
                    .count();
                let refused = self
                    .items
                    .iter()
                    .filter(|i| matches!(i.state, ItemState::Refused(_)))
                    .count();
                let mut text = format!("{} cleaned {}.", files(written), self.destination.label());
                if unverified > 0 {
                    text.push_str(&format!(" {unverified} did NOT verify; read them before sending."));
                }
                if refused > 0 {
                    text.push_str(&format!(" {refused} left untouched on purpose."));
                }
                if failed > 0 {
                    text.push_str(&format!(" {failed} failed."));
                }
                text
            }
        }
    }

    /// Start again from the file list, keeping the files and the profile.
    pub fn back_to_choosing(&mut self) {
        self.stage = Stage::Choose;
        self.invalidate();
    }
}

/// Sizes as a person reads them. Decimal units, which is what file managers
/// show, so the number matches what the user already saw.
pub fn human_bytes(n: u64) -> String {
    match n {
        n if n < 1_000 => format!("{n} B"),
        n if n < 1_000_000 => format!("{:.1} kB", n as f64 / 1_000.0),
        n => format!("{:.1} MB", n as f64 / 1_000_000.0),
    }
}

fn files(n: usize) -> String {
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

/// Same order the engine's reports use, so the GUI agrees with the text output.
fn severity(class: Class) -> u8 {
    match class {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture set, when it has been generated. These tests exercise the
    /// whole flow with no window open, which is the point of keeping the logic
    /// out of the drawing code.
    fn fixture(name: &str) -> Option<PathBuf> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/out")
            .join(name);
        path.exists().then_some(path)
    }

    macro_rules! fixtures_or_skip {
        ($($name:expr),+) => {{
            let mut paths = Vec::new();
            $(match fixture($name) {
                Some(p) => paths.push(p),
                None => {
                    eprintln!("skipping: fixtures/out absent, run fixtures/generate.py");
                    return;
                }
            })+
            paths
        }};
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fm-gui-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_session_cannot_run_anything() {
        let s = Session::default();
        assert_eq!(s.stage, Stage::Choose);
        assert!(!s.can_review());
        assert!(!s.can_run());
        assert!(s.headline().contains("Drop files"));
    }

    #[test]
    fn an_inert_profile_cannot_be_run() {
        let paths = fixtures_or_skip!("photo-phone.jpg");
        let mut s = Session::default();
        s.add_paths(&paths);
        assert!(s.can_review());
        s.set_profile(Profile::inspect_only());
        assert!(!s.can_review(), "a profile that changes nothing has nothing to run");
    }

    #[test]
    fn the_review_phase_writes_nothing() {
        let paths = fixtures_or_skip!("photo-phone.jpg");
        let before = std::fs::read(&paths[0]).unwrap();
        let mut s = Session::default();
        s.add_paths(&paths);
        s.review_blocking();
        assert_eq!(s.stage, Stage::Review);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), before, "the original was modified");
    }

    #[test]
    fn content_loss_has_to_be_acknowledged_before_a_run() {
        let paths = fixtures_or_skip!("photo-phone.jpg");
        let mut s = Session::default();
        s.add_paths(&paths);
        s.review_blocking();

        // The photo's embedded preview is content, so the run is held back.
        let loss = s.content_loss();
        assert!(!loss.is_empty(), "the embedded preview should count as content");
        assert!(!s.can_run());
        assert!(s.why_not_run().unwrap().contains("a reader can see"));

        s.acknowledged_content_loss = true;
        assert!(s.can_run());
        assert!(s.why_not_run().is_none());
    }

    #[test]
    fn changing_the_profile_sends_the_session_back_to_the_first_phase() {
        let paths = fixtures_or_skip!("photo-phone.jpg");
        let mut s = Session::default();
        s.add_paths(&paths);
        s.review_blocking();
        s.acknowledged_content_loss = true;
        assert!(s.can_run());

        s.set_profile(Profile::uniform());
        assert_eq!(s.stage, Stage::Choose, "a stale review must not be runnable");
        assert!(!s.can_run());
        assert!(!s.acknowledged_content_loss, "the acknowledgement was for the old profile");
        assert!(matches!(s.items[0].state, ItemState::Waiting));
    }

    #[test]
    fn adding_a_file_also_invalidates_the_review() {
        let paths = fixtures_or_skip!("photo-phone.jpg", "screenshot.png");
        let mut s = Session::default();
        s.add_paths(&paths[..1]);
        s.review_blocking();
        s.acknowledged_content_loss = true;
        s.add_paths(&paths[1..]);
        assert_eq!(s.stage, Stage::Choose);
        assert!(!s.acknowledged_content_loss);
    }

    #[test]
    fn a_signed_document_is_shown_as_refused_and_excluded() {
        let paths = fixtures_or_skip!("signed.pdf");
        let mut s = Session::default();
        s.add_paths(&paths);
        s.review_blocking();
        match &s.items[0].state {
            ItemState::Refused(why) => assert!(why.contains("signature"), "{why}"),
            other => panic!("expected a refusal, got {}", other.summary()),
        }
        assert!(!s.items[0].include, "a refused file must not be queued");
        assert!(!s.can_run());
    }

    #[test]
    fn the_file_opened_first_is_the_one_with_the_worst_problem() {
        let paths = fixtures_or_skip!("screenshot.png", "report-redacted.pdf", "photo-phone.jpg");
        let mut s = Session::default();
        s.add_paths(&paths);
        s.review_blocking();
        let opened = s.selected_item().expect("nothing was opened");
        assert!(
            opened.path.ends_with("report-redacted.pdf"),
            "opened {} instead of the failed redaction",
            opened.path.display()
        );
        assert!(opened.headline().contains("filled rectangle"));
    }

    #[test]
    fn duplicates_are_ignored() {
        let paths = fixtures_or_skip!("photo-phone.jpg");
        let mut s = Session::default();
        s.add_paths(&paths);
        s.add_paths(&paths);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn unsupported_files_are_listed_rather_than_dropped_silently() {
        let dir = scratch("skipped");
        let path = dir.join("notes.txt");
        std::fs::write(&path, b"just text").unwrap();
        let mut s = Session::default();
        s.add_paths(std::slice::from_ref(&dir));
        assert!(s.items.is_empty());
        assert_eq!(s.skipped.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_run_writes_beside_the_original_and_leaves_it_alone() {
        let paths = fixtures_or_skip!("photo-phone.jpg");
        let dir = scratch("run-beside");
        let source = dir.join("photo-phone.jpg");
        std::fs::copy(&paths[0], &source).unwrap();
        let original = std::fs::read(&source).unwrap();

        let mut s = Session::default();
        s.add_paths(std::slice::from_ref(&source));
        s.review_blocking();
        s.acknowledged_content_loss = true;
        s.run_blocking();

        assert_eq!(s.stage, Stage::Done);
        match &s.items[0].state {
            ItemState::Written { to, bytes, verified } => {
                assert!(to.ends_with("photo-phone-clean.jpg"), "{}", to.display());
                assert!(*verified);
                assert!(*bytes < original.len() as u64);
                assert!(to.exists());
            }
            other => panic!("not written: {}", other.summary()),
        }
        assert_eq!(std::fs::read(&source).unwrap(), original, "the original changed");
        assert!(s.headline().contains("cleaned"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn running_in_place_replaces_the_original() {
        let paths = fixtures_or_skip!("screenshot.png");
        let dir = scratch("run-in-place");
        let source = dir.join("screenshot.png");
        std::fs::copy(&paths[0], &source).unwrap();

        let mut s = Session { destination: Destination::InPlace, ..Default::default() };
        s.add_paths(std::slice::from_ref(&source));
        s.review_blocking();
        s.acknowledged_content_loss = true;
        s.run_blocking();

        assert!(source.exists(), "the file disappeared");
        let after = fm_core::inspect_bytes(&std::fs::read(&source).unwrap(), Some("png")).unwrap();
        assert!(after.findings.is_empty(), "{:?}", after.findings);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "a stray file was left behind");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn running_into_a_folder_creates_it_and_keeps_the_names() {
        let paths = fixtures_or_skip!("diagram.svg");
        let dir = scratch("run-folder");
        let out = dir.join("cleaned/deeper");

        let mut s = Session { destination: Destination::Folder(out.clone()), ..Default::default() };
        s.add_paths(&paths);
        s.review_blocking();
        s.acknowledged_content_loss = true;
        s.run_blocking();

        assert!(out.join("diagram.svg").exists(), "{}", out.display());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn excluding_a_file_keeps_it_out_of_the_run_and_out_of_the_warning() {
        let paths = fixtures_or_skip!("photo-phone.jpg", "screenshot.png");
        let dir = scratch("exclude");
        let mut sources = Vec::new();
        for p in &paths {
            let dest = dir.join(p.file_name().unwrap());
            std::fs::copy(p, &dest).unwrap();
            sources.push(dest);
        }

        let mut s = Session::default();
        s.add_paths(&sources);
        s.review_blocking();
        for item in s.items.iter_mut() {
            item.include = !item.path.ends_with("photo-phone.jpg");
        }
        assert!(
            s.content_loss().is_empty(),
            "an excluded file's content loss is still being counted: {:?}",
            s.content_loss()
        );
        assert!(s.can_run());

        s.run_blocking();
        assert!(!dir.join("photo-phone-clean.jpg").exists(), "an excluded file was cleaned");
        assert!(dir.join("screenshot-clean.png").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_shows_them() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(8_467), "8.5 kB");
        assert_eq!(human_bytes(6_715_000), "6.7 MB");
    }

    #[test]
    fn the_destination_is_said_in_words() {
        assert_eq!(Destination::Beside.label(), "beside the originals, renamed");
        assert_eq!(Destination::InPlace.label(), "replacing the originals");
        assert!(Destination::Folder(PathBuf::from("/tmp/x")).label().contains("/tmp/x"));
    }
}
