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

/// What the manifest recorded about a fixture when it was generated, for the
/// facts that are properties of the machine that generated it rather than
/// declarations about the format.
///
/// The thumbnail is encoded by ffmpeg, so its byte count belongs here: this
/// test asserted 5905 bytes and a runner with a different ffmpeg produced
/// 5904, which said nothing about the engine. The contract worth asserting is
/// that the engine reports the size the generator wrote down.
fn observed(fixture: &str, key: &str) -> Option<u64> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    manifest.get("fixtures")?.get(fixture)?.get("observed")?.get(key)?.as_u64()
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
    let declared = observed("photo-phone.jpg", "thumb_bytes")
        .expect("the manifest does not record the thumbnail size");
    assert_eq!(thumb.value.as_deref(), Some(format!("{declared} bytes").as_str()));
    assert!(declared > 4000, "the thumbnail is too small to be an uncropped original");
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

    // The parked text comes after a transform group that has already closed,
    // and the only thing reported as hidden is the text itself: the bleed
    // rectangle is anchored off the canvas and covers all of it, which is
    // artwork.
    let hidden: Vec<&str> = ins
        .findings
        .iter()
        .filter(|f| f.locator.starts_with("svg:offcanvas"))
        .map(|f| f.locator.as_str())
        .collect();
    assert_eq!(hidden, vec!["svg:offcanvas:text@40,400"], "{hidden:?}");
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
    assert!(text.contains(r#"width="220""#), "the bleed background was deleted:\n{text}");
    assert!(text.contains(r#"x="10" y="20""#), "the artwork in the group went:\n{text}");

    // And under every profile, including the one that rewrites most.
    for profile in Profile::builtins() {
        if profile.inert() {
            continue;
        }
        let out = clean_bytes(&data, ext(&path).as_deref(), &profile).unwrap();
        let text = String::from_utf8(out.data).unwrap();
        assert!(
            text.contains(r#"width="220""#) && text.contains(r#"x="10" y="20""#),
            "{}: visible artwork was removed:\n{text}",
            profile.name
        );
        assert!(!text.contains("-18 FTE"), "{}: the parked text survived", profile.name);
    }

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
    // Two in the catalog name tree, two reachable only through the page: an
    // annotation holding a file, and the page's own open action.
    let payloads: Vec<&str> =
        ins.in_class(Class::HiddenPayload).map(|f| f.locator.as_str()).collect();
    assert_eq!(
        payloads,
        vec![
            "pdf:names:EmbeddedFiles",
            "pdf:catalog:OpenAction",
            "pdf:page:1:AA",
            "pdf:page:1:annot:FileAttachment",
        ],
        "{payloads:?}"
    );
    assert!(values(&ins, Class::HiddenPayload).join(" ").contains("payroll-draft.csv"));
}

/// Unlinking an attachment leaves its bytes in the file, so the test is for
/// the bytes, not the pointer. `fixtures/checkpdf.py` separately asserts the
/// result is still a structurally sound PDF after the surgery.
#[test]
fn cleaning_the_report_removes_the_payloads_the_page_carried() {
    let (path, data) = fixture_or_skip!("report-redacted.pdf");
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    assert!(cleaned.verification.clean(), "{:?}", cleaned.verification.leftovers);

    for gone in [
        &b"payroll-draft.csv"[..],
        &b"Jan Prochazka,184000"[..],
        &b"mailDoc"[..],
        &b"FileAttachment"[..],
        &b"settle below 400k"[..],
    ] {
        assert!(
            !cleaned.data.windows(gone.len()).any(|w| w == gone),
            "{} survived the clean",
            String::from_utf8_lossy(gone)
        );
    }
    let changed: Vec<&str> = cleaned.changes.iter().map(|c| c.locator.as_str()).collect();
    assert!(changed.contains(&"pdf:page:1:annot:FileAttachment"), "{changed:?}");
    assert!(changed.contains(&"pdf:page:1:AA"), "{changed:?}");
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

/// Read with the OOXML part names, an ODF package has no properties at all,
/// which in a report is indistinguishable from a document that has none. The
/// fixture declares six identity fields, a generator, three dates, the editing
/// history, a deleted figure with its author and a first-page preview, none of
/// which this engine reads yet. So the contract is a refusal naming the
/// format, on inspect and on clean.
#[test]
fn an_opendocument_file_is_refused_by_name_rather_than_reported_clean() {
    let (path, data) = fixture_or_skip!("report.odt");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Opendocument);
    assert!(ins.refused(), "an ODF package was inspected as an Office document");
    assert!(ins.blockers[0].reason.contains("OpenDocument"), "{:?}", ins.blockers[0]);
    assert!(ins.blockers[0].detail.contains("meta.xml"), "{:?}", ins.blockers[0]);
    assert!(ins.findings.is_empty(), "a field count on ODF overstates what was read");

    match clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()) {
        Err(fm_core::Error::Refused(why)) => assert!(why.contains("OpenDocument"), "{why}"),
        Err(e) => panic!("expected a refusal, got the error {e}"),
        Ok(_) => panic!("an ODF package was cleaned, leaving every declared field in it"),
    }
}

/// EXIF says these fields are ASCII and a Windows tool wrote its codepage, so
/// nothing in the file declares the encoding. Read as UTF-8 every accented
/// letter became U+FFFD: the window drew tofu, and a report full of question
/// marks says less about the file than the file does.
#[test]
fn a_name_in_a_windows_codepage_is_shown_as_a_name() {
    let (path, data) = fixture_or_skip!("photo-codepage.jpg");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();

    let identity = values(&ins, Class::Identity).join(" | ");
    assert!(identity.contains("Jürgen Müller"), "{identity}");
    assert!(values(&ins, Class::Device).join(" ").contains("Müller Zeichenbüro"));
    assert!(identity.contains("Köln-Süd"), "{identity}");

    // Nowhere in the whole report, not only in the values.
    let everything: String = ins
        .findings
        .iter()
        .map(|f| {
            format!("{} {} {} {}", f.locator, f.label, f.value.clone().unwrap_or_default(),
                    f.note.clone().unwrap_or_default())
        })
        .collect();
    assert!(!everything.contains('\u{fffd}'), "a replacement character reached the report");

    // And it says the encoding was a fallback, because the same bytes in a
    // Central European codepage would be different letters.
    let caveats = ins.findings.iter().filter(|f| {
        f.note.as_deref().is_some_and(|n| n.contains("not UTF-8"))
    });
    assert!(caveats.count() >= 2, "the guess is not reported: {everything}");

    // Cleaning is unaffected by how the bytes read.
    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    assert!(cleaned.verification.clean(), "{:?}", cleaned.verification.leftovers);
    assert!(!cleaned.data.windows(9).any(|w| w == b"J\xfcrgen M"), "the name is still in the file");
}

/// The never-remove rule in the format where it is hardest: a whole eXIf block
/// is unusual in a PNG and worth dropping, and the orientation inside it must
/// not go with it. It was going, and the count said "1 preserved" while it
/// went, which is worse than either outcome on its own.
#[test]
fn a_pngs_orientation_and_colour_profile_survive_every_profile() {
    let (path, data) = fixture_or_skip!("photo-oriented.png");
    let before = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert!(values(&before, Class::Device).join(" ").contains("iPhone 14 Pro"));

    for profile in Profile::builtins() {
        if profile.inert() {
            continue;
        }
        let cleaned = clean_bytes(&data, ext(&path).as_deref(), &profile).unwrap();
        assert!(cleaned.verification.clean(), "{}: {:?}", profile.name, cleaned.verification.leftovers);
        let after = &cleaned.after;

        let orientation = after
            .findings
            .iter()
            .find(|f| f.locator == "exif:ifd0:0x0112")
            .unwrap_or_else(|| panic!("{}: the orientation is gone", profile.name));
        assert_eq!(orientation.value.as_deref(), Some("6"), "{}", profile.name);
        assert!(orientation.protected, "{}", profile.name);
        assert!(
            after.findings.iter().any(|f| f.locator == "png:iCCP"),
            "{}: the colour profile is gone",
            profile.name
        );

        // The camera goes, and is named in the diff as having gone.
        assert!(!values(after, Class::Device).join(" ").contains("iPhone 14 Pro"), "{}", profile.name);
        assert!(cleaned.changes.iter().any(|c| c.before.as_deref() == Some("iPhone 14 Pro")));

        // And nothing that stayed is listed as a change, which is how the
        // count came to claim a field it had dropped.
        for kept in ["exif:ifd0:0x0112", "png:iCCP"] {
            assert!(
                !cleaned.changes.iter().any(|c| c.locator == kept),
                "{}: {kept} both stayed and was reported as changed",
                profile.name
            );
        }
    }
}

/// Hostile rather than broken: every bracket matches and `checkpdf.py` agrees
/// the structure is sound, which is why a recursive parser walks straight into
/// it and the process aborts with a core dump. The depth is counted before the
/// parser sees the bytes, so the answer is a refusal and the rest of a folder
/// run carries on.
///
/// If this regresses, it will not fail: it will take the test binary down.
#[test]
fn a_pdf_nested_deeper_than_the_parser_survives_is_refused() {
    let (path, data) = fixture_or_skip!("nested.pdf");
    match inspect_bytes(&data, ext(&path).as_deref()) {
        Err(fm_core::Error::Refused(why)) => {
            assert!(why.contains("nest"), "{why}");
            assert!(why.contains("20000") || why.contains("20001"), "{why}");
        }
        Err(e) => panic!("expected a refusal naming the depth, got {e}"),
        Ok(ins) => panic!("a file nested 20000 deep was parsed into {} fields", ins.findings.len()),
    }
    assert!(clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).is_err());
}

/// An encrypted document used to be inspected as an ordinary one and cleaned
/// into a file that still said `/Encrypt` while its strings had been rewritten
/// in plaintext and its `/ID`, which is an input to the key, replaced. That
/// file is not cleaned, it is destroyed.
#[test]
fn an_encrypted_pdf_is_refused_with_nothing_reported_out_of_it() {
    let (path, data) = fixture_or_skip!("encrypted.pdf");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert!(ins.refused(), "an encrypted document was inspected as an ordinary one");
    assert!(ins.blockers[0].reason.contains("encrypted"), "{:?}", ins.blockers[0]);
    assert!(ins.blockers[0].detail.contains("Standard"), "{:?}", ins.blockers[0]);
    assert!(ins.findings.is_empty(), "ciphertext was reported as fields: {:?}", ins.findings);

    match clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()) {
        Err(fm_core::Error::Refused(why)) => assert!(why.contains("encrypted"), "{why}"),
        Err(e) => panic!("expected a refusal, got {e}"),
        Ok(_) => panic!("an encrypted document was cleaned"),
    }
}

/// The guarantee, asserted as a claim rather than left to an error path:
/// every change names a field the report named first, unless it says it is a
/// consequence of cleaning. The photograph used to lose `exif:exif:0x9000`
/// with no mention of it in the report or the diff.
#[test]
fn every_change_to_every_fixture_names_something_the_report_named_first() {
    let names = [
        "photo-phone.jpg",
        "photo-codepage.jpg",
        "photo-oriented.png",
        "screenshot.png",
        "diagram.svg",
        "letter.docx",
        "report-redacted.pdf",
        "report-incremental.pdf",
        "clip.mp4",
        "clip-ios.mov",
    ];
    for name in names {
        let Some((path, data)) = fixture(name) else { continue };
        for profile in Profile::builtins() {
            if profile.inert() {
                continue;
            }
            let cleaned = clean_bytes(&data, ext(&path).as_deref(), &profile).unwrap();
            let reported: std::collections::HashSet<&str> =
                cleaned.before.findings.iter().map(|f| f.locator.as_str()).collect();
            for change in &cleaned.changes {
                assert!(
                    change.derived || reported.contains(change.locator.as_str()),
                    "{name} under {}: {} was changed and never reported",
                    profile.name,
                    change.locator
                );
            }
        }
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
        "encrypted.pdf",
        "photo-codepage.jpg",
        "photo-oriented.png",
        "report.odt",
        "clip.mp4",
        "clip-ios.mov",
        "clip-truncated.mp4",
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
                    assert!(
                        matches!(name, "signed.pdf" | "report.odt" | "encrypted.pdf"),
                        "{name} was refused unexpectedly: {why}"
                    );
                }
                // An incomplete copy is the one case where refusing to answer
                // is the answer: see the fixture's declared contract.
                Err(fm_core::Error::Malformed { detail, .. }) => {
                    assert_eq!(name, "clip-truncated.mp4", "{name} is malformed: {detail}");
                    assert!(detail.contains("incomplete"), "{detail}");
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

/// What a recent iPhone writes: the values sit in a numbered `ilst` and the
/// names only in the sibling `keys` dictionary, so a handler that knows
/// four-byte atom types alone reports this file as holding nothing but its
/// filesystem dates. `ffprobe` reads the same keys back, which is what makes
/// the fixture evidence rather than an assumption.
#[test]
fn the_ios_clip_reveals_what_only_the_keys_dictionary_names() {
    let (path, data) = fixture_or_skip!("clip-ios.mov");
    let ins = inspect_bytes(&data, ext(&path).as_deref()).unwrap();
    assert_eq!(ins.format, FormatId::Mp4);

    let device = values(&ins, Class::Device).join(" | ");
    assert!(device.contains("iPhone 14 Pro"), "{device}");
    assert!(device.contains("Apple"), "{device}");
    assert!(device.contains("17.4.1"), "{device}");
    let location = values(&ins, Class::Location).join(" | ");
    assert!(location.contains("+50.0755+014.4378"), "{location}");
    assert!(values(&ins, Class::Time).join(" ").contains("2024-06-14T19:42:07"));
    let correlation = values(&ins, Class::Correlation).join(" | ");
    assert!(correlation.contains("9F2C14D8"), "{correlation}");

    // Each finding names the key it came from, so the report can say which
    // field it means rather than quoting an index.
    let labels: String = ins.findings.iter().map(|f| f.label.as_str()).collect();
    assert!(labels.contains("com.apple.quicktime.model"), "{labels}");

    let cleaned = clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).unwrap();
    assert!(cleaned.verification.clean(), "{:?}", cleaned.verification.leftovers);
    let after = inspect_bytes(&cleaned.data, ext(&path).as_deref()).unwrap();
    for class in [Class::Device, Class::Location, Class::Correlation] {
        assert!(values(&after, class).is_empty(), "{class:?} survived: {:?}", values(&after, class));
    }
    // The dictionary goes with the values: on its own it still names every
    // field the recorder wrote.
    assert!(!cleaned.data.windows(4).any(|w| w == b"keys"), "the keys dictionary survived");
}

/// A copy that stopped short. Every declared field is still in the bytes and
/// `ffprobe` still hands them out, so the honest answer is a refusal; the
/// field count this used to report was a false clean on a file that had lost
/// nothing.
#[test]
fn an_interrupted_copy_is_refused_rather_than_part_read() {
    let (path, data) = fixture_or_skip!("clip-truncated.mp4");
    match inspect_bytes(&data, ext(&path).as_deref()) {
        Err(fm_core::Error::Malformed { detail, .. }) => {
            assert!(detail.contains("incomplete"), "{detail}")
        }
        Err(e) => panic!("expected a refusal naming the cause, got {e}"),
        Ok(ins) => panic!("a truncated clip inspected as {} fields", ins.findings.len()),
    }
    assert!(clean_bytes(&data, ext(&path).as_deref(), &Profile::standard()).is_err());
}

/// The output has to be a file a player will still open, and the only honest
/// check for that is an independent demuxer. `ffprobe` is the one to hand.
#[test]
fn a_cleaned_clip_still_demuxes() {
    for name in ["clip.mp4", "clip-ios.mov"] {
        a_cleaned_clip_still_demuxes_one(name);
    }
}

fn a_cleaned_clip_still_demuxes_one(name: &str) {
    let Some((path, data)) = fixture(name) else {
        eprintln!("skipping: fixtures/out/{name} absent, run fixtures/generate.py");
        return;
    };
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

        let out = dir.join(format!("{name}.{}.mp4", profile.name));
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
        for leak in ["Marie", "iPhone", "50.07", "9F2C14D8", "17.4.1"] {
            assert!(!tags.contains(leak), "{name} under {}: ffprobe still sees {tags}", profile.name);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reading a cleaned PDF back through the library that wrote it proves the
/// library is self-consistent and nothing else. `fixtures/checkpdf.py` parses
/// the cross-reference table by hand and asserts every offset lands on the
/// object it claims to, which is exactly the failure a self-consistent library
/// would hide and which produces a file every other reader rejects.
#[test]
fn cleaned_pdfs_pass_an_independent_structural_check() {
    let checker = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/checkpdf.py");
    if !checker.exists() {
        eprintln!("skipping: fixtures/checkpdf.py is missing");
        return;
    }
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping: python3 is not installed");
        return;
    }

    let dir = std::env::temp_dir().join("fm-core-pdf-check");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut written: Vec<PathBuf> = Vec::new();

    for name in ["report-redacted.pdf", "report-incremental.pdf"] {
        let Some((path, data)) = fixture(name) else { continue };
        for profile in Profile::builtins() {
            if profile.inert() {
                continue;
            }
            let cleaned = clean_bytes(&data, ext(&path).as_deref(), &profile).unwrap();
            let out = dir.join(format!("{}-{name}", profile.name));
            std::fs::write(&out, &cleaned.data).unwrap();
            written.push(out);
        }
    }
    if written.is_empty() {
        eprintln!("skipping: fixtures/out absent, run fixtures/generate.py");
        return;
    }

    let output = std::process::Command::new("python3")
        .arg(&checker)
        .arg("--cleaned")
        .args(&written)
        .output()
        .expect("python3 would not start");
    let report = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(output.status.success(), "{report}");

    // A file the checker cannot read is not a file that passed. Saying so
    // loudly matters: a quiet skip is how a malformed trailer survived here
    // once, because the output whose structure was least verified was exactly
    // the output that went unchecked.
    assert!(!report.contains("UNCHECKED"), "{report}");
    assert_eq!(
        report.matches("PASS").count(),
        written.len(),
        "not every file was checked:\n{report}"
    );

    // Flattening is the claim that matters most, and it is checked by a parser
    // that has never heard of this engine: the incremental fixture goes in with
    // two revisions and has to come out with one.
    for line in report.lines().filter(|l| l.contains("incremental")) {
        assert!(line.contains("1 revision(s)"), "{line}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
