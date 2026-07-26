//! Preset geometry: mapping from PresentationML `prstGeom@prst` names to SVG
//! path commands. Common presets get real geometry; uncommon ones fall back to a
//! bounding-box rectangle.
//!
//! All path commands are expressed relative to a unit square (0,0)→(1,1) and
//! must be scaled to the shape's actual bounding box at render time.

/// Returns SVG path data for a preset geometry name, scaled to the given
/// bounding box (x, y, width, height). Returns `None` if the preset is unknown
/// (caller should fall back to a rectangle).
pub fn preset_path(name: &str, x: f64, y: f64, w: f64, h: f64) -> Option<String> {
    // Look up the unit-square path template
    let unit_path = unit_path_for(name)?;
    Some(scale_path(unit_path, x, y, w, h))
}

/// Returns the SVG path data for a bounding-box rectangle (fallback for
/// unknown presets).
pub fn bbox_rect_path(x: f64, y: f64, w: f64, h: f64) -> String {
    format!(
        "M{x},{y} L{},{y} L{},{} L{x},{} Z",
        x + w,
        x + w,
        y + h,
        y + h
    )
}

/// Unit-square path commands for common presets. Coordinates are in [0,1]×[0,1].
/// These are hand-authored approximations of the OOXML preset geometry guides.
fn unit_path_for(name: &str) -> Option<&'static str> {
    let path = match name {
        "rect" => "M0,0 L1,0 L1,1 L0,1 Z",
        "roundRect" => {
            // Corner radius ~16.67% of the shorter side
            "M0.167,0 L0.833,0 Q1,0 1,0.167 L1,0.833 Q1,1 0.833,1 L0.167,1 Q0,1 0,0.833 L0,0.167 Q0,0 0.167,0 Z"
        }
        "ellipse" => {
            // Approximated with cubic Béziers (4-arc approach)
            "M0.5,0 C0.776,0 1,0.224 1,0.5 C1,0.776 0.776,1 0.5,1 C0.224,1 0,0.776 0,0.5 C0,0.224 0.224,0 0.5,0 Z"
        }
        "triangle" | "isosTriangle" => "M0.5,0 L1,1 L0,1 Z",
        "rtTriangle" => "M0,0 L1,1 L0,1 Z",
        "diamond" => "M0.5,0 L1,0.5 L0.5,1 L0,0.5 Z",
        "pentagon" => "M0.5,0 L0.975,0.345 L0.794,0.905 L0.206,0.905 L0.025,0.345 Z",
        "hexagon" => "M0.25,0 L0.75,0 L1,0.5 L0.75,1 L0.25,1 L0,0.5 Z",
        "heptagon" => "M0.5,0 L0.81,0.19 L0.97,0.61 L0.79,0.95 L0.21,0.95 L0.03,0.61 L0.19,0.19 Z",
        "octagon" => "M0.293,0 L0.707,0 L1,0.293 L1,0.707 L0.707,1 L0.293,1 L0,0.707 L0,0.293 Z",
        "star4" => {
            "M0.5,0 L0.625,0.375 L1,0.5 L0.625,0.625 L0.5,1 L0.375,0.625 L0,0.5 L0.375,0.375 Z"
        }
        "star5" => {
            "M0.5,0 L0.612,0.345 L0.976,0.345 L0.682,0.559 L0.794,0.905 L0.5,0.691 L0.206,0.905 L0.318,0.559 L0.024,0.345 L0.388,0.345 Z"
        }
        "star6" => {
            "M0.5,0 L0.625,0.25 L0.933,0.25 L0.75,0.5 L0.933,0.75 L0.625,0.75 L0.5,1 L0.375,0.75 L0.067,0.75 L0.25,0.5 L0.067,0.25 L0.375,0.25 Z"
        }
        "arrow" | "rightArrow" => "M0,0.25 L0.6,0.25 L0.6,0 L1,0.5 L0.6,1 L0.6,0.75 L0,0.75 Z",
        "leftArrow" => "M0,0.5 L0.4,0 L0.4,0.25 L1,0.25 L1,0.75 L0.4,0.75 L0.4,1 Z",
        "upArrow" => "M0.5,0 L1,0.4 L0.75,0.4 L0.75,1 L0.25,1 L0.25,0.4 L0,0.4 Z",
        "downArrow" => "M0.25,0 L0.75,0 L0.75,0.6 L1,0.6 L0.5,1 L0,0.6 L0.25,0.6 Z",
        "heart" => {
            "M0.5,0.2 C0.5,0 0.75,-0.1 0.9,0.1 C1.05,0.3 1,0.5 0.5,1 C0,0.5 -0.05,0.3 0.1,0.1 C0.25,-0.1 0.5,0 0.5,0.2 Z"
        }
        "parallelogram" => "M0.25,0 L1,0 L0.75,1 L0,1 Z",
        "trapezoid" => "M0.2,0 L0.8,0 L1,1 L0,1 Z",
        "chevron" => "M0,0 L0.75,0 L1,0.5 L0.75,1 L0,1 L0.25,0.5 Z",
        "homePlate" => "M0,0 L0.8,0 L1,0.5 L0.8,1 L0,1 Z",
        "plus" | "cross" => {
            "M0.333,0 L0.667,0 L0.667,0.333 L1,0.333 L1,0.667 L0.667,0.667 L0.667,1 L0.333,1 L0.333,0.667 L0,0.667 L0,0.333 L0.333,0.333 Z"
        }
        "cloud" => {
            // Simplified cloud shape
            "M0.2,0.6 C0,0.6 0,0.4 0.15,0.35 C0.1,0.2 0.3,0.1 0.45,0.2 C0.5,0.05 0.75,0.05 0.8,0.2 C0.95,0.15 1.05,0.35 0.9,0.45 C1.05,0.55 1,0.75 0.85,0.75 L0.2,0.75 C0.05,0.75 0,0.65 0.2,0.6 Z"
        }
        "ribbon" | "ribbon2" => {
            "M0,0.2 L0.15,0.35 L0.15,0 L0.85,0 L0.85,0.35 L1,0.2 L1,0.8 L0.85,0.65 L0.85,1 L0.15,1 L0.15,0.65 L0,0.8 Z"
        }
        "donut" => {
            // Outer circle + inner circle (hole)
            "M0.5,0 C0.776,0 1,0.224 1,0.5 C1,0.776 0.776,1 0.5,1 C0.224,1 0,0.776 0,0.5 C0,0.224 0.224,0 0.5,0 Z M0.5,0.3 C0.39,0.3 0.3,0.39 0.3,0.5 C0.3,0.61 0.39,0.7 0.5,0.7 C0.61,0.7 0.7,0.61 0.7,0.5 C0.7,0.39 0.61,0.3 0.5,0.3 Z"
        }
        "flowChartProcess" => "M0,0 L1,0 L1,1 L0,1 Z",
        "flowChartDecision" => "M0.5,0 L1,0.5 L0.5,1 L0,0.5 Z",
        "flowChartTerminator" => "M0.2,0 L0.8,0 C1,0 1,1 0.8,1 L0.2,1 C0,1 0,0 0.2,0 Z",
        "callout1" | "wedgeRectCallout" => "M0,0 L1,0 L1,0.75 L0.6,0.75 L0.5,1 L0.4,0.75 L0,0.75 Z",
        "roundedRect" | "snip1Rect" | "snip2SameRect" => {
            // Alias for roundRect
            "M0.167,0 L0.833,0 Q1,0 1,0.167 L1,0.833 Q1,1 0.833,1 L0.167,1 Q0,1 0,0.833 L0,0.167 Q0,0 0.167,0 Z"
        }
        _ => return None,
    };
    Some(path)
}

/// Scale a unit-square path to the given bounding box.
/// Replaces coordinate values in the path string.
fn scale_path(unit_path: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let mut result = String::with_capacity(unit_path.len() * 2);
    let mut chars = unit_path.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            'M' | 'L' | 'Q' | 'C' | 'Z' => {
                result.push(c);
                if c == 'Z' {
                    continue;
                }
                // Parse coordinate pairs
                parse_and_scale_coords(&mut chars, &mut result, x, y, w, h, c);
            }
            ' ' => result.push(' '),
            _ => result.push(c),
        }
    }
    result
}

/// Parse coordinates after a path command and scale them.
fn parse_and_scale_coords(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    result: &mut String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    cmd: char,
) {
    // Number of coordinate pairs depends on command
    let pair_count = match cmd {
        'M' | 'L' => 1,
        'Q' => 2,
        'C' => 3,
        _ => 0,
    };

    for i in 0..pair_count {
        if i > 0 {
            result.push(' ');
        }
        // Parse x coordinate
        let ux = parse_number(chars);
        // Skip comma
        skip_comma(chars);
        // Parse y coordinate
        let uy = parse_number(chars);

        let sx = x + ux * w;
        let sy = y + uy * h;
        result.push_str(&format!("{sx:.1},{sy:.1}"));

        // Skip trailing space before next pair
        skip_spaces(chars);
    }
}

fn parse_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> f64 {
    skip_spaces(chars);
    let mut s = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' || c == '-' {
            s.push(c);
            chars.next();
        } else {
            break;
        }
    }
    s.parse::<f64>().unwrap_or(0.0)
}

fn skip_comma(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while let Some(&c) = chars.peek() {
        if c == ',' {
            chars.next();
            return;
        } else if c == ' ' {
            chars.next();
        } else {
            return;
        }
    }
}

fn skip_spaces(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while let Some(&c) = chars.peek() {
        if c == ' ' {
            chars.next();
        } else {
            break;
        }
    }
}

/// Returns the list of preset names considered "common" (have real geometry).
pub fn common_presets() -> &'static [&'static str] {
    &[
        "rect",
        "roundRect",
        "ellipse",
        "triangle",
        "isosTriangle",
        "rtTriangle",
        "diamond",
        "pentagon",
        "hexagon",
        "heptagon",
        "octagon",
        "star4",
        "star5",
        "star6",
        "arrow",
        "rightArrow",
        "leftArrow",
        "upArrow",
        "downArrow",
        "heart",
        "parallelogram",
        "trapezoid",
        "chevron",
        "homePlate",
        "plus",
        "cross",
        "cloud",
        "ribbon",
        "ribbon2",
        "donut",
        "flowChartProcess",
        "flowChartDecision",
        "flowChartTerminator",
        "callout1",
        "wedgeRectCallout",
        "roundedRect",
        "snip1Rect",
        "snip2SameRect",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_presets_produce_nonempty_paths() {
        for name in common_presets() {
            let path = preset_path(name, 0.0, 0.0, 100.0, 80.0);
            assert!(
                path.is_some(),
                "common preset '{name}' should produce a path"
            );
            let p = path.unwrap();
            assert!(!p.is_empty(), "path for '{name}' should not be empty");
            assert!(
                p.contains('M'),
                "path for '{name}' should contain M command"
            );
        }
    }

    #[test]
    fn unknown_preset_returns_none() {
        assert!(preset_path("unknownShape123", 0.0, 0.0, 100.0, 80.0).is_none());
        assert!(preset_path("veryRarePreset", 0.0, 0.0, 50.0, 50.0).is_none());
    }

    #[test]
    fn bbox_rect_path_produces_valid_rect() {
        let path = bbox_rect_path(10.0, 20.0, 100.0, 80.0);
        assert!(path.starts_with("M10,20"));
        assert!(path.contains("L110,20"));
        assert!(path.contains("L110,100"));
        assert!(path.contains("L10,100"));
        assert!(path.ends_with(" Z"));
    }

    #[test]
    fn rect_preset_scales_to_bbox() {
        let path = preset_path("rect", 10.0, 20.0, 200.0, 100.0).unwrap();
        // rect is M0,0 L1,0 L1,1 L0,1 Z → scaled to (10,20)+(200,100)
        assert!(path.contains("10.0,20.0")); // top-left
        assert!(path.contains("210.0,20.0")); // top-right
        assert!(path.contains("210.0,120.0")); // bottom-right
        assert!(path.contains("10.0,120.0")); // bottom-left
    }

    #[test]
    fn ellipse_preset_has_curves() {
        let path = preset_path("ellipse", 0.0, 0.0, 100.0, 100.0).unwrap();
        assert!(path.contains('C'), "ellipse should use cubic Bézier curves");
    }

    #[test]
    fn triangle_preset_has_three_points() {
        let path = preset_path("triangle", 0.0, 0.0, 100.0, 100.0).unwrap();
        // Should have M + 2 L commands + Z
        let l_count = path.matches('L').count();
        assert_eq!(l_count, 2, "triangle should have 2 line-to commands");
    }
}
