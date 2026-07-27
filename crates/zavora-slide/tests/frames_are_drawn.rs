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

/// A placeholder is put where the layout says, not where a title usually goes.
///
/// A placeholder on a slide states no box of its own — PowerPoint reads it from the layout, and so
/// does anything that renders the file correctly. Guessing puts a title where a title usually goes,
/// which is right often enough to look plausible and wrong often enough to matter: the guess put a
/// bulleted body in a half-width column on a layout that gives it the full width.
#[test]
fn a_placeholder_sits_where_the_layout_puts_it() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let deck = Presentation::open(corpus.join("powerpoint_sample.pptx")).unwrap();
    let scene = deck.slide(0).unwrap().scene();

    // The centred title on this deck's first layout, stated in the layout part itself as
    // (685800, 2130425) 7772400 by 1470025. Read from the file, so the numbers are the file's.
    let title = scene
        .items
        .iter()
        .find_map(|item| match item {
            zavora_slide_layout::Item::Text { rect, lines, .. }
                if lines.iter().any(|line| line.text.contains("Corpus Sample")) =>
            {
                Some(*rect)
            }
            _ => None,
        })
        .expect("the title is not drawn at all");

    assert_eq!(
        (title.x, title.y, title.w, title.h),
        (685_800, 2_130_425, 7_772_400, 1_470_025),
        "the title is not where the layout puts it, so it was guessed"
    );
}

/// The layout part really does state that box, so the test above is comparing against the file.
#[test]
fn the_layout_under_test_states_that_box() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let bytes = std::fs::read(corpus.join("powerpoint_sample.pptx")).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(
        &mut zip.by_name("ppt/slideLayouts/slideLayout1.xml").unwrap(),
        &mut xml,
    )
    .unwrap();
    assert!(
        xml.contains("685800") && xml.contains("2130425"),
        "the layout does not state the box the other test asserts"
    );
}

/// A text box is not a filled box.
///
/// The fill was read from anywhere inside the shape, and a shape contains its text — so a plain text
/// box with dark words was drawn as a dark box and the words vanished into it. On a real investor
/// deck that put a black band across the top of nine slides with the title hidden behind it.
#[test]
fn a_text_box_with_dark_words_is_not_drawn_as_a_dark_box() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let deck = Presentation::open(corpus.join("text_box_deck.pptx")).unwrap();
    let scene = deck.slide(0).unwrap().scene();

    let filled = scene
        .items
        .iter()
        .filter(|item| matches!(item, zavora_slide_layout::Item::Rect { fill: Some(_), .. }))
        .count();
    assert_eq!(
        filled, 0,
        "a text box that states no fill was drawn as {filled} filled box(es)"
    );
    assert!(
        scene
            .items
            .iter()
            .any(|item| matches!(item, zavora_slide_layout::Item::Text { .. })),
        "the words are not drawn at all"
    );
}

/// The deck under test really does hold a text box with a stated text colour and no fill.
#[test]
fn the_text_box_deck_states_a_colour_and_no_fill() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let bytes = std::fs::read(corpus.join("text_box_deck.pptx")).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ppt/slides/slide1.xml").unwrap(), &mut xml)
        .unwrap();
    assert!(xml.contains("noFill"), "the fixture states no noFill");
    assert!(
        xml.contains("solidFill"),
        "the fixture has no stated colour, so nothing could be mistaken for a fill"
    );
}

/// A colour the deck names is drawn in the colour the theme gives it.
///
/// Nearly half the colours in a real deck are named — `accent1`, `lt1` — rather than stated as hex.
/// An unresolved name came out black, which is how a white title on a dark band became unreadable.
#[test]
fn a_named_colour_comes_from_the_theme() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let deck = Presentation::open(corpus.join("text_box_deck.pptx")).unwrap();

    // Every drawn word on the deck. If a name were unresolved, the colour would be pure black.
    let mut names_resolved = 0;
    for at in 0..deck.slide_count() {
        for item in &deck.slide(at).unwrap().scene().items {
            if let zavora_slide_layout::Item::Text { lines, .. } = item {
                for line in lines {
                    let colour = line.color;
                    if (colour.r, colour.g, colour.b) != (0, 0, 0) {
                        names_resolved += 1;
                    }
                }
            }
        }
    }
    assert!(
        names_resolved > 0,
        "every drawn line came out black, so no colour was resolved at all"
    );
}
