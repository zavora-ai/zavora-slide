//! The scene a slide actually describes.
//!
//! Opening a presentation kept the slide's XML for editing and, for drawing, threw it away: every
//! run of text on the slide was gathered into one body placeholder and everything else — where the
//! shapes are, what colour they are, the pictures — was not read at all. A real deck therefore drew
//! as one block of text per slide, and a slide whose only content is a picture drew as nothing.
//!
//! This reads the slide's own shape tree instead: each shape where it sits, with the text it holds
//! and the fill it carries, and each picture. Anything it cannot make sense of is left out rather
//! than guessed at, which is the difference between a slide that is missing something and a slide
//! that is wrong.

use zavora_slide_layout::{Color, Item, ItemSource, Rect, Scene, TextFrameProps, TextLine};
use zavora_slide_oxml::xml::Element;

/// Where a placeholder sits when the slide does not say.
///
/// PowerPoint leaves the geometry off a placeholder and takes it from the layout the slide is based
/// on. Until the layout is read, these are the positions PowerPoint's own default layouts use, which
/// is what the great majority of decks are built from — so a title lands where a title goes rather
/// than being dropped for having no coordinates.
fn placeholder_rect(kind: &str, index: Option<u32>, width: i64, height: i64) -> Rect {
    // A sixteenth of the slide, which is the margin PowerPoint's layouts use.
    let margin = width / 16;
    match kind {
        "ctrTitle" => Rect {
            x: margin,
            y: height * 30 / 100,
            w: width - margin * 2,
            h: height * 22 / 100,
        },
        "title" => Rect {
            x: margin,
            y: height * 6 / 100,
            w: width - margin * 2,
            h: height * 18 / 100,
        },
        "subTitle" => Rect {
            x: margin,
            y: height * 55 / 100,
            w: width - margin * 2,
            h: height * 20 / 100,
        },
        "ftr" | "sldNum" | "dt" => Rect {
            x: margin,
            y: height * 92 / 100,
            w: width - margin * 2,
            h: height * 6 / 100,
        },
        // A body, and anything else with a place in the layout. Two of them side by side when the
        // deck numbers them, which is how a comparison slide is built.
        _ => match index {
            Some(1) => Rect {
                x: margin,
                y: height * 28 / 100,
                w: (width - margin * 3) / 2,
                h: height * 60 / 100,
            },
            Some(2) => Rect {
                x: margin * 2 + (width - margin * 3) / 2,
                y: height * 28 / 100,
                w: (width - margin * 3) / 2,
                h: height * 60 / 100,
            },
            _ => Rect {
                x: margin,
                y: height * 28 / 100,
                w: width - margin * 2,
                h: height * 62 / 100,
            },
        },
    }
}

/// A shape's own geometry, if it states one.
fn stated_rect(shape: &Element) -> Option<Rect> {
    let xfrm = shape.find_descendant(b"xfrm")?;
    let off = xfrm.children_named(b"off").next()?;
    let ext = xfrm.children_named(b"ext").next()?;
    let number = |value: Option<&[u8]>| -> Option<i64> {
        std::str::from_utf8(value?).ok()?.trim().parse::<i64>().ok()
    };
    Some(Rect {
        x: number(off.attr(b"x"))?,
        y: number(off.attr(b"y"))?,
        w: number(ext.attr(b"cx"))?,
        h: number(ext.attr(b"cy"))?,
    })
}

/// What kind of placeholder this shape is, if it is one.
fn placeholder_of(shape: &Element) -> Option<(String, Option<u32>)> {
    let ph = shape.find_descendant(b"ph")?;
    let kind = ph
        .attr(b"type")
        .and_then(|value| std::str::from_utf8(value).ok())
        .unwrap_or("body")
        .to_string();
    let index = ph
        .attr(b"idx")
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<u32>().ok());
    Some((kind, index))
}

/// A solid fill's colour, where the shape states one outright.
fn solid_fill(shape: &Element) -> Option<Color> {
    let fill = shape.find_descendant(b"solidFill")?;
    let value = fill.children_named(b"srgbClr").next()?.attr(b"val")?;
    Color::from_hex(std::str::from_utf8(value).ok()?)
}

/// The lines of text in a shape, with the size and weight each is set in.
fn lines_of(shape: &Element) -> Vec<TextLine> {
    let Some(body) = shape.find_descendant(b"txBody") else {
        return Vec::new();
    };
    body.children_named(b"p")
        .map(|paragraph| {
            let runs: Vec<&Element> = paragraph.children_named(b"r").collect();
            let text: String = runs
                .iter()
                .filter_map(|run| run.children_named(b"t").next())
                .map(|t| t.text_content())
                .collect();

            // The first run's properties stand for the line: a line set in two sizes is rare, and
            // drawing it in the first is closer than drawing it in none.
            let properties = runs.first().and_then(|run| run.children_named(b"rPr").next());
            let size_hundredths = properties
                .and_then(|rpr| rpr.attr(b"sz"))
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| value.trim().parse::<f64>().ok());
            let bold = properties
                .and_then(|rpr| rpr.attr(b"b"))
                .map(|value| value == b"1")
                .unwrap_or(false);
            let colour = properties.and_then(solid_fill);

            TextLine {
                text,
                // The size the run states, or the size a line of body text is set in when it
                // states none.
                size_pt: size_hundredths.map(|value| value / 100.0).unwrap_or(18.0),
                bold,
                color: colour.unwrap_or(Color::BLACK),
                ..TextLine::default()
            }
        })
        .filter(|line| !line.text.is_empty())
        .collect()
}

/// Build the scene a slide describes, from its own shape tree.
pub fn scene_from_dom(
    dom: &zavora_slide_oxml::SlideDom,
    width: i64,
    height: i64,
    images: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Scene {
    let mut scene = Scene::new(width, height);

    // Everything on the slide, in drawing order — pictures and tables included, which `shapes`
    // leaves out.
    for (index, shape) in dom.drawables().enumerate() {
        let name = shape.local_name();
        let source = Some(ItemSource::Shape(index));

        // Where it sits: what the slide says, or where the layout would put a placeholder.
        let rect = stated_rect(shape).or_else(|| {
            placeholder_of(shape)
                .map(|(kind, idx)| placeholder_rect(&kind, idx, width, height))
        });
        let Some(rect) = rect else { continue };

        match name {
            b"pic" => {
                // A picture. The bytes come from the package, by the relationship the shape names.
                let embed = shape
                    .find_descendant(b"blip")
                    // `r:embed`, qualified. Asking for "embed" matches nothing, and the picture
                    // was silently left off the slide.
                    .and_then(|blip| blip.attr(b"r:embed").or_else(|| blip.attr(b"embed")))
                    .and_then(|value| std::str::from_utf8(value).ok().map(str::to_string));
                if let Some(embed) = embed
                    && let Some(data) = images(&embed)
                {
                    scene.push(
                        Item::Image {
                            rect,
                            data,
                            crop: None,
                            rotation_deg: 0.0,
                        },
                        Some(ItemSource::Image(index)),
                    );
                }
            }
            _ => {
                // A filled or outlined body, where the shape has one. A placeholder usually does
                // not, and drawing a box behind every one of them would put grey rectangles over a
                // deck that has none.
                if let Some(fill) = solid_fill(shape) {
                    scene.push(
                        Item::Rect {
                            rect,
                            fill: Some(fill),
                            outline: None,
                        },
                        source,
                    );
                }

                let lines = lines_of(shape);
                if !lines.is_empty() {
                    let is_title = placeholder_of(shape)
                        .is_some_and(|(kind, _)| kind == "title" || kind == "ctrTitle");
                    scene.push(
                        Item::Text {
                            rect,
                            lines,
                            props: TextFrameProps {
                                // A title is centred in its box, as PowerPoint centres one; body
                                // text starts at the top.
                                anchor: if is_title {
                                    zavora_slide_layout::VerticalAnchor::Middle
                                } else {
                                    zavora_slide_layout::VerticalAnchor::Top
                                },
                                ..TextFrameProps::default()
                            },
                        },
                        source,
                    );
                }
            }
        }
    }

    scene
}
