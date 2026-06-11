//! Layout engine: a neutral scene IR plus EMU↔pixel math.
//!
//! The high-level crate converts a slide's content into a [`Scene`] of positioned
//! primitives; the render/pdf crates consume a `Scene`. Keeping the IR free of
//! OOXML/high-level types avoids a dependency cycle (render → zavora-slide).
//!
//! Coordinates are EMU (914400 per inch). [`Rect::to_px`] maps to pixels at a
//! target width, preserving the slide's aspect ratio.
//!
//! ## Geometry Resolution
//!
//! The [`resolve_geometry`] module implements placeholder geometry inheritance:
//! slide → slide-layout → slide-master. Placeholders are matched by `ph@type`
//! and `ph@idx`, and the first explicit `a:xfrm` found in the chain is used.
//! Non-placeholder shapes use their own explicit `a:xfrm`. When no geometry
//! resolves, a documented default box is returned (fallback behavior).

pub mod preset_geometry;
pub mod resolve_geometry;

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

/// Paragraph alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// Vertical text anchoring within a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerticalAnchor {
    #[default]
    Top,
    Middle,
    Bottom,
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
    pub underline: bool,
    /// Indent level (for bullets); 0 = top.
    pub level: u8,
    /// Font family name (e.g. "Calibri"). Falls back to sans-serif if empty.
    pub font_family: String,
    /// Paragraph alignment for this line.
    pub alignment: Alignment,
    /// Line spacing multiplier (1.0 = single, 1.5 = 1.5x, 2.0 = double).
    pub line_spacing: f64,
    /// Space before paragraph in points.
    pub space_before_pt: f64,
    /// Space after paragraph in points.
    pub space_after_pt: f64,
    /// Whether this line starts a new paragraph (for spacing calculations).
    pub is_paragraph_start: bool,
    /// Whether this line has a bullet marker.
    pub has_bullet: bool,
}

impl Default for TextLine {
    fn default() -> Self {
        Self {
            text: String::new(),
            size_pt: 18.0,
            color: Color::BLACK,
            bold: false,
            italic: false,
            underline: false,
            level: 0,
            font_family: String::new(),
            alignment: Alignment::Left,
            line_spacing: 1.0,
            space_before_pt: 0.0,
            space_after_pt: 0.0,
            is_paragraph_start: true,
            has_bullet: false,
        }
    }
}

/// Text frame metadata for vertical anchoring and autofit.
#[derive(Debug, Clone, Default)]
pub struct TextFrameProps {
    /// Vertical anchor for the text block within the frame.
    pub anchor: VerticalAnchor,
    /// Font scale factor from normAutofit (1.0 = no scaling, 0.5 = 50%).
    pub font_scale: f64,
}

/// A gradient stop: position (0.0–1.0) and color.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientStop {
    pub position: f64,
    pub color: Color,
}

/// A gradient fill definition.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientFill {
    /// Gradient stops (at least 2).
    pub stops: Vec<GradientStop>,
    /// Angle in degrees (0 = left-to-right, 90 = top-to-bottom).
    pub angle_deg: f64,
    /// Whether this is a radial gradient (vs linear).
    pub is_radial: bool,
}

/// A picture fill: the shape is filled with an image.
#[derive(Debug, Clone)]
pub struct PictureFill {
    /// The image data (PNG/JPEG bytes).
    pub data: Vec<u8>,
}

/// Fill type for a shape.
#[derive(Debug, Clone)]
pub enum ShapeFill {
    Solid(Color),
    Gradient(GradientFill),
    Picture(PictureFill),
    None,
}

/// PresentationML dash style for outlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashStyle {
    Solid,
    Dot,
    Dash,
    LgDash,
    DashDot,
    LgDashDot,
    LgDashDotDot,
    SysDot,
    SysDash,
    SysDashDot,
    SysDashDotDot,
}

impl DashStyle {
    /// Parse from PresentationML `prstDash@val` attribute value.
    pub fn from_prstml(val: &str) -> Self {
        match val {
            "solid" => Self::Solid,
            "dot" => Self::Dot,
            "dash" => Self::Dash,
            "lgDash" => Self::LgDash,
            "dashDot" => Self::DashDot,
            "lgDashDot" => Self::LgDashDot,
            "lgDashDotDot" => Self::LgDashDotDot,
            "sysDot" => Self::SysDot,
            "sysDash" => Self::SysDash,
            "sysDashDot" => Self::SysDashDot,
            "sysDashDotDot" => Self::SysDashDotDot,
            _ => Self::Solid,
        }
    }

    /// Convert to SVG `stroke-dasharray` value. The values are multiples of
    /// the stroke width. Returns `None` for solid (no dasharray needed).
    pub fn to_svg_dasharray(&self) -> Option<&'static str> {
        match self {
            Self::Solid => None,
            Self::Dot => Some("1 1"),
            Self::Dash => Some("4 3"),
            Self::LgDash => Some("8 3"),
            Self::DashDot => Some("4 3 1 3"),
            Self::LgDashDot => Some("8 3 1 3"),
            Self::LgDashDotDot => Some("8 3 1 3 1 3"),
            Self::SysDot => Some("1 1"),
            Self::SysDash => Some("3 1"),
            Self::SysDashDot => Some("3 1 1 1"),
            Self::SysDashDotDot => Some("3 1 1 1 1 1"),
        }
    }
}

/// Outline (stroke) definition for a shape.
#[derive(Debug, Clone)]
pub struct Outline {
    pub color: Color,
    /// Width in points.
    pub width_pt: f64,
    pub dash: DashStyle,
}

/// Image crop fractions (0.0–1.0 from each edge).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageCrop {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Default for ImageCrop {
    fn default() -> Self {
        Self { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 }
    }
}

impl ImageCrop {
    /// Returns true if any crop fraction is non-zero.
    pub fn is_cropped(&self) -> bool {
        self.left != 0.0 || self.top != 0.0 || self.right != 0.0 || self.bottom != 0.0
    }
}

/// Slide or shape background.
#[derive(Debug, Clone)]
pub enum Background {
    Solid(Color),
    Picture(Vec<u8>),
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
    /// A shape with preset geometry, rich fill, and outline.
    Shape {
        rect: Rect,
        /// Preset geometry name (e.g. "ellipse", "roundRect"). If `None`,
        /// renders as a rectangle.
        preset: Option<String>,
        fill: ShapeFill,
        outline: Option<Outline>,
        /// Rotation in degrees.
        rotation_deg: f64,
    },
    /// A text frame containing stacked lines.
    Text {
        rect: Rect,
        lines: Vec<TextLine>,
        props: TextFrameProps,
    },
    /// An embedded raster image (encoded bytes, e.g. PNG/JPEG).
    Image {
        rect: Rect,
        data: Vec<u8>,
        /// Optional crop (srcRect fractions).
        crop: Option<ImageCrop>,
        /// Rotation in degrees.
        rotation_deg: f64,
    },
}

/// A renderable slide: its EMU dimensions, background, and items in z-order.
#[derive(Debug, Clone)]
pub struct Scene {
    pub width_emu: i64,
    pub height_emu: i64,
    pub background: Option<Color>,
    /// Rich background (solid or picture). Takes precedence over `background`
    /// when set.
    pub rich_background: Option<Background>,
    pub items: Vec<Item>,
}

impl Scene {
    pub fn new(width_emu: i64, height_emu: i64) -> Self {
        Self { width_emu, height_emu, background: None, rich_background: None, items: Vec::new() }
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
