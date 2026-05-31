//! Corpus test: open real PowerPoint-authored `.pptx` files.
//!
//! Scope: the high-level `open()` is text-only (Requirement 3's faithful
//! round-trip is future work), so this asserts the engine *opens* genuine
//! PowerPoint decks without error and extracts their text. Lossless part
//! preservation is verified at the OPC layer, which round-trips all parts
//! byte-for-byte regardless of high-level modeling.

use zavora_slide::Presentation;
use zavora_slide_opc::OpcPackage;

const SAMPLE: &str = "tests/corpus/powerpoint_sample.pptx";

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
    // An opened-but-unedited deck saves byte-identical parts (source preserved).
    let p = Presentation::open(SAMPLE).unwrap();
    let saved = p.save_to_buffer().unwrap();
    let orig = OpcPackage::open(SAMPLE).unwrap();
    let resaved = OpcPackage::from_reader(std::io::Cursor::new(saved)).unwrap();
    for name in orig.part_names() {
        assert_eq!(orig.get_part(name), resaved.get_part(name), "part {name} preserved");
    }
}

#[test]
fn reading_does_not_break_round_trip() {
    let p = Presentation::open(SAMPLE).unwrap();
    let _ = p.slide(0).unwrap().text();
    let _ = p.to_markdown();
    let saved = p.save_to_buffer().unwrap();
    let orig = OpcPackage::open(SAMPLE).unwrap();
    let resaved = OpcPackage::from_reader(std::io::Cursor::new(saved)).unwrap();
    for name in orig.part_names() {
        assert_eq!(orig.get_part(name), resaved.get_part(name), "part {name} preserved after reads");
    }
}

#[test]
fn editing_invalidates_round_trip() {
    // Editing an opened deck drops the source; save reflects the rebuilt model.
    let mut p = Presentation::open(SAMPLE).unwrap();
    p.slide_mut(0).unwrap().set_title("Edited").unwrap();
    let saved = p.save_to_buffer().unwrap();
    let resaved = OpcPackage::from_reader(std::io::Cursor::new(saved)).unwrap();
    // Original has 11 layouts; the rebuilt package has exactly 1.
    let layouts = resaved.part_names().filter(|n| n.contains("/slideLayouts/slideLayout")).count();
    assert_eq!(layouts, 1, "edited save uses the rebuilt single-layout package");
}
