use super::*;
use crate::report::Verification;
use lopdf::{dictionary, Stream, StringFormat};

/// A one-page PDF with the usual information dictionary, built through lopdf so
/// that the cross-reference table is always valid.
fn build(content: &str, extra_catalog: Dictionary, info: Dictionary) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        "Contents" => content_id,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    for (k, v) in extra_catalog.iter() {
        catalog.set(k.to_vec(), v.clone());
    }
    let catalog_id = doc.add_object(catalog);
    let info_id = doc.add_object(info);
    doc.trailer.set("Root", catalog_id);
    doc.trailer.set("Info", info_id);
    doc.trailer.set(
        "ID",
        Object::Array(vec![
            Object::String(vec![0x9f, 0x2c, 0x41, 0xa0], StringFormat::Hexadecimal),
            Object::String(vec![0x9f, 0x2c, 0x41, 0xa0], StringFormat::Hexadecimal),
        ]),
    );
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn standard_info() -> Dictionary {
    dictionary! {
        "Title" => Object::string_literal("Quarterly review - FINAL"),
        "Author" => Object::string_literal("Marie Dvorakova"),
        "Subject" => Object::string_literal("Internal"),
        "Keywords" => Object::string_literal("confidential, draft"),
        "Creator" => Object::string_literal("Microsoft Word 2021"),
        "Producer" => Object::string_literal("Acrobat Distiller 23.0"),
        "CreationDate" => Object::string_literal("D:20240614194207+02'00'"),
        "ModDate" => Object::string_literal("D:20240615081133+02'00'"),
    }
}

/// The classic failed redaction: text drawn first, black rectangle second.
const REDACTED: &str = "BT /F1 11 Tf 72 720 Td (Quarterly review) Tj ET\n\
                        BT /F1 11 Tf 72 700 Td (Claimant bank account CZ65 0800 0000 1920 0014 5399) Tj ET\n\
                        0 0 0 rg 68 694 420 20 re f\n";

fn plain() -> Vec<u8> {
    build(REDACTED, Dictionary::new(), standard_info())
}

fn values(ins: &Inspection, class: Class) -> String {
    ins.in_class(class).filter_map(|f| f.value.clone()).collect::<Vec<_>>().join(" | ")
}

#[test]
fn finds_the_author_the_producer_the_dates_and_the_identifier() {
    let ins = Pdf.inspect(&plain()).unwrap();
    assert_eq!(ins.format, FormatId::Pdf);
    assert!(values(&ins, Class::Identity).contains("Marie Dvorakova"));
    assert!(values(&ins, Class::Device).contains("Acrobat Distiller 23.0"));
    assert!(values(&ins, Class::Time).contains("20240614194207"));
    assert!(values(&ins, Class::Correlation).contains("9f2c41a0"));
}

#[test]
fn finds_text_hiding_under_a_black_rectangle() {
    let ins = Pdf.inspect(&plain()).unwrap();
    let f = ins
        .in_class(Class::RedactionIntegrity)
        .next()
        .expect("covered text not found");
    assert!(f.value.as_deref().unwrap().contains("CZ65 0800"));
    assert_eq!(f.impact, Impact::Content);
    // The heading above the rectangle is not covered and must not be reported.
    assert_eq!(ins.in_class(Class::RedactionIntegrity).count(), 1);
}

#[test]
fn standard_reports_the_covered_text_without_touching_it() {
    let cleaned = Pdf.clean(&plain(), &Profile::standard()).unwrap();
    let text = String::from_utf8_lossy(&cleaned.data).to_string();
    assert!(text.contains("CZ65 0800"), "standard must only report, not enforce");
    let after = Pdf.inspect(&cleaned.data).unwrap();
    assert!(Verification::after(&after, &Profile::standard(), &cleaned.changes).clean());
}

#[test]
fn strict_removes_the_covered_text_and_keeps_the_rest_of_the_page() {
    let cleaned = Pdf.clean(&plain(), &Profile::strict()).unwrap();
    let after = Pdf.inspect(&cleaned.data).unwrap();
    assert!(after.in_class(Class::RedactionIntegrity).next().is_none());
    assert!(cleaned.changes.iter().any(|c| c.class == Class::RedactionIntegrity));

    let doc = Document::load_mem(&cleaned.data).unwrap();
    let page_id = *doc.get_pages().values().next().unwrap();
    let content = doc.get_and_decode_page_content(page_id).unwrap();
    let encoded = String::from_utf8_lossy(&Content { operations: content.operations }.encode().unwrap()).to_string();
    assert!(!encoded.contains("CZ65"), "{encoded}");
    assert!(encoded.contains("Quarterly review"), "the visible heading was lost: {encoded}");
    assert!(encoded.contains("re"), "the rectangle itself should stay: {encoded}");
}

/// The rewrite pins the cross-reference style, which is what makes the
/// structural-fingerprint finding a claim the engine can keep and what lets an
/// independent checker verify the output by hand.
#[test]
fn every_cleaned_pdf_comes_out_with_a_classic_cross_reference_table() {
    for profile in Profile::builtins() {
        if profile.inert() {
            continue;
        }
        let cleaned = Pdf.clean(&plain(), &profile).unwrap();
        let has_table = cleaned.data.windows(5).any(|w| w == b"\nxref");
        let has_stream = cleaned.data.windows(5).any(|w| w == b"/XRef");
        assert!(has_table, "{}: no cross-reference table", profile.name);
        assert!(!has_stream, "{}: the trailer still names a cross-reference stream", profile.name);
        assert!(
            !cleaned.data.windows(5).any(|w| w == b"/Prev"),
            "{}: the revision chain survived",
            profile.name
        );

        let after = Pdf.inspect(&cleaned.data).unwrap();
        assert!(
            after.in_class(Class::StructuralFingerprint).all(|f| f.protected),
            "{}: {:?}",
            profile.name,
            after.in_class(Class::StructuralFingerprint).map(|f| &f.locator).collect::<Vec<_>>()
        );
    }

    // And the input really was a stream, so the test is not vacuous.
    assert!(plain().windows(5).any(|w| w == b"/XRef"), "the fixture is not a stream any more");
}

#[test]
fn standard_strips_the_information_dictionary() {
    let cleaned = Pdf.clean(&plain(), &Profile::standard()).unwrap();
    let after = Pdf.inspect(&cleaned.data).unwrap();
    assert!(after.in_class(Class::Identity).next().is_none());
    assert!(after.in_class(Class::Device).next().is_none());
    // The date stays, because standard keeps the time class.
    assert!(values(&after, Class::Time).contains("20240614"));
    assert!(Verification::after(&after, &Profile::standard(), &cleaned.changes).clean());
}

/// The replaced identifier is the one case where a field in a class the profile
/// said to remove is supposed to still be present, so it is also the case that
/// shows what verification can and cannot settle on its own.
#[test]
fn a_replaced_identifier_is_resolved_by_the_diff_and_unresolved_without_it() {
    let cleaned = Pdf.clean(&plain(), &Profile::standard()).unwrap();
    let after = Pdf.inspect(&cleaned.data).unwrap();

    let with_diff = Verification::after(&after, &Profile::standard(), &cleaned.changes);
    assert!(with_diff.clean());
    assert!(with_diff.unresolved.is_empty());

    // Checking the finished file on its own cannot settle it either way, so it
    // is reported as unsettled rather than as a failure.
    let alone = Verification::of(&after, &Profile::standard());
    assert!(alone.clean(), "a file that may well be clean must not be called dirty");
    assert_eq!(alone.unresolved.len(), 1);
    assert_eq!(alone.unresolved[0].locator, "pdf:trailer:ID");
}

#[test]
fn the_identifier_is_replaced_rather_than_removed() {
    let cleaned = Pdf.clean(&plain(), &Profile::standard()).unwrap();
    let after = Pdf.inspect(&cleaned.data).unwrap();
    let id = after
        .in_class(Class::Correlation)
        .next()
        .expect("almost every PDF has an identifier; removing it is conspicuous");
    assert!(!id.value.as_deref().unwrap().contains("9f2c41a0"), "the original identifier survived");
}

#[test]
fn stripping_to_the_minimum_removes_the_identifier_outright() {
    let mut p = Profile::standard();
    p.blending = Blending::Minimal;
    let cleaned = Pdf.clean(&plain(), &p).unwrap();
    let after = Pdf.inspect(&cleaned.data).unwrap();
    assert!(after.in_class(Class::Correlation).next().is_none());
}

#[test]
fn deriving_the_identifier_twice_gives_the_same_answer() {
    let once = Pdf.clean(&plain(), &Profile::standard()).unwrap();
    let twice = Pdf.clean(&plain(), &Profile::standard()).unwrap();
    let id = |data: &[u8]| {
        values(&Pdf.inspect(data).unwrap(), Class::Correlation)
    };
    assert_eq!(id(&once.data), id(&twice.data));
}

#[test]
fn javascript_and_attachments_go() {
    let catalog = dictionary! {
        "OpenAction" => dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('hi')") },
        "Names" => dictionary! { "EmbeddedFiles" => dictionary! { "Names" => Object::Array(vec![]) } },
    };
    let data = build(REDACTED, catalog, standard_info());
    let ins = Pdf.inspect(&data).unwrap();
    assert_eq!(ins.in_class(Class::HiddenPayload).count(), 2, "{:?}", ins.findings);

    let cleaned = Pdf.clean(&data, &Profile::standard()).unwrap();
    let after = Pdf.inspect(&cleaned.data).unwrap();
    assert!(after.in_class(Class::HiddenPayload).next().is_none());
    assert!(Verification::after(&after, &Profile::standard(), &cleaned.changes).clean());
}

#[test]
fn a_signed_document_is_refused_rather_than_broken() {
    let mut doc = Document::with_version("1.7");
    let sig_id = doc.add_object(dictionary! {
        "Type" => "Sig",
        "Filter" => "Adobe.PPKLite",
        "Name" => Object::string_literal("Marie Dvorakova"),
    });
    let pages_id = doc.add_object(dictionary! { "Type" => "Pages", "Kids" => Object::Array(vec![]), "Count" => 0 });
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
        "AcroForm" => dictionary! { "SigFlags" => 3, "Fields" => vec![sig_id.into()] },
    });
    doc.trailer.set("Root", catalog_id);
    let mut data = Vec::new();
    doc.save_to(&mut data).unwrap();

    let ins = Pdf.inspect(&data).unwrap();
    assert!(ins.refused(), "a signature must be a blocker");
    assert!(ins.blockers[0].detail.contains("Marie Dvorakova"));
    // The name on the signature is reported, and marked as unremovable.
    assert!(ins.findings.iter().any(|f| f.locator == "pdf:sig:name" && f.protected));

    assert!(matches!(Pdf.clean(&data, &Profile::standard()), Err(Error::Refused(_))));
    assert!(matches!(
        crate::clean_bytes(&data, Some("pdf"), &Profile::standard()),
        Err(Error::Refused(_))
    ));
}

#[test]
fn the_accessibility_tree_is_never_removed() {
    let catalog = dictionary! {
        "StructTreeRoot" => dictionary! { "Type" => "StructTreeRoot" },
        "MarkInfo" => dictionary! { "Marked" => true },
    };
    let data = build(REDACTED, catalog, standard_info());
    let cleaned = Pdf.clean(&data, &Profile::uniform()).unwrap();
    let doc = Document::load_mem(&cleaned.data).unwrap();
    assert!(doc.catalog().unwrap().get(b"StructTreeRoot").is_ok());
}

#[test]
fn inspect_only_returns_the_bytes_untouched() {
    let data = plain();
    assert_eq!(Pdf.clean(&data, &Profile::inspect_only()).unwrap().data, data);
}

#[test]
fn every_profile_leaves_a_verifiably_clean_pdf() {
    let data = plain();
    for profile in Profile::builtins() {
        if profile.inert() {
            continue;
        }
        let cleaned = Pdf.clean(&data, &profile).unwrap();
        let after = Pdf.inspect(&cleaned.data).unwrap();
        let v = Verification::after(&after, &profile, &cleaned.changes);
        assert!(
            v.clean(),
            "{}: {:?}",
            profile.name,
            v.leftovers.iter().map(|f| &f.locator).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_stroked_outline_is_not_a_redaction() {
    // `S` strokes the rectangle instead of filling it, so nothing is hidden.
    let content = "BT /F1 11 Tf 72 700 Td (account 123) Tj ET\n68 694 420 20 re S\n";
    let data = build(content, Dictionary::new(), standard_info());
    let ins = Pdf.inspect(&data).unwrap();
    assert!(ins.in_class(Class::RedactionIntegrity).next().is_none());
}

#[test]
fn a_rectangle_drawn_before_the_text_is_not_a_redaction() {
    // Painting order decides: a highlight under the text hides nothing.
    let content = "1 1 0 rg 68 694 420 20 re f\nBT /F1 11 Tf 72 700 Td (account 123) Tj ET\n";
    let data = build(content, Dictionary::new(), standard_info());
    let ins = Pdf.inspect(&data).unwrap();
    assert!(ins.in_class(Class::RedactionIntegrity).next().is_none());
}

#[test]
fn a_transform_is_followed_when_deciding_what_covers_what() {
    // The text is placed at 72,100 in its own space and translated up by 600,
    // so it lands under the rectangle at 694.
    let content = "q 1 0 0 1 0 600 cm BT /F1 11 Tf 72 100 Td (account 123) Tj ET Q\n\
                   0 0 0 rg 68 694 420 20 re f\n";
    let data = build(content, Dictionary::new(), standard_info());
    let ins = Pdf.inspect(&data).unwrap();
    assert!(
        ins.in_class(Class::RedactionIntegrity).next().is_some(),
        "the transform was not applied"
    );
}
