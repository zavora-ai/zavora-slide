//! PresentationML scheme color enum and XML emitter.
//!
//! A `SchemeColor` represents a theme-color reference (`<a:schemeClr val="..."/>`)
//! that tracks theme changes rather than baking an RGB value. This is the
//! cross-cutting building block shared by text formatting, shape fills/lines,
//! and the render-side resolver.

use std::fmt;

/// All PresentationML scheme color values defined in the DrawingML color scheme.
///
/// These map 1:1 to the `val` attribute of `<a:schemeClr>` in ECMA-376.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemeColor {
    /// Dark 1 (typically mapped to window text / black).
    Dk1,
    /// Dark 2.
    Dk2,
    /// Light 1 (typically mapped to window / white).
    Lt1,
    /// Light 2.
    Lt2,
    /// Accent 1.
    Accent1,
    /// Accent 2.
    Accent2,
    /// Accent 3.
    Accent3,
    /// Accent 4.
    Accent4,
    /// Accent 5.
    Accent5,
    /// Accent 6.
    Accent6,
    /// Hyperlink color.
    Hlink,
    /// Followed hyperlink color.
    FolHlink,
}

impl SchemeColor {
    /// The `val` attribute string for `<a:schemeClr>`.
    pub fn val(self) -> &'static str {
        match self {
            Self::Dk1 => "dk1",
            Self::Dk2 => "dk2",
            Self::Lt1 => "lt1",
            Self::Lt2 => "lt2",
            Self::Accent1 => "accent1",
            Self::Accent2 => "accent2",
            Self::Accent3 => "accent3",
            Self::Accent4 => "accent4",
            Self::Accent5 => "accent5",
            Self::Accent6 => "accent6",
            Self::Hlink => "hlink",
            Self::FolHlink => "folHlink",
        }
    }

    /// Parse a scheme color from its `val` attribute string.
    ///
    /// Returns `None` if the string is not a recognized scheme color name.
    pub fn from_val(s: &str) -> Option<Self> {
        match s {
            "dk1" => Some(Self::Dk1),
            "dk2" => Some(Self::Dk2),
            "lt1" => Some(Self::Lt1),
            "lt2" => Some(Self::Lt2),
            "accent1" => Some(Self::Accent1),
            "accent2" => Some(Self::Accent2),
            "accent3" => Some(Self::Accent3),
            "accent4" => Some(Self::Accent4),
            "accent5" => Some(Self::Accent5),
            "accent6" => Some(Self::Accent6),
            "hlink" => Some(Self::Hlink),
            "folHlink" => Some(Self::FolHlink),
            _ => None,
        }
    }

    /// All scheme color variants in declaration order.
    pub const ALL: [SchemeColor; 12] = [
        Self::Dk1,
        Self::Dk2,
        Self::Lt1,
        Self::Lt2,
        Self::Accent1,
        Self::Accent2,
        Self::Accent3,
        Self::Accent4,
        Self::Accent5,
        Self::Accent6,
        Self::Hlink,
        Self::FolHlink,
    ];

    /// Emit the XML fragment `<a:schemeClr val="..."/>`.
    ///
    /// This is the self-closing form used when no child modifiers (tint, shade,
    /// lumMod, etc.) are present.
    pub fn to_xml(self) -> String {
        format!("<a:schemeClr val=\"{}\"/>", self.val())
    }

    /// Emit an opening `<a:schemeClr val="...">` tag (for use when child
    /// modifier elements will follow, closed by `</a:schemeClr>`).
    pub fn to_xml_open(self) -> String {
        format!("<a:schemeClr val=\"{}\">", self.val())
    }
}

impl fmt::Display for SchemeColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.val())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn val_round_trip() {
        for sc in SchemeColor::ALL {
            let val = sc.val();
            let parsed = SchemeColor::from_val(val).unwrap();
            assert_eq!(parsed, sc, "round-trip failed for {val}");
        }
    }

    #[test]
    fn from_val_unknown_returns_none() {
        assert_eq!(SchemeColor::from_val("unknown"), None);
        assert_eq!(SchemeColor::from_val(""), None);
        assert_eq!(SchemeColor::from_val("Accent1"), None); // case-sensitive
    }

    #[test]
    fn to_xml_self_closing() {
        assert_eq!(
            SchemeColor::Accent1.to_xml(),
            "<a:schemeClr val=\"accent1\"/>"
        );
        assert_eq!(
            SchemeColor::FolHlink.to_xml(),
            "<a:schemeClr val=\"folHlink\"/>"
        );
    }

    #[test]
    fn to_xml_open_tag() {
        assert_eq!(SchemeColor::Dk1.to_xml_open(), "<a:schemeClr val=\"dk1\">");
    }

    #[test]
    fn display_trait() {
        assert_eq!(format!("{}", SchemeColor::Lt2), "lt2");
        assert_eq!(format!("{}", SchemeColor::Hlink), "hlink");
    }

    #[test]
    fn all_has_twelve_variants() {
        assert_eq!(SchemeColor::ALL.len(), 12);
    }
}
