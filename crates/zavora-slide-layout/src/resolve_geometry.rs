//! Placeholder geometry inheritance: slide → slide-layout → slide-master.
//!
//! Resolves each shape's position/size by walking the inheritance chain.
//! Placeholders are matched by `ph@type` and `ph@idx`; the first explicit
//! `a:xfrm` (with `a:off` and `a:ext`) found wins. Non-placeholder shapes
//! use their own explicit `a:xfrm`. Rotation (`@rot`) is applied when present.
//!
//! ## Fallback
//!
//! When no geometry resolves for a placeholder (no `a:xfrm` in slide, layout,
//! or master), a documented default box is returned:
//! - Title: (457200, 274638, 8229600, 1143000) — standard PowerPoint title area
//! - Body: (457200, 1600200, 8229600, 4525963) — standard PowerPoint body area
//! - Other: (457200, 1600200, 8229600, 4525963) — same as body

use crate::Rect;

/// Resolved geometry for a single shape, in EMU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedGeometry {
    pub rect: Rect,
    /// Rotation in 60,000ths of a degree (e.g. 5400000 = 90°).
    pub rotation: i64,
}

/// Identifies a placeholder by its type and optional index.
/// In PresentationML, placeholders match on `ph@type` (e.g. "title", "body")
/// and `ph@idx` (numeric index distinguishing multiple placeholders of the
/// same type).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaceholderKey {
    /// The `ph@type` attribute value (e.g. "title", "body", "ctrTitle", "dt").
    /// Empty string means the default "body" type.
    pub ph_type: String,
    /// The `ph@idx` attribute value, if present.
    pub idx: Option<u32>,
}

/// Geometry extracted from a shape's `spPr > a:xfrm` element.
/// `None` means the shape has no explicit xfrm (inherits from parent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeXfrm {
    pub x: i64,
    pub y: i64,
    pub cx: i64,
    pub cy: i64,
    /// Rotation in 60,000ths of a degree.
    pub rot: i64,
}

/// A shape descriptor used as input to geometry resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeDescriptor {
    /// If this shape is a placeholder, its key for matching in the chain.
    pub placeholder: Option<PlaceholderKey>,
    /// Explicit xfrm on this shape (from `spPr > a:xfrm`), if present.
    pub xfrm: Option<ShapeXfrm>,
}

/// A collection of shape descriptors from a layout or master part, used as
/// the inheritance source for placeholder geometry.
#[derive(Debug, Clone, Default)]
pub struct PartShapes {
    pub shapes: Vec<ShapeDescriptor>,
}

impl PartShapes {
    /// Find the geometry for a placeholder matching the given key.
    /// Matching rules:
    /// - Match by (ph_type, idx) when idx is Some on both sides
    /// - Match by ph_type alone when idx is None on the query
    /// - "title" also matches "ctrTitle" (center title variant)
    /// - "body" also matches "subTitle" (subtitle variant)
    pub fn find_placeholder_xfrm(&self, key: &PlaceholderKey) -> Option<ShapeXfrm> {
        self.shapes.iter().find_map(|sd| {
            let pk = sd.placeholder.as_ref()?;
            if !ph_type_matches(&pk.ph_type, &key.ph_type) {
                return None;
            }
            // If both have idx, they must match
            if let (Some(a), Some(b)) = (pk.idx, key.idx)
                && a != b
            {
                return None;
            }
            sd.xfrm
        })
    }
}

/// Check if two placeholder types match, accounting for variants.
/// "title" matches "ctrTitle"; "body" matches "subTitle".
fn ph_type_matches(candidate: &str, query: &str) -> bool {
    if candidate == query {
        return true;
    }
    match query {
        "title" => candidate == "ctrTitle",
        "ctrTitle" => candidate == "title",
        "body" => candidate == "subTitle",
        "subTitle" => candidate == "body",
        _ => false,
    }
}

/// Default fallback geometry for placeholders when no xfrm resolves.
/// These match the standard PowerPoint slide master defaults for a 10"×7.5"
/// (9144000×6858000 EMU) slide.
fn default_fallback(ph_type: &str) -> Rect {
    match ph_type {
        "title" | "ctrTitle" => Rect {
            x: 457200,
            y: 274638,
            w: 8229600,
            h: 1143000,
        },
        _ => Rect {
            x: 457200,
            y: 1600200,
            w: 8229600,
            h: 4525963,
        },
    }
}

/// Resolve geometry for a single shape by walking the inheritance chain:
/// slide → layout → master.
///
/// - For non-placeholder shapes: uses the shape's own explicit `xfrm`.
///   Returns `None` if the shape has no xfrm (non-placeholder without
///   geometry cannot be positioned).
/// - For placeholders: walks slide xfrm → layout xfrm → master xfrm,
///   returning the first found. Falls back to a default box if none found.
pub fn resolve_shape_geometry(
    shape: &ShapeDescriptor,
    layout: &PartShapes,
    master: &PartShapes,
) -> Option<ResolvedGeometry> {
    match &shape.placeholder {
        None => {
            // Non-placeholder: must have its own explicit xfrm
            let xfrm = shape.xfrm?;
            Some(ResolvedGeometry {
                rect: Rect {
                    x: xfrm.x,
                    y: xfrm.y,
                    w: xfrm.cx,
                    h: xfrm.cy,
                },
                rotation: xfrm.rot,
            })
        }
        Some(key) => {
            // Placeholder: walk slide → layout → master
            // 1. Check slide's own xfrm
            if let Some(xfrm) = shape.xfrm {
                return Some(ResolvedGeometry {
                    rect: Rect {
                        x: xfrm.x,
                        y: xfrm.y,
                        w: xfrm.cx,
                        h: xfrm.cy,
                    },
                    rotation: xfrm.rot,
                });
            }
            // 2. Check layout
            if let Some(xfrm) = layout.find_placeholder_xfrm(key) {
                return Some(ResolvedGeometry {
                    rect: Rect {
                        x: xfrm.x,
                        y: xfrm.y,
                        w: xfrm.cx,
                        h: xfrm.cy,
                    },
                    rotation: xfrm.rot,
                });
            }
            // 3. Check master
            if let Some(xfrm) = master.find_placeholder_xfrm(key) {
                return Some(ResolvedGeometry {
                    rect: Rect {
                        x: xfrm.x,
                        y: xfrm.y,
                        w: xfrm.cx,
                        h: xfrm.cy,
                    },
                    rotation: xfrm.rot,
                });
            }
            // 4. Fallback to documented default box
            Some(ResolvedGeometry {
                rect: default_fallback(&key.ph_type),
                rotation: 0,
            })
        }
    }
}

/// Resolve geometry for all shapes on a slide, given the layout and master
/// shape collections. Returns a `ResolvedGeometry` for each input shape that
/// can be positioned (non-placeholder shapes without xfrm are skipped).
pub fn resolve_all_geometry(
    slide_shapes: &[ShapeDescriptor],
    layout: &PartShapes,
    master: &PartShapes,
) -> Vec<Option<ResolvedGeometry>> {
    slide_shapes
        .iter()
        .map(|shape| resolve_shape_geometry(shape, layout, master))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xfrm(x: i64, y: i64, cx: i64, cy: i64) -> ShapeXfrm {
        ShapeXfrm {
            x,
            y,
            cx,
            cy,
            rot: 0,
        }
    }

    fn xfrm_rot(x: i64, y: i64, cx: i64, cy: i64, rot: i64) -> ShapeXfrm {
        ShapeXfrm { x, y, cx, cy, rot }
    }

    fn ph_key(ph_type: &str, idx: Option<u32>) -> PlaceholderKey {
        PlaceholderKey {
            ph_type: ph_type.to_string(),
            idx,
        }
    }

    #[test]
    fn placeholder_inherits_from_layout_when_slide_has_no_xfrm() {
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("title", None)),
            xfrm: None, // no explicit xfrm on slide
        };
        let layout = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("title", None)),
                xfrm: Some(xfrm(100, 200, 8000000, 1000000)),
            }],
        };
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: Rect {
                    x: 100,
                    y: 200,
                    w: 8000000,
                    h: 1000000
                },
                rotation: 0,
            })
        );
    }

    #[test]
    fn placeholder_inherits_from_master_when_neither_slide_nor_layout_has_xfrm() {
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("body", Some(1))),
            xfrm: None,
        };
        let layout = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("body", Some(1))),
                xfrm: None, // layout also has no xfrm
            }],
        };
        let master = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("body", Some(1))),
                xfrm: Some(xfrm(457200, 1600200, 8229600, 4525963)),
            }],
        };

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: Rect {
                    x: 457200,
                    y: 1600200,
                    w: 8229600,
                    h: 4525963
                },
                rotation: 0,
            })
        );
    }

    #[test]
    fn non_placeholder_uses_own_explicit_xfrm() {
        let slide_shape = ShapeDescriptor {
            placeholder: None,
            xfrm: Some(xfrm(500000, 600000, 2000000, 1000000)),
        };
        let layout = PartShapes::default();
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: Rect {
                    x: 500000,
                    y: 600000,
                    w: 2000000,
                    h: 1000000
                },
                rotation: 0,
            })
        );
    }

    #[test]
    fn non_placeholder_without_xfrm_returns_none() {
        let slide_shape = ShapeDescriptor {
            placeholder: None,
            xfrm: None,
        };
        let layout = PartShapes::default();
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(resolved, None);
    }

    #[test]
    fn rotation_is_applied() {
        let slide_shape = ShapeDescriptor {
            placeholder: None,
            xfrm: Some(xfrm_rot(100, 200, 300, 400, 5400000)), // 90°
        };
        let layout = PartShapes::default();
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: Rect {
                    x: 100,
                    y: 200,
                    w: 300,
                    h: 400
                },
                rotation: 5400000,
            })
        );
    }

    #[test]
    fn rotation_inherited_from_layout() {
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("title", None)),
            xfrm: None,
        };
        let layout = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("title", None)),
                xfrm: Some(xfrm_rot(100, 200, 8000000, 1000000, 2700000)),
            }],
        };
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(resolved.unwrap().rotation, 2700000);
    }

    #[test]
    fn fallback_to_default_box_when_nothing_resolves() {
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("title", None)),
            xfrm: None,
        };
        let layout = PartShapes::default(); // no matching placeholder
        let master = PartShapes::default(); // no matching placeholder

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        let expected = default_fallback("title");
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: expected,
                rotation: 0,
            })
        );
    }

    #[test]
    fn fallback_body_default() {
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("body", Some(1))),
            xfrm: None,
        };
        let layout = PartShapes::default();
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        let expected = default_fallback("body");
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: expected,
                rotation: 0,
            })
        );
    }

    #[test]
    fn matching_by_ph_type_and_idx() {
        // Two body placeholders with different idx values
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("body", Some(2))),
            xfrm: None,
        };
        let layout = PartShapes {
            shapes: vec![
                ShapeDescriptor {
                    placeholder: Some(ph_key("body", Some(1))),
                    xfrm: Some(xfrm(100, 100, 4000000, 2000000)),
                },
                ShapeDescriptor {
                    placeholder: Some(ph_key("body", Some(2))),
                    xfrm: Some(xfrm(4200000, 100, 4000000, 2000000)),
                },
            ],
        };
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: Rect {
                    x: 4200000,
                    y: 100,
                    w: 4000000,
                    h: 2000000
                },
                rotation: 0,
            })
        );
    }

    #[test]
    fn slide_xfrm_takes_priority_over_layout_and_master() {
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("title", None)),
            xfrm: Some(xfrm(1000, 2000, 7000000, 900000)),
        };
        let layout = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("title", None)),
                xfrm: Some(xfrm(500, 500, 8000000, 1100000)),
            }],
        };
        let master = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("title", None)),
                xfrm: Some(xfrm(457200, 274638, 8229600, 1143000)),
            }],
        };

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        // Slide's own xfrm wins
        assert_eq!(
            resolved,
            Some(ResolvedGeometry {
                rect: Rect {
                    x: 1000,
                    y: 2000,
                    w: 7000000,
                    h: 900000
                },
                rotation: 0,
            })
        );
    }

    #[test]
    fn ctr_title_matches_title() {
        let slide_shape = ShapeDescriptor {
            placeholder: Some(ph_key("ctrTitle", None)),
            xfrm: None,
        };
        let layout = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("title", None)),
                xfrm: Some(xfrm(100, 200, 8000000, 1000000)),
            }],
        };
        let master = PartShapes::default();

        let resolved = resolve_shape_geometry(&slide_shape, &layout, &master);
        assert_eq!(
            resolved.unwrap().rect,
            Rect {
                x: 100,
                y: 200,
                w: 8000000,
                h: 1000000
            }
        );
    }

    #[test]
    fn resolve_all_geometry_batch() {
        let shapes = vec![
            ShapeDescriptor {
                placeholder: Some(ph_key("title", None)),
                xfrm: Some(xfrm(100, 200, 8000000, 1000000)),
            },
            ShapeDescriptor {
                placeholder: None,
                xfrm: None, // non-placeholder without xfrm → None
            },
            ShapeDescriptor {
                placeholder: Some(ph_key("body", Some(1))),
                xfrm: None,
            },
        ];
        let layout = PartShapes {
            shapes: vec![ShapeDescriptor {
                placeholder: Some(ph_key("body", Some(1))),
                xfrm: Some(xfrm(457200, 1600200, 8229600, 4525963)),
            }],
        };
        let master = PartShapes::default();

        let results = resolve_all_geometry(&shapes, &layout, &master);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_some()); // title with own xfrm
        assert!(results[1].is_none()); // non-placeholder, no xfrm
        assert!(results[2].is_some()); // body inherited from layout
    }
}
