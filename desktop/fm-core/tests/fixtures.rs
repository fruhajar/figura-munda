//! The engine against the generated fixture set.
//!
//! `fixtures/manifest.json` declares, field by field, what each fixture
//! contains. These tests assert that the inspect phase finds those things and
//! that the clean phase removes them, which makes the manifest the contract
//! between the Python generator and the Rust engine.
//!
//! `fixtures/out` is gitignored, so every test skips when it is absent. Run
//! `python3 fixtures/generate.py` first.

use fm_core::model::{Class, FormatId};
use fm_core::profile::Profile;
use fm_core::report::{Report, Verification};
use fm_core::{clean_bytes, inspect_bytes};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> Option<(PathBuf, Vec<u8>)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/out")
        .join(name);
    let data = std::fs::read(&path).ok()?;
    Some((path, data))
}

/// Skip, loudly, rather than fail when the fixtures have not been generated.
macro_rules! fixture_or_skip {
    ($name:expr) => {
        match fixture($name) {
            Some(v) => v,
            None => {
                eprintln!("skipping: fixtures/out/{} absent, run fixtures/generate.py", $name);
                return;
            }
        }
    };
}

fn ext(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_lowercase())
}

fn values(ins: &fm_core::Inspection, class: Class) -> Vec<String> {
    ins.in_class(class)
        .filter_map(|f| f.value.clone())
        .collect()
}

#[test]
fn phone_photo_reveals_every_class_the_manifest_declares() {
    let (path, data) = fixture_or_skip!("photo-phone.jpg");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Jpeg);

    for class in [
        Class::Identity,
        Class::Device,
        Class::Location,
        Class::Time,
        Class::Correlation,
        Class::HiddenPayload,
    ] {
        assert!(
            ins.in_class(class).next().is_some(),
            "nothing found for {class}"
        );
    }

    // The declared values, as a person would recognise them.
    let identity = values(&ins, Class::Identity).join(" | ");
    assert!(identity.contains("Marie Dvorakova"), "{identity}");
    let device = values(&ins, Class::Device).join(" | ");
    assert!(device.contains("iPhone 14 Pro"), "{device}");
    assert!(device.contains("BX-4471902-TT"), "body serial missing: {device}");
    assert!(device.contains("LN-88213-04"), "lens serial missing: {device}");

    // GPS is reported in degrees, not as three rationals.
    let location = values(&ins, Class::Location).join(" | ");
    assert!(location.contains("50.07550"), "{location}");
    assert!(location.contains("14.4378"), "{location}");

    // The uncropped original: the fixture's thumbnail is a different picture.
    let thumb = ins
        .in_class(Class::HiddenPayload)
        .find(|f| f.locator == "exif:ifd1:thumbnail")
        .expect("thumbnail not reported");
    assert_eq!(thumb.value.as_deref(), Some("5905 bytes"));
    assert_eq!(thumb.impact, fm_core::Impact::Content, "a real preview is content, not metadata");

    // The colour profile is found but marked never-remove.
    let icc = ins
        .findings
        .iter()
        .find(|f| f.locator == "jpeg:app2:icc")
        .expect("ICC profile not reported");
    assert!(icc.protected);

    let headline = Report::new(&ins).headline();
    assert!(headline.starts_with("This photo "), "{headline}");
}

#[test]
fn standard_strips_the_phone_photo_but_keeps_the_date_and_the_profile() {
    let (path, data) = fixture_or_skip!("photo-phone.jpg");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();

    assert!(
        cleaned.verification.clean(),
        "leftovers: {:?}",
        cleaned.verification.leftovers.iter().map(|f| &f.locator).collect::<Vec<_>>()
    );
    for class in [Class::Identity, Class::Device, Class::Location, Class::Correlation, Class::HiddenPayload] {
        let left: Vec<&str> = cleaned
            .after
            .in_class(class)
            .filter(|f| !f.protected)
            .map(|f| f.locator.as_str())
            .collect();
        assert!(left.is_empty(), "{class} survived: {left:?}");
    }

    // The capture date is kept, because standard keeps the time class.
    let time = values(&cleaned.after, Class::Time).join(" | ");
    assert!(time.contains("2024:06:14 19:42:07"), "{time}");

    // The profile stays, and so does orientation.
    assert!(cleaned.after.findings.iter().any(|f| f.locator == "jpeg:app2:icc" && f.protected));
    assert!(cleaned
        .after
        .findings
        .iter()
        .any(|f| f.locator == "exif:ifd0:0x0112" && f.protected));

    // The embedded original is gone, and the file is much smaller for it.
    assert!(cleaned.bytes_saved() > 5000, "saved {}", cleaned.bytes_saved());
    assert!(cleaned.changes.iter().any(|c| c.locator == "exif:ifd1:thumbnail"));

    // Still a decodable JPEG.
    assert_eq!(&cleaned.data[..3], &[0xFF, 0xD8, 0xFF]);
    assert!(cleaned.data.ends_with(&[0xFF, 0xD9]));
}

#[test]
fn strict_coarsens_the_capture_date_to_the_day() {
    let (path, data) = fixture_or_skip!("photo-phone.jpg");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::strict()).unwrap();
    let time = values(&cleaned.after, Class::Time).join(" | ");
    assert!(time.contains("2024:06:14 00:00:00"), "{time}");
    assert!(!time.contains("19:42"), "the time of day survived: {time}");
}

#[test]
fn uniform_normalises_the_capture_date_to_the_shared_constant() {
    let (path, data) = fixture_or_skip!("photo-phone.jpg");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::uniform()).unwrap();
    let time = values(&cleaned.after, Class::Time).join(" | ");
    assert!(time.contains("1980:01:01 00:00:00"), "{time}");
}

/// Keeping any date at all costs the largest crowd, and the engine says so
/// rather than pretending otherwise. This is the trade-off the profile table
/// exists to expose, so it is asserted in both directions.
#[test]
fn keeping_the_date_costs_the_largest_crowd() {
    let (path, data) = fixture_or_skip!("photo-phone.jpg");

    let kept = clean_bytes(&data, ext(&path).as_deref(), &Profile::uniform()).unwrap();
    let sig = kept.verification.signature.expect("no signature matched");
    assert_eq!(sig.id, "jpeg-date-only");
    assert_eq!(sig.crowd, fm_core::anonset::Crowd::Medium);

    let mut dateless = Profile::uniform();
    dateless.actions.insert(Class::Time, fm_core::Action::Remove);
    let gone = clean_bytes(&data, ext(&path).as_deref(), &dateless).unwrap();
    let sig = gone.verification.signature.expect("no signature matched");
    assert_eq!(sig.crowd, fm_core::anonset::Crowd::Large);
    // Orientation has to survive or the picture ends up on its side, so the
    // target is the shape a resize pipeline leaves, not an empty file.
    assert_eq!(sig.id, "jpeg-orientation-only");
}

#[test]
fn cleaning_is_idempotent() {
    let (path, data) = fixture_or_skip!("photo-phone.jpg");
    let once = clean_bytes(&data, ext(&path).as_deref(), &Profile::uniform()).unwrap();
    let twice = clean_bytes(&once.data, ext(&path).as_deref(), &Profile::uniform()).unwrap();
    assert_eq!(once.data, twice.data, "a second pass changed the file again");
}

#[test]
fn screenshot_reveals_its_author_software_and_host() {
    let (path, data) = fixture_or_skip!("screenshot.png");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Png);
    assert!(values(&ins, Class::Identity).join(" ").contains("Marie Dvorakova"));
    assert!(values(&ins, Class::Device).join(" ").contains("GNOME Screenshot 45.1"));
    assert!(ins.in_class(Class::Correlation).next().is_some(), "XMP IDs not found");
    assert!(ins.in_class(Class::Time).next().is_some());
}

#[test]
fn standard_leaves_the_screenshot_with_nothing_but_its_pixels() {
    let (path, data) = fixture_or_skip!("screenshot.png");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    assert!(cleaned.verification.clean(), "{:?}", cleaned.verification.leftovers);
    let v = Verification::of(&cleaned.after, &Profile::standard());
    assert!(v.clean());
    // A PNG with no text chunks is the common case, so the crowd is large. The
    // date goes with them: a dated PNG would be the outlier.
    assert_eq!(v.signature.unwrap().id, "png-plain");
    assert_eq!(v.signature.unwrap().crowd, fm_core::anonset::Crowd::Large);
    assert!(cleaned
        .changes
        .iter()
        .any(|c| c.label.contains("match common tools")));
}

#[test]
fn every_profile_leaves_every_fixture_verifiably_clean() {
    for name in ["photo-phone.jpg", "screenshot.png"] {
        let Some((path, data)) = fixture(name) else { continue };
        for profile in Profile::builtins() {
            if profile.name == "inspect" {
                continue;
            }
            let cleaned = clean_bytes(&data, ext(&path).as_deref(), &profile).unwrap();
            assert!(
                cleaned.verification.clean(),
                "{name} under {}: {:?}",
                profile.name,
                cleaned.verification.leftovers.iter().map(|f| &f.locator).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn the_drawing_reveals_its_editor_its_filename_and_the_plan_parked_off_canvas() {
    let (path, data) = fixture_or_skip!("diagram.svg");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Svg);

    assert!(values(&ins, Class::Identity).join(" ").contains("Marie Dvorakova"));
    assert!(values(&ins, Class::Device).join(" ").contains("1.3.2"));
    let correlation = values(&ins, Class::Correlation).join(" ");
    assert!(correlation.contains("org-chart-INTERNAL-v4.svg"), "{correlation}");
    assert!(ins.shape.contains(&"svg:offcanvas".to_string()), "{:?}", ins.shape);
}

#[test]
fn cleaning_the_drawing_takes_the_off_canvas_text_with_it() {
    let (path, data) = fixture_or_skip!("diagram.svg");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    assert!(cleaned.verification.clean(), "{:?}", cleaned.verification.leftovers);

    let text = String::from_utf8(cleaned.data.clone()).unwrap();
    assert!(!text.contains("-18 FTE"), "the superseded plan survived:\n{text}");
    assert!(!text.contains("Marie Dvorakova"), "{text}");
    assert!(!text.contains("org-chart-INTERNAL"), "{text}");
    // Still a drawing: the visible content and the viewBox are untouched.
    assert!(text.contains("viewBox"), "{text}");

    // Removing visible content is the kind of change that has to be confirmed,
    // so it must be flagged before anything is written.
    let destructive = fm_core::report::destructive(&cleaned.before, &Profile::standard());
    assert!(destructive.iter().any(|f| f.locator.starts_with("svg:offcanvas")));
}

#[test]
fn the_letter_reveals_two_authors_a_company_and_a_superseded_figure() {
    let (path, data) = fixture_or_skip!("letter.docx");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Ooxml);

    let identity = values(&ins, Class::Identity).join(" | ");
    assert!(identity.contains("Marie Dvorakova"), "{identity}");
    assert!(identity.contains("Petr Novak"), "{identity}");
    assert!(identity.contains("Severin Holding a.s."), "{identity}");
    assert!(identity.contains("Jan Prochazka"), "{identity}");
    assert!(identity.contains("sev-internal-memo.dotx"), "{identity}");

    // The save identifiers, which the manifest declares six of.
    let correlation = values(&ins, Class::Correlation).join(" | ");
    assert!(correlation.contains("3F2A9C41"), "the document GUID is missing: {correlation}");
    assert!(ins.in_class(Class::Correlation).any(|f| f.locator.contains("rsid")), "{correlation}");

    // Tracked changes, and the names inside them.
    let tracked = ins
        .in_class(Class::History)
        .find(|f| f.locator.contains("tracked"))
        .expect("tracked changes not found");
    assert!(tracked.note.as_deref().unwrap_or_default().contains("Novak"));

    // The editing-time total and the thumbnail.
    assert!(values(&ins, Class::Time).join(" ").contains("2471"));
    assert!(ins.in_class(Class::HiddenPayload).any(|f| f.locator.contains("thumbnail")));

    // The container's own stamps, which the manifest pins to 2019-03-14.
    let structural = values(&ins, Class::StructuralFingerprint).join(" | ");
    assert!(structural.contains("2019-03-14"), "{structural}");
}

#[test]
fn cleaning_the_letter_leaves_a_document_that_still_opens() {
    let (path, data) = fixture_or_skip!("letter.docx");
    for profile in Profile::builtins() {
        if profile.inert() {
            continue;
        }
        let cleaned = clean_bytes(&data, ext(&path).as_deref(), &profile).unwrap();
        assert!(
            cleaned.verification.clean(),
            "{}: {:?}",
            profile.name,
            cleaned.verification.leftovers.iter().map(|f| &f.locator).collect::<Vec<_>>()
        );

        let text = String::from_utf8_lossy(&cleaned.data).to_string();
        assert!(!text.contains("Marie Dvorakova"), "{} leaked a name", profile.name);

        // The parts a reader needs are all still there.
        let names: Vec<String> = fm_core::format::zip::read(&cleaned.data)
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert!(names.contains(&"[Content_Types].xml".to_string()), "{names:?}");
        assert!(names.contains(&"word/document.xml".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("thumbnail")), "{names:?}");
        assert!(!names.iter().any(|n| n == "word/comments.xml"), "{names:?}");
    }
}

/// Resolving the history means accepting what was inserted and discarding what
/// was deleted. The alternative, keeping the markup and stripping the author
/// names, would leave the deleted sentence sitting in the file.
#[test]
fn cleaning_the_letter_accepts_the_insertion_and_discards_the_deletion() {
    let (path, data) = fixture_or_skip!("letter.docx");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    let document = fm_core::format::zip::read(&cleaned.data)
        .unwrap()
        .into_iter()
        .find(|m| m.name == "word/document.xml")
        .expect("document.xml missing");
    let xml = String::from_utf8_lossy(&document.data).to_string();

    assert!(!xml.contains("Previous cap"), "the deleted sentence survived: {xml}");
    assert!(!xml.contains("400,000"), "the superseded figure survived: {xml}");
    assert!(
        xml.contains("Settlement authority raised to CZK 1,950,000."),
        "the accepted text was lost: {xml}"
    );
    assert!(!xml.contains("w:del"), "deletion markup survived: {xml}");
    assert!(!xml.contains("w:ins"), "insertion markup survived: {xml}");
    assert!(!xml.contains("rsid"), "{xml}");
    assert!(!xml.contains("commentReference"), "{xml}");
}

#[test]
fn the_redacted_report_still_holds_the_account_number() {
    let (path, data) = fixture_or_skip!("report-redacted.pdf");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Pdf);

    let covered = ins
        .in_class(Class::RedactionIntegrity)
        .next()
        .expect("the covered text was not found");
    assert!(
        covered.value.as_deref().unwrap().contains("CZ65 0800 0000 1920 0014 5399"),
        "{:?}",
        covered.value
    );

    assert!(values(&ins, Class::Identity).join(" ").contains("Marie Dvorakova"));
    assert!(values(&ins, Class::Device).join(" ").contains("Acrobat Distiller 23.0"));
    assert!(values(&ins, Class::Correlation).join(" ").contains("9f2c41a055b14e7c9b306d1e2a77c004"));
    assert_eq!(ins.in_class(Class::HiddenPayload).count(), 2, "attachment and JavaScript");
}

#[test]
fn standard_reports_the_failed_redaction_and_strict_enforces_it() {
    let (path, data) = fixture_or_skip!("report-redacted.pdf");

    let reported = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    assert!(
        String::from_utf8_lossy(&reported.data).contains("CZ65 0800"),
        "standard must report the redaction, not silently change the page"
    );

    let enforced = clean_bytes(&data, ext(&path).as_deref(), &Profile::strict()).unwrap();
    assert!(
        !String::from_utf8_lossy(&enforced.data).contains("CZ65 0800"),
        "strict must actually remove the covered text"
    );
    let after = inspect_bytes(&enforced.data, Some("pdf")).unwrap();
    assert!(after.in_class(Class::RedactionIntegrity).next().is_none());
    assert!(enforced.verification.clean(), "{:?}", enforced.verification.leftovers);

    // Removing text is a content change, so it has to be flagged beforehand.
    let destructive = fm_core::report::destructive(&enforced.before, &Profile::strict());
    assert!(destructive.iter().any(|f| f.class == Class::RedactionIntegrity));
}

#[test]
fn the_incremental_report_gives_up_its_superseded_figure() {
    let (path, data) = fixture_or_skip!("report-incremental.pdf");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();

    let history = ins
        .in_class(Class::History)
        .find(|f| f.locator == "pdf:incremental")
        .expect("the earlier revision was not reported");
    let shown = history.value.as_deref().unwrap_or_default();
    assert!(shown.contains("2 revisions"), "{shown}");
    assert!(
        shown.contains("1,950,000") || shown.contains("1 950 000"),
        "the recoverable figure was not shown to the user: {shown}"
    );
}

#[test]
fn cleaning_flattens_the_revisions_away() {
    let (path, data) = fixture_or_skip!("report-incremental.pdf");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    assert!(cleaned.verification.clean(), "{:?}", cleaned.verification.leftovers);

    let text = String::from_utf8_lossy(&cleaned.data).to_string();
    assert!(!text.contains("1,950,000"), "the superseded revision survived");
    assert!(!text.contains("/Prev"), "the revision chain survived");
    let after = inspect_bytes(&cleaned.data, Some("pdf")).unwrap();
    assert!(after.in_class(Class::History).next().is_none());

    // And the current revision is intact: v2 replaced the figure with a word.
    assert!(text.contains("[REDACTED]"), "the current revision's text was lost");
}

#[test]
fn a_signed_pdf_is_refused_and_says_why() {
    let (path, data) = fixture_or_skip!("signed.pdf");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert!(ins.refused(), "a signed document must not be cleaned silently");
    assert!(ins.blockers[0].reason.contains("signed"));
    assert!(ins.blockers[0].detail.contains("Marie Dvorakova"));

    match clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()) {
        Err(fm_core::Error::Refused(why)) => assert!(why.contains("signature"), "{why}"),
        Err(e) => panic!("expected a refusal, got the error {e}"),
        Ok(_) => panic!("a signed document was cleaned instead of refused"),
    }
}

#[test]
fn every_fixture_is_either_cleaned_verifiably_or_refused_with_a_reason() {
    let names = [
        "photo-phone.jpg",
        "screenshot.png",
        "diagram.svg",
        "letter.docx",
        "report-redacted.pdf",
        "report-incremental.pdf",
        "signed.pdf",
        "clip.mp4",
    ];
    for name in names {
        let Some((path, data)) = fixture(name) else { continue };
        for profile in Profile::builtins() {
            if profile.inert() {
                continue;
            }
            match clean_bytes(&data, ext(&path).as_deref(), &profile) {
                Ok(cleaned) => assert!(
                    cleaned.verification.clean(),
                    "{name} under {}: {:?}",
                    profile.name,
                    cleaned.verification.leftovers.iter().map(|f| &f.locator).collect::<Vec<_>>()
                ),
                Err(fm_core::Error::Refused(why)) => {
                    assert!(!why.is_empty(), "{name} refused with no reason");
                    assert_eq!(name, "signed.pdf", "{name} was refused unexpectedly: {why}");
                }
                Err(e) => panic!("{name} under {}: {e}", profile.name),
            }
        }
    }
}

#[test]
fn the_clip_reveals_where_and_on_what_it_was_recorded() {
    let (path, data) = fixture_or_skip!("clip.mp4");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Mp4);

    let location = values(&ins, Class::Location).join(" | ");
    assert!(location.contains("+50.0755+014.4378"), "{location}");
    let device = values(&ins, Class::Device).join(" | ");
    assert!(device.contains("iPhone 14 Pro"), "{device}");
    assert!(device.contains("Apple"), "{device}");
    assert!(values(&ins, Class::Identity).join(" ").contains("Marie Dvorakova"));
    // The mvhd creation time, which the manifest pins to the same instant.
    assert!(values(&ins, Class::Time).join(" ").contains("2024-06-14T19:42:07Z"));
    assert!(ins.shape.contains(&"mp4:udta".to_string()));
}

/// The output has to be a file a player will still open, and the only honest
/// check for that is an independent demuxer. `ffprobe` is the one to hand.
#[test]
fn a_cleaned_clip_still_demuxes() {
    let (path, data) = fixture_or_skip!("clip.mp4");
    if std::process::Command::new("ffprobe").arg("-version").output().is_err() {
        eprintln!("skipping: ffprobe is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("fm-core-mp4-test");
    std::fs::create_dir_all(&dir).unwrap();

    for profile in Profile::builtins() {
        if profile.inert() {
            continue;
        }
        let cleaned = clean_bytes(&data, ext(&path).as_deref(), &profile).unwrap();
        assert!(cleaned.verification.clean(), "{:?}", cleaned.verification.leftovers);

        let out = dir.join(format!("{}.mp4", profile.name));
        std::fs::write(&out, &cleaned.data).unwrap();
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&probe.stderr).to_string();
        let stdout = String::from_utf8_lossy(&probe.stdout).trim().to_string();
        assert!(probe.status.success() && stderr.is_empty(), "{}: ffprobe said {stderr}", profile.name);
        let duration: f64 = stdout.parse().unwrap_or(0.0);
        assert!(duration > 0.0, "{}: ffprobe found no duration", profile.name);

        // And ffprobe must no longer see the metadata either.
        let tags = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "format_tags", "-of", "default=noprint_wrappers=1"])
            .arg(&out)
            .output()
            .unwrap();
        let tags = String::from_utf8_lossy(&tags.stdout).to_string();
        assert!(!tags.contains("Marie"), "{}: ffprobe still sees {tags}", profile.name);
        assert!(!tags.contains("iPhone"), "{}: ffprobe still sees {tags}", profile.name);
        assert!(!tags.contains("50.07"), "{}: ffprobe still sees {tags}", profile.name);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
