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
    assert_eq!(thumb.value.as_deref(), Some("5905 bytes of JPEG"));

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
