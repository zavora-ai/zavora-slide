//! Corpus test: open real PowerPoint-authored `.pptx` files.
//!
//! Asserts the engine opens genuine PowerPoint decks, extracts their text, and
//! round-trips them faithfully: an unedited deck saves byte-identical; editing a
//! slide overlays only that slide while preserving every other part; structural
//! changes fall back to a full model rebuild.

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
fn editing_overlays_only_edited_slide() {
    // Editing slide 0 re-authors just that slide; master/layouts/theme and the
    // other slides stay byte-identical (overlay save, not a full rebuild).
    let mut p = Presentation::open(SAMPLE).unwrap();
    p.slide_mut(0).unwrap().set_title("Edited Title").unwrap();
    let saved = p.save_to_buffer().unwrap();
    let orig = OpcPackage::open(SAMPLE).unwrap();
    let out = OpcPackage::from_reader(std::io::Cursor::new(saved)).unwrap();

    // All 11 original layouts survive (no rebuild to a single layout).
    let layouts = out.part_names().filter(|n| n.contains("/slideLayouts/slideLayout")).count();
    assert_eq!(layouts, 11, "original layouts preserved");

    // Everything except the edited slide is byte-identical.
    for name in orig.part_names() {
        if name == "/ppt/slides/slide1.xml" {
            continue;
        }
        assert_eq!(orig.get_part(name), out.get_part(name), "part {name} preserved");
    }

    // The edited slide carries the new title and links to its original layout.
    let s1 = String::from_utf8(out.get_part("/ppt/slides/slide1.xml").unwrap().to_vec()).unwrap();
    assert!(s1.contains("Edited Title"), "edited slide has new title");
    assert!(
        out.get_part_rels("/ppt/slides/slide1.xml")
            .unwrap()
            .items
            .iter()
            .any(|r| r.target.contains("slideLayout")),
        "edited slide keeps a layout relationship"
    );
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
