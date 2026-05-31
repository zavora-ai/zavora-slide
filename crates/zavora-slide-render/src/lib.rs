//! Slide rasterization: [`Scene`] → SVG (primary, web-client friendly) and PNG
//! (via resvg). Text is laid out as simple top-anchored lines; full shaping is
//! delegated to resvg's font stack for PNG.

use base64::Engine;
use zavora_slide_layout::{Color, Item, Scene};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("PNG rasterization failed")]
    Raster,
    #[error("invalid SVG: {0}")]
    Svg(String),
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn rgb(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

/// Greedy word-wrap to a pixel width, estimating glyph advance at ~0.55em
/// (adequate for sans-serif without a full shaping pass). Always yields ≥1 line.
fn wrap(text: &str, font_px: f64, max_px: f64) -> Vec<String> {
    let char_w = font_px * 0.55;
    let max_chars = (max_px / char_w).floor().max(1.0) as usize;
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.chars().count() + 1 + word.chars().count() <= max_chars {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Render a scene to an SVG document string at the given pixel width.
pub fn scene_to_svg(scene: &Scene, target_px_w: u32) -> String {
    let h = scene.px_height(target_px_w);
    let sw = scene.width_emu;
    let mut s = String::new();
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{target_px_w}\" height=\"{h}\" \
         viewBox=\"0 0 {target_px_w} {h}\">"
    ));
    // Background.
    let bg = scene.background.unwrap_or(Color::WHITE);
    s.push_str(&format!("<rect width=\"{target_px_w}\" height=\"{h}\" fill=\"{}\"/>", rgb(bg)));

    let scale = target_px_w as f64 / sw as f64;
    // 1 point = 12700 EMU, and `scale` is pixels-per-EMU.
    let px_per_pt = zavora_slide_layout::EMU_PER_POINT * scale;
    for item in &scene.items {
        match item {
            Item::Rect { rect, fill, outline } => {
                let (x, y, w, hh) = rect.to_px(sw, target_px_w);
                let fill_attr = fill.map(rgb).unwrap_or_else(|| "none".into());
                let (stroke, sw_attr) = match outline {
                    Some((c, pt)) => (rgb(*c), (*pt * px_per_pt).max(0.5)),
                    None => ("none".into(), 0.0),
                };
                s.push_str(&format!(
                    "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{hh:.1}\" \
                     fill=\"{fill_attr}\" stroke=\"{stroke}\" stroke-width=\"{sw_attr:.2}\"/>"
                ));
            }
            Item::Text { rect, lines } => {
                let (x, y, w, _) = rect.to_px(sw, target_px_w);
                let mut cursor_y = y as f64;
                for ln in lines {
                    let px = ln.size_pt * px_per_pt; // pt→px at this scale
                    let weight = if ln.bold { " font-weight=\"bold\"" } else { "" };
                    let style = if ln.italic { " font-style=\"italic\"" } else { "" };
                    let indent = x as f64 + (ln.level as f64) * px * 1.5;
                    // A bullet marker for indented body lines (not titles at lvl 0).
                    let marker = if ln.level > 0 { "• " } else { "" };
                    let avail = (w as f64 - (indent - x as f64)).max(px * 4.0);
                    for (k, seg) in wrap(&format!("{marker}{}", ln.text), px, avail).into_iter().enumerate() {
                        cursor_y += px * 1.2;
                        // Hang-indent wrapped continuation lines under the text.
                        let lx = if k == 0 { indent } else { indent + px };
                        s.push_str(&format!(
                            "<text x=\"{lx:.1}\" y=\"{cursor_y:.1}\" font-family=\"sans-serif\" \
                             font-size=\"{px:.1}\" fill=\"{col}\"{weight}{style}>{t}</text>",
                            col = rgb(ln.color),
                            t = esc(&seg)
                        ));
                    }
                }
            }
            Item::Image { rect, data } => {
                let (x, y, w, hh) = rect.to_px(sw, target_px_w);
                let mime = if data.starts_with(&[0xFF, 0xD8]) { "jpeg" } else { "png" };
                let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                s.push_str(&format!(
                    "<image x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{hh:.1}\" \
                     href=\"data:image/{mime};base64,{b64}\"/>"
                ));
            }
        }
    }
    s.push_str("</svg>");
    s
}

/// Render a scene to PNG bytes at the given pixel width (via resvg).
pub fn scene_to_png(scene: &Scene, target_px_w: u32) -> Result<Vec<u8>, RenderError> {
    let svg = scene_to_svg(scene, target_px_w);
    let opt = build_options();
    let tree = resvg::usvg::Tree::from_str(&svg, &opt).map_err(|e| RenderError::Svg(e.to_string()))?;
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).ok_or(RenderError::Raster)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|_| RenderError::Raster)
}

/// usvg options with a font database. With `bundled-fonts`, LiberationSans
/// (Arial-metric-compatible) is embedded and set as the default sans-serif, so
/// text rasterizes even on hosts with no installed fonts. System fonts are also
/// loaded so named families resolve when available.
fn build_options() -> resvg::usvg::Options<'static> {
    let mut opt = resvg::usvg::Options::default();
    let db = opt.fontdb_mut();
    db.load_system_fonts();
    #[cfg(feature = "bundled-fonts")]
    {
        db.load_font_data(include_bytes!("../fonts/LiberationSans-Regular.ttf").to_vec());
        db.load_font_data(include_bytes!("../fonts/LiberationSans-Bold.ttf").to_vec());
        // Default sans-serif → the bundled face, so text rasterizes even with no
        // system fonts. (Named families still resolve from system fonts above.)
        db.set_sans_serif_family("Liberation Sans");
    }
    opt
}

#[cfg(test)]
mod tests {
    use super::*;
    use zavora_slide_layout::{Item, Rect, Scene, TextLine};

    fn sample() -> Scene {
        let mut s = Scene::new(12192000, 6858000);
        s.background = Some(Color { r: 245, g: 245, b: 245 });
        s.items.push(Item::Rect {
            rect: Rect { x: 914400, y: 914400, w: 1828800, h: 914400 },
            fill: Some(Color { r: 68, g: 114, b: 196 }),
            outline: None,
        });
        s.items.push(Item::Text {
            rect: Rect { x: 914400, y: 457200, w: 9000000, h: 914400 },
            lines: vec![TextLine { text: "Title <&>".into(), size_pt: 32.0, color: Color::BLACK, bold: true, italic: false, level: 0 }],
        });
        s
    }

    #[test]
    fn wrap_splits_and_never_empty() {
        let many = wrap("alpha beta gamma delta epsilon", 20.0, 80.0);
        assert!(many.len() > 1, "long text should wrap, got {many:?}");
        assert_eq!(wrap("solo", 20.0, 10.0), vec!["solo".to_string()]);
        assert_eq!(wrap("", 20.0, 100.0), vec![String::new()]);
    }

    #[test]
    fn svg_has_shapes_and_escaped_text() {
        let svg = scene_to_svg(&sample(), 1280);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("width=\"1280\" height=\"720\""));
        assert!(svg.contains("fill=\"#4472C4\""));
        assert!(svg.contains("Title &lt;&amp;&gt;"));
        assert!(svg.contains("font-weight=\"bold\""));
    }

    #[test]
    fn png_renders_nonempty() {
        let png = scene_to_png(&sample(), 640).unwrap();
        assert!(png.len() > 100);
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn png_actually_renders_dark_text_pixels() {
        // Decode the PNG and confirm it contains dark pixels (text/shape), i.e.
        // glyphs actually rasterized rather than a blank canvas.
        let png = scene_to_png(&sample(), 640).unwrap();
        let mut dec = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
        let dark = dec
            .pixels_mut()
            .iter()
            .filter(|p| p.red() < 100 && p.green() < 100 && p.blue() < 100 && p.alpha() > 0)
            .count();
        assert!(dark > 50, "expected rendered dark pixels, got {dark}");
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn bundled_font_is_loaded() {
        let mut opt = build_options();
        let q = resvg::usvg::fontdb::Query {
            families: &[resvg::usvg::fontdb::Family::Name("Liberation Sans")],
            ..Default::default()
        };
        assert!(opt.fontdb_mut().query(&q).is_some(), "bundled Liberation Sans should be queryable");
    }
}
