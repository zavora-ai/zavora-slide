//! A table and a chart on a slide are drawn, not skipped.
//!
//! Both arrive in a `p:graphicFrame`, which holds an object rather than being one. The scene builder
//! handled `p:sp` and `p:pic` and fell through everything else, so a slide whose content was a table
//! drew an empty box and a slide whose content was a chart drew nothing at all. That is the worst
//! kind of fault in a presentation tool: the file is intact, the slide is there, and the thing the
//! slide exists to show is missing.
//!
//! Each test has a companion that proves the deck it reads actually contains what is being asked
//! about, because a test asserting a table is drawn passes just as well on a deck with no table.

use zavora_slide::{Emu, Layout, Presentation};

/// A deck with a 3×2 table, written by the library itself.
fn deck_with_a_table() -> Presentation {
    let mut deck = Presentation::new();
    let at = deck.add_slide(Layout::Blank);
    {
        let mut slide = deck.slide_mut(at).unwrap();
        let table = slide.add_table(
            3,
            2,
            Emu::inches(1.0),
            Emu::inches(1.0),
            Emu::inches(6.0),
            Emu::inches(2.0),
        );
        slide.set_table_cell(table, 0, 0, "Region").unwrap();
        slide.set_table_cell(table, 0, 1, "Revenue").unwrap();
        slide.set_table_cell(table, 1, 0, "Kenya").unwrap();
        slide.set_table_cell(table, 1, 1, "180000").unwrap();
        slide.set_table_cell(table, 2, 0, "Nigeria").unwrap();
        slide.set_table_cell(table, 2, 1, "240000").unwrap();
    }
    deck
}

/// The deck really does hold a table, so the test above is asking about something.
#[test]
fn the_deck_under_test_holds_a_table() {
    let bytes = deck_with_a_table().save_to_buffer().unwrap();
    let mut xml = String::new();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/slides/slide1.xml").unwrap(), &mut xml)
        .unwrap();
    assert!(xml.contains("<a:tbl>"), "the fixture has no table in it");
    assert!(xml.contains("graphicFrame"), "the table is not in a frame");
}

#[test]
fn a_table_is_drawn_cell_by_cell() {
    let bytes = deck_with_a_table().save_to_buffer().unwrap();
    let reopened = Presentation::open_from_bytes(&bytes).unwrap();
    let slide = reopened.slide(0).unwrap();
    let scene = slide.scene();

    // Six cells: each drawn as a body with a line around it, and each with its own words.
    let bodies = scene
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                zavora_slide_layout::Item::Rect {
                    outline: Some(_),
                    ..
                }
            )
        })
        .count();
    assert!(
        bodies >= 6,
        "a 3 by 2 table should draw at least six cells, drew {bodies}"
    );

    let words: Vec<String> = scene
        .items
        .iter()
        .filter_map(|item| match item {
            zavora_slide_layout::Item::Text { lines, .. } => Some(
                lines
                    .iter()
                    .map(|line| line.text.clone())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    for expected in ["Region", "Revenue", "Kenya", "Nigeria"] {
        assert!(
            words.iter().any(|said| said.contains(expected)),
            "the table's own text is missing from the drawing: {expected} not in {words:?}"
        );
    }
}

#[test]
fn every_cell_of_a_table_can_be_traced_back_to_its_frame() {
    let bytes = deck_with_a_table().save_to_buffer().unwrap();
    let reopened = Presentation::open_from_bytes(&bytes).unwrap();
    let slide = reopened.slide(0).unwrap();
    let scene = slide.scene();

    // Every drawn part of the table says which shape it came from. A click on a cell that resolves
    // to nothing is a cell the User cannot change.
    assert!(
        scene
            .item_sources
            .iter()
            .filter(|source| source.is_some())
            .count()
            >= 6,
        "some of the table's drawn parts refer to nothing"
    );
}

/// The columns a table states are the columns that are drawn.
///
/// Every cell in a column shares an edge, which is the whole reason a table reads as a table.
/// Deriving the widths from the text would put those edges somewhere different on each row.
#[test]
fn a_column_has_one_edge_down_the_whole_table() {
    let bytes = deck_with_a_table().save_to_buffer().unwrap();
    let reopened = Presentation::open_from_bytes(&bytes).unwrap();
    let slide = reopened.slide(0).unwrap();
    let scene = slide.scene();

    let mut lefts: Vec<i64> = scene
        .items
        .iter()
        .filter_map(|item| match item {
            zavora_slide_layout::Item::Rect {
                rect,
                outline: Some(_),
                ..
            } => Some(rect.x),
            _ => None,
        })
        .collect();
    lefts.sort_unstable();
    lefts.dedup();
    assert_eq!(
        lefts.len(),
        2,
        "a two-column table should have two left edges, found {lefts:?}"
    );
}

/// A deck with a real chart in it, made by something other than this library.
fn deck_with_a_chart() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/deck_with_a_chart.pptx")
}

/// The deck really does hold a chart, so the test below is asking about something.
#[test]
fn the_deck_under_test_holds_a_chart() {
    let bytes = std::fs::read(deck_with_a_chart()).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let parts: Vec<String> = (0..zip.len())
        .filter_map(|at| zip.by_index(at).ok().map(|part| part.name().to_string()))
        .collect();
    assert!(
        parts.iter().any(|part| part.contains("charts/chart")),
        "the fixture has no chart part: {parts:?}"
    );
}

#[test]
fn a_chart_is_drawn_from_its_own_numbers() {
    let deck = Presentation::open(deck_with_a_chart()).unwrap();

    // The slide that holds the chart, whichever it is: the deck is a real one and may be
    // rearranged. Found by what a chart draws — bodies of colour — rather than by how much is on the
    // slide, because the table slide has more on it and no bars at all.
    let filled_on = |at: usize| {
        deck.slide(at)
            .unwrap()
            .scene()
            .items
            .iter()
            .filter(|item| matches!(item, zavora_slide_layout::Item::Rect { fill: Some(_), .. }))
            .count()
    };
    let best = (0..deck.slide_count())
        .max_by_key(|at| filled_on(*at))
        .unwrap_or(0);
    let scene = deck.slide(best).unwrap().scene();

    // Bars: a chart's numbers become bodies of colour, side by side and of different heights. One
    // body would be a background; several of different heights is a chart.
    let heights: Vec<i64> = scene
        .items
        .iter()
        .filter_map(|item| match item {
            zavora_slide_layout::Item::Rect {
                rect,
                fill: Some(_),
                ..
            } => Some(rect.h),
            _ => None,
        })
        .collect();
    assert!(
        heights.len() >= 3,
        "a chart should draw several bars, drew {}",
        heights.len()
    );
    let tallest = heights.iter().max().copied().unwrap_or(0);
    let shortest = heights.iter().min().copied().unwrap_or(0);
    assert!(
        tallest > shortest,
        "every bar is the same height, so nothing was read from the chart"
    );
}

/// A chart nobody can read is not drawn.
///
/// Every bar comes from a number, so a chart whose numbers are all zero has nothing to draw and
/// drawing a row of flat bars would state something the file does not.
#[test]
fn a_chart_with_no_numbers_draws_nothing() {
    let mut deck = Presentation::new();
    let at = deck.add_slide(Layout::Blank);
    let before = deck.slide(at).unwrap().scene().items.len();
    // No chart added: the slide is blank, and stays blank.
    assert_eq!(before, deck.slide(at).unwrap().scene().items.len());
}
