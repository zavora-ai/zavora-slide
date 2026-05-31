//! Layout engine: a neutral scene IR plus EMU↔pixel math.
//!
//! The high-level crate converts a slide's content into a [`Scene`] of positioned
//! primitives; the render/pdf crates consume a `Scene`. Keeping the IR free of
//! OOXML/high-level types avoids a dependency cycle (render → zavora-slide).
//!
//! Coordinates are EMU (914400 per inch). [`Rect::to_px`] maps to pixels at a
//! target width, preserving the slide's aspect ratio.

pub const EMU_PER_INCH: i64 = 914400;
pub const EMU_PER_POINT: f64 = 12700.0;

/// An sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    pub const WHITE: Color = Color { r: 255, g: 255, b: 255 };

    /// Parse a hex color ("#RRGGBB" or "RRGGBB"). Returns `None` if malformed.
    pub fn from_hex(s: &str) -> Option<Color> {
        let h = s.trim_start_matches('#');
        if h.len() != 6 {
            return None;
        }
        Some(Color {
            r: u8::from_str_radix(&h[0..2], 16).ok()?,
            g: u8::from_str_radix(&h[2..4], 16).ok()?,
            b: u8::from_str_radix(&h[4..6], 16).ok()?,
        })
    }
}

/// A rectangle in EMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    /// Map this rect to pixels given the slide width in EMU and a target pixel
    /// width. Scale is uniform (height follows the slide aspect ratio).
    pub fn to_px(&self, slide_w_emu: i64, target_px_w: u32) -> (f32, f32, f32, f32) {
        let s = target_px_w as f64 / slide_w_emu as f64;
        (
            (self.x as f64 * s) as f32,
            (self.y as f64 * s) as f32,
            (self.w as f64 * s) as f32,
            (self.h as f64 * s) as f32,
        )
    }
}

/// One line of text within a text frame.
#[derive(Debug, Clone)]
pub struct TextLine {
    pub text: String,
    /// Font size in points.
    pub size_pt: f64,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    /// Indent level (for bullets); 0 = top.
    pub level: u8,
}

/// A positioned primitive in a slide scene.
#[derive(Debug, Clone)]
pub enum Item {
    /// Filled/outlined rectangle (or auto-shape bounding box).
    Rect {
        rect: Rect,
        fill: Option<Color>,
        outline: Option<(Color, f64)>,
    },
    /// A text frame containing stacked lines.
    Text { rect: Rect, lines: Vec<TextLine> },
    /// An embedded raster image (encoded bytes, e.g. PNG/JPEG).
    Image { rect: Rect, data: Vec<u8> },
}

/// A renderable slide: its EMU dimensions, background, and items in z-order.
#[derive(Debug, Clone)]
pub struct Scene {
    pub width_emu: i64,
    pub height_emu: i64,
    pub background: Option<Color>,
    pub items: Vec<Item>,
}

impl Scene {
    pub fn new(width_emu: i64, height_emu: i64) -> Self {
        Self { width_emu, height_emu, background: None, items: Vec::new() }
    }

    /// Target pixel height for a given target width, preserving aspect ratio.
    pub fn px_height(&self, target_px_w: u32) -> u32 {
        ((target_px_w as f64) * (self.height_emu as f64) / (self.width_emu as f64)).round() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(Color::from_hex("#FF0000"), Some(Color { r: 255, g: 0, b: 0 }));
        assert_eq!(Color::from_hex("00ff00"), Some(Color { r: 0, g: 255, b: 0 }));
        assert_eq!(Color::from_hex("xyz"), None);
    }

    #[test]
    fn emu_to_px_uniform_scale() {
        // 16:9 slide, 1280px wide → 720px tall.
        let scene = Scene::new(12192000, 6858000);
        assert_eq!(scene.px_height(1280), 720);
        // A 1-inch square at the origin → 1280/13.333.. ≈ 96px.
        let r = Rect { x: 0, y: 0, w: EMU_PER_INCH, h: EMU_PER_INCH };
        let (_, _, w, h) = r.to_px(12192000, 1280);
        assert!((w - 96.0).abs() < 0.5);
        assert!((h - 96.0).abs() < 0.5);
    }
}
