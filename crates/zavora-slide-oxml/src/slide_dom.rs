//! Semantic slide model over the lossless [`Document`] tree.
//!
//! This is the production editing surface: it locates PresentationML structures
//! (the shape tree, placeholders, text bodies, paragraphs, runs) *as references
//! into the DOM*, so edits mutate exactly those nodes and every other byte of the
//! slide is preserved. Nothing is copied into a parallel model — the DOM is the
//! single source of truth.

use crate::error::{OxmlError, Result};
use crate::scheme_color::SchemeColor;
use crate::xml::{Document, Element, Node};

// ─── Paragraph property types ───────────────────────────────────────────────

/// Spacing value: either in hundredths of a point (absolute) or thousandths of a
/// percent (relative, e.g. 100000 = 100%).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpacingValue {
    /// Absolute spacing in hundredths of a point (e.g. 1200 = 12pt).
    Points(u32),
    /// Relative spacing in thousandths of a percent (e.g. 150000 = 150%).
    Percent(u32),
}

/// Bullet style for a paragraph.
#[derive(Debug, Clone, PartialEq)]
pub enum BulletKind {
    /// No bullet (`<a:buNone/>`).
    None,
    /// Character bullet (`<a:buChar char="…"/>`).
    Char(String),
    /// Auto-numbered bullet (`<a:buAutoNum type="…"/>`).
    AutoNum(String),
}

/// Auto-fit mode for a text body's `<a:bodyPr>`.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoFit {
    /// No auto-fit — removes any existing autofit child from `bodyPr`.
    None,
    /// Shrink text to fit the frame (`<a:normAutofit fontScale="..."/>`).
    /// `font_scale` is in thousandths of a percent (e.g. 90000 = 90%).
    /// Omit the `fontScale` attribute when `None` or 100000.
    ShrinkToFit { font_scale: Option<u32> },
    /// Resize the shape to fit the text (`<a:spAutoFit/>`).
    ResizeShape,
}

// ─── Fill/color types (Req 8.1, 8.3) ────────────────────────────────────────

/// Color specification: either a literal RGB hex or a theme-color reference.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpec {
    /// 6-hex-digit RGB color (e.g. "FF0000" for red). No `#` prefix.
    Rgb(String),
    /// Theme color reference — emits `<a:schemeClr val="..."/>`.
    Theme(SchemeColor),
}

/// Fill specification for a shape's `spPr`.
///
/// Each variant maps to a DrawingML fill element inside `<p:spPr>` (or `<a:spPr>`).
/// The `set_shape_fill` method removes any existing fill and inserts the new one
/// in schema-valid order via `insert_child_ordered`.
#[derive(Debug, Clone, PartialEq)]
pub enum FillSpec {
    /// Solid color fill → `<a:solidFill>`.
    Solid { color: ColorSpec },
    /// Gradient fill → `<a:gradFill>` with `<a:gsLst>` and `<a:lin>`.
    /// Each stop is (position 0.0–1.0, color).
    Gradient {
        stops: Vec<(f64, ColorSpec)>,
        /// Linear gradient angle in degrees (0 = left-to-right).
        angle_deg: f64,
    },
    /// Pattern fill → `<a:pattFill prst="...">`.
    Pattern {
        preset: String,
        fg: ColorSpec,
        bg: ColorSpec,
    },
    /// Picture fill → `<a:blipFill>` referencing an embedded image by r:id.
    Picture { r_id: String },
    /// No fill → `<a:noFill/>`.
    None,
}

/// Line (outline) specification for a shape's `spPr`.
///
/// Each variant maps to an `<a:ln>` element inside `<p:spPr>` (or `<a:spPr>`).
/// The `set_shape_line` method removes any existing `<a:ln>` and inserts the new
/// one in schema-valid order via `insert_child_ordered`.
#[derive(Debug, Clone, PartialEq)]
pub enum LineSpec {
    /// Styled outline: emits `<a:ln w="..."><a:solidFill>...</a:solidFill><a:prstDash val="..."/></a:ln>`.
    Styled {
        /// Line color (RGB or theme reference).
        color: ColorSpec,
        /// Line width in EMU.
        width_emu: i64,
        /// Optional preset dash style (e.g. "dash", "dot", "lgDash", "sysDot").
        /// When `None`, no `<a:prstDash>` is emitted (solid line).
        dash: Option<String>,
    },
    /// No outline: emits `<a:ln><a:noFill/></a:ln>`.
    None,
}

/// Click action for a shape: either an external URL or a jump to another slide.
/// Satisfies Requirements 11.2 and 11.3.
#[derive(Debug, Clone, PartialEq)]
pub enum ClickAction {
    /// Links to an external URL via a relationship.
    ExternalUrl { r_id: String },
    /// Jumps to another slide (action = "ppaction://hlinksldjump").
    JumpToSlide { r_id: String, action: String },
}

/// Connector type for `add_connector`. Satisfies Requirement 9.2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectorType {
    /// Straight connector — uses `straightConnector1` preset.
    Straight,
    /// Elbow (right-angle) connector — uses `bentConnector3` preset.
    Elbow,
    /// Curved connector — uses `curvedConnector3` preset.
    Curved,
}

impl ConnectorType {
    /// The PresentationML preset geometry name for this connector type.
    fn prst(&self) -> &'static str {
        match self {
            ConnectorType::Straight => "straightConnector1",
            ConnectorType::Elbow => "bentConnector3",
            ConnectorType::Curved => "curvedConnector3",
        }
    }
}

/// Anchor point for a connector endpoint. Identifies a shape and a connection
/// site index on that shape. Satisfies Requirement 9.2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnectorAnchor {
    /// The id of the shape to connect to (`p:cNvPr@id`).
    pub shape_id: u32,
    /// The connection site index on that shape (0–3 for top/right/bottom/left
    /// on most shapes).
    pub connection_idx: u32,
}

// ─── Freeform path builder (Req 9.3) ─────────────────────────────────────────

/// A path segment in a freeform shape.
#[derive(Debug, Clone, PartialEq)]
enum PathSegment {
    /// `<a:moveTo><a:pt x="..." y="..."/></a:moveTo>`
    MoveTo { x: i64, y: i64 },
    /// `<a:lnTo><a:pt x="..." y="..."/></a:lnTo>`
    LineTo { x: i64, y: i64 },
    /// `<a:cubicBezTo><a:pt .../><a:pt .../><a:pt .../></a:cubicBezTo>`
    CubicTo {
        x1: i64,
        y1: i64,
        x2: i64,
        y2: i64,
        x: i64,
        y: i64,
    },
    /// `<a:close/>`
    Close,
}

/// Builder for freeform shape geometry (`<a:custGeom>`). Accumulates path
/// segments (move/line/cubic/close) and produces the complete `<a:custGeom>`
/// XML fragment. Satisfies Requirement 9.3.
#[derive(Debug, Clone, PartialEq)]
pub struct FreeformPath {
    width: i64,
    height: i64,
    segments: Vec<PathSegment>,
}

impl FreeformPath {
    /// Create a new freeform path builder with the given coordinate space
    /// dimensions (used as `<a:path w="..." h="...">`).
    pub fn new(width: i64, height: i64) -> Self {
        Self {
            width,
            height,
            segments: Vec::new(),
        }
    }

    /// Add a moveTo segment — moves the current point without drawing.
    pub fn move_to(&mut self, x: i64, y: i64) -> &mut Self {
        self.segments.push(PathSegment::MoveTo { x, y });
        self
    }

    /// Add a lineTo segment — draws a straight line to the given point.
    pub fn line_to(&mut self, x: i64, y: i64) -> &mut Self {
        self.segments.push(PathSegment::LineTo { x, y });
        self
    }

    /// Add a cubicBezTo segment — draws a cubic Bézier curve with two control
    /// points (x1,y1), (x2,y2) and endpoint (x,y).
    pub fn cubic_to(&mut self, x1: i64, y1: i64, x2: i64, y2: i64, x: i64, y: i64) -> &mut Self {
        self.segments.push(PathSegment::CubicTo {
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        });
        self
    }

    /// Close the current sub-path.
    pub fn close(&mut self) -> &mut Self {
        self.segments.push(PathSegment::Close);
        self
    }

    /// Produce the `<a:custGeom>` XML fragment containing the accumulated path
    /// segments.
    pub fn build_xml(&self) -> String {
        let mut path_children = String::new();
        for seg in &self.segments {
            match seg {
                PathSegment::MoveTo { x, y } => {
                    path_children
                        .push_str(&format!("<a:moveTo><a:pt x=\"{x}\" y=\"{y}\"/></a:moveTo>"));
                }
                PathSegment::LineTo { x, y } => {
                    path_children
                        .push_str(&format!("<a:lnTo><a:pt x=\"{x}\" y=\"{y}\"/></a:lnTo>"));
                }
                PathSegment::CubicTo {
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                } => {
                    path_children.push_str(&format!(
                        "<a:cubicBezTo>\
                         <a:pt x=\"{x1}\" y=\"{y1}\"/>\
                         <a:pt x=\"{x2}\" y=\"{y2}\"/>\
                         <a:pt x=\"{x}\" y=\"{y}\"/>\
                         </a:cubicBezTo>"
                    ));
                }
                PathSegment::Close => {
                    path_children.push_str("<a:close/>");
                }
            }
        }

        format!(
            "<a:custGeom>\
             <a:avLst/>\
             <a:gdLst/>\
             <a:ahLst/>\
             <a:cxnLst/>\
             <a:rect l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
             <a:pathLst>\
             <a:path w=\"{}\" h=\"{}\">{}</a:path>\
             </a:pathLst>\
             </a:custGeom>",
            self.width, self.height, path_children
        )
    }
}

/// Shape inventory entry: metadata about a shape in the `spTree`.
/// Satisfies Requirement 7.3.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeInfo {
    /// The shape's unique id (`p:cNvPr@id`).
    pub id: u32,
    /// The shape's name (`p:cNvPr@name`).
    pub name: String,
    /// The shape type tag: "sp", "pic", "graphicFrame", "cxnSp", or "grpSp".
    pub shape_type: String,
    /// Geometry from `<a:xfrm>`: (x, y, cx, cy) in EMU, if present.
    pub geometry: Option<(i64, i64, i64, i64)>,
    /// Concatenated text content from `<p:txBody>` or `<a:txBody>`, if present.
    pub text: Option<String>,
}

/// A slide part (`ppt/slides/slideN.xml`) as an editable DOM.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideDom {
    doc: Document,
}

impl SlideDom {
    /// Parse a slide part into the editable DOM.
    pub fn parse(src: &[u8]) -> Result<SlideDom> {
        Ok(SlideDom {
            doc: Document::parse(src)?,
        })
    }

    /// Serialize back to bytes (byte-identical when unedited).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.doc.to_bytes()
    }

    /// The `<p:spTree>` element (under `p:sld > p:cSld > p:spTree`).
    fn sp_tree(&self) -> Option<&Element> {
        self.doc.root()?.find_descendant(b"spTree")
    }

    fn sp_tree_mut(&mut self) -> Option<&mut Element> {
        find_descendant_mut(self.doc.root_mut()?, b"spTree")
    }

    /// Top-level shapes (`p:sp`) in document order.
    pub fn shapes(&self) -> impl Iterator<Item = &Element> {
        self.sp_tree()
            .into_iter()
            .flat_map(|t| t.children_named(b"sp"))
    }

    /// Everything on the slide, in the order it is drawn: autoshapes, pictures, tables, charts,
    /// connectors and groups.
    ///
    /// `shapes` yields only `p:sp`, which is right for the editing calls that address a shape by
    /// index and wrong for drawing: a slide whose content is a picture has no `p:sp` at all, so it
    /// drew as nothing.
    pub fn drawables(&self) -> impl Iterator<Item = &Element> {
        self.sp_tree().into_iter().flat_map(|tree| {
            tree.child_elements().filter(|child| {
                matches!(
                    child.local_name(),
                    b"sp" | b"pic" | b"graphicFrame" | b"cxnSp" | b"grpSp"
                )
            })
        })
    }

    /// Plain text of every shape, one paragraph per line (read accessor).
    pub fn text(&self) -> String {
        let mut lines = Vec::new();
        for sp in self.shapes() {
            if let Some(tx) = sp.find_descendant(b"txBody") {
                for p in tx.children_named(b"p") {
                    lines.push(paragraph_text(p));
                }
            }
        }
        lines.join("\n")
    }

    /// The placeholder type of a shape (`p:ph@type`), defaulting to "body" when a
    /// `p:ph` exists without an explicit type (PresentationML's default), or
    /// `None` for a non-placeholder shape.
    fn shape_ph_type(sp: &Element) -> Option<String> {
        let ph = sp.find_descendant(b"ph")?;
        Some(match ph.attr(b"type") {
            Some(t) => String::from_utf8_lossy(t).into_owned(),
            None => "body".to_string(),
        })
    }

    /// Find the first shape whose placeholder type matches `ph_type`. Title
    /// matching also accepts "ctrTitle" (center title on title-slide layouts).
    fn find_placeholder_mut(&mut self, ph_type: &str) -> Option<&mut Element> {
        let tree = self.sp_tree_mut()?;
        for n in &mut tree.children {
            if let Node::Element(sp) = n {
                if sp.local_name() != b"sp" {
                    continue;
                }
                if let Some(t) = SlideDom::shape_ph_type(sp) {
                    let hit = t == ph_type
                        || (ph_type == "title" && t == "ctrTitle")
                        || (ph_type == "body" && t == "subTitle");
                    if hit {
                        return Some(sp);
                    }
                }
            }
        }
        None
    }

    /// Set the text of the title placeholder in place, preserving its shape
    /// properties, placeholder binding, and run formatting. Returns an error if
    /// the slide has no title placeholder (caller may then choose to add one).
    pub fn set_title(&mut self, text: &str) -> Result<()> {
        let sp = self
            .find_placeholder_mut("title")
            .ok_or_else(|| OxmlError::Parse("no title placeholder on slide".into()))?;
        set_shape_text(sp, text)
    }

    /// Number of paragraphs in the body placeholder, if present.
    pub fn body_paragraph_count(&self) -> usize {
        self.shapes()
            .filter_map(|sp| {
                SlideDom::shape_ph_type(sp)
                    .filter(|t| t == "body")
                    .map(|_| sp)
            })
            .next()
            .and_then(|sp| sp.find_descendant(b"txBody"))
            .map(|tx| tx.children_named(b"p").count())
            .unwrap_or(0)
    }

    /// Replace the body placeholder's paragraphs with `items` (text + indent
    /// level), one paragraph each. The first existing paragraph's `a:pPr` and the
    /// first run's `a:rPr` are reused as formatting templates so styling carries
    /// over; the shape's properties and placeholder binding are untouched.
    /// Returns an error if there is no body placeholder.
    pub fn set_body_bullets(&mut self, items: &[(String, u8)]) -> Result<()> {
        let sp = self
            .find_placeholder_mut("body")
            .ok_or_else(|| OxmlError::Parse("no body placeholder on slide".into()))?;
        let tx = find_descendant_mut(sp, b"txBody")
            .ok_or_else(|| OxmlError::Parse("body placeholder has no txBody".into()))?;

        // Capture formatting templates from the first existing paragraph/run.
        let ppr_tmpl: Option<Element> = tx
            .children_named(b"p")
            .next()
            .and_then(|p| p.children_named(b"pPr").next().cloned());
        let rpr_tmpl: Option<Element> = tx
            .children_named(b"p")
            .flat_map(|p| p.children_named(b"r"))
            .next()
            .and_then(|r| r.children_named(b"rPr").next().cloned());

        // Drop existing paragraphs (keep bodyPr/lstStyle and any other children).
        tx.children.retain(|n| match n {
            Node::Element(e) => e.local_name() != b"p",
            Node::Raw(_) => true,
        });
        for (text, level) in items {
            tx.children.push(Node::Element(make_paragraph(
                text, *level, &ppr_tmpl, &rpr_tmpl,
            )));
        }
        Ok(())
    }

    /// Append a text box (`p:sp`, non-placeholder) to the shape tree at the given
    /// EMU position/size, with one paragraph of `text`. Existing shapes are
    /// untouched. Returns the new shape's id.
    pub fn add_text_box(&mut self, text: &str, x: i64, y: i64, cx: i64, cy: i64) -> Result<u32> {
        let id = self.next_shape_id();
        let sp_xml = format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"TextBox {id}\"/>\
             <p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
             <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>\
             <p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>",
            crate::xml::escape_public(text)
        );
        let doc = Document::parse(sp_xml.as_bytes())?;
        let sp = doc
            .nodes
            .into_iter()
            .find(|n| matches!(n, Node::Element(_)))
            .ok_or_else(|| OxmlError::Parse("authored sp did not parse".into()))?;
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        tree.children.push(sp);
        Ok(id)
    }

    /// Append an auto-shape (`p:sp`) with the given preset geometry to the shape
    /// tree at the given EMU position/size. Emits `<a:prstGeom prst="{name}">
    /// <a:avLst/></a:prstGeom>`. Existing shapes are untouched. Returns the new
    /// shape's id. Satisfies Requirement 9.1.
    pub fn add_autoshape(&mut self, prst: &str, x: i64, y: i64, cx: i64, cy: i64) -> Result<u32> {
        let id = self.next_shape_id();
        let sp_xml = format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Shape {id}\"/>\
             <p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
             <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/>\
             <a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom></p:spPr>\
             <p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"
        );
        let doc = Document::parse(sp_xml.as_bytes())?;
        let sp = doc
            .nodes
            .into_iter()
            .find(|n| matches!(n, Node::Element(_)))
            .ok_or_else(|| OxmlError::Parse("authored sp did not parse".into()))?;
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        tree.children.push(sp);
        Ok(id)
    }

    // ─── Connectors (Req 9.2) ─────────────────────────────────────────────

    /// Append a connector shape (`<p:cxnSp>`) to the shape tree at the given
    /// EMU position/size, with the specified connector type and optional start/end
    /// anchors. Emits `<p:cxnSp>` with `<p:nvCxnSpPr>` (containing `<p:cNvPr>`
    /// and `<p:cNvCxnSpPr>` with optional `<a:stCxn>`/`<a:endCxn>`) and
    /// `<p:spPr>` with `<a:xfrm>` and `<a:prstGeom>`. Existing shapes are
    /// untouched. Returns the new connector's shape id. Satisfies Requirement 9.2.
    #[allow(clippy::too_many_arguments)]
    pub fn add_connector(
        &mut self,
        connector_type: ConnectorType,
        start: Option<ConnectorAnchor>,
        end: Option<ConnectorAnchor>,
        x: i64,
        y: i64,
        cx: i64,
        cy: i64,
    ) -> Result<u32> {
        let id = self.next_shape_id();
        let prst = connector_type.prst();

        // Build the <p:cNvCxnSpPr> content with optional stCxn/endCxn.
        let mut cxn_sp_pr_children = String::new();
        if let Some(anchor) = start {
            cxn_sp_pr_children.push_str(&format!(
                "<a:stCxn id=\"{}\" idx=\"{}\"/>",
                anchor.shape_id, anchor.connection_idx
            ));
        }
        if let Some(anchor) = end {
            cxn_sp_pr_children.push_str(&format!(
                "<a:endCxn id=\"{}\" idx=\"{}\"/>",
                anchor.shape_id, anchor.connection_idx
            ));
        }

        let cxn_sp_pr_xml = if cxn_sp_pr_children.is_empty() {
            "<p:cNvCxnSpPr/>".to_string()
        } else {
            format!("<p:cNvCxnSpPr>{cxn_sp_pr_children}</p:cNvCxnSpPr>")
        };

        let cxn_xml = format!(
            "<p:cxnSp>\
             <p:nvCxnSpPr>\
             <p:cNvPr id=\"{id}\" name=\"Connector {id}\"/>\
             {cxn_sp_pr_xml}\
             <p:nvPr/>\
             </p:nvCxnSpPr>\
             <p:spPr>\
             <a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom>\
             </p:spPr>\
             </p:cxnSp>"
        );

        let doc = Document::parse(cxn_xml.as_bytes())?;
        let cxn_node = doc
            .nodes
            .into_iter()
            .find(|n| matches!(n, Node::Element(_)))
            .ok_or_else(|| OxmlError::Parse("authored p:cxnSp did not parse".into()))?;
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        tree.children.push(cxn_node);
        Ok(id)
    }

    // ─── Freeform shapes (Req 9.3) ────────────────────────────────────────

    /// Append a freeform shape (`<p:sp>` with `<a:custGeom>`) to the shape tree
    /// at the given EMU position/size, using the path segments from the provided
    /// `FreeformPath` builder. Existing shapes are untouched. Returns the new
    /// shape's id. Satisfies Requirement 9.3.
    pub fn add_freeform(
        &mut self,
        path: &FreeformPath,
        x: i64,
        y: i64,
        cx: i64,
        cy: i64,
    ) -> Result<u32> {
        let id = self.next_shape_id();
        let cust_geom = path.build_xml();
        let sp_xml = format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Freeform {id}\"/>\
             <p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
             <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/>\
             <a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             {cust_geom}</p:spPr>\
             <p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"
        );
        let doc = Document::parse(sp_xml.as_bytes())?;
        let sp = doc
            .nodes
            .into_iter()
            .find(|n| matches!(n, Node::Element(_)))
            .ok_or_else(|| OxmlError::Parse("authored freeform sp did not parse".into()))?;
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        tree.children.push(sp);
        Ok(id)
    }

    // ─── Group shapes (Req 9.4) ──────────────────────────────────────────

    /// Return the child shape elements inside the group shape (`<p:grpSp>`) at
    /// `shape_idx` in the `spTree`. Child shapes are any of `<p:sp>`, `<p:pic>`,
    /// `<p:graphicFrame>`, `<p:cxnSp>`, or `<p:grpSp>` (nested groups).
    ///
    /// Returns an error if the shape at `shape_idx` is not a `<p:grpSp>`.
    /// Satisfies Requirement 9.4.
    pub fn group_shapes(&self, shape_idx: usize) -> Result<Vec<&Element>> {
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        let tree = self
            .sp_tree()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let el = tree
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(e),
                _ => None,
            })
            .nth(shape_idx)
            .ok_or_else(|| OxmlError::Parse(format!("no shape at index {shape_idx}")))?;
        if el.local_name() != b"grpSp" {
            return Err(OxmlError::Parse(format!(
                "shape at index {shape_idx} is not a group (is {:?})",
                String::from_utf8_lossy(el.local_name())
            )));
        }
        // Collect child shapes inside the group (skip grpSpPr, nvGrpSpPr, etc.).
        let child_shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        Ok(el
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) if child_shape_locals.contains(&e.local_name()) => Some(e),
                _ => None,
            })
            .collect())
    }

    /// Add a new `<p:sp>` with `<a:prstGeom>` as a child of the group shape at
    /// `group_shape_idx`. The shape is inserted inside the group's child list
    /// (after `<p:grpSpPr>`). Returns the new shape's id.
    ///
    /// Returns an error if the shape at `group_shape_idx` is not a `<p:grpSp>`.
    /// Satisfies Requirement 9.4.
    pub fn add_shape_to_group(
        &mut self,
        group_shape_idx: usize,
        prst: &str,
        x: i64,
        y: i64,
        cx: i64,
        cy: i64,
    ) -> Result<u32> {
        let id = self.next_shape_id();
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let grp = tree
            .children
            .iter_mut()
            .filter_map(|n| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(e),
                _ => None,
            })
            .nth(group_shape_idx)
            .ok_or_else(|| OxmlError::Parse(format!("no shape at index {group_shape_idx}")))?;
        if grp.local_name() != b"grpSp" {
            return Err(OxmlError::Parse(format!(
                "shape at index {group_shape_idx} is not a group (is {:?})",
                String::from_utf8_lossy(grp.local_name())
            )));
        }

        // Build the new shape XML.
        let sp_xml = format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Shape {id}\"/>\
             <p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
             <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/>\
             <a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom></p:spPr>\
             <p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"
        );
        let doc = Document::parse(sp_xml.as_bytes())?;
        let sp = doc
            .nodes
            .into_iter()
            .find(|n| matches!(n, Node::Element(_)))
            .ok_or_else(|| OxmlError::Parse("authored sp did not parse".into()))?;

        // Insert after grpSpPr (the last non-shape child before shape children).
        // Find the position after the last of nvGrpSpPr/grpSpPr.
        let insert_pos = grp
            .children
            .iter()
            .rposition(|n| {
                matches!(n, Node::Element(e) if e.local_name() == b"grpSpPr" || e.local_name() == b"nvGrpSpPr")
            })
            .map(|i| i + 1)
            .unwrap_or(grp.children.len());
        grp.children.insert(insert_pos, sp);

        Ok(id)
    }

    // ─── Paragraph-level editing (Req 1.1–1.4) ────────────────────────────

    /// Return the text body (`<p:txBody>` or `<a:txBody>`) for the shape at the
    /// given index in the shape tree. Returns a typed error if the shape doesn't
    /// exist or has no text body.
    fn text_body_by_index(&self, shape_idx: usize) -> Result<&Element> {
        let sp = self
            .shapes()
            .nth(shape_idx)
            .ok_or_else(|| OxmlError::Parse(format!("no shape at index {shape_idx}")))?;
        sp.find_descendant(b"txBody")
            .ok_or_else(|| OxmlError::Parse(format!("shape at index {shape_idx} has no text body")))
    }

    /// Mutable text body for the shape at `shape_idx`.
    fn text_body_by_index_mut(&mut self, shape_idx: usize) -> Result<&mut Element> {
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let sp = tree
            .children
            .iter_mut()
            .filter_map(|n| match n {
                Node::Element(e) if e.local_name() == b"sp" => Some(e),
                _ => None,
            })
            .nth(shape_idx)
            .ok_or_else(|| OxmlError::Parse(format!("no shape at index {shape_idx}")))?;
        find_descendant_mut(sp, b"txBody")
            .ok_or_else(|| OxmlError::Parse(format!("shape at index {shape_idx} has no text body")))
    }

    /// Return the paragraphs (`<a:p>`) in the text body of the shape at
    /// `shape_idx` as a vector of references. Satisfies Requirement 1.1.
    pub fn paragraphs(&self, shape_idx: usize) -> Result<Vec<&Element>> {
        let tx = self.text_body_by_index(shape_idx)?;
        Ok(tx.children_named(b"p").collect())
    }

    /// Append a new paragraph with `text` to the text body of the shape at
    /// `shape_idx`. The paragraph contains a single run with the given text.
    /// Returns a typed error if the text body doesn't exist (Req 1.4).
    pub fn add_paragraph(&mut self, shape_idx: usize, text: &str) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p = make_simple_paragraph(text);
        tx.children.push(Node::Element(p));
        Ok(())
    }

    /// Insert a new paragraph with `text` at `para_idx` within the text body of
    /// the shape at `shape_idx`. Only the new `<a:p>` is added; sibling
    /// paragraphs remain byte-identical (Req 1.2).
    pub fn insert_paragraph(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        text: &str,
    ) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        // Find the position of the Nth <a:p> among all children (which may
        // include bodyPr, lstStyle, whitespace nodes, etc.).
        let insert_pos = nth_p_child_position(tx, para_idx)?;
        let p = make_simple_paragraph(text);
        tx.insert_child_at(insert_pos, Node::Element(p));
        Ok(())
    }

    /// Delete the paragraph at `para_idx` from the text body of the shape at
    /// `shape_idx`. Sibling paragraphs remain byte-identical (Req 1.2).
    pub fn delete_paragraph(&mut self, shape_idx: usize, para_idx: usize) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let pos = nth_p_child_position(tx, para_idx)?;
        // Verify it's actually a paragraph at that position.
        match &tx.children[pos] {
            Node::Element(e) if e.local_name() == b"p" => {}
            _ => {
                return Err(OxmlError::Parse(format!(
                    "child at position {pos} is not a paragraph"
                )));
            }
        }
        tx.children.remove(pos);
        Ok(())
    }

    /// Move the paragraph at `from_idx` to `to_idx` within the text body of the
    /// shape at `shape_idx`. Sibling paragraphs remain byte-identical (Req 1.2).
    pub fn reorder_paragraph(
        &mut self,
        shape_idx: usize,
        from_idx: usize,
        to_idx: usize,
    ) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let from_pos = nth_p_child_position_snapshot(tx, from_idx)?;
        // Remove the paragraph from its current position.
        let node = tx.children.remove(from_pos);
        // Now find the target position (after removal, indices shift).
        let to_pos = nth_p_child_position_for_insert(tx, to_idx)?;
        tx.children.insert(to_pos, node);
        Ok(())
    }

    // ─── Run-level editing (Req 2.1, 2.2) ────────────────────────────────

    /// Return the run and line-break elements (`<a:r>` and `<a:br>`) in the
    /// paragraph at `para_idx` of the shape at `shape_idx`. Satisfies Req 2.1.
    pub fn runs(&self, shape_idx: usize, para_idx: usize) -> Result<Vec<&Element>> {
        let tx = self.text_body_by_index(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        Ok(para
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) if e.local_name() == b"r" || e.local_name() == b"br" => Some(e),
                _ => None,
            })
            .collect())
    }

    /// Append a new `<a:r>` run with `text` to the paragraph at `para_idx` in
    /// the shape at `shape_idx`. Sibling runs remain byte-identical (Req 2.2).
    pub fn add_run(&mut self, shape_idx: usize, para_idx: usize, text: &str) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let run = make_run(text, None);
        para.children.push(Node::Element(run));
        Ok(())
    }

    /// Insert a new `<a:r>` run with `text` at `run_idx` within the paragraph at
    /// `para_idx` in the shape at `shape_idx`. Sibling runs remain byte-identical
    /// (Req 2.2).
    pub fn insert_run(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        run_idx: usize,
        text: &str,
    ) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let insert_pos = nth_run_child_position(para, run_idx)?;
        let run = make_run(text, None);
        para.insert_child_at(insert_pos, Node::Element(run));
        Ok(())
    }

    /// Delete the run (or line break) at `run_idx` within the paragraph at
    /// `para_idx` in the shape at `shape_idx`. Sibling runs remain byte-identical
    /// (Req 2.2).
    pub fn delete_run(&mut self, shape_idx: usize, para_idx: usize, run_idx: usize) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let pos = nth_run_child_position(para, run_idx)?;
        // Verify it's a run or line break.
        match &para.children[pos] {
            Node::Element(e) if e.local_name() == b"r" || e.local_name() == b"br" => {}
            _ => {
                return Err(OxmlError::Parse(format!(
                    "child at run position {pos} is not a run or line break"
                )));
            }
        }
        para.children.remove(pos);
        Ok(())
    }

    /// Edit the text content of the run at `run_idx` within the paragraph at
    /// `para_idx` in the shape at `shape_idx`. The run's `a:rPr` is preserved
    /// byte-for-byte; every other sibling run and its `a:rPr` remain byte-identical
    /// (Req 2.2).
    pub fn edit_run_text(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        run_idx: usize,
        new_text: &str,
    ) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let pos = nth_run_child_position(para, run_idx)?;
        let run = match &mut para.children[pos] {
            Node::Element(e) if e.local_name() == b"r" => e,
            _ => {
                return Err(OxmlError::Parse(format!(
                    "element at run index {run_idx} is not an <a:r> run"
                )));
            }
        };
        // Find the <a:t> child and update its text content.
        let t_el = run
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"t" => Some(e),
                _ => None,
            })
            .ok_or_else(|| OxmlError::Parse("run has no <a:t> child".to_string()))?;
        t_el.set_text_content(new_text);
        Ok(())
    }

    /// Append an `<a:br/>` line break element to the paragraph at `para_idx` in
    /// the shape at `shape_idx`. Sibling runs remain byte-identical (Req 2.1).
    pub fn add_line_break(&mut self, shape_idx: usize, para_idx: usize) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let mut br = new_element(b"a:br");
        br.self_closing = true;
        para.children.push(Node::Element(br));
        Ok(())
    }

    /// Insert an `<a:br/>` line break at `run_idx` within the paragraph at
    /// `para_idx` in the shape at `shape_idx`. Sibling runs remain byte-identical
    /// (Req 2.1).
    pub fn insert_line_break(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        run_idx: usize,
    ) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let insert_pos = nth_run_child_position(para, run_idx)?;
        let mut br = new_element(b"a:br");
        br.self_closing = true;
        para.insert_child_at(insert_pos, Node::Element(br));
        Ok(())
    }

    // ─── Paragraph property setters (Req 1.3) ──────────────────────────────

    /// Set the alignment (`algn` attribute) on the paragraph at `para_idx` in the
    /// shape at `shape_idx`. Creates `<a:pPr>` if absent; preserves unspecified
    /// properties.
    pub fn set_paragraph_alignment(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        algn: &str,
    ) -> Result<()> {
        let ppr = self.ensure_ppr(shape_idx, para_idx)?;
        ppr.set_attr(b"algn", algn.as_bytes());
        Ok(())
    }

    /// Set the indent level (`lvl` attribute) on the paragraph at `para_idx`.
    pub fn set_paragraph_indent_level(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        lvl: u8,
    ) -> Result<()> {
        let ppr = self.ensure_ppr(shape_idx, para_idx)?;
        ppr.set_attr(b"lvl", lvl.to_string().as_bytes());
        Ok(())
    }

    /// Set space-before on the paragraph at `para_idx`. Writes `<a:spcBef>` with
    /// either `<a:spcPts>` or `<a:spcPct>` depending on the value.
    pub fn set_paragraph_space_before(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        value: SpacingValue,
    ) -> Result<()> {
        let ppr = self.ensure_ppr(shape_idx, para_idx)?;
        set_spacing_child(ppr, b"spcBef", value);
        Ok(())
    }

    /// Set space-after on the paragraph at `para_idx`.
    pub fn set_paragraph_space_after(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        value: SpacingValue,
    ) -> Result<()> {
        let ppr = self.ensure_ppr(shape_idx, para_idx)?;
        set_spacing_child(ppr, b"spcAft", value);
        Ok(())
    }

    /// Set line spacing on the paragraph at `para_idx`. Writes `<a:lnSpc>`.
    pub fn set_paragraph_line_spacing(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        value: SpacingValue,
    ) -> Result<()> {
        let ppr = self.ensure_ppr(shape_idx, para_idx)?;
        set_spacing_child(ppr, b"lnSpc", value);
        Ok(())
    }

    /// Set the bullet style on the paragraph at `para_idx`.
    pub fn set_paragraph_bullet(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        bullet: &BulletKind,
    ) -> Result<()> {
        let ppr = self.ensure_ppr(shape_idx, para_idx)?;
        // Remove any existing bullet elements.
        ppr.children.retain(|n| {
            !matches!(n, Node::Element(e) if matches!(
                e.local_name(),
                b"buNone" | b"buChar" | b"buAutoNum"
            ))
        });
        let bullet_node = match bullet {
            BulletKind::None => {
                let mut el = new_element(b"a:buNone");
                el.self_closing = true;
                el
            }
            BulletKind::Char(ch) => {
                let mut el = new_element(b"a:buChar");
                el.self_closing = true;
                el.set_attr(b"char", ch.as_bytes());
                el
            }
            BulletKind::AutoNum(typ) => {
                let mut el = new_element(b"a:buAutoNum");
                el.self_closing = true;
                el.set_attr(b"type", typ.as_bytes());
                el
            }
        };
        ppr.insert_child_ordered(Node::Element(bullet_node));
        Ok(())
    }

    /// Ensure the paragraph at `para_idx` in shape `shape_idx` has an `<a:pPr>`
    /// child, creating one if absent. Returns a mutable reference to the pPr.
    fn ensure_ppr(&mut self, shape_idx: usize, para_idx: usize) -> Result<&mut Element> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        // Find or create pPr as the first child element of the paragraph.
        let has_ppr = para
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"pPr"));
        if !has_ppr {
            let mut ppr = new_element(b"a:pPr");
            ppr.self_closing = true;
            para.children.insert(0, Node::Element(ppr));
        }
        // Return mutable reference to the pPr.
        let ppr = para
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"pPr" => Some(e),
                _ => None,
            })
            .unwrap();
        Ok(ppr)
    }

    // ─── Auto-fit (Req 3.1) ────────────────────────────────────────────────

    /// Set the auto-fit mode on the text body of the shape at `shape_idx`.
    /// Locates `<a:bodyPr>`, removes any existing autofit child (`normAutofit`,
    /// `spAutoFit`, `noAutofit`), and inserts the correct child element.
    pub fn set_autofit(&mut self, shape_idx: usize, autofit: &AutoFit) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let body_pr = first_child_named_mut(tx, b"bodyPr").ok_or_else(|| {
            OxmlError::Parse(format!(
                "shape at index {shape_idx} text body has no bodyPr"
            ))
        })?;

        // Remove any existing autofit children.
        body_pr.children.retain(|n| {
            !matches!(n, Node::Element(e) if matches!(
                e.local_name(),
                b"normAutofit" | b"spAutoFit" | b"noAutofit"
            ))
        });

        match autofit {
            AutoFit::None => {
                // No autofit child needed — absence means no auto-fit.
                // (Some producers emit <a:noAutofit/> explicitly; we omit it
                // since the schema default is no auto-fit when absent.)
            }
            AutoFit::ShrinkToFit { font_scale } => {
                let mut el = new_element(b"a:normAutofit");
                el.self_closing = true;
                // Emit fontScale only when present and not 100000 (the default).
                if let Some(scale) = font_scale
                    && *scale != 100_000
                {
                    el.set_attr(b"fontScale", scale.to_string().as_bytes());
                }
                if body_pr.self_closing {
                    body_pr.self_closing = false;
                    body_pr.dirty = true;
                }
                // Insert as first child of bodyPr (autofit elements come early
                // in the schema sequence for CT_TextBodyProperties).
                body_pr.children.insert(0, Node::Element(el));
            }
            AutoFit::ResizeShape => {
                let mut el = new_element(b"a:spAutoFit");
                el.self_closing = true;
                if body_pr.self_closing {
                    body_pr.self_closing = false;
                    body_pr.dirty = true;
                }
                body_pr.children.insert(0, Node::Element(el));
            }
        }

        Ok(())
    }

    // ─── Shape geometry (Req 7.1, 7.4) ────────────────────────────────────

    /// Return a mutable reference to the shape element at `shape_idx` in the
    /// shape tree. Works for any shape type (`p:sp`, `p:pic`, `p:graphicFrame`,
    /// `p:cxnSp`, `p:grpSp`).
    fn shape_element_mut(&mut self, shape_idx: usize) -> Result<&mut Element> {
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        tree.children
            .iter_mut()
            .filter_map(|n| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(e),
                _ => None,
            })
            .nth(shape_idx)
            .ok_or_else(|| OxmlError::Parse(format!("no shape at index {shape_idx}")))
    }

    /// Ensure the shape at `shape_idx` has `<p:spPr>` (or `<a:spPr>`) with an
    /// `<a:xfrm>` child. Creates both if absent, using `insert_child_ordered` for
    /// schema-valid positioning. Returns a mutable reference to the `<a:xfrm>`.
    fn ensure_xfrm(&mut self, shape_idx: usize) -> Result<&mut Element> {
        let sp = self.shape_element_mut(shape_idx)?;

        // Ensure spPr exists. Look for either `p:spPr` or `spPr` (local name).
        let has_sppr = sp
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"spPr"));
        if !has_sppr {
            let mut sppr = new_element(b"p:spPr");
            sppr.self_closing = true;
            // spPr typically comes after nvSpPr/nvPicPr/nvCxnSpPr and before txBody.
            // Append it — the shape structure doesn't use insert_child_ordered at
            // the shape level, but spPr is always present in well-formed shapes.
            sp.children.push(Node::Element(sppr));
        }

        // Get mutable reference to spPr.
        let sppr = sp
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"spPr" => Some(e),
                _ => None,
            })
            .unwrap();

        // Ensure xfrm exists inside spPr.
        let has_xfrm = sppr
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"xfrm"));
        if !has_xfrm {
            let mut xfrm = new_element(b"a:xfrm");
            xfrm.self_closing = true;
            sppr.insert_child_ordered(Node::Element(xfrm));
        }

        // Return mutable reference to xfrm.
        let xfrm = sppr
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"xfrm" => Some(e),
                _ => None,
            })
            .unwrap();
        Ok(xfrm)
    }

    /// Set the position (`<a:off x="..." y="..."/>`) of the shape at `shape_idx`.
    /// Creates `spPr` and `xfrm` if absent. Sibling shapes are preserved
    /// byte-for-byte (Req 7.4).
    pub fn set_shape_position(&mut self, shape_idx: usize, x: i64, y: i64) -> Result<()> {
        let xfrm = self.ensure_xfrm(shape_idx)?;
        set_or_create_off(xfrm, x, y);
        Ok(())
    }

    /// Set the size (`<a:ext cx="..." cy="..."/>`) of the shape at `shape_idx`.
    /// Creates `spPr` and `xfrm` if absent. Sibling shapes are preserved
    /// byte-for-byte (Req 7.4).
    pub fn set_shape_size(&mut self, shape_idx: usize, cx: i64, cy: i64) -> Result<()> {
        let xfrm = self.ensure_xfrm(shape_idx)?;
        set_or_create_ext(xfrm, cx, cy);
        Ok(())
    }

    /// Set the rotation (`rot` attribute on `<a:xfrm>`) of the shape at
    /// `shape_idx`, in 60,000ths of a degree. Creates `spPr` and `xfrm` if
    /// absent. Sibling shapes are preserved byte-for-byte (Req 7.4).
    pub fn set_shape_rotation(&mut self, shape_idx: usize, rot: i64) -> Result<()> {
        let xfrm = self.ensure_xfrm(shape_idx)?;
        xfrm.set_attr(b"rot", rot.to_string().as_bytes());
        Ok(())
    }

    /// Convenience method: set position, size, and optionally rotation on the
    /// shape at `shape_idx` in one call. Creates `spPr` and `xfrm` if absent.
    /// Sibling shapes are preserved byte-for-byte (Req 7.4).
    pub fn set_shape_geometry(
        &mut self,
        shape_idx: usize,
        x: i64,
        y: i64,
        cx: i64,
        cy: i64,
        rot: Option<i64>,
    ) -> Result<()> {
        let xfrm = self.ensure_xfrm(shape_idx)?;
        set_or_create_off(xfrm, x, y);
        set_or_create_ext(xfrm, cx, cy);
        if let Some(r) = rot {
            xfrm.set_attr(b"rot", r.to_string().as_bytes());
        }
        Ok(())
    }

    // ─── Shape fill (Req 8.1, 8.3) ─────────────────────────────────────────

    /// Set the fill of the shape at `shape_idx`. Locates (or creates) the
    /// shape's `spPr`, removes any existing fill element (`solidFill`, `gradFill`,
    /// `pattFill`, `blipFill`, `noFill`, `grpFill`), and inserts the new fill
    /// element in schema-valid order via `insert_child_ordered`. Sibling shapes
    /// are preserved byte-for-byte (Req 7.4).
    pub fn set_shape_fill(&mut self, shape_idx: usize, fill: &FillSpec) -> Result<()> {
        let sp = self.shape_element_mut(shape_idx)?;

        // Ensure spPr exists.
        let has_sppr = sp
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"spPr"));
        if !has_sppr {
            let mut sppr = new_element(b"p:spPr");
            sppr.self_closing = true;
            sp.children.push(Node::Element(sppr));
        }

        // Get mutable reference to spPr.
        let sppr = sp
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"spPr" => Some(e),
                _ => None,
            })
            .unwrap();

        // Remove any existing fill element.
        sppr.children
            .retain(|n| !matches!(n, Node::Element(e) if is_fill(e.local_name())));

        // Build and insert the new fill element.
        let fill_node = build_fill_element(fill);
        if sppr.self_closing {
            sppr.self_closing = false;
            sppr.dirty = true;
        }
        sppr.insert_child_ordered(fill_node);

        Ok(())
    }

    // ─── Shape line/outline (Req 8.2, 8.3) ────────────────────────────────

    /// Set the outline (line) of the shape at `shape_idx`. Locates (or creates)
    /// the shape's `spPr`, removes any existing `<a:ln>` element, and inserts
    /// the new `<a:ln>` element in schema-valid order via `insert_child_ordered`.
    /// Sibling shapes are preserved byte-for-byte (Req 7.4).
    pub fn set_shape_line(&mut self, shape_idx: usize, line: &LineSpec) -> Result<()> {
        let sp = self.shape_element_mut(shape_idx)?;

        // Ensure spPr exists.
        let has_sppr = sp
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"spPr"));
        if !has_sppr {
            let mut sppr = new_element(b"p:spPr");
            sppr.self_closing = true;
            sp.children.push(Node::Element(sppr));
        }

        // Get mutable reference to spPr.
        let sppr = sp
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"spPr" => Some(e),
                _ => None,
            })
            .unwrap();

        // Remove any existing <a:ln> element.
        sppr.children
            .retain(|n| !matches!(n, Node::Element(e) if e.local_name() == b"ln"));

        // Build and insert the new <a:ln> element.
        let ln_node = build_line_element(line);
        if sppr.self_closing {
            sppr.self_closing = false;
            sppr.dirty = true;
        }
        sppr.insert_child_ordered(ln_node);

        Ok(())
    }

    // ─── Shape lifecycle: delete + reorder (Req 7.2, 7.4) ────────────────

    /// All top-level shape elements in the `spTree` (any of `p:sp`, `p:pic`,
    /// `p:graphicFrame`, `p:cxnSp`, `p:grpSp`) in document order.
    pub fn all_shapes(&self) -> Vec<&Element> {
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        self.sp_tree()
            .into_iter()
            .flat_map(|t| {
                t.children.iter().filter_map(|n| match n {
                    Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(e),
                    _ => None,
                })
            })
            .collect()
    }

    /// Delete the shape at `shape_idx` (0-based, across all shape types) from
    /// the `spTree`. Sibling shapes remain byte-for-byte identical (Req 7.4).
    /// Returns an error if the index is out of range.
    pub fn delete_shape(&mut self, shape_idx: usize) -> Result<()> {
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        // Find the child-index of the Nth shape element.
        let mut count = 0usize;
        let mut target_pos = None;
        for (i, child) in tree.children.iter().enumerate() {
            if let Node::Element(e) = child
                && shape_locals.contains(&e.local_name())
            {
                if count == shape_idx {
                    target_pos = Some(i);
                    break;
                }
                count += 1;
            }
        }
        let pos = target_pos.ok_or_else(|| {
            OxmlError::Parse(format!(
                "shape index {shape_idx} out of range (have {count} shapes)"
            ))
        })?;
        tree.children.remove(pos);
        Ok(())
    }

    /// Move the shape at `from_idx` to `to_idx` within the `spTree`, changing
    /// its z-order position. Sibling shapes remain byte-for-byte identical
    /// (Req 7.4). Returns an error if either index is out of range.
    pub fn reorder_shape(&mut self, from_idx: usize, to_idx: usize) -> Result<()> {
        if from_idx == to_idx {
            return Ok(());
        }
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];

        // Collect the child-indices of all shape elements.
        let shape_positions: Vec<usize> = tree
            .children
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(i),
                _ => None,
            })
            .collect();

        let shape_count = shape_positions.len();
        if from_idx >= shape_count {
            return Err(OxmlError::Parse(format!(
                "from_idx {from_idx} out of range (have {shape_count} shapes)"
            )));
        }
        if to_idx >= shape_count {
            return Err(OxmlError::Parse(format!(
                "to_idx {to_idx} out of range (have {shape_count} shapes)"
            )));
        }

        let from_pos = shape_positions[from_idx];
        // Remove the shape from its current position.
        let node = tree.children.remove(from_pos);

        // Recalculate shape positions after removal.
        let shape_positions_after: Vec<usize> = tree
            .children
            .iter()
            .enumerate()
            .filter_map(|(i, n)| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(i),
                _ => None,
            })
            .collect();

        // Determine the insertion position: insert before the element currently
        // at to_idx, or after the last shape if to_idx == remaining count.
        let insert_pos = if to_idx < shape_positions_after.len() {
            shape_positions_after[to_idx]
        } else {
            // Insert after the last shape element.
            shape_positions_after
                .last()
                .map(|&last| last + 1)
                .unwrap_or(tree.children.len())
        };

        tree.children.insert(insert_pos, node);
        Ok(())
    }

    /// Return a shape inventory: metadata for every shape in the `spTree`.
    /// Satisfies Requirement 7.3.
    pub fn shape_inventory(&self) -> Vec<ShapeInfo> {
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        let tree = match self.sp_tree() {
            Some(t) => t,
            None => return Vec::new(),
        };
        tree.children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => {
                    Some(extract_shape_info(e))
                }
                _ => None,
            })
            .collect()
    }

    /// Next free shape id = 1 + max existing `p:cNvPr@id` (ids must be unique
    /// within the slide; the group shape is id 1).
    fn next_shape_id(&self) -> u32 {
        let mut max = 1u32;
        if let Some(tree) = self.sp_tree() {
            collect_max_cnvpr_id(tree, &mut max);
        }
        max + 1
    }

    /// Apply character formatting to every run of a placeholder (by type, e.g.
    /// "title" or "body"), mutating each run's `a:rPr` in place. Only the fields
    /// set in `fmt` change; all other run/shape content is preserved.
    pub fn format_placeholder(&mut self, ph_type: &str, fmt: &RunFormat) -> Result<()> {
        let sp = self
            .find_placeholder_mut(ph_type)
            .ok_or_else(|| OxmlError::Parse(format!("no {ph_type} placeholder on slide")))?;
        let tx = find_descendant_mut(sp, b"txBody")
            .ok_or_else(|| OxmlError::Parse("placeholder has no txBody".into()))?;
        let mut touched = 0;
        for p in tx.children.iter_mut() {
            if let Node::Element(para) = p
                && para.local_name() == b"p"
            {
                for r in para.children.iter_mut() {
                    if let Node::Element(run) = r
                        && run.local_name() == b"r"
                    {
                        apply_run_format(run, fmt);
                        touched += 1;
                    }
                }
            }
        }
        if touched == 0 {
            return Err(OxmlError::Parse("placeholder has no runs to format".into()));
        }
        Ok(())
    }

    /// Apply character formatting to a specific run identified by shape index,
    /// paragraph index, and run index. Only the fields set in `fmt` change; all
    /// sibling runs and their `a:rPr` remain byte-identical (Req 2.2, 2.3).
    pub fn format_run(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        run_idx: usize,
        fmt: &RunFormat,
    ) -> Result<()> {
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let pos = nth_run_child_position(para, run_idx)?;
        let run = match &mut para.children[pos] {
            Node::Element(e) if e.local_name() == b"r" => e,
            _ => {
                return Err(OxmlError::Parse(format!(
                    "element at run index {run_idx} is not an <a:r> run"
                )));
            }
        };
        apply_run_format(run, fmt);
        Ok(())
    }

    // ─── Table editing (Req 6.1) ──────────────────────────────────────────

    /// Return a reference to the `<a:tbl>` element inside the `<p:graphicFrame>`
    /// at `shape_idx` (0-based across all shape types in the spTree). Returns an
    /// error if the shape doesn't exist, isn't a graphicFrame, or doesn't contain
    /// a table.
    pub fn table_element(&self, shape_idx: usize) -> Result<&Element> {
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        let tree = self
            .sp_tree()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let el = tree
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(e),
                _ => None,
            })
            .nth(shape_idx)
            .ok_or_else(|| OxmlError::Parse(format!("no shape at index {shape_idx}")))?;
        if el.local_name() != b"graphicFrame" {
            return Err(OxmlError::Parse(format!(
                "shape at index {shape_idx} is not a graphicFrame (is {:?})",
                String::from_utf8_lossy(el.local_name())
            )));
        }
        el.find_descendant(b"tbl").ok_or_else(|| {
            OxmlError::Parse(format!(
                "graphicFrame at index {shape_idx} does not contain a table"
            ))
        })
    }

    /// Return a mutable reference to the `<a:tbl>` element inside the
    /// `<p:graphicFrame>` at `shape_idx`.
    pub fn table_element_mut(&mut self, shape_idx: usize) -> Result<&mut Element> {
        let shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        let el = tree
            .children
            .iter_mut()
            .filter_map(|n| match n {
                Node::Element(e) if shape_locals.contains(&e.local_name()) => Some(e),
                _ => None,
            })
            .nth(shape_idx)
            .ok_or_else(|| OxmlError::Parse(format!("no shape at index {shape_idx}")))?;
        if el.local_name() != b"graphicFrame" {
            return Err(OxmlError::Parse(format!(
                "shape at index {shape_idx} is not a graphicFrame (is {:?})",
                String::from_utf8_lossy(el.local_name())
            )));
        }
        find_descendant_mut(el, b"tbl").ok_or_else(|| {
            OxmlError::Parse(format!(
                "graphicFrame at index {shape_idx} does not contain a table"
            ))
        })
    }

    /// Append a new row (`<a:tr h="...">`) to the table at `shape_idx`. The row
    /// contains one `<a:tc>` per column (matching the `<a:tblGrid>` column count),
    /// each with a minimal `<a:txBody>` containing one empty paragraph. Preserves
    /// existing rows' content and formatting (Req 6.1).
    pub fn add_table_row(&mut self, shape_idx: usize, height_emu: i64) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;
        let col_count = tbl_grid_col_count(tbl);
        let tr = make_table_row(height_emu, col_count);
        tbl.children.push(Node::Element(tr));
        Ok(())
    }

    /// Insert a new row at `row_idx` (0-based among existing `<a:tr>` elements)
    /// in the table at `shape_idx`. The row contains one `<a:tc>` per column.
    /// Preserves existing rows' content and formatting (Req 6.1).
    pub fn insert_table_row(
        &mut self,
        shape_idx: usize,
        row_idx: usize,
        height_emu: i64,
    ) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;
        let col_count = tbl_grid_col_count(tbl);
        let tr = make_table_row(height_emu, col_count);
        let insert_pos = nth_tr_child_position_for_insert(tbl, row_idx)?;
        tbl.insert_child_at(insert_pos, Node::Element(tr));
        Ok(())
    }

    /// Remove the row at `row_idx` (0-based among `<a:tr>` elements) from the
    /// table at `shape_idx`. Preserves other rows' content and formatting
    /// (Req 6.1).
    pub fn remove_table_row(&mut self, shape_idx: usize, row_idx: usize) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;
        let pos = nth_tr_child_position(tbl, row_idx)?;
        tbl.children.remove(pos);
        Ok(())
    }

    /// Append a column to the table at `shape_idx`: adds a `<a:gridCol w="..."/>`
    /// to `<a:tblGrid>` and appends one `<a:tc>` (with minimal txBody) to every
    /// existing `<a:tr>`. Preserves existing cells' content and formatting
    /// (Req 6.1).
    pub fn add_table_column(&mut self, shape_idx: usize, width_emu: i64) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;
        // Add gridCol to tblGrid.
        let tbl_grid = first_child_named_mut(tbl, b"tblGrid")
            .ok_or_else(|| OxmlError::Parse("table has no tblGrid".into()))?;
        let mut grid_col = new_element(b"a:gridCol");
        grid_col.self_closing = true;
        grid_col.set_attr(b"w", width_emu.to_string().as_bytes());
        tbl_grid.children.push(Node::Element(grid_col));

        // Append a tc to every existing row.
        for child in tbl.children.iter_mut() {
            if let Node::Element(tr) = child
                && tr.local_name() == b"tr"
            {
                let tc = make_table_cell();
                tr.children.push(Node::Element(tc));
            }
        }
        Ok(())
    }

    /// Remove the column at `col_idx` (0-based) from the table at `shape_idx`:
    /// removes the `<a:gridCol>` at `col_idx` from `<a:tblGrid>` and removes the
    /// corresponding `<a:tc>` from every `<a:tr>`. Preserves other cells' content
    /// and formatting (Req 6.1).
    pub fn remove_table_column(&mut self, shape_idx: usize, col_idx: usize) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;

        // Remove gridCol from tblGrid.
        let tbl_grid = first_child_named_mut(tbl, b"tblGrid")
            .ok_or_else(|| OxmlError::Parse("table has no tblGrid".into()))?;
        let grid_col_pos = nth_child_position_by_name(tbl_grid, b"gridCol", col_idx)?;
        tbl_grid.children.remove(grid_col_pos);

        // Remove the tc at col_idx from every row.
        for child in tbl.children.iter_mut() {
            if let Node::Element(tr) = child
                && tr.local_name() == b"tr"
            {
                let tc_pos = nth_child_position_by_name(tr, b"tc", col_idx);
                if let Ok(pos) = tc_pos {
                    tr.children.remove(pos);
                }
            }
        }
        Ok(())
    }

    // ─── Table merge/split (Req 6.2) ─────────────────────────────────────

    /// Merge a rectangular range of cells in the table at `shape_idx`.
    ///
    /// The range is defined by `start_row..=end_row` and `start_col..=end_col`
    /// (0-based, inclusive). The origin cell (`start_row`, `start_col`) receives
    /// `gridSpan` and `rowSpan` attributes; cells covered horizontally get
    /// `hMerge="1"` and cells covered vertically get `vMerge="1"`. Covered cells'
    /// text content is cleared (empty txBody). The origin cell's content is
    /// preserved. (Req 6.2)
    pub fn merge_table_cells(
        &mut self,
        shape_idx: usize,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> Result<()> {
        // Validate range.
        if start_row > end_row || start_col > end_col {
            return Err(OxmlError::Parse(format!(
                "invalid merge range: start ({start_row},{start_col}) must be <= end ({end_row},{end_col})"
            )));
        }

        let tbl = self.table_element_mut(shape_idx)?;
        let row_count = tbl.children_named(b"tr").count();
        let col_count = tbl_grid_col_count(tbl);

        if end_row >= row_count {
            return Err(OxmlError::Parse(format!(
                "end_row {end_row} out of range (table has {row_count} rows)"
            )));
        }
        if end_col >= col_count {
            return Err(OxmlError::Parse(format!(
                "end_col {end_col} out of range (table has {col_count} columns)"
            )));
        }

        let col_span = end_col - start_col + 1;
        let row_span = end_row - start_row + 1;

        // Iterate over all rows in the merge range.
        let mut row_idx = 0usize;
        for child in tbl.children.iter_mut() {
            if let Node::Element(tr) = child
                && tr.local_name() == b"tr"
            {
                if row_idx >= start_row && row_idx <= end_row {
                    // Iterate over cells in this row within the column range.
                    let mut col_idx = 0usize;
                    for tc_node in tr.children.iter_mut() {
                        if let Node::Element(tc) = tc_node
                            && tc.local_name() == b"tc"
                        {
                            if col_idx >= start_col && col_idx <= end_col {
                                if row_idx == start_row && col_idx == start_col {
                                    // Origin cell: set gridSpan and rowSpan.
                                    if col_span > 1 {
                                        tc.set_attr(b"gridSpan", col_span.to_string().as_bytes());
                                    }
                                    if row_span > 1 {
                                        tc.set_attr(b"rowSpan", row_span.to_string().as_bytes());
                                    }
                                    // Origin cell content is preserved.
                                } else {
                                    // Covered cell: set merge attributes and clear content.
                                    if row_idx > start_row {
                                        tc.set_attr(b"vMerge", b"1");
                                    }
                                    if col_idx > start_col {
                                        tc.set_attr(b"hMerge", b"1");
                                    }
                                    // Clear the cell's text content (replace txBody with empty one).
                                    clear_cell_content(tc);
                                }
                            }
                            col_idx += 1;
                        }
                    }
                }
                row_idx += 1;
            }
        }

        Ok(())
    }

    /// Split a previously merged cell at (`row`, `col`) in the table at
    /// `shape_idx`.
    ///
    /// Removes `gridSpan` and `rowSpan` attributes from the origin cell, and
    /// removes `hMerge` and `vMerge` attributes from all cells that were covered
    /// by this merge. The covered cells retain their (empty) txBody. (Req 6.2)
    pub fn split_table_cell(&mut self, shape_idx: usize, row: usize, col: usize) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;
        let row_count = tbl.children_named(b"tr").count();
        let col_count = tbl_grid_col_count(tbl);

        if row >= row_count {
            return Err(OxmlError::Parse(format!(
                "row {row} out of range (table has {row_count} rows)"
            )));
        }
        if col >= col_count {
            return Err(OxmlError::Parse(format!(
                "col {col} out of range (table has {col_count} columns)"
            )));
        }

        // First pass: read the gridSpan and rowSpan from the origin cell.
        let (grid_span, row_span) = {
            let mut r_idx = 0usize;
            let mut gs = 1usize;
            let mut rs = 1usize;
            for child in tbl.children.iter() {
                if let Node::Element(tr) = child
                    && tr.local_name() == b"tr"
                {
                    if r_idx == row {
                        let mut c_idx = 0usize;
                        for tc_node in tr.children.iter() {
                            if let Node::Element(tc) = tc_node
                                && tc.local_name() == b"tc"
                            {
                                if c_idx == col {
                                    gs = tc
                                        .attr(b"gridSpan")
                                        .and_then(|v| {
                                            std::str::from_utf8(v).ok()?.parse::<usize>().ok()
                                        })
                                        .unwrap_or(1);
                                    rs = tc
                                        .attr(b"rowSpan")
                                        .and_then(|v| {
                                            std::str::from_utf8(v).ok()?.parse::<usize>().ok()
                                        })
                                        .unwrap_or(1);
                                    break;
                                }
                                c_idx += 1;
                            }
                        }
                        break;
                    }
                    r_idx += 1;
                }
            }
            (gs, rs)
        };

        // Second pass: remove merge attributes from origin and covered cells.
        let end_row = row + row_span - 1;
        let end_col = col + grid_span - 1;

        let mut r_idx = 0usize;
        for child in tbl.children.iter_mut() {
            if let Node::Element(tr) = child
                && tr.local_name() == b"tr"
            {
                if r_idx >= row && r_idx <= end_row {
                    let mut c_idx = 0usize;
                    for tc_node in tr.children.iter_mut() {
                        if let Node::Element(tc) = tc_node
                            && tc.local_name() == b"tc"
                        {
                            if c_idx >= col && c_idx <= end_col {
                                if r_idx == row && c_idx == col {
                                    // Origin cell: remove gridSpan and rowSpan.
                                    tc.attrs
                                        .retain(|(k, _)| k != b"gridSpan" && k != b"rowSpan");
                                    tc.dirty = true;
                                } else {
                                    // Covered cell: remove hMerge and vMerge.
                                    tc.attrs.retain(|(k, _)| k != b"hMerge" && k != b"vMerge");
                                    tc.dirty = true;
                                }
                            }
                            c_idx += 1;
                        }
                    }
                }
                r_idx += 1;
            }
        }

        Ok(())
    }

    // ─── Table column width / row height / cell props (Req 6.3–6.5) ───────

    /// Set the width of the column at `col_idx` in the table at `shape_idx`.
    /// Updates the `w` attribute on the `<a:gridCol>` element. (Req 6.3)
    pub fn set_column_width(
        &mut self,
        shape_idx: usize,
        col_idx: usize,
        width_emu: i64,
    ) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;
        let tbl_grid = first_child_named_mut(tbl, b"tblGrid")
            .ok_or_else(|| OxmlError::Parse("table has no tblGrid".into()))?;
        let pos = nth_child_position_by_name(tbl_grid, b"gridCol", col_idx)?;
        let grid_col = match &mut tbl_grid.children[pos] {
            Node::Element(e) => e,
            _ => unreachable!(),
        };
        grid_col.set_attr(b"w", width_emu.to_string().as_bytes());
        Ok(())
    }

    /// Set the height of the row at `row_idx` in the table at `shape_idx`.
    /// Updates the `h` attribute on the `<a:tr>` element. (Req 6.3)
    pub fn set_row_height(
        &mut self,
        shape_idx: usize,
        row_idx: usize,
        height_emu: i64,
    ) -> Result<()> {
        let tbl = self.table_element_mut(shape_idx)?;
        let pos = nth_tr_child_position(tbl, row_idx)?;
        let tr = match &mut tbl.children[pos] {
            Node::Element(e) => e,
            _ => unreachable!(),
        };
        tr.set_attr(b"h", height_emu.to_string().as_bytes());
        Ok(())
    }

    /// Set the text content of the cell at (`row_idx`, `col_idx`) in the table
    /// at `shape_idx`. Replaces the cell's `<a:txBody>` content with a single
    /// paragraph containing the text. Cell text reuses Part A on the cell
    /// `a:txBody`. (Req 6.4)
    pub fn set_cell_text(
        &mut self,
        shape_idx: usize,
        row_idx: usize,
        col_idx: usize,
        text: &str,
    ) -> Result<()> {
        let tc = self.table_cell_mut(shape_idx, row_idx, col_idx)?;
        // Replace the txBody with a fresh one containing the text.
        tc.children
            .retain(|n| !matches!(n, Node::Element(e) if e.local_name() == b"txBody"));
        let mut tx_body = new_element(b"a:txBody");
        let mut body_pr = new_element(b"a:bodyPr");
        body_pr.self_closing = true;
        tx_body.children.push(Node::Element(body_pr));
        let mut lst_style = new_element(b"a:lstStyle");
        lst_style.self_closing = true;
        tx_body.children.push(Node::Element(lst_style));
        let p = make_simple_paragraph(text);
        tx_body.children.push(Node::Element(p));
        // Insert txBody before tcPr if tcPr is last, or just push.
        // In the schema, txBody comes before tcPr is wrong — actually tcPr comes
        // after txBody. The schema order is: txBody, then tcPr. But actually in
        // OOXML the order is: tcPr first, then txBody. Let's check: in the ECMA
        // spec, CT_TableCell = (tcPr?, txBody). So tcPr comes first.
        // We'll insert txBody at the end (after any tcPr).
        tc.children.push(Node::Element(tx_body));
        Ok(())
    }

    /// Set the paragraph alignment in the cell at (`row_idx`, `col_idx`) in the
    /// table at `shape_idx`. Sets the `algn` attribute on the first paragraph's
    /// `<a:pPr>` (creates pPr if absent). (Req 6.4)
    pub fn set_cell_alignment(
        &mut self,
        shape_idx: usize,
        row_idx: usize,
        col_idx: usize,
        algn: &str,
    ) -> Result<()> {
        let tc = self.table_cell_mut(shape_idx, row_idx, col_idx)?;
        let tx = find_descendant_mut(tc, b"txBody")
            .ok_or_else(|| OxmlError::Parse("table cell has no txBody".into()))?;
        // Find or create the first paragraph's pPr.
        let para = first_child_named_mut(tx, b"p")
            .ok_or_else(|| OxmlError::Parse("cell txBody has no paragraph".into()))?;
        let has_ppr = para
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"pPr"));
        if !has_ppr {
            let mut ppr = new_element(b"a:pPr");
            ppr.self_closing = true;
            para.children.insert(0, Node::Element(ppr));
        }
        let ppr = para
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"pPr" => Some(e),
                _ => None,
            })
            .unwrap();
        ppr.set_attr(b"algn", algn.as_bytes());
        Ok(())
    }

    /// Set the fill on the cell at (`row_idx`, `col_idx`) in the table at
    /// `shape_idx`. Sets the fill on the cell's `<a:tcPr>` element (creates
    /// tcPr if absent). (Req 6.4)
    pub fn set_cell_fill(
        &mut self,
        shape_idx: usize,
        row_idx: usize,
        col_idx: usize,
        fill: &FillSpec,
    ) -> Result<()> {
        let tc = self.table_cell_mut(shape_idx, row_idx, col_idx)?;
        let tcpr = ensure_tc_pr(tc);
        // Remove any existing fill element from tcPr.
        tcpr.children
            .retain(|n| !matches!(n, Node::Element(e) if is_fill(e.local_name())));
        let fill_node = build_fill_element(fill);
        if tcpr.self_closing {
            tcpr.self_closing = false;
            tcpr.dirty = true;
        }
        tcpr.children.push(fill_node);
        Ok(())
    }

    /// Set margin attributes (`marL`, `marT`, `marR`, `marB`) on the cell at
    /// (`row_idx`, `col_idx`) in the table at `shape_idx`. Values are in EMU.
    /// Creates `<a:tcPr>` if absent. (Req 6.4)
    #[allow(clippy::too_many_arguments)]
    pub fn set_cell_margins(
        &mut self,
        shape_idx: usize,
        row_idx: usize,
        col_idx: usize,
        left: i64,
        top: i64,
        right: i64,
        bottom: i64,
    ) -> Result<()> {
        let tc = self.table_cell_mut(shape_idx, row_idx, col_idx)?;
        let tcpr = ensure_tc_pr(tc);
        tcpr.set_attr(b"marL", left.to_string().as_bytes());
        tcpr.set_attr(b"marT", top.to_string().as_bytes());
        tcpr.set_attr(b"marR", right.to_string().as_bytes());
        tcpr.set_attr(b"marB", bottom.to_string().as_bytes());
        Ok(())
    }

    /// Return a mutable reference to the `<a:tc>` element at (`row_idx`,
    /// `col_idx`) in the table at `shape_idx`.
    fn table_cell_mut(
        &mut self,
        shape_idx: usize,
        row_idx: usize,
        col_idx: usize,
    ) -> Result<&mut Element> {
        let tbl = self.table_element_mut(shape_idx)?;
        let tr_pos = nth_tr_child_position(tbl, row_idx)?;
        let tr = match &mut tbl.children[tr_pos] {
            Node::Element(e) => e,
            _ => unreachable!(),
        };
        let tc_pos = nth_child_position_by_name(tr, b"tc", col_idx)?;
        let tc = match &mut tr.children[tc_pos] {
            Node::Element(e) => e,
            _ => unreachable!(),
        };
        Ok(tc)
    }

    // ─── Image crop (Req 10.1) ────────────────────────────────────────────

    /// Set image crop on the picture shape at `shape_idx` via `<a:srcRect>`.
    ///
    /// Values are in thousandths of a percent (e.g. 25000 = 25%). The method
    /// locates the `<p:pic>` at `shape_idx`, finds or creates `<a:srcRect>`
    /// inside its `<p:blipFill>` (or `<a:blipFill>`), and sets the `l`, `t`,
    /// `r`, `b` attributes. Returns an error if the shape is not a picture.
    /// Sibling shapes are preserved byte-for-byte (Req 10.1).
    pub fn set_image_crop(
        &mut self,
        shape_idx: usize,
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
    ) -> Result<()> {
        let sp = self.shape_element_mut(shape_idx)?;

        // Verify this is a picture shape.
        if sp.local_name() != b"pic" {
            return Err(OxmlError::Parse(format!(
                "shape at index {shape_idx} is not a picture (is {:?})",
                String::from_utf8_lossy(sp.local_name())
            )));
        }

        // Find the blipFill element (could be `p:blipFill` or `a:blipFill`).
        let blip_fill = sp
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"blipFill" => Some(e),
                _ => None,
            })
            .ok_or_else(|| {
                OxmlError::Parse(format!("picture at index {shape_idx} has no blipFill"))
            })?;

        // Find or create <a:srcRect> inside blipFill.
        let has_src_rect = blip_fill
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"srcRect"));

        if !has_src_rect {
            let mut src_rect = new_element(b"a:srcRect");
            src_rect.self_closing = true;
            // srcRect comes after <a:blip> and before <a:stretch>/<a:tile> in
            // the CT_BlipFillProperties schema sequence.
            // Find the position after <a:blip> (if present).
            let insert_pos = blip_fill
                .children
                .iter()
                .position(|n| matches!(n, Node::Element(e) if e.local_name() == b"blip"))
                .map(|i| i + 1)
                .unwrap_or(0);
            blip_fill
                .children
                .insert(insert_pos, Node::Element(src_rect));
        }

        // Get mutable reference to srcRect and set attributes.
        let src_rect = blip_fill
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"srcRect" => Some(e),
                _ => None,
            })
            .unwrap();

        src_rect.set_attr(b"l", left.to_string().as_bytes());
        src_rect.set_attr(b"t", top.to_string().as_bytes());
        src_rect.set_attr(b"r", right.to_string().as_bytes());
        src_rect.set_attr(b"b", bottom.to_string().as_bytes());

        Ok(())
    }

    // ─── Image insert (Req 10.3) ──────────────────────────────────────────

    /// Insert a `<p:pic>` element into the `spTree` with the given geometry and
    /// relationship ID. Returns the new shape id. This is the DOM-level operation;
    /// the caller handles the media part and relationship. Sibling shapes are
    /// preserved byte-for-byte.
    pub fn insert_picture(
        &mut self,
        x: i64,
        y: i64,
        cx: i64,
        cy: i64,
        r_id: &str,
        name: &str,
    ) -> Result<u32> {
        let id = self.next_shape_id();
        let pic_xml = format!(
            "<p:pic>\
             <p:nvPicPr><p:cNvPr id=\"{id}\" name=\"{name}\"/>\
             <p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr>\
             <p:nvPr/></p:nvPicPr>\
             <p:blipFill><a:blip r:embed=\"{r_id}\"/>\
             <a:stretch><a:fillRect/></a:stretch></p:blipFill>\
             <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/>\
             <a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
            id = id,
            name = crate::xml::escape_public(name),
            r_id = r_id,
            x = x,
            y = y,
            cx = cx,
            cy = cy,
        );
        let doc = Document::parse(pic_xml.as_bytes())?;
        let pic_node = doc
            .nodes
            .into_iter()
            .find(|n| matches!(n, Node::Element(_)))
            .ok_or_else(|| OxmlError::Parse("authored p:pic did not parse".into()))?;
        let tree = self
            .sp_tree_mut()
            .ok_or_else(|| OxmlError::Parse("slide has no spTree".into()))?;
        tree.children.push(pic_node);
        Ok(id)
    }

    // ─── Run hyperlink (Req 11.1, 11.3) ──────────────────────────────────

    /// Set a run-level hyperlink to an external URL on the run at
    /// (`shape_idx`, `para_idx`, `run_idx`).
    ///
    /// This inserts `<a:hlinkClick r:id="{r_id}"/>` into the run's `<a:rPr>`,
    /// using `insert_child_ordered` for schema-valid positioning (hlinkClick
    /// comes after fonts in the CT_TextCharacterProperties sequence). Any
    /// existing `<a:hlinkClick>` is removed first.
    ///
    /// The caller is responsible for adding the external relationship to the
    /// slide's `.rels` file (the `r_id` maps to the URL). Sibling runs are
    /// preserved byte-for-byte (Req 11.3).
    pub fn set_run_hyperlink(
        &mut self,
        shape_idx: usize,
        para_idx: usize,
        run_idx: usize,
        url: &str,
        r_id: &str,
    ) -> Result<()> {
        let _ = url; // URL is for documentation; the r_id is what goes into the XML.
        let tx = self.text_body_by_index_mut(shape_idx)?;
        let p_pos = nth_p_child_position(tx, para_idx)?;
        let para = match &mut tx.children[p_pos] {
            Node::Element(e) if e.local_name() == b"p" => e,
            _ => {
                return Err(OxmlError::Parse(
                    "child at position is not a paragraph".to_string(),
                ));
            }
        };
        let pos = nth_run_child_position(para, run_idx)?;
        let run = match &mut para.children[pos] {
            Node::Element(e) if e.local_name() == b"r" => e,
            _ => {
                return Err(OxmlError::Parse(format!(
                    "element at run index {run_idx} is not an <a:r> run"
                )));
            }
        };

        // Find or create <a:rPr> as the first child of the run.
        let has_rpr = run
            .children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"rPr"));
        if !has_rpr {
            let mut rpr = new_element(b"a:rPr");
            rpr.self_closing = true;
            run.children.insert(0, Node::Element(rpr));
        }

        let rpr = run
            .children
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) if e.local_name() == b"rPr" => Some(e),
                _ => None,
            })
            .unwrap();

        // Remove any existing hlinkClick.
        rpr.children
            .retain(|n| !matches!(n, Node::Element(e) if e.local_name() == b"hlinkClick"));

        // Build the new <a:hlinkClick r:id="..."/> element.
        let mut hlink = new_element(b"a:hlinkClick");
        hlink.self_closing = true;
        hlink.set_attr(b"r:id", r_id.as_bytes());

        // If rPr was self-closing, open it up.
        if rpr.self_closing {
            rpr.self_closing = false;
            rpr.dirty = true;
        }

        // Insert in schema-valid order (hlinkClick is rank 18 in RPR_ORDER).
        rpr.insert_child_ordered(Node::Element(hlink));

        Ok(())
    }

    // ─── Shape click action (Req 11.2, 11.3) ─────────────────────────────

    /// Set a shape-level click action on the shape at `shape_idx`.
    ///
    /// This inserts `<a:hlinkClick r:id="..." [action="..."]/>` into the shape's
    /// `<p:cNvPr>` (or `<a:cNvPr>`). Any existing `<a:hlinkClick>` is removed
    /// first.
    ///
    /// For `ClickAction::ExternalUrl`: emits `<a:hlinkClick r:id="{r_id}"/>`.
    /// For `ClickAction::JumpToSlide`: emits `<a:hlinkClick r:id="{r_id}" action="{action}"/>`.
    ///
    /// The caller is responsible for adding the appropriate relationship to the
    /// slide's `.rels` file. Sibling shapes are preserved byte-for-byte (Req 11.3).
    pub fn set_shape_click_action(&mut self, shape_idx: usize, action: &ClickAction) -> Result<()> {
        let sp = self.shape_element_mut(shape_idx)?;

        // Find the cNvPr element (inside nvSpPr/nvPicPr/nvCxnSpPr/nvGrpSpPr/nvGraphicFramePr).
        let cnvpr = find_descendant_mut(sp, b"cNvPr").ok_or_else(|| {
            OxmlError::Parse(format!("shape at index {shape_idx} has no cNvPr element"))
        })?;

        // Remove any existing <a:hlinkClick> from cNvPr.
        cnvpr
            .children
            .retain(|n| !matches!(n, Node::Element(e) if e.local_name() == b"hlinkClick"));

        // Build the new <a:hlinkClick .../> element.
        let mut hlink = new_element(b"a:hlinkClick");
        hlink.self_closing = true;
        match action {
            ClickAction::ExternalUrl { r_id } => {
                hlink.set_attr(b"r:id", r_id.as_bytes());
            }
            ClickAction::JumpToSlide { r_id, action: act } => {
                hlink.set_attr(b"r:id", r_id.as_bytes());
                hlink.set_attr(b"action", act.as_bytes());
            }
        }

        // If cNvPr was self-closing, open it up to accept children.
        if cnvpr.self_closing {
            cnvpr.self_closing = false;
            cnvpr.dirty = true;
        }

        cnvpr.children.push(Node::Element(hlink));

        Ok(())
    }

    // ─── Footer / slide-number / date placeholders (Req 13.2) ────────────

    /// Find a placeholder shape by its `ph@type` attribute value.
    /// Returns a mutable reference to the shape element.
    fn find_ph_by_type_mut(&mut self, ph_type: &str) -> Option<&mut Element> {
        let tree = self.sp_tree_mut()?;
        for child in &mut tree.children {
            if let Node::Element(sp) = child {
                if sp.local_name() != b"sp" {
                    continue;
                }
                if let Some(ph) = sp.find_descendant(b"ph")
                    && let Some(t) = ph.attr(b"type")
                    && t == ph_type.as_bytes()
                {
                    return Some(sp);
                }
            }
        }
        None
    }

    /// Set the text of the footer placeholder (`ph@type="ftr"`).
    ///
    /// Locates the footer placeholder shape and sets its text body content.
    /// Returns an error if no footer placeholder exists on this slide.
    /// Sibling shapes remain byte-for-byte identical.
    pub fn set_footer_text(&mut self, text: &str) -> Result<()> {
        let sp = self.find_ph_by_type_mut("ftr").ok_or_else(|| {
            OxmlError::Parse("slide has no footer placeholder (ph@type=\"ftr\")".into())
        })?;
        set_placeholder_text(sp, text)
    }

    /// Set the text of the slide-number placeholder (`ph@type="sldNum"`).
    ///
    /// Locates the slide-number placeholder shape and sets its text body content.
    /// Returns an error if no slide-number placeholder exists on this slide.
    /// Sibling shapes remain byte-for-byte identical.
    pub fn set_slide_number_text(&mut self, text: &str) -> Result<()> {
        let sp = self.find_ph_by_type_mut("sldNum").ok_or_else(|| {
            OxmlError::Parse("slide has no slide-number placeholder (ph@type=\"sldNum\")".into())
        })?;
        set_placeholder_text(sp, text)
    }

    /// Show or hide the slide-number placeholder (`ph@type="sldNum"`).
    ///
    /// When `visible` is false, sets the shape's `<p:spPr>` visibility attribute
    /// (or adds a `<p:sp>` visibility marker). When true, removes any visibility
    /// override. Returns an error if no slide-number placeholder exists.
    pub fn set_slide_number_visible(&mut self, visible: bool) -> Result<()> {
        let sp = self.find_ph_by_type_mut("sldNum").ok_or_else(|| {
            OxmlError::Parse("slide has no slide-number placeholder (ph@type=\"sldNum\")".into())
        })?;
        set_shape_visibility(sp, visible)
    }

    /// Set the text of the date placeholder (`ph@type="dt"`).
    ///
    /// Locates the date placeholder shape and sets its text body content.
    /// Returns an error if no date placeholder exists on this slide.
    /// Sibling shapes remain byte-for-byte identical.
    pub fn set_date_text(&mut self, text: &str) -> Result<()> {
        let sp = self.find_ph_by_type_mut("dt").ok_or_else(|| {
            OxmlError::Parse("slide has no date placeholder (ph@type=\"dt\")".into())
        })?;
        set_placeholder_text(sp, text)
    }

    /// Show or hide the footer placeholder (`ph@type="ftr"`).
    pub fn set_footer_visible(&mut self, visible: bool) -> Result<()> {
        let sp = self.find_ph_by_type_mut("ftr").ok_or_else(|| {
            OxmlError::Parse("slide has no footer placeholder (ph@type=\"ftr\")".into())
        })?;
        set_shape_visibility(sp, visible)
    }

    /// Show or hide the date placeholder (`ph@type="dt"`).
    pub fn set_date_visible(&mut self, visible: bool) -> Result<()> {
        let sp = self.find_ph_by_type_mut("dt").ok_or_else(|| {
            OxmlError::Parse("slide has no date placeholder (ph@type=\"dt\")".into())
        })?;
        set_shape_visibility(sp, visible)
    }
}

/// Set the text of a placeholder shape's txBody. Creates a txBody with a single
/// paragraph if none exists. Preserves the shape's other properties.
fn set_placeholder_text(sp: &mut Element, text: &str) -> Result<()> {
    // Try to find existing txBody.
    if let Some(tx) = find_descendant_mut(sp, b"txBody") {
        // Capture the first run's rPr for formatting preservation.
        let rpr_tmpl: Option<Element> = tx
            .children_named(b"p")
            .flat_map(|p| p.children_named(b"r"))
            .next()
            .and_then(|r| r.children_named(b"rPr").next().cloned());

        // Replace all paragraphs with a single one containing the new text.
        tx.children.retain(|n| match n {
            Node::Element(e) => e.local_name() != b"p",
            Node::Raw(_) => true,
        });
        let mut p = new_element(b"a:p");
        p.children.push(Node::Element(make_run(text, rpr_tmpl)));
        tx.children.push(Node::Element(p));
    } else {
        // No txBody exists — create one with bodyPr + lstStyle + paragraph.
        let mut tx = new_element(b"p:txBody");
        let mut body_pr = new_element(b"a:bodyPr");
        body_pr.self_closing = true;
        tx.children.push(Node::Element(body_pr));
        let mut lst = new_element(b"a:lstStyle");
        lst.self_closing = true;
        tx.children.push(Node::Element(lst));
        let mut p = new_element(b"a:p");
        p.children.push(Node::Element(make_run(text, None)));
        tx.children.push(Node::Element(p));
        sp.children.push(Node::Element(tx));
        if sp.self_closing {
            sp.self_closing = false;
            sp.dirty = true;
        }
    }
    Ok(())
}

/// Set shape visibility via the `<p:cNvPr>` `hidden` attribute.
/// When `visible` is false, sets `hidden="1"` on the cNvPr. When true, removes it.
fn set_shape_visibility(sp: &mut Element, visible: bool) -> Result<()> {
    let cnvpr = find_descendant_mut(sp, b"cNvPr")
        .ok_or_else(|| OxmlError::Parse("shape has no cNvPr element".into()))?;
    if visible {
        // Remove the hidden attribute if present.
        cnvpr.attrs.retain(|(k, _)| k != b"hidden");
        cnvpr.dirty = true;
    } else {
        cnvpr.set_attr(b"hidden", b"1");
    }
    Ok(())
}

/// Character formatting to apply to runs. `None` fields are left unchanged.
#[derive(Debug, Clone, Default)]
pub struct RunFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub size_pt: Option<f64>,
    /// Solid text color as a 6-hex-digit RRGGBB string (no `#`).
    pub color: Option<String>,
    /// Latin typeface name.
    pub font: Option<String>,
    /// Strikethrough style: "sngStrike", "dblStrike", or "noStrike".
    pub strikethrough: Option<String>,
    /// Baseline offset in thousandths of a percent. Positive = superscript (e.g.
    /// 30000), negative = subscript (e.g. -25000).
    pub baseline: Option<i32>,
    /// Language tag (e.g. "en-US", "fr-FR").
    pub lang: Option<String>,
    /// Underline style enum value (e.g. "sng", "dbl", "heavy", "dotted", "dash",
    /// "wavy", "none", etc.). When set, takes precedence over the boolean
    /// `underline` field.
    pub underline_style: Option<String>,
    /// Theme color reference. Emits `<a:solidFill><a:schemeClr val="..."/></a:solidFill>`
    /// instead of `<a:srgbClr>`, so the color tracks theme changes (Req 2.4).
    pub theme_color: Option<SchemeColor>,
}

/// Apply formatting to a single `<a:r>` run: find or create its `<a:rPr>` (which
/// the schema requires as the first child) and set only the requested attrs.
fn apply_run_format(run: &mut Element, fmt: &RunFormat) {
    let rpr_idx = run
        .children
        .iter()
        .position(|n| matches!(n, Node::Element(e) if e.local_name() == b"rPr"));
    let idx = match rpr_idx {
        Some(i) => i,
        None => {
            let mut rpr = new_element(b"a:rPr");
            rpr.self_closing = true;
            run.children.insert(0, Node::Element(rpr));
            0
        }
    };
    if let Node::Element(rpr) = &mut run.children[idx] {
        if let Some(b) = fmt.bold {
            rpr.set_attr(b"b", if b { b"1" } else { b"0" });
        }
        if let Some(i) = fmt.italic {
            rpr.set_attr(b"i", if i { b"1" } else { b"0" });
        }
        // underline_style takes precedence over the boolean underline field.
        if let Some(style) = &fmt.underline_style {
            rpr.set_attr(b"u", style.as_bytes());
        } else if let Some(u) = fmt.underline {
            rpr.set_attr(b"u", if u { b"sng" } else { b"none" });
        }
        if let Some(sz) = fmt.size_pt {
            rpr.set_attr(b"sz", ((sz * 100.0) as i64).to_string().as_bytes());
        }
        if let Some(strike) = &fmt.strikethrough {
            rpr.set_attr(b"strike", strike.as_bytes());
        }
        if let Some(bl) = fmt.baseline {
            rpr.set_attr(b"baseline", bl.to_string().as_bytes());
        }
        if let Some(lang) = &fmt.lang {
            rpr.set_attr(b"lang", lang.as_bytes());
        }
        // Theme color takes precedence over RGB color (Req 2.4).
        if let Some(sc) = &fmt.theme_color {
            set_rpr_theme_color(rpr, *sc);
        } else if let Some(hex) = &fmt.color {
            set_rpr_color(rpr, hex);
        }
        if let Some(font) = &fmt.font {
            set_rpr_font(rpr, font);
        }
        // A self-closing rPr that gained children must become an open/close pair.
        if rpr.self_closing && !rpr.children.is_empty() {
            rpr.self_closing = false;
            rpr.dirty = true;
        }
    }
}

/// Upsert `<a:solidFill><a:srgbClr val=.../></a:solidFill>` in an rPr. Placed
/// after a leading `<a:ln>` if present (schema fill order), replacing any
/// existing fill element.
fn set_rpr_color(rpr: &mut Element, hex: &str) {
    let val = hex.trim_start_matches('#').to_uppercase();
    let fill_xml = format!("<a:solidFill><a:srgbClr val=\"{val}\"/></a:solidFill>");
    let fill = parse_fragment(&fill_xml);
    // Remove any existing fill element (solidFill/noFill/gradFill/...).
    rpr.children
        .retain(|n| !matches!(n, Node::Element(e) if is_fill(e.local_name())));
    let pos = rpr
        .children
        .iter()
        .position(|n| matches!(n, Node::Element(e) if e.local_name() == b"ln"))
        .map(|i| i + 1)
        .unwrap_or(0);
    rpr.children.insert(pos, fill);
}

/// Upsert `<a:solidFill><a:schemeClr val="..."/></a:solidFill>` in an rPr.
/// This emits a theme-color reference that tracks theme changes (Req 2.4),
/// rather than baking an RGB value.
fn set_rpr_theme_color(rpr: &mut Element, sc: SchemeColor) {
    let fill_xml = format!(
        "<a:solidFill><a:schemeClr val=\"{}\"/></a:solidFill>",
        sc.val()
    );
    let fill = parse_fragment(&fill_xml);
    // Remove any existing fill element (solidFill/noFill/gradFill/...).
    rpr.children
        .retain(|n| !matches!(n, Node::Element(e) if is_fill(e.local_name())));
    let pos = rpr
        .children
        .iter()
        .position(|n| matches!(n, Node::Element(e) if e.local_name() == b"ln"))
        .map(|i| i + 1)
        .unwrap_or(0);
    rpr.children.insert(pos, fill);
}

/// Upsert `<a:latin typeface=.../>` in an rPr (fonts follow fills/effects in the
/// schema), replacing any existing `<a:latin>`.
fn set_rpr_font(rpr: &mut Element, font: &str) {
    let latin = parse_fragment(&format!("<a:latin typeface=\"{}\"/>", escape_attr(font)));
    rpr.children
        .retain(|n| !matches!(n, Node::Element(e) if e.local_name() == b"latin"));
    rpr.children.push(latin);
}

fn is_fill(local: &[u8]) -> bool {
    matches!(
        local,
        b"noFill" | b"solidFill" | b"gradFill" | b"blipFill" | b"pattFill" | b"grpFill"
    )
}

/// Build the XML fragment for a color spec: either `<a:srgbClr val="..."/>`
/// or `<a:schemeClr val="..."/>`.
fn build_color_element(color: &ColorSpec) -> Node {
    match color {
        ColorSpec::Rgb(hex) => {
            let val = hex.trim_start_matches('#').to_uppercase();
            parse_fragment(&format!("<a:srgbClr val=\"{val}\"/>"))
        }
        ColorSpec::Theme(sc) => parse_fragment(&format!("<a:schemeClr val=\"{}\"/>", sc.val())),
    }
}

/// Build the fill element node for a `FillSpec`.
fn build_fill_element(fill: &FillSpec) -> Node {
    match fill {
        FillSpec::None => {
            let mut el = new_element(b"a:noFill");
            el.self_closing = true;
            Node::Element(el)
        }
        FillSpec::Solid { color } => {
            let mut el = new_element(b"a:solidFill");
            el.children.push(build_color_element(color));
            Node::Element(el)
        }
        FillSpec::Gradient { stops, angle_deg } => {
            // <a:gradFill><a:gsLst>..stops..</a:gsLst><a:lin ang="..." scaled="0"/></a:gradFill>
            let mut grad = new_element(b"a:gradFill");

            let mut gs_lst = new_element(b"a:gsLst");
            for (pos, color) in stops {
                // Position is in thousandths of a percent (0–100000).
                let pos_val = (*pos * 100_000.0) as u64;
                let mut gs = new_element(b"a:gs");
                gs.set_attr(b"pos", pos_val.to_string().as_bytes());
                gs.children.push(build_color_element(color));
                gs_lst.children.push(Node::Element(gs));
            }
            grad.children.push(Node::Element(gs_lst));

            // Linear direction: angle in 60,000ths of a degree.
            let ang_val = (*angle_deg * 60_000.0) as i64;
            let mut lin = new_element(b"a:lin");
            lin.self_closing = true;
            lin.set_attr(b"ang", ang_val.to_string().as_bytes());
            lin.set_attr(b"scaled", b"0");
            grad.children.push(Node::Element(lin));

            Node::Element(grad)
        }
        FillSpec::Pattern { preset, fg, bg } => {
            // <a:pattFill prst="..."><a:fgClr>...</a:fgClr><a:bgClr>...</a:bgClr></a:pattFill>
            let mut patt = new_element(b"a:pattFill");
            patt.set_attr(b"prst", preset.as_bytes());

            let mut fg_el = new_element(b"a:fgClr");
            fg_el.children.push(build_color_element(fg));
            patt.children.push(Node::Element(fg_el));

            let mut bg_el = new_element(b"a:bgClr");
            bg_el.children.push(build_color_element(bg));
            patt.children.push(Node::Element(bg_el));

            Node::Element(patt)
        }
        FillSpec::Picture { r_id } => {
            // <a:blipFill><a:blip r:embed="..."/><a:stretch><a:fillRect/></a:stretch></a:blipFill>
            let xml = format!(
                "<a:blipFill><a:blip r:embed=\"{}\"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>",
                escape_attr(r_id)
            );
            parse_fragment(&xml)
        }
    }
}

/// Build the `<a:ln>` element node for a `LineSpec`.
fn build_line_element(line: &LineSpec) -> Node {
    match line {
        LineSpec::None => {
            // <a:ln><a:noFill/></a:ln>
            let mut ln = new_element(b"a:ln");
            let mut no_fill = new_element(b"a:noFill");
            no_fill.self_closing = true;
            ln.children.push(Node::Element(no_fill));
            Node::Element(ln)
        }
        LineSpec::Styled {
            color,
            width_emu,
            dash,
        } => {
            // <a:ln w="..."><a:solidFill>...</a:solidFill><a:prstDash val="..."/></a:ln>
            let mut ln = new_element(b"a:ln");
            ln.set_attr(b"w", width_emu.to_string().as_bytes());

            // Solid fill with the color.
            let mut solid_fill = new_element(b"a:solidFill");
            solid_fill.children.push(build_color_element(color));
            ln.children.push(Node::Element(solid_fill));

            // Optional dash style.
            if let Some(dash_val) = dash {
                let mut prst_dash = new_element(b"a:prstDash");
                prst_dash.self_closing = true;
                prst_dash.set_attr(b"val", dash_val.as_bytes());
                ln.children.push(Node::Element(prst_dash));
            }

            Node::Element(ln)
        }
    }
}

/// Parse a single-element XML fragment into a DOM node.
fn parse_fragment(xml: &str) -> Node {
    Document::parse(xml.as_bytes())
        .ok()
        .and_then(|d| d.nodes.into_iter().find(|n| matches!(n, Node::Element(_))))
        .expect("internal fragment must parse")
}

/// Minimal attribute-value escape for authored typeface names.
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// Build an `<a:p>` with optional pPr template (level applied) and one run.
fn make_paragraph(
    text: &str,
    level: u8,
    ppr_tmpl: &Option<Element>,
    rpr_tmpl: &Option<Element>,
) -> Element {
    let mut p = new_element(b"a:p");
    // Paragraph properties: clone the template (preserving bullet/indent styling)
    // and set the outline level; omit pPr entirely for a clean level-0 paragraph
    // when there is no template.
    if let Some(tmpl) = ppr_tmpl {
        let mut ppr = tmpl.clone();
        if level > 0 {
            ppr.set_attr(b"lvl", level.to_string().as_bytes());
        } else {
            ppr.attrs.retain(|(k, _)| k != b"lvl");
            ppr.dirty = true;
        }
        p.children.push(Node::Element(ppr));
    } else if level > 0 {
        let mut ppr = new_element(b"a:pPr");
        ppr.self_closing = true;
        ppr.set_attr(b"lvl", level.to_string().as_bytes());
        p.children.push(Node::Element(ppr));
    }
    p.children
        .push(Node::Element(make_run(text, rpr_tmpl.clone())));
    p
}

/// Replace a shape's text with a single run, reusing the first existing run's
/// `a:rPr` (formatting) when present so styling is preserved. Mutates the DOM in
/// place; all non-text nodes (spPr, nvSpPr, bodyPr, lstStyle) are untouched.
fn set_shape_text(sp: &mut Element, text: &str) -> Result<()> {
    let tx = find_descendant_mut(sp, b"txBody")
        .ok_or_else(|| OxmlError::Parse("placeholder has no txBody".into()))?;

    // Capture the first run's rPr (verbatim bytes) to preserve formatting.
    let preserved_rpr: Option<Element> = tx
        .children_named(b"p")
        .flat_map(|p| p.children_named(b"r"))
        .next()
        .and_then(|r| r.children_named(b"rPr").next().cloned());

    // Find the first paragraph; replace its runs with one formatted run.
    let first_p = first_child_named_mut(tx, b"p")
        .ok_or_else(|| OxmlError::Parse("txBody has no paragraph".into()))?;

    // Drop existing runs, keep paragraph properties (a:pPr) intact.
    first_p.children.retain(|n| match n {
        Node::Element(e) => e.local_name() != b"r",
        Node::Raw(_) => true,
    });
    first_p
        .children
        .push(Node::Element(make_run(text, preserved_rpr)));
    Ok(())
}

/// Build an `<a:r>` element with optional preserved `<a:rPr>` and the given text.
fn make_run(text: &str, rpr: Option<Element>) -> Element {
    let mut run = new_element(b"a:r");
    if let Some(rpr) = rpr {
        run.children.push(Node::Element(rpr));
    }
    let mut t = new_element(b"a:t");
    t.set_text_content(text);
    run.children.push(Node::Element(t));
    run
}

/// Construct a fresh (dirty) element with the given qualified name.
fn new_element(name: &[u8]) -> Element {
    Element {
        name: name.to_vec(),
        raw_start: Vec::new(),
        raw_end: Vec::new(),
        self_closing: false,
        dirty: true,
        attrs: Vec::new(),
        children: Vec::new(),
    }
}
/// Recursively track the maximum `p:cNvPr@id` in a subtree.
fn collect_max_cnvpr_id(el: &Element, max: &mut u32) {
    if el.local_name() == b"cNvPr"
        && let Some(id) = el
            .attr(b"id")
            .and_then(|v| std::str::from_utf8(v).ok()?.parse::<u32>().ok())
    {
        *max = (*max).max(id);
    }
    for c in &el.children {
        if let Node::Element(e) = c {
            collect_max_cnvpr_id(e, max);
        }
    }
}

/// Extract `ShapeInfo` from a shape element (`p:sp`, `p:pic`, etc.).
fn extract_shape_info(el: &Element) -> ShapeInfo {
    let shape_type = String::from_utf8_lossy(el.local_name()).into_owned();

    // Find cNvPr for id and name. It's inside nvSpPr/nvPicPr/nvCxnSpPr/nvGrpSpPr/nvGraphicFramePr.
    let cnvpr = el.find_descendant(b"cNvPr");
    let id = cnvpr
        .and_then(|c| c.attr(b"id"))
        .and_then(|v| std::str::from_utf8(v).ok()?.parse::<u32>().ok())
        .unwrap_or(0);
    let name = cnvpr
        .and_then(|c| c.attr(b"name"))
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_default();

    // Geometry from xfrm (inside spPr or grpSpPr).
    let geometry = el.find_descendant(b"xfrm").and_then(|xfrm| {
        let off = xfrm.children.iter().find_map(|n| match n {
            Node::Element(e) if e.local_name() == b"off" => Some(e),
            _ => None,
        })?;
        let ext = xfrm.children.iter().find_map(|n| match n {
            Node::Element(e) if e.local_name() == b"ext" => Some(e),
            _ => None,
        })?;
        let x = off
            .attr(b"x")
            .and_then(|v| std::str::from_utf8(v).ok()?.parse::<i64>().ok())?;
        let y = off
            .attr(b"y")
            .and_then(|v| std::str::from_utf8(v).ok()?.parse::<i64>().ok())?;
        let cx = ext
            .attr(b"cx")
            .and_then(|v| std::str::from_utf8(v).ok()?.parse::<i64>().ok())?;
        let cy = ext
            .attr(b"cy")
            .and_then(|v| std::str::from_utf8(v).ok()?.parse::<i64>().ok())?;
        Some((x, y, cx, cy))
    });

    // Text from txBody: concatenate all paragraphs' run text.
    let text = el.find_descendant(b"txBody").map(|tx| {
        let mut lines = Vec::new();
        for p in tx.children_named(b"p") {
            lines.push(paragraph_text(p));
        }
        lines.join("\n")
    });

    ShapeInfo {
        id,
        name,
        shape_type,
        geometry,
        text,
    }
}

/// Concatenated text of a paragraph's runs (`a:r > a:t`).
fn paragraph_text(p: &Element) -> String {
    let mut s = String::new();
    for r in p.children_named(b"r") {
        for t in r.children_named(b"t") {
            s.push_str(&t.text_content());
        }
    }
    s
}

/// Depth-first mutable search for the first descendant with `local` name.
fn find_descendant_mut<'a>(el: &'a mut Element, local: &[u8]) -> Option<&'a mut Element> {
    for child in el.children.iter_mut() {
        if let Node::Element(e) = child {
            if e.local_name() == local {
                return Some(e);
            }
            if let Some(found) = find_descendant_mut(e, local) {
                return Some(found);
            }
        }
    }
    None
}

/// First direct child element with `local` name (mutable).
fn first_child_named_mut<'a>(el: &'a mut Element, local: &[u8]) -> Option<&'a mut Element> {
    el.children.iter_mut().find_map(|n| match n {
        Node::Element(e) if e.local_name() == local => Some(e),
        _ => None,
    })
}

// ─── Paragraph helpers ──────────────────────────────────────────────────────

/// Build a simple `<a:p>` with one `<a:r><a:t>text</a:t></a:r>`.
fn make_simple_paragraph(text: &str) -> Element {
    let mut p = new_element(b"a:p");
    p.children.push(Node::Element(make_run(text, None)));
    p
}

/// Find the child-index of the Nth `<a:p>` element within a txBody.
/// Returns an error if `para_idx` is out of range.
fn nth_p_child_position(tx: &Element, para_idx: usize) -> Result<usize> {
    let mut count = 0;
    for (i, child) in tx.children.iter().enumerate() {
        if let Node::Element(e) = child
            && e.local_name() == b"p"
        {
            if count == para_idx {
                return Ok(i);
            }
            count += 1;
        }
    }
    Err(OxmlError::Parse(format!(
        "paragraph index {para_idx} out of range (have {count} paragraphs)"
    )))
}

/// Same as `nth_p_child_position` but takes a snapshot of positions. Used after
/// a removal to find the correct insertion point.
fn nth_p_child_position_snapshot(tx: &Element, para_idx: usize) -> Result<usize> {
    nth_p_child_position(tx, para_idx)
}

/// Find the child-index where the Nth `<a:p>` would be inserted. If `para_idx`
/// equals the paragraph count, returns the position after the last paragraph.
fn nth_p_child_position_for_insert(tx: &Element, para_idx: usize) -> Result<usize> {
    let mut count = 0;
    let mut last_p_end = tx.children.len(); // default: append at end
    for (i, child) in tx.children.iter().enumerate() {
        if let Node::Element(e) = child
            && e.local_name() == b"p"
        {
            if count == para_idx {
                return Ok(i);
            }
            count += 1;
            last_p_end = i + 1;
        }
    }
    // If para_idx == count, insert after the last paragraph.
    if para_idx == count {
        return Ok(last_p_end);
    }
    Err(OxmlError::Parse(format!(
        "paragraph insert index {para_idx} out of range (have {count} paragraphs)"
    )))
}

/// Find the child-index of the Nth `<a:r>` or `<a:br>` element within a paragraph.
/// Returns an error if `run_idx` is out of range.
fn nth_run_child_position(para: &Element, run_idx: usize) -> Result<usize> {
    let mut count = 0;
    for (i, child) in para.children.iter().enumerate() {
        if let Node::Element(e) = child
            && (e.local_name() == b"r" || e.local_name() == b"br")
        {
            if count == run_idx {
                return Ok(i);
            }
            count += 1;
        }
    }
    Err(OxmlError::Parse(format!(
        "run index {run_idx} out of range (have {count} runs/breaks)"
    )))
}

/// Set a spacing child element (`spcBef`, `spcAft`, or `lnSpc`) on a `<a:pPr>`.
/// Removes any existing element with the same name, then inserts in schema order.
fn set_spacing_child(ppr: &mut Element, local_name: &[u8], value: SpacingValue) {
    // Remove existing element with this name.
    ppr.children
        .retain(|n| !matches!(n, Node::Element(e) if e.local_name() == local_name));

    // Build the spacing element: <a:spcBef><a:spcPts val="..."/></a:spcBef>
    // or <a:spcBef><a:spcPct val="..."/></a:spcBef>
    let qualified_name = format!("a:{}", String::from_utf8_lossy(local_name));
    let mut wrapper = new_element(qualified_name.as_bytes());
    let (child_name, val) = match value {
        SpacingValue::Points(pts) => ("a:spcPts", pts),
        SpacingValue::Percent(pct) => ("a:spcPct", pct),
    };
    let mut child = new_element(child_name.as_bytes());
    child.self_closing = true;
    child.set_attr(b"val", val.to_string().as_bytes());
    wrapper.children.push(Node::Element(child));
    // A self-closing pPr that gains children must become open/close.
    if ppr.self_closing {
        ppr.self_closing = false;
        ppr.dirty = true;
    }
    ppr.insert_child_ordered(Node::Element(wrapper));
}

// ─── Geometry helpers ───────────────────────────────────────────────────────

/// Set or create `<a:off x="..." y="..."/>` as a child of `<a:xfrm>`.
/// If an `<a:off>` already exists, updates its attributes in place; otherwise
/// creates one and inserts it (off comes before ext in the schema).
fn set_or_create_off(xfrm: &mut Element, x: i64, y: i64) {
    if xfrm.self_closing {
        xfrm.self_closing = false;
        xfrm.dirty = true;
    }
    let existing = xfrm.children.iter_mut().find_map(|n| match n {
        Node::Element(e) if e.local_name() == b"off" => Some(e),
        _ => None,
    });
    if let Some(off) = existing {
        off.set_attr(b"x", x.to_string().as_bytes());
        off.set_attr(b"y", y.to_string().as_bytes());
    } else {
        let mut off = new_element(b"a:off");
        off.self_closing = true;
        off.set_attr(b"x", x.to_string().as_bytes());
        off.set_attr(b"y", y.to_string().as_bytes());
        // Insert at position 0 (off comes before ext in xfrm).
        xfrm.children.insert(0, Node::Element(off));
    }
}

/// Set or create `<a:ext cx="..." cy="..."/>` as a child of `<a:xfrm>`.
/// If an `<a:ext>` already exists, updates its attributes in place; otherwise
/// creates one and inserts it after `<a:off>` (if present).
fn set_or_create_ext(xfrm: &mut Element, cx: i64, cy: i64) {
    if xfrm.self_closing {
        xfrm.self_closing = false;
        xfrm.dirty = true;
    }
    let existing = xfrm.children.iter_mut().find_map(|n| match n {
        Node::Element(e) if e.local_name() == b"ext" => Some(e),
        _ => None,
    });
    if let Some(ext) = existing {
        ext.set_attr(b"cx", cx.to_string().as_bytes());
        ext.set_attr(b"cy", cy.to_string().as_bytes());
    } else {
        let mut ext = new_element(b"a:ext");
        ext.self_closing = true;
        ext.set_attr(b"cx", cx.to_string().as_bytes());
        ext.set_attr(b"cy", cy.to_string().as_bytes());
        // Insert after <a:off> if present, otherwise at position 0.
        let pos = xfrm
            .children
            .iter()
            .position(|n| matches!(n, Node::Element(e) if e.local_name() == b"off"))
            .map(|i| i + 1)
            .unwrap_or(0);
        xfrm.children.insert(pos, Node::Element(ext));
    }
}

// ─── Table helpers (Req 6.1) ────────────────────────────────────────────────

/// Count the number of `<a:gridCol>` elements in the table's `<a:tblGrid>`.
fn tbl_grid_col_count(tbl: &Element) -> usize {
    tbl.children
        .iter()
        .find_map(|n| match n {
            Node::Element(e) if e.local_name() == b"tblGrid" => Some(e),
            _ => None,
        })
        .map(|grid| grid.children_named(b"gridCol").count())
        .unwrap_or(0)
}

/// Build a new `<a:tr h="...">` element with `col_count` empty `<a:tc>` cells.
fn make_table_row(height_emu: i64, col_count: usize) -> Element {
    let mut tr = new_element(b"a:tr");
    tr.set_attr(b"h", height_emu.to_string().as_bytes());
    for _ in 0..col_count {
        tr.children.push(Node::Element(make_table_cell()));
    }
    tr
}

/// Build a minimal `<a:tc>` element with a `<a:txBody>` containing one empty
/// paragraph: `<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody></a:tc>`.
fn make_table_cell() -> Element {
    let mut tc = new_element(b"a:tc");

    let mut tx_body = new_element(b"a:txBody");

    let mut body_pr = new_element(b"a:bodyPr");
    body_pr.self_closing = true;
    tx_body.children.push(Node::Element(body_pr));

    let mut lst_style = new_element(b"a:lstStyle");
    lst_style.self_closing = true;
    tx_body.children.push(Node::Element(lst_style));

    let mut p = new_element(b"a:p");
    p.self_closing = true;
    tx_body.children.push(Node::Element(p));

    tc.children.push(Node::Element(tx_body));
    tc
}

/// Find the child-index of the Nth `<a:tr>` element within a `<a:tbl>`.
/// Returns an error if `row_idx` is out of range.
fn nth_tr_child_position(tbl: &Element, row_idx: usize) -> Result<usize> {
    let mut count = 0;
    for (i, child) in tbl.children.iter().enumerate() {
        if let Node::Element(e) = child
            && e.local_name() == b"tr"
        {
            if count == row_idx {
                return Ok(i);
            }
            count += 1;
        }
    }
    Err(OxmlError::Parse(format!(
        "row index {row_idx} out of range (have {count} rows)"
    )))
}

/// Find the child-index where the Nth `<a:tr>` would be inserted. If `row_idx`
/// equals the row count, returns the position after the last row (append).
fn nth_tr_child_position_for_insert(tbl: &Element, row_idx: usize) -> Result<usize> {
    let mut count = 0;
    let mut last_tr_end = tbl.children.len();
    for (i, child) in tbl.children.iter().enumerate() {
        if let Node::Element(e) = child
            && e.local_name() == b"tr"
        {
            if count == row_idx {
                return Ok(i);
            }
            count += 1;
            last_tr_end = i + 1;
        }
    }
    if row_idx == count {
        return Ok(last_tr_end);
    }
    Err(OxmlError::Parse(format!(
        "row insert index {row_idx} out of range (have {count} rows)"
    )))
}

/// Find the child-index of the Nth element with `local_name` within a parent.
/// Returns an error if the index is out of range.
fn nth_child_position_by_name(parent: &Element, local_name: &[u8], idx: usize) -> Result<usize> {
    let mut count = 0;
    for (i, child) in parent.children.iter().enumerate() {
        if let Node::Element(e) = child
            && e.local_name() == local_name
        {
            if count == idx {
                return Ok(i);
            }
            count += 1;
        }
    }
    Err(OxmlError::Parse(format!(
        "{} index {} out of range (have {} elements)",
        String::from_utf8_lossy(local_name),
        idx,
        count
    )))
}

/// Clear a table cell's text content, replacing its `<a:txBody>` with a minimal
/// empty one: `<a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody>`.
/// Used when merging cells to clear covered cells' content.
fn clear_cell_content(tc: &mut Element) {
    // Remove existing txBody.
    tc.children
        .retain(|n| !matches!(n, Node::Element(e) if e.local_name() == b"txBody"));
    // Add a fresh empty txBody.
    let mut tx_body = new_element(b"a:txBody");

    let mut body_pr = new_element(b"a:bodyPr");
    body_pr.self_closing = true;
    tx_body.children.push(Node::Element(body_pr));

    let mut lst_style = new_element(b"a:lstStyle");
    lst_style.self_closing = true;
    tx_body.children.push(Node::Element(lst_style));

    let mut p = new_element(b"a:p");
    p.self_closing = true;
    tx_body.children.push(Node::Element(p));

    tc.children.push(Node::Element(tx_body));
}

/// Ensure a table cell (`<a:tc>`) has a `<a:tcPr>` child element, creating one
/// if absent. Returns a mutable reference to the `<a:tcPr>`. In the ECMA-376
/// schema, `tcPr` is the first child of `tc` (before `txBody`).
fn ensure_tc_pr(tc: &mut Element) -> &mut Element {
    let has_tcpr = tc
        .children
        .iter()
        .any(|n| matches!(n, Node::Element(e) if e.local_name() == b"tcPr"));
    if !has_tcpr {
        let mut tcpr = new_element(b"a:tcPr");
        tcpr.self_closing = true;
        // tcPr comes first in the schema (before txBody).
        tc.children.insert(0, Node::Element(tcpr));
    }
    tc.children
        .iter_mut()
        .find_map(|n| match n {
            Node::Element(e) if e.local_name() == b"tcPr" => Some(e),
            _ => None,
        })
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A realistic title+body slide (as PowerPoint writes it).
    const SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" b="1"/><a:t>Old Title</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Body 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Bullet one</a:t></a:r></a:p><a:p><a:r><a:t>Bullet two</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn unedited_slide_round_trips_byte_for_byte() {
        let dom = SlideDom::parse(SLIDE).unwrap();
        assert_eq!(dom.to_bytes(), SLIDE);
    }

    #[test]
    fn reads_text_and_counts() {
        let dom = SlideDom::parse(SLIDE).unwrap();
        assert_eq!(dom.text(), "Old Title\nBullet one\nBullet two");
        assert_eq!(dom.body_paragraph_count(), 2);
    }

    #[test]
    fn set_title_is_surgical() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_title("New Title").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // New text present; old gone.
        assert!(s.contains("<a:t>New Title</a:t>"), "{s}");
        assert!(!s.contains("Old Title"));
        // Preserved the title run's formatting (b="1") and the whole body shape.
        assert!(
            s.contains(r#"<a:rPr lang="en-US" b="1"/>"#),
            "rPr preserved: {s}"
        );
        assert!(s.contains("<a:t>Bullet one</a:t>"), "body untouched: {s}");
        assert!(s.contains("<a:t>Bullet two</a:t>"));
        // Title shape's spPr/placeholder binding intact.
        assert!(s.contains(r#"<p:ph type="title"/>"#));
    }

    #[test]
    fn set_body_bullets_preserves_title_and_formatting() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_body_bullets(&[("New A".into(), 0), ("New B".into(), 1)])
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // New bullets present; old body text gone.
        assert!(s.contains("<a:t>New A</a:t>"), "{s}");
        assert!(s.contains("<a:t>New B</a:t>"));
        assert!(!s.contains("Bullet one"));
        assert!(!s.contains("Bullet two"));
        // Level applied on the nested bullet.
        assert!(s.contains(r#"lvl="1""#), "level set: {s}");
        // Title shape untouched (text + formatting preserved).
        assert!(s.contains("<a:t>Old Title</a:t>"), "title preserved: {s}");
        assert!(s.contains(r#"<a:rPr lang="en-US" b="1"/>"#));
    }

    #[test]
    fn format_placeholder_mutates_existing_rpr_in_place() {
        // Title run already has b="1"; bolding off + italic on must mutate that
        // rPr, not duplicate it, and leave the body shape byte-identical.
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.format_placeholder(
            "title",
            &RunFormat {
                bold: Some(false),
                italic: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // The single title rPr now carries b="0" i="1" (order: existing b first).
        assert!(
            s.contains(r#"<a:rPr lang="en-US" b="0" i="1"/>"#),
            "title rPr mutated: {s}"
        );
        // Body shape untouched.
        assert!(
            s.contains("<a:r><a:t>Bullet one</a:t></a:r>"),
            "body verbatim: {s}"
        );
    }

    #[test]
    fn format_placeholder_creates_rpr_when_missing() {
        // Body runs have no rPr; setting size must insert one as the first child
        // of each run (before a:t), preserving the text.
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.format_placeholder(
            "body",
            &RunFormat {
                size_pt: Some(20.0),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:r><a:rPr sz="2000"/><a:t>Bullet one</a:t></a:r>"#),
            "rPr inserted: {s}"
        );
        assert!(s.contains(r#"<a:r><a:rPr sz="2000"/><a:t>Bullet two</a:t></a:r>"#));
        // Title shape untouched.
        assert!(
            s.contains(r#"<a:rPr lang="en-US" b="1"/>"#),
            "title verbatim: {s}"
        );
    }

    #[test]
    fn add_text_box_appends_with_unique_id_preserving_existing() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let id = dom.add_text_box("Hello box", 100, 200, 300, 400).unwrap();
        // Existing shapes use ids 2 and 3 → new id is 4.
        assert_eq!(id, 4);
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"<p:cNvPr id="4" name="TextBox 4"/>"#), "{s}");
        assert!(s.contains("<a:t>Hello box</a:t>"));
        assert!(s.contains(r#"txBox="1""#));
        // Both original shapes preserved verbatim.
        assert!(s.contains("<a:t>Old Title</a:t>"));
        assert!(s.contains("<a:t>Bullet one</a:t>"));
        // New shape sits inside the spTree.
        assert!(dom.shapes().count() == 3, "now three p:sp shapes");
    }

    #[test]
    fn format_placeholder_sets_color_and_font() {
        // Title run has rPr b="1"; add color + font → solidFill then latin children.
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.format_placeholder(
            "title",
            &RunFormat {
                color: Some("FF0000".into()),
                font: Some("Calibri".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // rPr opened (was self-closing) and carries both children in schema order.
        assert!(s.contains(r#"<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:latin typeface="Calibri"/></a:rPr>"#), "{s}");
        // Body shape still untouched.
        assert!(s.contains("<a:r><a:t>Bullet one</a:t></a:r>"));
    }

    #[test]
    fn set_title_preserves_body_shape_byte_for_byte() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_title("X").unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        let src = String::from_utf8(SLIDE.to_vec()).unwrap();
        // Everything from the body shape onward is byte-identical to the source.
        let anchor = "<p:sp><p:nvSpPr><p:cNvPr id=\"3\"";
        assert_eq!(
            &out[out.find(anchor).unwrap()..],
            &src[src.find(anchor).unwrap()..]
        );
    }

    // ─── Paragraph-level editing tests (Req 1.1–1.4) ───────────────────────

    #[test]
    fn paragraphs_returns_correct_count() {
        let dom = SlideDom::parse(SLIDE).unwrap();
        // Shape 0 = title (1 paragraph), shape 1 = body (2 paragraphs).
        assert_eq!(dom.paragraphs(0).unwrap().len(), 1);
        assert_eq!(dom.paragraphs(1).unwrap().len(), 2);
    }

    #[test]
    fn paragraphs_error_on_missing_shape() {
        let dom = SlideDom::parse(SLIDE).unwrap();
        assert!(dom.paragraphs(99).is_err());
    }

    #[test]
    fn add_paragraph_appends_and_preserves_siblings() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.add_paragraph(1, "New para").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:t>New para</a:t>"), "{s}");
        // Existing paragraphs preserved byte-identical.
        assert!(s.contains("<a:r><a:t>Bullet one</a:t></a:r>"));
        assert!(s.contains("<a:r><a:t>Bullet two</a:t></a:r>"));
        // Title shape untouched.
        assert!(s.contains("<a:t>Old Title</a:t>"));
        assert_eq!(dom.paragraphs(1).unwrap().len(), 3);
    }

    #[test]
    fn insert_paragraph_at_beginning() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.insert_paragraph(1, 0, "First").unwrap();
        let paras = dom.paragraphs(1).unwrap();
        assert_eq!(paras.len(), 3);
        assert_eq!(paragraph_text(paras[0]), "First");
        assert_eq!(paragraph_text(paras[1]), "Bullet one");
        assert_eq!(paragraph_text(paras[2]), "Bullet two");
    }

    #[test]
    fn insert_paragraph_in_middle() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.insert_paragraph(1, 1, "Middle").unwrap();
        let paras = dom.paragraphs(1).unwrap();
        assert_eq!(paras.len(), 3);
        assert_eq!(paragraph_text(paras[0]), "Bullet one");
        assert_eq!(paragraph_text(paras[1]), "Middle");
        assert_eq!(paragraph_text(paras[2]), "Bullet two");
    }

    #[test]
    fn insert_paragraph_preserves_siblings_byte_identical() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.insert_paragraph(1, 1, "X").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // The original paragraphs' raw bytes are still present verbatim.
        assert!(s.contains("<a:p><a:r><a:t>Bullet one</a:t></a:r></a:p>"));
        assert!(s.contains("<a:p><a:r><a:t>Bullet two</a:t></a:r></a:p>"));
        // Title shape is byte-identical.
        assert!(s.contains(r#"<a:rPr lang="en-US" b="1"/>"#));
    }

    #[test]
    fn delete_paragraph_removes_correct_one() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.delete_paragraph(1, 0).unwrap();
        let paras = dom.paragraphs(1).unwrap();
        assert_eq!(paras.len(), 1);
        assert_eq!(paragraph_text(paras[0]), "Bullet two");
    }

    #[test]
    fn delete_paragraph_preserves_siblings() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.delete_paragraph(1, 0).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:p><a:r><a:t>Bullet two</a:t></a:r></a:p>"));
        assert!(s.contains("<a:t>Old Title</a:t>"));
    }

    #[test]
    fn delete_paragraph_out_of_range_errors() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        assert!(dom.delete_paragraph(1, 5).is_err());
    }

    #[test]
    fn reorder_paragraph_moves_forward() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.reorder_paragraph(1, 0, 1).unwrap();
        let paras = dom.paragraphs(1).unwrap();
        assert_eq!(paras.len(), 2);
        assert_eq!(paragraph_text(paras[0]), "Bullet two");
        assert_eq!(paragraph_text(paras[1]), "Bullet one");
    }

    #[test]
    fn reorder_paragraph_preserves_content() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.reorder_paragraph(1, 1, 0).unwrap();
        let paras = dom.paragraphs(1).unwrap();
        assert_eq!(paragraph_text(paras[0]), "Bullet two");
        assert_eq!(paragraph_text(paras[1]), "Bullet one");
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:t>Old Title</a:t>"));
    }

    #[test]
    fn error_on_missing_text_body() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp></p:spTree></p:cSld></p:sld>"#;
        let dom = SlideDom::parse(xml).unwrap();
        let err = dom.paragraphs(0);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("no text body"), "got: {msg}");
    }

    #[test]
    fn error_on_nonexistent_shape_index() {
        let dom = SlideDom::parse(SLIDE).unwrap();
        let err = dom.paragraphs(99);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("no shape at index"), "got: {msg}");
    }

    // ─── Paragraph property tests (Req 1.3) ────────────────────────────────

    #[test]
    fn set_alignment_creates_ppr_if_absent() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_alignment(1, 0, "ctr").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"algn="ctr""#), "alignment set: {s}");
        // Second paragraph untouched.
        assert!(s.contains("<a:p><a:r><a:t>Bullet two</a:t></a:r></a:p>"));
    }

    #[test]
    fn set_alignment_preserves_existing_ppr_attrs() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="S"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:pPr lvl="1"/><a:r><a:t>Hi</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let mut dom = SlideDom::parse(xml).unwrap();
        dom.set_paragraph_alignment(0, 0, "r").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"lvl="1""#), "lvl preserved: {s}");
        assert!(s.contains(r#"algn="r""#), "algn set: {s}");
    }

    #[test]
    fn set_indent_level() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_indent_level(1, 0, 2).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"lvl="2""#), "level set: {s}");
    }

    #[test]
    fn set_space_before_points() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_space_before(1, 0, SpacingValue::Points(600))
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:spcBef"), "spcBef present: {s}");
        assert!(s.contains(r#"<a:spcPts val="600"/>"#), "spcPts: {s}");
    }

    #[test]
    fn set_space_after_percent() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_space_after(1, 0, SpacingValue::Percent(50000))
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:spcAft"), "spcAft present: {s}");
        assert!(s.contains(r#"<a:spcPct val="50000"/>"#), "spcPct: {s}");
    }

    #[test]
    fn set_line_spacing() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_line_spacing(1, 0, SpacingValue::Percent(150000))
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:lnSpc"), "lnSpc present: {s}");
        assert!(s.contains(r#"<a:spcPct val="150000"/>"#), "150%: {s}");
    }

    #[test]
    fn set_bullet_char() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_bullet(1, 0, &BulletKind::Char("•".into()))
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:buChar"), "buChar present: {s}");
    }

    #[test]
    fn set_bullet_auto_num() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_bullet(1, 0, &BulletKind::AutoNum("arabicPeriod".into()))
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:buAutoNum"), "buAutoNum present: {s}");
        assert!(s.contains(r#"type="arabicPeriod""#), "type attr: {s}");
    }

    #[test]
    fn set_bullet_none() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_bullet(1, 0, &BulletKind::None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:buNone"), "buNone present: {s}");
    }

    #[test]
    fn set_bullet_replaces_existing() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_bullet(1, 0, &BulletKind::Char("-".into()))
            .unwrap();
        dom.set_paragraph_bullet(1, 0, &BulletKind::None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:buNone"), "buNone present: {s}");
        assert!(!s.contains("a:buChar"), "buChar removed: {s}");
    }

    #[test]
    fn spacing_in_schema_order() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_space_before(1, 0, SpacingValue::Points(400))
            .unwrap();
        dom.set_paragraph_line_spacing(1, 0, SpacingValue::Percent(120000))
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        let ln_pos = s.find("a:lnSpc").unwrap();
        let spc_pos = s.find("a:spcBef").unwrap();
        assert!(ln_pos < spc_pos, "lnSpc before spcBef per schema: {s}");
    }

    #[test]
    fn paragraph_props_preserve_sibling_paragraphs() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_paragraph_alignment(1, 0, "ctr").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:p><a:r><a:t>Bullet two</a:t></a:r></a:p>"));
        assert!(s.contains(r#"<a:rPr lang="en-US" b="1"/>"#));
    }

    // ─── Run-level editing tests (Req 2.1, 2.2) ────────────────────────────

    /// A slide with a paragraph containing two formatted runs for testing run ops.
    const MULTI_RUN_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" b="1" sz="2400"/><a:t>Hello</a:t></a:r><a:r><a:rPr lang="en-US" i="1" sz="1800"/><a:t> World</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn runs_returns_correct_count() {
        let dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].local_name(), b"r");
        assert_eq!(runs[1].local_name(), b"r");
    }

    #[test]
    fn runs_error_on_out_of_range_para() {
        let dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        assert!(dom.runs(0, 99).is_err());
    }

    #[test]
    fn runs_error_on_missing_shape() {
        let dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        assert!(dom.runs(99, 0).is_err());
    }

    #[test]
    fn add_run_appends_and_preserves_siblings() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.add_run(0, 0, "!").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // New run appended.
        assert!(s.contains("<a:t>!</a:t>"), "{s}");
        // Existing runs preserved byte-identical (including their rPr).
        assert!(
            s.contains(r#"<a:r><a:rPr lang="en-US" b="1" sz="2400"/><a:t>Hello</a:t></a:r>"#),
            "first run preserved: {s}"
        );
        assert!(
            s.contains(r#"<a:r><a:rPr lang="en-US" i="1" sz="1800"/><a:t> World</a:t></a:r>"#),
            "second run preserved: {s}"
        );
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 3);
    }

    #[test]
    fn insert_run_at_beginning() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_run(0, 0, 0, "Start ").unwrap();
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 3);
        // The inserted run is first.
        assert_eq!(
            runs[0].find_descendant(b"t").unwrap().text_content(),
            "Start "
        );
        // Original runs shifted.
        assert_eq!(
            runs[1].find_descendant(b"t").unwrap().text_content(),
            "Hello"
        );
        assert_eq!(
            runs[2].find_descendant(b"t").unwrap().text_content(),
            " World"
        );
    }

    #[test]
    fn insert_run_in_middle() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_run(0, 0, 1, " Middle").unwrap();
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(
            runs[0].find_descendant(b"t").unwrap().text_content(),
            "Hello"
        );
        assert_eq!(
            runs[1].find_descendant(b"t").unwrap().text_content(),
            " Middle"
        );
        assert_eq!(
            runs[2].find_descendant(b"t").unwrap().text_content(),
            " World"
        );
    }

    #[test]
    fn insert_run_preserves_sibling_rpr_byte_for_byte() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_run(0, 0, 1, "X").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Both original runs' rPr are byte-identical.
        assert!(
            s.contains(r#"<a:rPr lang="en-US" b="1" sz="2400"/>"#),
            "first rPr preserved: {s}"
        );
        assert!(
            s.contains(r#"<a:rPr lang="en-US" i="1" sz="1800"/>"#),
            "second rPr preserved: {s}"
        );
    }

    #[test]
    fn delete_run_removes_correct_one() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.delete_run(0, 0, 0).unwrap();
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].find_descendant(b"t").unwrap().text_content(),
            " World"
        );
    }

    #[test]
    fn delete_run_preserves_sibling() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.delete_run(0, 0, 0).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Second run preserved byte-identical.
        assert!(
            s.contains(r#"<a:r><a:rPr lang="en-US" i="1" sz="1800"/><a:t> World</a:t></a:r>"#),
            "sibling preserved: {s}"
        );
    }

    #[test]
    fn delete_run_out_of_range_errors() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        assert!(dom.delete_run(0, 0, 99).is_err());
    }

    #[test]
    fn edit_run_text_changes_only_text_preserves_rpr() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.edit_run_text(0, 0, 0, "Goodbye").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Text changed.
        assert!(s.contains("<a:t>Goodbye</a:t>"), "text changed: {s}");
        assert!(!s.contains("<a:t>Hello</a:t>"), "old text gone: {s}");
        // The edited run's rPr is preserved byte-for-byte.
        assert!(
            s.contains(r#"<a:rPr lang="en-US" b="1" sz="2400"/>"#),
            "edited run rPr preserved: {s}"
        );
        // The sibling run is completely byte-identical.
        assert!(
            s.contains(r#"<a:r><a:rPr lang="en-US" i="1" sz="1800"/><a:t> World</a:t></a:r>"#),
            "sibling run byte-identical: {s}"
        );
    }

    #[test]
    fn edit_run_text_preserves_all_sibling_runs_byte_for_byte() {
        // Edit the second run; verify the first run is byte-identical to source.
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.edit_run_text(0, 0, 1, " Universe").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        let src = String::from_utf8(MULTI_RUN_SLIDE.to_vec()).unwrap();
        // First run is byte-identical in both.
        let first_run = r#"<a:r><a:rPr lang="en-US" b="1" sz="2400"/><a:t>Hello</a:t></a:r>"#;
        assert!(s.contains(first_run), "first run byte-identical: {s}");
        assert!(src.contains(first_run));
        // Second run text changed, rPr preserved.
        assert!(s.contains("<a:t> Universe</a:t>"), "new text: {s}");
        assert!(
            s.contains(r#"<a:rPr lang="en-US" i="1" sz="1800"/>"#),
            "second rPr preserved: {s}"
        );
    }

    #[test]
    fn edit_run_text_error_on_line_break() {
        // Insert a line break, then try to edit it — should error.
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_line_break(0, 0, 1).unwrap();
        let err = dom.edit_run_text(0, 0, 1, "oops");
        assert!(err.is_err());
    }

    #[test]
    fn add_line_break_appends() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.add_line_break(0, 0).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:br/>"), "br appended: {s}");
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[2].local_name(), b"br");
    }

    #[test]
    fn insert_line_break_at_position() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_line_break(0, 0, 1).unwrap();
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].local_name(), b"r");
        assert_eq!(runs[1].local_name(), b"br");
        assert_eq!(runs[2].local_name(), b"r");
    }

    #[test]
    fn insert_line_break_preserves_sibling_runs() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_line_break(0, 0, 1).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:r><a:rPr lang="en-US" b="1" sz="2400"/><a:t>Hello</a:t></a:r>"#),
            "first run preserved: {s}"
        );
        assert!(
            s.contains(r#"<a:r><a:rPr lang="en-US" i="1" sz="1800"/><a:t> World</a:t></a:r>"#),
            "second run preserved: {s}"
        );
    }

    #[test]
    fn delete_line_break() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_line_break(0, 0, 1).unwrap();
        // Now delete the line break (at run index 1).
        dom.delete_run(0, 0, 1).unwrap();
        let runs = dom.runs(0, 0).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].local_name(), b"r");
        assert_eq!(runs[1].local_name(), b"r");
    }

    #[test]
    fn run_ops_error_on_missing_text_body() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp></p:spTree></p:cSld></p:sld>"#;
        let mut dom = SlideDom::parse(xml).unwrap();
        assert!(dom.runs(0, 0).is_err());
        assert!(dom.add_run(0, 0, "x").is_err());
        assert!(dom.insert_run(0, 0, 0, "x").is_err());
        assert!(dom.delete_run(0, 0, 0).is_err());
        assert!(dom.edit_run_text(0, 0, 0, "x").is_err());
        assert!(dom.add_line_break(0, 0).is_err());
        assert!(dom.insert_line_break(0, 0, 0).is_err());
    }

    // ─── Run-format breadth tests (Req 2.3, 2.4) ───────────────────────────

    #[test]
    fn strikethrough_attribute_emission() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                strikethrough: Some("sngStrike".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"strike="sngStrike""#), "strike attr set: {s}");
    }

    #[test]
    fn strikethrough_double_strike() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                strikethrough: Some("dblStrike".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"strike="dblStrike""#), "dblStrike: {s}");
    }

    #[test]
    fn strikethrough_no_strike() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                strikethrough: Some("noStrike".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"strike="noStrike""#), "noStrike: {s}");
    }

    #[test]
    fn baseline_superscript_emission() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                baseline: Some(30000),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"baseline="30000""#),
            "superscript baseline: {s}"
        );
    }

    #[test]
    fn baseline_subscript_emission() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                baseline: Some(-25000),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"baseline="-25000""#),
            "subscript baseline: {s}"
        );
    }

    #[test]
    fn lang_tag_emission() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            1,
            &RunFormat {
                lang: Some("fr-FR".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"lang="fr-FR""#), "lang tag set: {s}");
    }

    #[test]
    fn underline_style_sng() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                underline_style: Some("sng".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"u="sng""#), "underline sng: {s}");
    }

    #[test]
    fn underline_style_wavy_heavy() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                underline_style: Some("wavyHeavy".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"u="wavyHeavy""#), "underline wavyHeavy: {s}");
    }

    #[test]
    fn underline_style_dotted() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                underline_style: Some("dotted".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"u="dotted""#), "underline dotted: {s}");
    }

    #[test]
    fn underline_style_overrides_boolean_underline() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        // Both underline (bool) and underline_style set — style wins.
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                underline: Some(true),
                underline_style: Some("dbl".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"u="dbl""#), "style overrides bool: {s}");
        // Should NOT contain u="sng" from the boolean path.
        assert!(!s.contains(r#"u="sng""#), "no sng from bool: {s}");
    }

    #[test]
    fn theme_color_emits_scheme_clr_not_srgb_clr() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                theme_color: Some(SchemeColor::Accent1),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:solidFill><a:schemeClr val="accent1"/></a:solidFill>"#),
            "schemeClr emitted: {s}"
        );
        // Must NOT contain srgbClr.
        assert!(!s.contains("srgbClr"), "no srgbClr: {s}");
    }

    #[test]
    fn theme_color_replaces_existing_rgb_color() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        // First set an RGB color.
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                color: Some("FF0000".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("srgbClr"), "RGB set first: {s}");
        // Now set a theme color — should replace the RGB fill.
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                theme_color: Some(SchemeColor::Dk2),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:solidFill><a:schemeClr val="dk2"/></a:solidFill>"#),
            "theme color replaced RGB: {s}"
        );
        assert!(!s.contains("srgbClr"), "srgbClr removed: {s}");
    }

    #[test]
    fn theme_color_takes_precedence_over_color_in_same_format() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        // Both color and theme_color set — theme_color wins (Req 2.4).
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                color: Some("00FF00".into()),
                theme_color: Some(SchemeColor::Accent3),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:schemeClr val="accent3"/>"#),
            "theme_color wins: {s}"
        );
        assert!(!s.contains("srgbClr"), "no srgbClr when theme set: {s}");
    }

    #[test]
    fn format_run_preserves_sibling_runs_byte_for_byte() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        // Format only the first run with multiple new properties.
        dom.format_run(
            0,
            0,
            0,
            &RunFormat {
                strikethrough: Some("sngStrike".into()),
                baseline: Some(30000),
                lang: Some("de-DE".into()),
                underline_style: Some("heavy".into()),
                theme_color: Some(SchemeColor::Accent6),
                ..Default::default()
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // The second run must be byte-identical to the source.
        assert!(
            s.contains(r#"<a:r><a:rPr lang="en-US" i="1" sz="1800"/><a:t> World</a:t></a:r>"#),
            "sibling run byte-identical: {s}"
        );
    }

    #[test]
    fn format_run_error_on_invalid_run_index() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        let err = dom.format_run(
            0,
            0,
            99,
            &RunFormat {
                bold: Some(true),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn format_run_error_on_line_break() {
        let mut dom = SlideDom::parse(MULTI_RUN_SLIDE).unwrap();
        dom.insert_line_break(0, 0, 1).unwrap();
        // Run index 1 is now a line break — format_run should error.
        let err = dom.format_run(
            0,
            0,
            1,
            &RunFormat {
                bold: Some(true),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    // ─── Auto-fit tests (Req 3.1) ──────────────────────────────────────────

    /// A slide with a shape that has a bodyPr with existing attributes.
    const AUTOFIT_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr wrap="square" anchor="ctr"/><a:lstStyle/><a:p><a:r><a:t>Text</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn set_autofit_shrink_to_fit_with_font_scale() {
        let mut dom = SlideDom::parse(AUTOFIT_SLIDE).unwrap();
        dom.set_autofit(
            0,
            &AutoFit::ShrinkToFit {
                font_scale: Some(90000),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:normAutofit fontScale="90000"/>"#),
            "normAutofit emitted: {s}"
        );
        // bodyPr attributes preserved.
        assert!(s.contains(r#"wrap="square""#), "wrap preserved: {s}");
        assert!(s.contains(r#"anchor="ctr""#), "anchor preserved: {s}");
    }

    #[test]
    fn set_autofit_resize_shape() {
        let mut dom = SlideDom::parse(AUTOFIT_SLIDE).unwrap();
        dom.set_autofit(0, &AutoFit::ResizeShape).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:spAutoFit/>"), "spAutoFit emitted: {s}");
        // bodyPr attributes preserved.
        assert!(s.contains(r#"wrap="square""#), "wrap preserved: {s}");
        assert!(s.contains(r#"anchor="ctr""#), "anchor preserved: {s}");
    }

    #[test]
    fn set_autofit_none_removes_existing() {
        // Start with normAutofit, then set None to remove it.
        let mut dom = SlideDom::parse(AUTOFIT_SLIDE).unwrap();
        dom.set_autofit(
            0,
            &AutoFit::ShrinkToFit {
                font_scale: Some(80000),
            },
        )
        .unwrap();
        // Verify it was set.
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("normAutofit"), "normAutofit present: {s}");
        // Now remove it.
        dom.set_autofit(0, &AutoFit::None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(!s.contains("normAutofit"), "normAutofit removed: {s}");
        assert!(!s.contains("spAutoFit"), "spAutoFit absent: {s}");
        assert!(!s.contains("noAutofit"), "noAutofit absent: {s}");
        // bodyPr attributes still preserved.
        assert!(s.contains(r#"wrap="square""#), "wrap preserved: {s}");
        assert!(s.contains(r#"anchor="ctr""#), "anchor preserved: {s}");
    }

    #[test]
    fn set_autofit_replaces_one_mode_with_another() {
        let mut dom = SlideDom::parse(AUTOFIT_SLIDE).unwrap();
        // Set shrink-to-fit first.
        dom.set_autofit(
            0,
            &AutoFit::ShrinkToFit {
                font_scale: Some(75000),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("normAutofit"), "normAutofit set: {s}");
        // Replace with resize-shape.
        dom.set_autofit(0, &AutoFit::ResizeShape).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:spAutoFit/>"), "spAutoFit now: {s}");
        assert!(!s.contains("normAutofit"), "normAutofit gone: {s}");
    }

    #[test]
    fn set_autofit_preserves_other_bodypr_children() {
        // bodyPr with an existing child (e.g. noAutofit) and attributes.
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr wrap="square"><a:noAutofit/><a:prstTxWarp prst="textNoShape"><a:avLst/></a:prstTxWarp></a:bodyPr><a:lstStyle/><a:p><a:r><a:t>Hi</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let mut dom = SlideDom::parse(xml).unwrap();
        dom.set_autofit(
            0,
            &AutoFit::ShrinkToFit {
                font_scale: Some(62500),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // noAutofit removed, normAutofit inserted.
        assert!(!s.contains("noAutofit"), "noAutofit removed: {s}");
        assert!(
            s.contains(r#"<a:normAutofit fontScale="62500"/>"#),
            "normAutofit: {s}"
        );
        // Other bodyPr child (prstTxWarp) preserved.
        assert!(s.contains("prstTxWarp"), "prstTxWarp preserved: {s}");
        // bodyPr attribute preserved.
        assert!(s.contains(r#"wrap="square""#), "wrap preserved: {s}");
    }

    #[test]
    fn set_autofit_error_on_no_text_body() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp></p:spTree></p:cSld></p:sld>"#;
        let mut dom = SlideDom::parse(xml).unwrap();
        let err = dom.set_autofit(0, &AutoFit::ResizeShape);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("no text body"), "got: {msg}");
    }

    #[test]
    fn set_autofit_shrink_to_fit_omits_font_scale_when_none() {
        let mut dom = SlideDom::parse(AUTOFIT_SLIDE).unwrap();
        dom.set_autofit(0, &AutoFit::ShrinkToFit { font_scale: None })
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains("<a:normAutofit/>"),
            "normAutofit without fontScale: {s}"
        );
        assert!(!s.contains("fontScale"), "no fontScale attr: {s}");
    }

    #[test]
    fn set_autofit_shrink_to_fit_omits_font_scale_when_100000() {
        let mut dom = SlideDom::parse(AUTOFIT_SLIDE).unwrap();
        dom.set_autofit(
            0,
            &AutoFit::ShrinkToFit {
                font_scale: Some(100_000),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains("<a:normAutofit/>"),
            "normAutofit without fontScale at 100%: {s}"
        );
        assert!(!s.contains("fontScale"), "no fontScale attr at 100%: {s}");
    }

    // ─── Shape geometry tests (Req 7.1, 7.4) ───────────────────────────────

    /// A slide with two shapes, the first having an existing xfrm.
    const GEOM_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="3000" cy="4000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Shape 2"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="500" y="600"/><a:ext cx="700" cy="800"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>World</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn set_shape_position_updates_existing_xfrm() {
        let mut dom = SlideDom::parse(GEOM_SLIDE).unwrap();
        dom.set_shape_position(0, 1000, 2000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"x="1000""#), "x updated: {s}");
        assert!(s.contains(r#"y="2000""#), "y updated: {s}");
        // ext preserved.
        assert!(s.contains(r#"cx="3000""#), "cx preserved: {s}");
        assert!(s.contains(r#"cy="4000""#), "cy preserved: {s}");
    }

    #[test]
    fn set_shape_size_updates_existing_xfrm() {
        let mut dom = SlideDom::parse(GEOM_SLIDE).unwrap();
        dom.set_shape_size(0, 5000, 6000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"cx="5000""#), "cx updated: {s}");
        assert!(s.contains(r#"cy="6000""#), "cy updated: {s}");
        // off preserved.
        assert!(s.contains(r#"x="100""#), "x preserved: {s}");
        assert!(s.contains(r#"y="200""#), "y preserved: {s}");
    }

    #[test]
    fn set_shape_rotation_sets_rot_attr() {
        let mut dom = SlideDom::parse(GEOM_SLIDE).unwrap();
        dom.set_shape_rotation(0, 5400000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"rot="5400000""#), "rot set: {s}");
        // off and ext preserved.
        assert!(s.contains(r#"x="100""#), "x preserved: {s}");
        assert!(s.contains(r#"cx="3000""#), "cx preserved: {s}");
    }

    #[test]
    fn set_shape_geometry_sets_all_at_once() {
        let mut dom = SlideDom::parse(GEOM_SLIDE).unwrap();
        dom.set_shape_geometry(0, 111, 222, 333, 444, Some(900000))
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"x="111""#), "x: {s}");
        assert!(s.contains(r#"y="222""#), "y: {s}");
        assert!(s.contains(r#"cx="333""#), "cx: {s}");
        assert!(s.contains(r#"cy="444""#), "cy: {s}");
        assert!(s.contains(r#"rot="900000""#), "rot: {s}");
    }

    #[test]
    fn set_shape_geometry_without_rotation() {
        let mut dom = SlideDom::parse(GEOM_SLIDE).unwrap();
        dom.set_shape_geometry(0, 10, 20, 30, 40, None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"x="10""#), "x: {s}");
        assert!(s.contains(r#"y="20""#), "y: {s}");
        assert!(s.contains(r#"cx="30""#), "cx: {s}");
        assert!(s.contains(r#"cy="40""#), "cy: {s}");
        // No rot attribute should be added.
        assert!(!s.contains("rot="), "no rot: {s}");
    }

    #[test]
    fn geometry_creates_xfrm_when_absent() {
        // Shape with spPr but no xfrm.
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hi</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let mut dom = SlideDom::parse(xml).unwrap();
        dom.set_shape_position(0, 999, 888).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:xfrm"), "xfrm created: {s}");
        assert!(s.contains(r#"x="999""#), "x set: {s}");
        assert!(s.contains(r#"y="888""#), "y set: {s}");
        // prstGeom still present (xfrm inserted before it per schema order).
        assert!(s.contains("prstGeom"), "prstGeom preserved: {s}");
    }

    #[test]
    fn geometry_creates_sppr_and_xfrm_when_both_absent() {
        // Shape with no spPr at all.
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hi</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let mut dom = SlideDom::parse(xml).unwrap();
        dom.set_shape_size(0, 1234, 5678).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("p:spPr"), "spPr created: {s}");
        assert!(s.contains("a:xfrm"), "xfrm created: {s}");
        assert!(s.contains(r#"cx="1234""#), "cx set: {s}");
        assert!(s.contains(r#"cy="5678""#), "cy set: {s}");
    }

    #[test]
    fn geometry_preserves_sibling_shapes_byte_for_byte() {
        let mut dom = SlideDom::parse(GEOM_SLIDE).unwrap();
        let src = String::from_utf8(GEOM_SLIDE.to_vec()).unwrap();
        // Edit only shape 0.
        dom.set_shape_position(0, 9999, 8888).unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        // Shape 2 (id="3") must be byte-identical to the source.
        let anchor = r#"<p:sp><p:nvSpPr><p:cNvPr id="3""#;
        let src_shape2 = &src[src.find(anchor).unwrap()..];
        let out_shape2 = &out[out.find(anchor).unwrap()..];
        assert_eq!(out_shape2, src_shape2, "sibling shape byte-identical");
    }

    #[test]
    fn geometry_error_on_invalid_shape_index() {
        let mut dom = SlideDom::parse(GEOM_SLIDE).unwrap();
        let err = dom.set_shape_position(99, 0, 0);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("no shape at index"), "got: {msg}");
    }

    // ─── Shape delete/reorder/inventory tests (Req 7.2, 7.3, 7.4) ──────────

    /// A slide with three shapes of different types for delete/reorder testing.
    const MULTI_SHAPE_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="3000" cy="4000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody></p:sp><p:pic><p:nvPicPr><p:cNvPr id="3" name="Picture 2"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/></p:blipFill><p:spPr><a:xfrm><a:off x="500" y="600"/><a:ext cx="700" cy="800"/></a:xfrm></p:spPr></p:pic><p:sp><p:nvSpPr><p:cNvPr id="4" name="TextBox 3"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="900" y="1000"/><a:ext cx="1100" cy="1200"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>World</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn delete_shape_removes_and_preserves_siblings() {
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        let src = String::from_utf8(MULTI_SHAPE_SLIDE.to_vec()).unwrap();
        // Delete the picture (index 1).
        dom.delete_shape(1).unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        // Picture is gone.
        assert!(!out.contains("Picture 2"), "pic removed: {out}");
        assert!(!out.contains("p:pic"), "p:pic tag removed: {out}");
        // Remaining shapes preserved byte-for-byte.
        let anchor0 = r#"<p:sp><p:nvSpPr><p:cNvPr id="2""#;
        let anchor2 = r#"<p:sp><p:nvSpPr><p:cNvPr id="4""#;
        // Shape 0 (Title 1) preserved.
        let src_s0_start = src.find(anchor0).unwrap();
        let src_s0_end = src.find("<p:pic>").unwrap();
        let out_s0_start = out.find(anchor0).unwrap();
        let out_s0_end = out.find(anchor2).unwrap();
        assert_eq!(
            &out[out_s0_start..out_s0_end],
            &src[src_s0_start..src_s0_end],
            "shape 0 preserved"
        );
        // Shape 2 (TextBox 3) preserved.
        assert!(out.contains("<a:t>World</a:t>"), "shape 2 text preserved");
    }

    #[test]
    fn delete_shape_error_on_invalid_index() {
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        let err = dom.delete_shape(99);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("out of range"), "got: {msg}");
    }

    #[test]
    fn reorder_shape_moves_to_front() {
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        // Move shape 2 (TextBox 3) to position 0.
        dom.reorder_shape(2, 0).unwrap();
        let inv = dom.shape_inventory();
        assert_eq!(inv[0].name, "TextBox 3");
        assert_eq!(inv[1].name, "Title 1");
        assert_eq!(inv[2].name, "Picture 2");
    }

    #[test]
    fn reorder_shape_moves_to_back() {
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        // Move shape 0 (Title 1) to position 2.
        dom.reorder_shape(0, 2).unwrap();
        let inv = dom.shape_inventory();
        assert_eq!(inv[0].name, "Picture 2");
        assert_eq!(inv[1].name, "TextBox 3");
        assert_eq!(inv[2].name, "Title 1");
    }

    #[test]
    fn reorder_shape_noop_same_index() {
        let dom_before = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        dom.reorder_shape(1, 1).unwrap();
        // Byte-identical when from == to.
        assert_eq!(dom.to_bytes(), dom_before.to_bytes());
    }

    #[test]
    fn reorder_shape_preserves_siblings_byte_for_byte() {
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        let src = String::from_utf8(MULTI_SHAPE_SLIDE.to_vec()).unwrap();
        // Move shape 0 to position 1 — shape 2 (TextBox 3) should be untouched.
        dom.reorder_shape(0, 1).unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        // TextBox 3 content preserved.
        let anchor = r#"<p:sp><p:nvSpPr><p:cNvPr id="4" name="TextBox 3"/>"#;
        let src_frag = &src[src.find(anchor).unwrap()..src.rfind("</p:spTree>").unwrap()];
        let out_frag = &out[out.find(anchor).unwrap()..out.rfind("</p:spTree>").unwrap()];
        assert_eq!(out_frag, src_frag, "sibling shape byte-identical");
    }

    #[test]
    fn reorder_shape_error_on_invalid_from() {
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        let err = dom.reorder_shape(99, 0);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("from_idx"), "got: {msg}");
    }

    #[test]
    fn reorder_shape_error_on_invalid_to() {
        let mut dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        let err = dom.reorder_shape(0, 99);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("to_idx"), "got: {msg}");
    }

    #[test]
    fn shape_inventory_returns_correct_info() {
        let dom = SlideDom::parse(MULTI_SHAPE_SLIDE).unwrap();
        let inv = dom.shape_inventory();
        assert_eq!(inv.len(), 3);

        // Shape 0: sp with text.
        assert_eq!(inv[0].id, 2);
        assert_eq!(inv[0].name, "Title 1");
        assert_eq!(inv[0].shape_type, "sp");
        assert_eq!(inv[0].geometry, Some((100, 200, 3000, 4000)));
        assert_eq!(inv[0].text, Some("Hello".to_string()));

        // Shape 1: pic (no text body).
        assert_eq!(inv[1].id, 3);
        assert_eq!(inv[1].name, "Picture 2");
        assert_eq!(inv[1].shape_type, "pic");
        assert_eq!(inv[1].geometry, Some((500, 600, 700, 800)));
        assert_eq!(inv[1].text, None);

        // Shape 2: sp with text.
        assert_eq!(inv[2].id, 4);
        assert_eq!(inv[2].name, "TextBox 3");
        assert_eq!(inv[2].shape_type, "sp");
        assert_eq!(inv[2].geometry, Some((900, 1000, 1100, 1200)));
        assert_eq!(inv[2].text, Some("World".to_string()));
    }

    #[test]
    fn shape_inventory_empty_on_no_shapes() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree></p:spTree></p:cSld></p:sld>"#;
        let dom = SlideDom::parse(xml).unwrap();
        assert_eq!(dom.shape_inventory().len(), 0);
    }

    // ─── Shape fill tests (Req 8.1, 8.3) ───────────────────────────────────

    /// A slide with two shapes: one with an existing solidFill, one plain.
    const FILL_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="0000FF"/></a:solidFill></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Shape 2"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="200" y="200"/><a:ext cx="300" cy="300"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>World</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn set_shape_fill_solid_rgb() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_fill(
            0,
            &FillSpec::Solid {
                color: ColorSpec::Rgb("FF0000".into()),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>"#),
            "solid RGB fill: {s}"
        );
        // Old fill removed.
        assert!(!s.contains(r#"val="0000FF""#), "old fill gone: {s}");
    }

    #[test]
    fn set_shape_fill_solid_theme_color() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_fill(
            0,
            &FillSpec::Solid {
                color: ColorSpec::Theme(SchemeColor::Accent1),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:solidFill><a:schemeClr val="accent1"/></a:solidFill>"#),
            "theme color fill: {s}"
        );
        // Old fill removed.
        assert!(!s.contains(r#"val="0000FF""#), "old fill gone: {s}");
    }

    #[test]
    fn set_shape_fill_gradient() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_fill(
            0,
            &FillSpec::Gradient {
                stops: vec![
                    (0.0, ColorSpec::Rgb("FF0000".into())),
                    (1.0, ColorSpec::Rgb("0000FF".into())),
                ],
                angle_deg: 90.0,
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:gradFill>"), "gradFill present: {s}");
        assert!(s.contains("<a:gsLst>"), "gsLst present: {s}");
        assert!(s.contains(r#"<a:gs pos="0">"#), "first stop: {s}");
        assert!(s.contains(r#"<a:gs pos="100000">"#), "second stop: {s}");
        assert!(s.contains(r#"val="FF0000""#), "first color: {s}");
        assert!(s.contains(r#"val="0000FF""#), "second color: {s}");
        assert!(s.contains(r#"ang="5400000""#), "angle 90deg: {s}");
        // Old solidFill removed.
        assert!(!s.contains("<a:solidFill>"), "old fill gone: {s}");
    }

    #[test]
    fn set_shape_fill_pattern() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_fill(
            0,
            &FillSpec::Pattern {
                preset: "ltDnDiag".into(),
                fg: ColorSpec::Rgb("000000".into()),
                bg: ColorSpec::Rgb("FFFFFF".into()),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:pattFill prst="ltDnDiag">"#),
            "pattFill: {s}"
        );
        assert!(s.contains("<a:fgClr>"), "fgClr: {s}");
        assert!(s.contains("<a:bgClr>"), "bgClr: {s}");
        assert!(s.contains(r#"val="000000""#), "fg color: {s}");
        assert!(s.contains(r#"val="FFFFFF""#), "bg color: {s}");
    }

    #[test]
    fn set_shape_fill_no_fill() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_fill(0, &FillSpec::None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:noFill/>"), "noFill: {s}");
        // Old solidFill removed.
        assert!(!s.contains("<a:solidFill>"), "old fill gone: {s}");
    }

    #[test]
    fn set_shape_fill_replaces_existing_fill_type() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        // First set a gradient.
        dom.set_shape_fill(
            0,
            &FillSpec::Gradient {
                stops: vec![
                    (0.0, ColorSpec::Rgb("AABBCC".into())),
                    (1.0, ColorSpec::Rgb("DDEEFF".into())),
                ],
                angle_deg: 45.0,
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:gradFill>"), "gradient set: {s}");
        assert!(!s.contains("<a:solidFill>"), "solid removed: {s}");

        // Now replace with solid.
        dom.set_shape_fill(
            0,
            &FillSpec::Solid {
                color: ColorSpec::Rgb("112233".into()),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:solidFill>"), "solid set: {s}");
        assert!(!s.contains("<a:gradFill>"), "gradient removed: {s}");
    }

    #[test]
    fn set_shape_fill_preserves_sibling_shapes() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        let before = String::from_utf8(dom.to_bytes()).unwrap();
        // Extract the second shape's XML from the source.
        let shape2_anchor = r#"<p:sp><p:nvSpPr><p:cNvPr id="3""#;
        let shape2_before = &before[before.find(shape2_anchor).unwrap()..];

        // Modify the first shape's fill.
        dom.set_shape_fill(
            0,
            &FillSpec::Solid {
                color: ColorSpec::Rgb("AABBCC".into()),
            },
        )
        .unwrap();
        let after = String::from_utf8(dom.to_bytes()).unwrap();
        let shape2_after = &after[after.find(shape2_anchor).unwrap()..];

        // The second shape is byte-for-byte identical.
        assert_eq!(shape2_before, shape2_after, "sibling shape preserved");
    }

    #[test]
    fn set_shape_fill_picture() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_fill(
            0,
            &FillSpec::Picture {
                r_id: "rId5".into(),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:blipFill>"), "blipFill: {s}");
        assert!(s.contains(r#"r:embed="rId5""#), "r:embed: {s}");
        assert!(s.contains("<a:stretch>"), "stretch: {s}");
        assert!(s.contains("<a:fillRect/>"), "fillRect: {s}");
    }

    #[test]
    fn set_shape_fill_on_shape_without_existing_fill() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        // Shape at index 1 has no fill element.
        dom.set_shape_fill(
            1,
            &FillSpec::Solid {
                color: ColorSpec::Theme(SchemeColor::Dk1),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:solidFill><a:schemeClr val="dk1"/></a:solidFill>"#),
            "fill on shape 2: {s}"
        );
        // Shape 1's text still present.
        assert!(
            s.contains("<a:t>World</a:t>"),
            "shape 2 text preserved: {s}"
        );
    }

    #[test]
    fn set_shape_fill_schema_order_fill_after_geometry() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_fill(
            0,
            &FillSpec::Solid {
                color: ColorSpec::Rgb("AABBCC".into()),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // In the SPPR_ORDER table: xfrm < prstGeom < solidFill.
        // So solidFill must come after prstGeom.
        let geom_pos = s.find("a:prstGeom").unwrap();
        let fill_pos = s.find("a:solidFill").unwrap();
        assert!(
            fill_pos > geom_pos,
            "fill after geometry in schema order: {s}"
        );
    }

    // ─── Shape line tests (Req 8.2, 8.3) ───────────────────────────────────

    #[test]
    fn set_shape_line_rgb_color_and_width() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_line(
            0,
            &LineSpec::Styled {
                color: ColorSpec::Rgb("FF0000".into()),
                width_emu: 12700,
                dash: None,
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"<a:ln w="12700">"#), "ln with width: {s}");
        assert!(
            s.contains(r#"<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>"#),
            "solid fill in ln: {s}"
        );
        // No dash element when dash is None.
        assert!(!s.contains("a:prstDash"), "no dash: {s}");
    }

    #[test]
    fn set_shape_line_theme_color() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_line(
            0,
            &LineSpec::Styled {
                color: ColorSpec::Theme(SchemeColor::Accent2),
                width_emu: 25400,
                dash: None,
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"<a:ln w="25400">"#), "ln with width: {s}");
        assert!(
            s.contains(r#"<a:solidFill><a:schemeClr val="accent2"/></a:solidFill>"#),
            "theme color in ln: {s}"
        );
    }

    #[test]
    fn set_shape_line_with_dash_style() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_line(
            0,
            &LineSpec::Styled {
                color: ColorSpec::Rgb("00FF00".into()),
                width_emu: 9525,
                dash: Some("dash".into()),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"<a:ln w="9525">"#), "ln with width: {s}");
        assert!(s.contains(r#"<a:prstDash val="dash"/>"#), "dash style: {s}");
    }

    #[test]
    fn set_shape_line_no_line() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        dom.set_shape_line(0, &LineSpec::None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:ln><a:noFill/></a:ln>"), "no-line: {s}");
    }

    #[test]
    fn set_shape_line_replaces_existing_line() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        // First set a styled line.
        dom.set_shape_line(
            0,
            &LineSpec::Styled {
                color: ColorSpec::Rgb("AABBCC".into()),
                width_emu: 12700,
                dash: Some("dot".into()),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"val="AABBCC""#), "first line set: {s}");

        // Now replace with no-line.
        dom.set_shape_line(0, &LineSpec::None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains("<a:ln><a:noFill/></a:ln>"),
            "replaced with no-line: {s}"
        );
        // Old line gone.
        assert!(!s.contains(r#"val="AABBCC""#), "old line removed: {s}");
        assert!(!s.contains("a:prstDash"), "old dash removed: {s}");
    }

    #[test]
    fn set_shape_line_preserves_sibling_shapes() {
        let mut dom = SlideDom::parse(FILL_SLIDE).unwrap();
        let before = String::from_utf8(dom.to_bytes()).unwrap();
        // Extract the second shape's XML (Shape 2).
        let shape2_start = before.find(r#"id="3""#).unwrap();
        let shape2_before = &before[shape2_start..];

        // Modify the first shape's line.
        dom.set_shape_line(
            0,
            &LineSpec::Styled {
                color: ColorSpec::Rgb("112233".into()),
                width_emu: 19050,
                dash: None,
            },
        )
        .unwrap();

        let after = String::from_utf8(dom.to_bytes()).unwrap();
        let shape2_after_start = after.find(r#"id="3""#).unwrap();
        let shape2_after = &after[shape2_after_start..];
        assert_eq!(
            shape2_before, shape2_after,
            "sibling shape preserved byte-for-byte"
        );
    }

    // ─── Table editing tests (Req 6.1) ─────────────────────────────────────

    /// A realistic slide with a graphicFrame containing a 2-column, 2-row table.
    const TABLE_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Slide Title</a:t></a:r></a:p></p:txBody></p:sp><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Table 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="100" y="200"/><a:ext cx="5000" cy="3000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid><a:gridCol w="2500"/><a:gridCol w="2500"/></a:tblGrid><a:tr h="370840"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>A1</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>B1</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr h="370840"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>A2</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>B2</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn table_element_finds_tbl() {
        let dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        // Shape 0 = p:sp (title), shape 1 = p:graphicFrame (table).
        let tbl = dom.table_element(1).unwrap();
        assert_eq!(tbl.local_name(), b"tbl");
    }

    #[test]
    fn table_element_error_on_non_table_shape() {
        let dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        // Shape 0 is a p:sp, not a graphicFrame.
        let err = dom.table_element(0);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("not a graphicFrame"), "got: {msg}");
    }

    #[test]
    fn table_element_error_on_invalid_index() {
        let dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        let err = dom.table_element(99);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("no shape at index"), "got: {msg}");
    }

    #[test]
    fn add_table_row_increases_row_count() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.add_table_row(1, 400000).unwrap();
        let tbl = dom.table_element(1).unwrap();
        let rows: Vec<_> = tbl.children_named(b"tr").collect();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn add_table_row_has_correct_cell_count() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.add_table_row(1, 400000).unwrap();
        let tbl = dom.table_element(1).unwrap();
        let rows: Vec<_> = tbl.children_named(b"tr").collect();
        // New row (last) should have 2 cells matching the 2 gridCols.
        let new_row = rows[2];
        let cells: Vec<_> = new_row.children_named(b"tc").collect();
        assert_eq!(cells.len(), 2);
    }

    #[test]
    fn add_table_row_sets_height() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.add_table_row(1, 500000).unwrap();
        let tbl = dom.table_element(1).unwrap();
        let rows: Vec<_> = tbl.children_named(b"tr").collect();
        let new_row = rows[2];
        let h = new_row
            .attr(b"h")
            .map(|v| String::from_utf8_lossy(v).into_owned());
        assert_eq!(h.as_deref(), Some("500000"));
    }

    #[test]
    fn insert_table_row_at_position() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.insert_table_row(1, 1, 300000).unwrap();
        let tbl = dom.table_element(1).unwrap();
        let rows: Vec<_> = tbl.children_named(b"tr").collect();
        assert_eq!(rows.len(), 3);
        // The inserted row is at index 1; original row 1 (with A2/B2) is now at index 2.
        let inserted_row = rows[1];
        let h = inserted_row
            .attr(b"h")
            .map(|v| String::from_utf8_lossy(v).into_owned());
        assert_eq!(h.as_deref(), Some("300000"));
        // Original second row still has its content.
        let last_row = rows[2];
        let cell_text: String = last_row
            .children_named(b"tc")
            .flat_map(|tc| tc.find_descendant(b"t"))
            .map(|t| t.text_content())
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(cell_text, "A2,B2");
    }

    #[test]
    fn remove_table_row_preserves_other_rows() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.remove_table_row(1, 0).unwrap();
        let tbl = dom.table_element(1).unwrap();
        let rows: Vec<_> = tbl.children_named(b"tr").collect();
        assert_eq!(rows.len(), 1);
        // Remaining row should be the original second row (A2, B2).
        let cell_text: String = rows[0]
            .children_named(b"tc")
            .flat_map(|tc| tc.find_descendant(b"t"))
            .map(|t| t.text_content())
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(cell_text, "A2,B2");
    }

    #[test]
    fn remove_table_row_out_of_range_errors() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        let err = dom.remove_table_row(1, 99);
        assert!(err.is_err());
    }

    #[test]
    fn add_table_column_increases_column_count() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.add_table_column(1, 1500).unwrap();
        let tbl = dom.table_element(1).unwrap();
        let grid = tbl.children_named(b"tblGrid").next().unwrap();
        let cols: Vec<_> = grid.children_named(b"gridCol").collect();
        assert_eq!(cols.len(), 3);
    }

    #[test]
    fn add_table_column_adds_cell_to_each_row() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.add_table_column(1, 1500).unwrap();
        let tbl = dom.table_element(1).unwrap();
        for tr in tbl.children_named(b"tr") {
            let cells: Vec<_> = tr.children_named(b"tc").collect();
            assert_eq!(
                cells.len(),
                3,
                "each row should have 3 cells after adding a column"
            );
        }
    }

    #[test]
    fn add_table_column_sets_width() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.add_table_column(1, 3000).unwrap();
        let tbl = dom.table_element(1).unwrap();
        let grid = tbl.children_named(b"tblGrid").next().unwrap();
        let cols: Vec<_> = grid.children_named(b"gridCol").collect();
        let w = cols[2]
            .attr(b"w")
            .map(|v| String::from_utf8_lossy(v).into_owned());
        assert_eq!(w.as_deref(), Some("3000"));
    }

    #[test]
    fn remove_table_column_removes_correct_gridcol_and_cells() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        // Remove column 0 (the first column).
        dom.remove_table_column(1, 0).unwrap();
        let tbl = dom.table_element(1).unwrap();
        // Grid should have 1 column left.
        let grid = tbl.children_named(b"tblGrid").next().unwrap();
        let cols: Vec<_> = grid.children_named(b"gridCol").collect();
        assert_eq!(cols.len(), 1);
        // Each row should have 1 cell left (the B column).
        let rows: Vec<_> = tbl.children_named(b"tr").collect();
        for (i, tr) in rows.iter().enumerate() {
            let cells: Vec<_> = tr.children_named(b"tc").collect();
            assert_eq!(cells.len(), 1, "row {i} should have 1 cell");
            // The remaining cell should be the B column.
            let text = cells[0]
                .find_descendant(b"t")
                .map(|t| t.text_content())
                .unwrap_or_default();
            let expected = format!("B{}", i + 1);
            assert_eq!(text, expected);
        }
    }

    #[test]
    fn remove_table_column_out_of_range_errors() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        let err = dom.remove_table_column(1, 99);
        assert!(err.is_err());
    }

    #[test]
    fn table_operations_preserve_existing_cell_content() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        // Add a row, then verify original cells are preserved.
        dom.add_table_row(1, 400000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:t>A1</a:t>"), "A1 preserved: {s}");
        assert!(s.contains("<a:t>B1</a:t>"), "B1 preserved: {s}");
        assert!(s.contains("<a:t>A2</a:t>"), "A2 preserved: {s}");
        assert!(s.contains("<a:t>B2</a:t>"), "B2 preserved: {s}");
    }

    #[test]
    fn table_operations_preserve_title_shape() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.add_table_row(1, 400000).unwrap();
        dom.add_table_column(1, 1000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Title shape is untouched.
        assert!(s.contains("<a:t>Slide Title</a:t>"), "title preserved: {s}");
        assert!(s.contains(r#"<p:ph type="title"/>"#), "ph preserved: {s}");
    }

    #[test]
    fn table_unedited_round_trips() {
        let dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        assert_eq!(dom.to_bytes(), TABLE_SLIDE);
    }

    // ─── Table merge/split tests (Req 6.2) ─────────────────────────────────

    /// A 3×3 table for merge/split testing.
    const TABLE_3X3_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="2" name="Table 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/><a:ext cx="9000" cy="6000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid><a:gridCol w="3000"/><a:gridCol w="3000"/><a:gridCol w="3000"/></a:tblGrid><a:tr h="2000"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>A1</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>B1</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>C1</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr h="2000"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>A2</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>B2</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>C2</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr h="2000"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>A3</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>B3</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>C3</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn merge_2x2_sets_correct_attributes() {
        let mut dom = SlideDom::parse(TABLE_3X3_SLIDE).unwrap();
        // Merge cells (0,0) to (1,1) — a 2×2 block.
        dom.merge_table_cells(0, 0, 0, 1, 1).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Origin cell (A1) gets gridSpan="2" and rowSpan="2".
        assert!(s.contains(r#"gridSpan="2""#), "gridSpan on origin: {s}");
        assert!(s.contains(r#"rowSpan="2""#), "rowSpan on origin: {s}");
        // Origin cell content preserved.
        assert!(s.contains("<a:t>A1</a:t>"), "origin content preserved: {s}");
        // Covered cells get hMerge/vMerge.
        assert!(s.contains(r#"hMerge="1""#), "hMerge on covered: {s}");
        assert!(s.contains(r#"vMerge="1""#), "vMerge on covered: {s}");
        // Cells outside the merge range are untouched.
        assert!(s.contains("<a:t>C1</a:t>"), "C1 preserved: {s}");
        assert!(s.contains("<a:t>C2</a:t>"), "C2 preserved: {s}");
        assert!(s.contains("<a:t>A3</a:t>"), "A3 preserved: {s}");
    }

    #[test]
    fn merge_horizontal_1x3_sets_gridspan_only() {
        let mut dom = SlideDom::parse(TABLE_3X3_SLIDE).unwrap();
        // Merge row 0, cols 0..2 — a 1×3 horizontal merge.
        dom.merge_table_cells(0, 0, 0, 0, 2).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Origin cell gets gridSpan="3", no rowSpan (since row_span == 1).
        assert!(s.contains(r#"gridSpan="3""#), "gridSpan=3: {s}");
        assert!(
            !s.contains(r#"rowSpan=""#),
            "no rowSpan for single-row merge: {s}"
        );
        // Covered cells (B1, C1) get hMerge="1".
        // Count occurrences of hMerge="1".
        let h_merge_count = s.matches(r#"hMerge="1""#).count();
        assert_eq!(h_merge_count, 2, "two cells with hMerge: {s}");
        // No vMerge since it's a single-row merge.
        assert!(
            !s.contains(r#"vMerge="1""#),
            "no vMerge for horizontal merge: {s}"
        );
        // Origin content preserved.
        assert!(s.contains("<a:t>A1</a:t>"), "origin preserved: {s}");
    }

    #[test]
    fn merge_vertical_3x1_sets_rowspan_only() {
        let mut dom = SlideDom::parse(TABLE_3X3_SLIDE).unwrap();
        // Merge col 0, rows 0..2 — a 3×1 vertical merge.
        dom.merge_table_cells(0, 0, 0, 2, 0).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Origin cell gets rowSpan="3", no gridSpan (since col_span == 1).
        assert!(s.contains(r#"rowSpan="3""#), "rowSpan=3: {s}");
        assert!(
            !s.contains(r#"gridSpan=""#),
            "no gridSpan for single-col merge: {s}"
        );
        // Covered cells (A2, A3) get vMerge="1".
        let v_merge_count = s.matches(r#"vMerge="1""#).count();
        assert_eq!(v_merge_count, 2, "two cells with vMerge: {s}");
        // No hMerge since it's a single-column merge.
        assert!(
            !s.contains(r#"hMerge="1""#),
            "no hMerge for vertical merge: {s}"
        );
        // Origin content preserved.
        assert!(s.contains("<a:t>A1</a:t>"), "origin preserved: {s}");
    }

    #[test]
    fn split_merged_cell_removes_all_merge_attributes() {
        let mut dom = SlideDom::parse(TABLE_3X3_SLIDE).unwrap();
        // First merge a 2×2 block.
        dom.merge_table_cells(0, 0, 0, 1, 1).unwrap();
        // Verify merge attributes are present.
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"gridSpan="2""#));
        assert!(s.contains(r#"rowSpan="2""#));
        // Now split.
        dom.split_table_cell(0, 0, 0).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // All merge attributes removed.
        assert!(!s.contains("gridSpan"), "gridSpan removed: {s}");
        assert!(!s.contains("rowSpan"), "rowSpan removed: {s}");
        assert!(!s.contains("hMerge"), "hMerge removed: {s}");
        assert!(!s.contains("vMerge"), "vMerge removed: {s}");
    }

    #[test]
    fn merge_error_on_invalid_range_out_of_bounds() {
        let mut dom = SlideDom::parse(TABLE_3X3_SLIDE).unwrap();
        // end_row out of bounds (table has 3 rows, indices 0..2).
        let err = dom.merge_table_cells(0, 0, 0, 5, 0);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("out of range"), "got: {msg}");
    }

    #[test]
    fn merge_error_on_invalid_range_start_gt_end() {
        let mut dom = SlideDom::parse(TABLE_3X3_SLIDE).unwrap();
        // start_row > end_row.
        let err = dom.merge_table_cells(0, 2, 0, 0, 0);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("invalid merge range"), "got: {msg}");
    }

    #[test]
    fn merge_preserves_origin_cell_content() {
        let mut dom = SlideDom::parse(TABLE_3X3_SLIDE).unwrap();
        dom.merge_table_cells(0, 0, 0, 1, 1).unwrap();
        let tbl = dom.table_element(0).unwrap();
        // Get the origin cell (row 0, col 0).
        let first_row = tbl.children_named(b"tr").next().unwrap();
        let origin_cell = first_row.children_named(b"tc").next().unwrap();
        // Origin cell should still have its text content.
        let text = origin_cell
            .find_descendant(b"t")
            .map(|t| t.text_content())
            .unwrap_or_default();
        assert_eq!(text, "A1", "origin cell content preserved");
    }

    // ─── Table column width / row height / cell props tests (Req 6.3–6.5) ──

    #[test]
    fn set_column_width_updates_gridcol_attribute() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_column_width(1, 0, 5000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // The first gridCol should now have w="5000".
        assert!(s.contains(r#"<a:gridCol w="5000"/>"#), "width updated: {s}");
        // Second gridCol unchanged.
        assert!(
            s.contains(r#"<a:gridCol w="2500"/>"#),
            "other col preserved: {s}"
        );
    }

    #[test]
    fn set_column_width_out_of_range_errors() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        let err = dom.set_column_width(1, 99, 5000);
        assert!(err.is_err());
    }

    #[test]
    fn set_row_height_updates_tr_attribute() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_row_height(1, 0, 500000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // First row should have h="500000".
        assert!(s.contains(r#"h="500000""#), "height updated: {s}");
        // Second row unchanged.
        assert!(s.contains(r#"h="370840""#), "other row preserved: {s}");
    }

    #[test]
    fn set_row_height_out_of_range_errors() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        let err = dom.set_row_height(1, 99, 500000);
        assert!(err.is_err());
    }

    #[test]
    fn set_cell_text_replaces_content() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_cell_text(1, 0, 0, "New Text").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:t>New Text</a:t>"), "new text present: {s}");
        // Original A1 text gone.
        assert!(!s.contains("<a:t>A1</a:t>"), "old text removed: {s}");
        // Other cells preserved.
        assert!(s.contains("<a:t>B1</a:t>"), "B1 preserved: {s}");
        assert!(s.contains("<a:t>A2</a:t>"), "A2 preserved: {s}");
        assert!(s.contains("<a:t>B2</a:t>"), "B2 preserved: {s}");
    }

    #[test]
    fn set_cell_text_preserves_other_cells() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_cell_text(1, 1, 1, "Changed").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("<a:t>Changed</a:t>"), "new text: {s}");
        // All other cells preserved.
        assert!(s.contains("<a:t>A1</a:t>"), "A1 preserved: {s}");
        assert!(s.contains("<a:t>B1</a:t>"), "B1 preserved: {s}");
        assert!(s.contains("<a:t>A2</a:t>"), "A2 preserved: {s}");
    }

    #[test]
    fn set_cell_alignment_sets_algn_on_ppr() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_cell_alignment(1, 0, 0, "ctr").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"algn="ctr""#), "alignment set: {s}");
        // Other cells preserved.
        assert!(s.contains("<a:t>B1</a:t>"), "B1 preserved: {s}");
    }

    #[test]
    fn set_cell_fill_adds_solid_fill_to_tcpr() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        let fill = FillSpec::Solid {
            color: ColorSpec::Rgb("FF0000".into()),
        };
        dom.set_cell_fill(1, 0, 0, &fill).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:tcPr"), "tcPr created: {s}");
        assert!(
            s.contains(r#"<a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>"#),
            "fill set: {s}"
        );
        // Other cells preserved.
        assert!(s.contains("<a:t>B1</a:t>"), "B1 preserved: {s}");
    }

    #[test]
    fn set_cell_fill_no_fill() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_cell_fill(1, 0, 0, &FillSpec::None).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains("a:noFill"), "noFill set: {s}");
    }

    #[test]
    fn set_cell_margins_sets_all_margin_attrs() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_cell_margins(1, 0, 0, 91440, 45720, 91440, 45720)
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"marL="91440""#), "marL set: {s}");
        assert!(s.contains(r#"marT="45720""#), "marT set: {s}");
        assert!(s.contains(r#"marR="91440""#), "marR set: {s}");
        assert!(s.contains(r#"marB="45720""#), "marB set: {s}");
        // Other cells preserved.
        assert!(s.contains("<a:t>B1</a:t>"), "B1 preserved: {s}");
    }

    #[test]
    fn set_cell_margins_preserves_existing_tcpr_attrs() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        // First set fill (which creates tcPr), then set margins.
        let fill = FillSpec::Solid {
            color: ColorSpec::Rgb("00FF00".into()),
        };
        dom.set_cell_fill(1, 0, 0, &fill).unwrap();
        dom.set_cell_margins(1, 0, 0, 100, 200, 300, 400).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Both fill and margins present.
        assert!(s.contains(r#"val="00FF00""#), "fill preserved: {s}");
        assert!(s.contains(r#"marL="100""#), "marL set: {s}");
        assert!(s.contains(r#"marB="400""#), "marB set: {s}");
    }

    #[test]
    fn table_cell_operations_are_surgical() {
        let mut dom = SlideDom::parse(TABLE_SLIDE).unwrap();
        dom.set_column_width(1, 0, 9999).unwrap();
        dom.set_row_height(1, 1, 888888).unwrap();
        dom.set_cell_text(1, 0, 1, "Modified").unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Title shape is completely untouched.
        assert!(s.contains("<a:t>Slide Title</a:t>"), "title preserved: {s}");
        assert!(s.contains(r#"<p:ph type="title"/>"#), "ph preserved: {s}");
    }

    // ─── Image crop + rotation tests (Req 10.1, 10.2) ──────────────────────

    /// A slide with a picture shape for crop/rotation testing.
    const PIC_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="3000" cy="4000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hello</a:t></a:r></a:p></p:txBody></p:sp><p:pic><p:nvPicPr><p:cNvPr id="3" name="Picture 2"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="500" y="600"/><a:ext cx="700" cy="800"/></a:xfrm></p:spPr></p:pic><p:sp><p:nvSpPr><p:cNvPr id="4" name="TextBox 3"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="900" y="1000"/><a:ext cx="1100" cy="1200"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>World</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn set_image_crop_creates_src_rect() {
        let mut dom = SlideDom::parse(PIC_SLIDE).unwrap();
        // Shape index 1 is the picture.
        dom.set_image_crop(1, 10000, 20000, 30000, 40000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"l="10000""#), "left crop: {s}");
        assert!(s.contains(r#"t="20000""#), "top crop: {s}");
        assert!(s.contains(r#"r="30000""#), "right crop: {s}");
        assert!(s.contains(r#"b="40000""#), "bottom crop: {s}");
        assert!(s.contains("srcRect"), "srcRect element present: {s}");
    }

    #[test]
    fn set_image_crop_updates_existing_src_rect() {
        let mut dom = SlideDom::parse(PIC_SLIDE).unwrap();
        // Set crop twice — second call should update, not duplicate.
        dom.set_image_crop(1, 5000, 5000, 5000, 5000).unwrap();
        dom.set_image_crop(1, 25000, 15000, 25000, 15000).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"l="25000""#), "updated left: {s}");
        assert!(s.contains(r#"t="15000""#), "updated top: {s}");
        assert!(s.contains(r#"r="25000""#), "updated right: {s}");
        assert!(s.contains(r#"b="15000""#), "updated bottom: {s}");
        // Only one srcRect element.
        let count = s.matches("srcRect").count();
        assert_eq!(count, 1, "only one srcRect: {s}");
    }

    #[test]
    fn set_image_crop_zero_values() {
        let mut dom = SlideDom::parse(PIC_SLIDE).unwrap();
        dom.set_image_crop(1, 0, 0, 0, 0).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"l="0""#), "zero left: {s}");
        assert!(s.contains(r#"t="0""#), "zero top: {s}");
        assert!(s.contains(r#"r="0""#), "zero right: {s}");
        assert!(s.contains(r#"b="0""#), "zero bottom: {s}");
    }

    #[test]
    fn set_image_crop_error_on_non_picture() {
        let mut dom = SlideDom::parse(PIC_SLIDE).unwrap();
        // Shape index 0 is a regular sp, not a pic.
        let err = dom.set_image_crop(0, 10000, 10000, 10000, 10000);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("not a picture"), "got: {msg}");
    }

    #[test]
    fn set_shape_rotation_on_picture() {
        let mut dom = SlideDom::parse(PIC_SLIDE).unwrap();
        // Shape index 1 is the picture — set_shape_rotation works on all types.
        dom.set_shape_rotation(1, 5400000).unwrap(); // 90 degrees
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(s.contains(r#"rot="5400000""#), "rotation set: {s}");
    }

    #[test]
    fn set_image_crop_preserves_sibling_shapes() {
        let src = String::from_utf8(PIC_SLIDE.to_vec()).unwrap();
        let mut dom = SlideDom::parse(PIC_SLIDE).unwrap();
        dom.set_image_crop(1, 10000, 20000, 30000, 40000).unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        // Title shape (shape 0) preserved byte-for-byte.
        let title_anchor = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/>"#;
        let pic_anchor = "<p:pic>";
        let src_title = &src[src.find(title_anchor).unwrap()..src.find(pic_anchor).unwrap()];
        let out_title = &out[out.find(title_anchor).unwrap()..out.find(pic_anchor).unwrap()];
        assert_eq!(src_title, out_title, "title shape preserved");
        // TextBox shape (shape 2) preserved byte-for-byte.
        assert!(
            out.contains("<a:t>World</a:t>"),
            "textbox text preserved: {out}"
        );
        let tb_anchor = r#"<p:sp><p:nvSpPr><p:cNvPr id="4" name="TextBox 3"/>"#;
        let src_tb = &src[src.find(tb_anchor).unwrap()..];
        let out_tb = &out[out.find(tb_anchor).unwrap()..];
        assert_eq!(src_tb, out_tb, "textbox shape preserved");
    }

    // ─── Run hyperlink tests (Req 11.1, 11.3) ─────────────────────────────

    /// A slide with two shapes, each with runs — for hyperlink testing.
    const HLINK_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" b="1"/><a:t>Click here</a:t></a:r><a:r><a:rPr lang="en-US"/><a:t> for more</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Body 2"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>No link</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn set_run_hyperlink_creates_hlinkclick_with_correct_rid() {
        let mut dom = SlideDom::parse(HLINK_SLIDE).unwrap();
        dom.set_run_hyperlink(0, 0, 0, "https://example.com", "rId5")
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // hlinkClick with the correct r:id is present inside the first run's rPr.
        assert!(
            s.contains(r#"<a:hlinkClick r:id="rId5"/>"#),
            "hlinkClick present: {s}"
        );
        // The run text is preserved.
        assert!(
            s.contains("<a:t>Click here</a:t>"),
            "run text preserved: {s}"
        );
        // The rPr attributes (lang, b) are preserved.
        assert!(s.contains(r#"lang="en-US""#), "lang preserved: {s}");
        assert!(s.contains(r#"b="1""#), "bold preserved: {s}");
    }

    #[test]
    fn set_run_hyperlink_replaces_existing() {
        let mut dom = SlideDom::parse(HLINK_SLIDE).unwrap();
        // Set a hyperlink, then replace it with a different one.
        dom.set_run_hyperlink(0, 0, 0, "https://old.com", "rId3")
            .unwrap();
        dom.set_run_hyperlink(0, 0, 0, "https://new.com", "rId7")
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Only the new r:id should be present.
        assert!(
            s.contains(r#"<a:hlinkClick r:id="rId7"/>"#),
            "new hlinkClick: {s}"
        );
        assert!(!s.contains(r#"r:id="rId3""#), "old hlinkClick removed: {s}");
        // Only one hlinkClick element in the entire document.
        assert_eq!(
            s.matches("hlinkClick").count(),
            1,
            "exactly one hlinkClick element: {s}"
        );
    }

    #[test]
    fn set_run_hyperlink_preserves_sibling_runs_byte_for_byte() {
        let src = String::from_utf8(HLINK_SLIDE.to_vec()).unwrap();
        let mut dom = SlideDom::parse(HLINK_SLIDE).unwrap();
        dom.set_run_hyperlink(0, 0, 0, "https://example.com", "rId5")
            .unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        // The second run in the same paragraph is byte-identical.
        let sibling_run = r#"<a:r><a:rPr lang="en-US"/><a:t> for more</a:t></a:r>"#;
        assert!(out.contains(sibling_run), "sibling run preserved: {out}");
        // The second shape is byte-identical.
        let body_anchor = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Body 2"/>"#;
        let src_body = &src[src.find(body_anchor).unwrap()..];
        let out_body = &out[out.find(body_anchor).unwrap()..];
        assert_eq!(src_body, out_body, "second shape preserved byte-for-byte");
    }

    #[test]
    fn set_run_hyperlink_error_on_invalid_shape_index() {
        let mut dom = SlideDom::parse(HLINK_SLIDE).unwrap();
        let err = dom.set_run_hyperlink(99, 0, 0, "https://example.com", "rId1");
        assert!(err.is_err());
    }

    #[test]
    fn set_run_hyperlink_error_on_invalid_para_index() {
        let mut dom = SlideDom::parse(HLINK_SLIDE).unwrap();
        let err = dom.set_run_hyperlink(0, 99, 0, "https://example.com", "rId1");
        assert!(err.is_err());
    }

    #[test]
    fn set_run_hyperlink_error_on_invalid_run_index() {
        let mut dom = SlideDom::parse(HLINK_SLIDE).unwrap();
        let err = dom.set_run_hyperlink(0, 0, 99, "https://example.com", "rId1");
        assert!(err.is_err());
    }

    #[test]
    fn set_run_hyperlink_creates_rpr_when_missing() {
        // The second shape's run has no rPr — setting a hyperlink should create one.
        let mut dom = SlideDom::parse(HLINK_SLIDE).unwrap();
        dom.set_run_hyperlink(1, 0, 0, "https://example.com", "rId2")
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // The run now has an rPr with hlinkClick.
        assert!(
            s.contains(r#"<a:rPr><a:hlinkClick r:id="rId2"/></a:rPr><a:t>No link</a:t>"#),
            "rPr created with hlinkClick: {s}"
        );
    }

    // ─── Shape click action tests (Req 11.2, 11.3) ───────────────────────

    /// A slide with two shapes for shape click action testing.
    const CLICK_ACTION_SLIDE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>First</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Shape 2"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Second</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn set_shape_click_action_external_url() {
        let mut dom = SlideDom::parse(CLICK_ACTION_SLIDE).unwrap();
        dom.set_shape_click_action(
            0,
            &ClickAction::ExternalUrl {
                r_id: "rId4".to_string(),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:hlinkClick r:id="rId4"/>"#),
            "hlinkClick with r:id present: {s}"
        );
        // It should be inside the cNvPr of the first shape.
        let cnvpr_pos = s.find(r#"<p:cNvPr id="2" name="Shape 1">"#).unwrap();
        let hlink_pos = s.find(r#"<a:hlinkClick r:id="rId4"/>"#).unwrap();
        assert!(hlink_pos > cnvpr_pos, "hlinkClick is inside cNvPr: {s}");
    }

    #[test]
    fn set_shape_click_action_jump_to_slide() {
        let mut dom = SlideDom::parse(CLICK_ACTION_SLIDE).unwrap();
        dom.set_shape_click_action(
            0,
            &ClickAction::JumpToSlide {
                r_id: "rId6".to_string(),
                action: "ppaction://hlinksldjump".to_string(),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:hlinkClick r:id="rId6" action="ppaction://hlinksldjump"/>"#),
            "hlinkClick with r:id and action present: {s}"
        );
    }

    #[test]
    fn set_shape_click_action_replaces_existing() {
        let mut dom = SlideDom::parse(CLICK_ACTION_SLIDE).unwrap();
        // Set an external URL first.
        dom.set_shape_click_action(
            0,
            &ClickAction::ExternalUrl {
                r_id: "rId3".to_string(),
            },
        )
        .unwrap();
        // Replace with a jump-to-slide action.
        dom.set_shape_click_action(
            0,
            &ClickAction::JumpToSlide {
                r_id: "rId8".to_string(),
                action: "ppaction://hlinksldjump".to_string(),
            },
        )
        .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Only the new action should be present.
        assert!(
            s.contains(r#"<a:hlinkClick r:id="rId8" action="ppaction://hlinksldjump"/>"#),
            "new hlinkClick present: {s}"
        );
        assert!(!s.contains(r#"r:id="rId3""#), "old hlinkClick removed: {s}");
        // Only one hlinkClick in the first shape's cNvPr.
        assert_eq!(
            s.matches("hlinkClick").count(),
            1,
            "exactly one hlinkClick element: {s}"
        );
    }

    #[test]
    fn set_shape_click_action_preserves_sibling_shapes() {
        let src = String::from_utf8(CLICK_ACTION_SLIDE.to_vec()).unwrap();
        let mut dom = SlideDom::parse(CLICK_ACTION_SLIDE).unwrap();
        dom.set_shape_click_action(
            0,
            &ClickAction::ExternalUrl {
                r_id: "rId5".to_string(),
            },
        )
        .unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        // The second shape should be byte-identical to the source.
        let anchor = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Shape 2"/>"#;
        assert_eq!(
            &out[out.find(anchor).unwrap()..],
            &src[src.find(anchor).unwrap()..],
            "sibling shape preserved byte-for-byte"
        );
    }

    #[test]
    fn set_shape_click_action_error_on_invalid_shape_index() {
        let mut dom = SlideDom::parse(CLICK_ACTION_SLIDE).unwrap();
        let err = dom.set_shape_click_action(
            99,
            &ClickAction::ExternalUrl {
                r_id: "rId1".to_string(),
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn add_autoshape_emits_prst_geom() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let id = dom
            .add_autoshape("star5", 100000, 200000, 300000, 400000)
            .unwrap();
        assert!(id > 0);
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:prstGeom prst="star5"><a:avLst/></a:prstGeom>"#),
            "prstGeom with star5 present: {s}"
        );
        assert!(
            s.contains(r#"<a:off x="100000" y="200000"/>"#),
            "position correct: {s}"
        );
        assert!(
            s.contains(r#"<a:ext cx="300000" cy="400000"/>"#),
            "size correct: {s}"
        );
        // Original shapes preserved.
        assert!(s.contains("<a:t>Old Title</a:t>"), "title preserved: {s}");
        assert!(s.contains("<a:t>Bullet one</a:t>"), "body preserved: {s}");
    }

    #[test]
    fn add_autoshape_various_presets() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        for prst in &[
            "rect",
            "ellipse",
            "roundRect",
            "flowChartDecision",
            "actionButtonHome",
        ] {
            let id = dom.add_autoshape(prst, 0, 0, 100000, 100000).unwrap();
            assert!(id > 0);
            let s = String::from_utf8(dom.to_bytes()).unwrap();
            assert!(
                s.contains(&format!(
                    r#"<a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom>"#
                )),
                "prstGeom with {prst} present: {s}"
            );
        }
    }

    // ─── Connector tests (Req 9.2) ─────────────────────────────────────────

    #[test]
    fn add_straight_connector_creates_correct_xml() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let id = dom
            .add_connector(ConnectorType::Straight, None, None, 100, 200, 5000, 0)
            .unwrap();
        assert_eq!(id, 4); // existing shapes use ids 2 and 3
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Verify the cxnSp element structure.
        assert!(s.contains("<p:cxnSp>"), "cxnSp present: {s}");
        assert!(
            s.contains(&format!(r#"<p:cNvPr id="{id}" name="Connector {id}"/>"#)),
            "cNvPr with correct id/name: {s}"
        );
        assert!(
            s.contains("<p:cNvCxnSpPr/>"),
            "self-closing cNvCxnSpPr (no anchors): {s}"
        );
        assert!(
            s.contains(r#"<a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom>"#),
            "straight preset: {s}"
        );
        assert!(s.contains(r#"<a:off x="100" y="200"/>"#), "position: {s}");
        assert!(s.contains(r#"<a:ext cx="5000" cy="0"/>"#), "size: {s}");
    }

    #[test]
    fn add_elbow_connector_with_anchors() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let start = ConnectorAnchor {
            shape_id: 2,
            connection_idx: 1,
        };
        let end = ConnectorAnchor {
            shape_id: 3,
            connection_idx: 3,
        };
        let id = dom
            .add_connector(
                ConnectorType::Elbow,
                Some(start),
                Some(end),
                0,
                0,
                3000,
                2000,
            )
            .unwrap();
        assert_eq!(id, 4);
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:prstGeom prst="bentConnector3"><a:avLst/></a:prstGeom>"#),
            "elbow preset: {s}"
        );
        assert!(
            s.contains(r#"<a:stCxn id="2" idx="1"/>"#),
            "start anchor: {s}"
        );
        assert!(
            s.contains(r#"<a:endCxn id="3" idx="3"/>"#),
            "end anchor: {s}"
        );
    }

    #[test]
    fn add_curved_connector() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let id = dom
            .add_connector(ConnectorType::Curved, None, None, 500, 600, 4000, 3000)
            .unwrap();
        assert_eq!(id, 4);
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        assert!(
            s.contains(r#"<a:prstGeom prst="curvedConnector3"><a:avLst/></a:prstGeom>"#),
            "curved preset: {s}"
        );
        assert!(s.contains(r#"<a:off x="500" y="600"/>"#), "position: {s}");
        assert!(s.contains(r#"<a:ext cx="4000" cy="3000"/>"#), "size: {s}");
    }

    #[test]
    fn connector_without_anchors_free_floating() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let id = dom
            .add_connector(ConnectorType::Straight, None, None, 0, 0, 1000, 1000)
            .unwrap();
        assert_eq!(id, 4);
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // No stCxn or endCxn elements.
        assert!(!s.contains("a:stCxn"), "no start anchor: {s}");
        assert!(!s.contains("a:endCxn"), "no end anchor: {s}");
        // cNvCxnSpPr is self-closing (empty).
        assert!(s.contains("<p:cNvCxnSpPr/>"), "empty cNvCxnSpPr: {s}");
    }

    #[test]
    fn connector_preserves_sibling_shapes() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.add_connector(ConnectorType::Elbow, None, None, 0, 0, 1000, 1000)
            .unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Both original shapes preserved verbatim.
        assert!(s.contains("<a:t>Old Title</a:t>"), "title preserved: {s}");
        assert!(s.contains("<a:t>Bullet one</a:t>"), "body preserved: {s}");
        assert!(s.contains("<a:t>Bullet two</a:t>"), "body preserved: {s}");
        assert!(
            s.contains(r#"<a:rPr lang="en-US" b="1"/>"#),
            "title rPr preserved: {s}"
        );
    }

    // ─── Freeform path builder tests (Req 9.3) ─────────────────────────────

    #[test]
    fn freeform_triangle_path_produces_correct_xml() {
        let mut path = FreeformPath::new(1000, 1000);
        path.move_to(500, 0)
            .line_to(1000, 1000)
            .line_to(0, 1000)
            .close();
        let xml = path.build_xml();
        assert!(
            xml.contains(r#"<a:path w="1000" h="1000">"#),
            "path dimensions: {xml}"
        );
        assert!(
            xml.contains(r#"<a:moveTo><a:pt x="500" y="0"/></a:moveTo>"#),
            "moveTo: {xml}"
        );
        assert!(
            xml.contains(r#"<a:lnTo><a:pt x="1000" y="1000"/></a:lnTo>"#),
            "first lineTo: {xml}"
        );
        assert!(
            xml.contains(r#"<a:lnTo><a:pt x="0" y="1000"/></a:lnTo>"#),
            "second lineTo: {xml}"
        );
        assert!(xml.contains("<a:close/>"), "close: {xml}");
        // Verify the custGeom wrapper structure.
        assert!(xml.contains("<a:custGeom>"), "custGeom open: {xml}");
        assert!(xml.contains("<a:avLst/>"), "avLst: {xml}");
        assert!(xml.contains("<a:gdLst/>"), "gdLst: {xml}");
        assert!(xml.contains("<a:ahLst/>"), "ahLst: {xml}");
        assert!(xml.contains("<a:cxnLst/>"), "cxnLst: {xml}");
        assert!(
            xml.contains(r#"<a:rect l="0" t="0" r="0" b="0"/>"#),
            "rect: {xml}"
        );
        assert!(xml.contains("<a:pathLst>"), "pathLst: {xml}");
    }

    #[test]
    fn freeform_cubic_path_produces_correct_xml() {
        let mut path = FreeformPath::new(2000, 1500);
        path.move_to(0, 750)
            .cubic_to(500, 0, 1500, 0, 2000, 750)
            .line_to(2000, 1500)
            .line_to(0, 1500)
            .close();
        let xml = path.build_xml();
        assert!(
            xml.contains(r#"<a:path w="2000" h="1500">"#),
            "path dimensions: {xml}"
        );
        assert!(
            xml.contains(r#"<a:moveTo><a:pt x="0" y="750"/></a:moveTo>"#),
            "moveTo: {xml}"
        );
        assert!(
            xml.contains(
                r#"<a:cubicBezTo><a:pt x="500" y="0"/><a:pt x="1500" y="0"/><a:pt x="2000" y="750"/></a:cubicBezTo>"#
            ),
            "cubicBezTo: {xml}"
        );
        assert!(
            xml.contains(r#"<a:lnTo><a:pt x="2000" y="1500"/></a:lnTo>"#),
            "lineTo after cubic: {xml}"
        );
        assert!(xml.contains("<a:close/>"), "close: {xml}");
    }

    #[test]
    fn add_freeform_inserts_shape_into_sptree() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let mut path = FreeformPath::new(500, 500);
        path.move_to(0, 0).line_to(500, 0).line_to(250, 500).close();
        let id = dom
            .add_freeform(&path, 100000, 200000, 300000, 400000)
            .unwrap();
        assert_eq!(id, 4); // existing shapes use ids 2 and 3
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Verify the shape is present with correct structure.
        assert!(s.contains("<p:sp>"), "sp element present");
        assert!(
            s.contains(&format!(r#"<p:cNvPr id="{id}" name="Freeform {id}"/>"#)),
            "cNvPr with correct id/name: {s}"
        );
        assert!(
            s.contains(r#"<a:off x="100000" y="200000"/>"#),
            "position: {s}"
        );
        assert!(
            s.contains(r#"<a:ext cx="300000" cy="400000"/>"#),
            "size: {s}"
        );
        // Verify custGeom is present (not prstGeom).
        assert!(s.contains("<a:custGeom>"), "custGeom present: {s}");
        assert!(
            s.contains(r#"<a:path w="500" h="500">"#),
            "path dimensions: {s}"
        );
        assert!(
            s.contains(r#"<a:moveTo><a:pt x="0" y="0"/></a:moveTo>"#),
            "moveTo in shape: {s}"
        );
        assert!(
            s.contains(r#"<a:lnTo><a:pt x="500" y="0"/></a:lnTo>"#),
            "lineTo in shape: {s}"
        );
        assert!(s.contains("<a:close/>"), "close in shape: {s}");
    }

    #[test]
    fn add_freeform_preserves_sibling_shapes() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        let mut path = FreeformPath::new(100, 100);
        path.move_to(0, 0).line_to(100, 100);
        dom.add_freeform(&path, 0, 0, 100, 100).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // Both original shapes preserved verbatim.
        assert!(s.contains("<a:t>Old Title</a:t>"), "title preserved: {s}");
        assert!(s.contains("<a:t>Bullet one</a:t>"), "body preserved: {s}");
        assert!(s.contains("<a:t>Bullet two</a:t>"), "body preserved: {s}");
        assert!(
            s.contains(r#"<a:rPr lang="en-US" b="1"/>"#),
            "title rPr preserved: {s}"
        );
    }

    // ─── Group shape tests (Req 9.4) ──────────────────────────────────────

    /// A slide with a group shape containing two child shapes (a rectangle and
    /// an ellipse), plus a sibling shape outside the group.
    const SLIDE_WITH_GROUP: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:grpSp><p:nvGrpSpPr><p:cNvPr id="2" name="Group 1"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="5000000" cy="3000000"/><a:chOff x="0" y="0"/><a:chExt cx="5000000" cy="3000000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="3" name="Rect 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="1000" cy="800"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="4" name="Ellipse 1"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="2000" y="500"/><a:ext cx="1500" cy="1500"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp></p:grpSp><p:sp><p:nvSpPr><p:cNvPr id="5" name="Outside Shape"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="6000000" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Outside</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;

    /// A slide with a nested group: outer group contains an inner group and a shape.
    const SLIDE_WITH_NESTED_GROUP: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:grpSp><p:nvGrpSpPr><p:cNvPr id="2" name="Outer Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="8000000" cy="5000000"/><a:chOff x="0" y="0"/><a:chExt cx="8000000" cy="5000000"/></a:xfrm></p:grpSpPr><p:grpSp><p:nvGrpSpPr><p:cNvPr id="3" name="Inner Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="4000000" cy="3000000"/><a:chOff x="0" y="0"/><a:chExt cx="4000000" cy="3000000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="4" name="Nested Rect"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100" y="100"/><a:ext cx="500" cy="500"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp></p:grpSp><p:sp><p:nvSpPr><p:cNvPr id="5" name="Sibling in Outer"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="5000000" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp></p:grpSp></p:spTree></p:cSld></p:sld>"#;

    #[test]
    fn group_shapes_returns_child_shapes() {
        let dom = SlideDom::parse(SLIDE_WITH_GROUP).unwrap();
        // Shape 0 is the group (grpSp), shape 1 is the outside shape.
        let children = dom.group_shapes(0).unwrap();
        assert_eq!(children.len(), 2, "group has 2 child shapes");
        // First child is the rectangle.
        assert_eq!(children[0].local_name(), b"sp");
        let cnvpr0 = children[0].find_descendant(b"cNvPr").unwrap();
        assert_eq!(cnvpr0.attr(b"name").unwrap(), b"Rect 1");
        // Second child is the ellipse.
        assert_eq!(children[1].local_name(), b"sp");
        let cnvpr1 = children[1].find_descendant(b"cNvPr").unwrap();
        assert_eq!(cnvpr1.attr(b"name").unwrap(), b"Ellipse 1");
    }

    #[test]
    fn group_shapes_error_when_not_a_group() {
        let dom = SlideDom::parse(SLIDE_WITH_GROUP).unwrap();
        // Shape 1 is a regular sp, not a group.
        let result = dom.group_shapes(1);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a group"),
            "error mentions not a group: {err_msg}"
        );
    }

    #[test]
    fn group_shapes_error_on_out_of_range() {
        let dom = SlideDom::parse(SLIDE_WITH_GROUP).unwrap();
        let result = dom.group_shapes(99);
        assert!(result.is_err());
    }

    #[test]
    fn nested_groups_can_be_traversed() {
        let dom = SlideDom::parse(SLIDE_WITH_NESTED_GROUP).unwrap();
        // Shape 0 is the outer group.
        let outer_children = dom.group_shapes(0).unwrap();
        assert_eq!(
            outer_children.len(),
            2,
            "outer group has 2 children (inner group + shape)"
        );
        // First child is the inner group.
        assert_eq!(outer_children[0].local_name(), b"grpSp");
        let inner_cnvpr = outer_children[0].find_descendant(b"cNvPr").unwrap();
        assert_eq!(inner_cnvpr.attr(b"name").unwrap(), b"Inner Group");
        // Second child is a sibling shape in the outer group.
        assert_eq!(outer_children[1].local_name(), b"sp");
        let sibling_cnvpr = outer_children[1].find_descendant(b"cNvPr").unwrap();
        assert_eq!(sibling_cnvpr.attr(b"name").unwrap(), b"Sibling in Outer");

        // We can also traverse the inner group's children by finding it in the
        // outer group's child list. The inner group contains one shape.
        let inner_grp = outer_children[0];
        let child_shape_locals: &[&[u8]] = &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"];
        let inner_children: Vec<&Element> = inner_grp
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) if child_shape_locals.contains(&e.local_name()) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(inner_children.len(), 1, "inner group has 1 child shape");
        let nested_cnvpr = inner_children[0].find_descendant(b"cNvPr").unwrap();
        assert_eq!(nested_cnvpr.attr(b"name").unwrap(), b"Nested Rect");
    }

    #[test]
    fn add_shape_to_group_inserts_correctly() {
        let mut dom = SlideDom::parse(SLIDE_WITH_GROUP).unwrap();
        let id = dom
            .add_shape_to_group(0, "roundRect", 500, 600, 2000, 1000)
            .unwrap();
        // Existing shapes use ids 2..5, so new id is 6.
        assert_eq!(id, 6);
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // The new shape is inside the group.
        assert!(
            s.contains(&format!(r#"<p:cNvPr id="{id}" name="Shape {id}"/>"#)),
            "new shape has correct id/name: {s}"
        );
        assert!(
            s.contains(r#"<a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom>"#),
            "preset geometry correct: {s}"
        );
        // Verify the new shape is inside the grpSp (before the closing </p:grpSp>).
        let grp_end = s.find("</p:grpSp>").expect("grpSp closing tag");
        let new_shape_pos = s
            .find(&format!(r#"id="{id}""#))
            .expect("new shape in output");
        assert!(
            new_shape_pos < grp_end,
            "new shape is inside the group element"
        );
        // Group now has 3 child shapes.
        let children = dom.group_shapes(0).unwrap();
        assert_eq!(children.len(), 3, "group now has 3 children");
    }

    #[test]
    fn add_shape_to_group_error_when_not_a_group() {
        let mut dom = SlideDom::parse(SLIDE_WITH_GROUP).unwrap();
        // Shape 1 is a regular sp, not a group.
        let result = dom.add_shape_to_group(1, "rect", 0, 0, 100, 100);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a group"),
            "error mentions not a group: {err_msg}"
        );
    }

    #[test]
    fn add_shape_to_group_preserves_sibling_shapes_outside() {
        let mut dom = SlideDom::parse(SLIDE_WITH_GROUP).unwrap();
        dom.add_shape_to_group(0, "rect", 0, 0, 100, 100).unwrap();
        let s = String::from_utf8(dom.to_bytes()).unwrap();
        // The outside shape is preserved verbatim.
        assert!(
            s.contains(r#"<p:cNvPr id="5" name="Outside Shape"/>"#),
            "outside shape preserved: {s}"
        );
        assert!(
            s.contains("<a:t>Outside</a:t>"),
            "outside shape text preserved: {s}"
        );
        // The original group children are preserved.
        assert!(s.contains(r#"name="Rect 1""#), "rect preserved: {s}");
        assert!(s.contains(r#"name="Ellipse 1""#), "ellipse preserved: {s}");
    }

    #[test]
    fn group_slide_round_trips_byte_for_byte_when_unedited() {
        let dom = SlideDom::parse(SLIDE_WITH_GROUP).unwrap();
        assert_eq!(dom.to_bytes(), SLIDE_WITH_GROUP);
    }
}
