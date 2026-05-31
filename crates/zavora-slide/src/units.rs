//! Units and enumerations for the high-level API.

/// English Metric Units — the PresentationML coordinate unit (914400 per inch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Emu(pub i64);

impl Emu {
    pub const PER_INCH: i64 = 914400;
    pub const PER_POINT: i64 = 12700;
    pub const PER_CM: i64 = 360000;

    pub fn inches(v: f64) -> Emu {
        Emu((v * Self::PER_INCH as f64).round() as i64)
    }
    pub fn points(v: f64) -> Emu {
        Emu((v * Self::PER_POINT as f64).round() as i64)
    }
    pub fn cm(v: f64) -> Emu {
        Emu((v * Self::PER_CM as f64).round() as i64)
    }
}

/// Built-in slide layouts (placeholder arrangements resolved in later phases).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Title,
    TitleContent,
    SectionHeader,
    TwoContent,
    Blank,
}

impl Layout {
    /// Parse a layout name (case-insensitive, snake/camel tolerant).
    pub fn parse(s: &str) -> Option<Layout> {
        match s.to_ascii_lowercase().replace(['_', '-', ' '], "").as_str() {
            "title" => Some(Layout::Title),
            "titlecontent" | "content" => Some(Layout::TitleContent),
            "sectionheader" | "section" => Some(Layout::SectionHeader),
            "twocontent" | "two" => Some(Layout::TwoContent),
            "blank" => Some(Layout::Blank),
            _ => None,
        }
    }
}

/// Deck slide-size presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideSize {
    /// 16:9 widescreen (default).
    Widescreen,
    /// 4:3 standard.
    Standard,
    /// 16:10.
    Wide16x10,
}

impl SlideSize {
    /// (cx, cy, type) in EMU.
    pub fn dims(self) -> (i64, i64, &'static str) {
        match self {
            SlideSize::Widescreen => (12192000, 6858000, "screen16x9"),
            SlideSize::Standard => (9144000, 6858000, "screen4x3"),
            SlideSize::Wide16x10 => (12192000, 7620000, "screen16x10"),
        }
    }
}

/// Output format for slide rasterization (Phase 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderFormat {
    Png,
    Svg,
}

/// DrawingML auto-shape preset geometries (Phase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapePreset {
    Rect,
    RoundRect,
    Ellipse,
    Triangle,
    Arrow,
    Line,
    Callout,
}

impl ShapePreset {
    /// PresentationML `prstGeom@prst` name.
    pub fn prst(self) -> &'static str {
        match self {
            ShapePreset::Rect => "rect",
            ShapePreset::RoundRect => "roundRect",
            ShapePreset::Ellipse => "ellipse",
            ShapePreset::Triangle => "triangle",
            ShapePreset::Arrow => "rightArrow",
            ShapePreset::Line => "line",
            ShapePreset::Callout => "wedgeRectCallout",
        }
    }

    /// Parse a preset name (case-insensitive).
    pub fn parse(s: &str) -> Option<ShapePreset> {
        match s.to_ascii_lowercase().replace(['_', '-', ' '], "").as_str() {
            "rect" | "rectangle" => Some(ShapePreset::Rect),
            "roundrect" | "roundedrectangle" => Some(ShapePreset::RoundRect),
            "ellipse" | "oval" | "circle" => Some(ShapePreset::Ellipse),
            "triangle" => Some(ShapePreset::Triangle),
            "arrow" => Some(ShapePreset::Arrow),
            "line" => Some(ShapePreset::Line),
            "callout" => Some(ShapePreset::Callout),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emu_conversions() {
        assert_eq!(Emu::inches(1.0), Emu(914400));
        assert_eq!(Emu::points(1.0), Emu(12700));
        assert_eq!(Emu::cm(1.0), Emu(360000));
    }

    #[test]
    fn layout_parse() {
        assert_eq!(Layout::parse("Title Content"), Some(Layout::TitleContent));
        assert_eq!(Layout::parse("blank"), Some(Layout::Blank));
        assert_eq!(Layout::parse("nope"), None);
    }
}
