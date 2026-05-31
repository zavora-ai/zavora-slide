//! Corpus test: open real PowerPoint-authored `.pptx` files.
//!
//! Asserts the engine opens genuine PowerPoint decks, extracts their text, and
//! round-trips them faithfully: an unedited deck saves byte-identical; editing a
//! slide overlays only that slide while preserving every other part; structural
//! changes fall back to a full model rebuild.

use zavora_slide::Presentation;
use zavora_slide_opc::OpcPackage;
use zavora_slide_oxml::Document;

const SAMPLE: &str = "tests/corpus/powerpoint_sample.pptx";

/// All byte-comparable entries of a package: parts, every part's `.rels`, the
/// package `.rels`, and `[Content_Types].xml`. Used to assert true byte-fidelity
/// (the earlier `get_part`-only checks silently skipped rels/content-types).
fn package_entries(pkg: &OpcPackage) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut m = std::collections::BTreeMap::new();
    for name in pkg.part_names() {
        m.insert(name.to_string(), pkg.get_part(name).unwrap().to_vec());
    }
    for (part, rels) in &pkg.part_rels {
        m.insert(format!("rels::{part}"), rels.to_xml().unwrap());
    }
    m.insert("rels::PACKAGE".into(), pkg.package_rels.to_xml().unwrap());
    m.insert("[Content_Types].xml".into(), pkg.content_types.to_xml().unwrap());
    m
}

fn reopen(buf: Vec<u8>) -> OpcPackage {
    OpcPackage::from_reader(std::io::Cursor::new(buf)).unwrap()
}

#[test]
fn dom_round_trips_every_xml_part_byte_for_byte() {
    // The lossless XML DOM is the foundation for surgical editing: every real
    // PowerPoint-authored part must parse and re-serialize byte-identical.
    let pkg = OpcPackage::open(SAMPLE).unwrap();
    let mut checked = 0;
    for name in pkg.part_names() {
        if !name.ends_with(".xml") {
            continue;
        }
        let src = pkg.get_part(name).unwrap();
        let doc = Document::parse(src).unwrap_or_else(|e| panic!("parse {name}: {e}"));
        assert_eq!(doc.to_bytes(), src, "DOM byte-faithful for {name}");
        checked += 1;
    }
    assert!(checked >= 5, "expected several xml parts, got {checked}");
}

#[test]
fn opened_slides_carry_byte_faithful_dom() {
    // Every opened slide parses into a DOM that re-serializes byte-identical to
    // its source part — the foundation for surgical editing.
    let p = Presentation::open(SAMPLE).unwrap();
    let pkg = OpcPackage::open(SAMPLE).unwrap();
    for i in 0..p.slide_count() {
        let (part, dom_bytes) = p.slide_dom_debug(i).expect("opened slide has a DOM");
        assert_eq!(dom_bytes, pkg.get_part(&part).unwrap(), "DOM faithful for {part}");
    }
}

#[test]
fn opens_real_powerpoint_deck() {
    let p = Presentation::open(SAMPLE).expect("open PowerPoint-authored deck");
    assert_eq!(p.slide_count(), 3, "should recover all slides");
    let md = p.to_markdown();
    // Text authored by python-pptx is recovered.
    assert!(md.contains("Corpus Sample"), "title text extracted");
    assert!(md.contains("First item"), "bullet text extracted");
    assert!(md.contains("Free text box"), "text-box content extracted");
}

#[test]
fn opc_layer_round_trips_all_parts() {
    // Open → save → reopen at the package layer; every part is byte-preserved.
    let pkg = OpcPackage::open(SAMPLE).unwrap();
    let mut buf = std::io::Cursor::new(Vec::new());
    pkg.write_to(&mut buf).unwrap();
    buf.set_position(0);
    let pkg2 = OpcPackage::from_reader(buf).unwrap();

    let mut names: Vec<_> = pkg.part_names().collect();
    names.sort();
    let mut names2: Vec<_> = pkg2.part_names().collect();
    names2.sort();
    assert_eq!(names, names2, "part set preserved across round-trip");

    for name in pkg.part_names() {
        assert_eq!(
            pkg.get_part(name),
            pkg2.get_part(name),
            "part {name} byte-preserved"
        );
    }
}

#[test]
fn high_level_open_save_is_faithful() {
    // An opened-but-unedited deck saves byte-identical — EVERY entry, including
    // all .rels and [Content_Types].xml (not just the parts).
    let p = Presentation::open(SAMPLE).unwrap();
    let orig = package_entries(&OpcPackage::open(SAMPLE).unwrap());
    let resaved = package_entries(&reopen(p.save_to_buffer().unwrap()));
    assert_eq!(orig, resaved, "unedited open->save is byte-identical for all entries");
}

#[test]
fn reading_does_not_break_round_trip() {
    let p = Presentation::open(SAMPLE).unwrap();
    let _ = p.slide(0).unwrap().text();
    let _ = p.to_markdown();
    let orig = package_entries(&OpcPackage::open(SAMPLE).unwrap());
    let resaved = package_entries(&reopen(p.save_to_buffer().unwrap()));
    assert_eq!(orig, resaved, "reads must not perturb any entry");
}

#[test]
fn editing_is_surgical_dom_based() {
    // Editing slide 0's title mutates only that slide's DOM. EVERY other entry —
    // master, 11 layouts, theme, other slides, all .rels, content-types — is
    // byte-identical. Within the edited slide, only the title run changes.
    let mut p = Presentation::open(SAMPLE).unwrap();
    p.slide_mut(0).unwrap().set_title("Edited Title").unwrap();
    let orig = package_entries(&OpcPackage::open(SAMPLE).unwrap());
    let out = package_entries(&reopen(p.save_to_buffer().unwrap()));

    // Exactly one entry differs: the edited slide body.
    let diffs: Vec<&String> = orig
        .keys()
        .filter(|k| orig.get(*k) != out.get(*k))
        .collect();
    assert_eq!(diffs, vec!["/ppt/slides/slide1.xml"], "only the edited slide changes");
    assert_eq!(orig.keys().collect::<Vec<_>>(), out.keys().collect::<Vec<_>>(), "no entries added/removed");

    // New title present; the subtitle shape on slide 1 is preserved verbatim.
    let s1 = String::from_utf8(out["/ppt/slides/slide1.xml"].clone()).unwrap();
    assert!(s1.contains("Edited Title"), "new title in edited slide");
    assert!(!s1.contains("Corpus Sample"), "old title gone");
    assert!(s1.contains("Authored by python-pptx"), "subtitle preserved verbatim");
}

#[test]
fn add_text_box_to_opened_slide_is_surgical() {
    // Adding a text box to slide 0 changes only that slide; the new box appears.
    let mut p = Presentation::open(SAMPLE).unwrap();
    p.slide_mut(0).unwrap().add_text_box(
        "Inserted box",
        zavora_slide::Emu::inches(1.0),
        zavora_slide::Emu::inches(1.0),
        zavora_slide::Emu::inches(3.0),
        zavora_slide::Emu::inches(1.0),
    );
    let orig = package_entries(&OpcPackage::open(SAMPLE).unwrap());
    let out = package_entries(&reopen(p.save_to_buffer().unwrap()));
    let diffs: Vec<&String> = orig.keys().filter(|k| orig.get(*k) != out.get(*k)).collect();
    assert_eq!(diffs, vec!["/ppt/slides/slide1.xml"], "only edited slide changes");
    let s1 = String::from_utf8(out["/ppt/slides/slide1.xml"].clone()).unwrap();
    assert!(s1.contains("<a:t>Inserted box</a:t>"), "new box present: {s1}");
    assert!(s1.contains("Corpus Sample"), "original title preserved");
}

#[test]
fn structural_edit_falls_back_to_rebuild() {
    // Adding a slide is a structural change → full rebuild from the model.
    let mut p = Presentation::open(SAMPLE).unwrap();
    p.add_slide(zavora_slide::Layout::Blank);
    let saved = p.save_to_buffer().unwrap();
    let out = OpcPackage::from_reader(std::io::Cursor::new(saved)).unwrap();
    let layouts = out.part_names().filter(|n| n.contains("/slideLayouts/slideLayout")).count();
    assert_eq!(layouts, 1, "structural edit rebuilds with the engine's single layout");
}
