//! High-level slide model and editing handle.
//!
//! A created slide owns a list of DrawingML shapes; serialization wraps them in
//! the canonical blank-slide shell (matching the part PowerPoint accepts). Title
//! and body placeholders are placed with explicit geometry derived from the
//! slide size, so they render regardless of the (single, Phase 0) layout —
//! per-layout placeholder geometry resolution is a later (layout) phase.

use zavora_slide_oxml::{Paragraph, Run, RunProps, Shape, TextBody};

use crate::error::{Result, SlideError};
use crate::units::Emu;

/// One bullet line for [`Slide::add_bullets`].
#[derive(Debug, Clone)]
pub struct Bullet {
    pub text: String,
    pub level: u8,
    pub bold: bool,
}

impl Bullet {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), level: 0, bold: false }
    }
}

/// A shape/slide fill.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    /// Solid color (hex, with or without `#`).
    Solid(String),
}

impl Fill {
    fn bg_xml(&self) -> String {
        match self {
            Fill::Solid(hex) => {
                let h = hex.trim_start_matches('#').to_uppercase();
                format!(
                    "<p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"{h}\"/></a:solidFill>\
                     <a:effectLst/></p:bgPr></p:bg>"
                )
            }
        }
    }
}

/// Source of an image to embed.
#[derive(Debug, Clone)]
pub enum ImageSrc {
    /// Read from a filesystem path (extension determines the format).
    Path(std::path::PathBuf),
    /// Raw bytes with an explicit extension ("png", "jpg", "jpeg").
    Bytes { data: Vec<u8>, ext: String },
}

/// An embedded image: its media bytes/extension plus placement.
#[derive(Debug, Clone)]
pub struct ImageMedia {
    pub ext: String,
    pub data: Vec<u8>,
    pub embed_rid: String,
    pub id: u32,
    pub x: i64,
    pub y: i64,
    pub cx: i64,
    pub cy: i64,
}

impl ImageMedia {
    fn pic_xml(&self) -> String {
        format!(
            "<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"Picture {id}\"/>\
             <p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
             <p:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>\
             <p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
            id = self.id,
            rid = self.embed_rid,
            x = self.x,
            y = self.y,
            cx = self.cx,
            cy = self.cy
        )
    }
}

/// Identifies a table within a slide (its index in `SlideData::tables`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableId(pub usize);

/// A table rendered as a `p:graphicFrame` / `a:tbl`.
#[derive(Debug, Clone)]
pub struct Table {
    pub id: u32,
    pub x: i64,
    pub y: i64,
    pub cx: i64,
    pub cy: i64,
    pub rows: usize,
    pub cols: usize,
    /// Cell text in row-major order (`rows * cols` entries).
    pub cells: Vec<String>,
}

impl Table {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    fn graphic_frame_xml(&self) -> String {
        let col_w = if self.cols > 0 { self.cx / self.cols as i64 } else { self.cx };
        let row_h = if self.rows > 0 { self.cy / self.rows as i64 } else { self.cy };
        let grid: String = (0..self.cols)
            .map(|_| format!("<a:gridCol w=\"{col_w}\"/>"))
            .collect();
        let mut rows_xml = String::new();
        for r in 0..self.rows {
            rows_xml.push_str(&format!("<a:tr h=\"{row_h}\">"));
            for c in 0..self.cols {
                let text = self.cells.get(r * self.cols + c).map(String::as_str).unwrap_or("");
                rows_xml.push_str(&format!(
                    "<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{}</a:t></a:r></a:p>\
                     </a:txBody><a:tcPr/></a:tc>",
                    Self::esc(text)
                ));
            }
            rows_xml.push_str("</a:tr>");
        }
        format!(
            "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{id}\" name=\"Table {id}\"/>\
             <p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>\
             <p:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></p:xfrm>\
             <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\">\
             <a:tbl><a:tblPr firstRow=\"1\" bandRow=\"1\">\
             <a:tableStyleId>{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}</a:tableStyleId></a:tblPr>\
             <a:tblGrid>{grid}</a:tblGrid>{rows_xml}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>",
            id = self.id,
            x = self.x,
            y = self.y,
            cx = self.cx,
            cy = self.cy
        )
    }
}

/// Stored slide content: the shapes injected into the slide's `spTree`.
#[derive(Debug, Clone, Default)]
pub struct SlideData {
    pub shapes: Vec<Shape>,
    /// Embedded images (rendered as `p:pic` and written as media parts on save).
    pub images: Vec<ImageMedia>,
    /// Tables (rendered as `p:graphicFrame` / `a:tbl`).
    pub tables: Vec<Table>,
    /// Speaker notes text, if any (emitted as a notesSlide part on save).
    pub notes: Option<String>,
    /// Optional slide background fill.
    pub background: Option<Fill>,
    /// Next shape id (group shape is id 1, so authored shapes start at 2).
    next_id: u32,
}

impl SlideData {
    pub fn new() -> Self {
        Self {
            shapes: Vec::new(),
            images: Vec::new(),
            tables: Vec::new(),
            notes: None,
            background: None,
            next_id: 2,
        }
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Find the first placeholder shape of the given type.
    fn find_ph(&mut self, ph_type: &str) -> Option<&mut Shape> {
        self.shapes
            .iter_mut()
            .find(|s| s.placeholder.as_ref().is_some_and(|p| p.ph_type == ph_type))
    }

    /// Serialize to a complete slide part.
    pub fn to_xml(&self) -> Vec<u8> {
        let shapes: String = self.shapes.iter().map(Shape::to_xml).collect();
        let pics: String = self.images.iter().map(ImageMedia::pic_xml).collect();
        let tbls: String = self.tables.iter().map(Table::graphic_frame_xml).collect();
        let bg = self.background.as_ref().map(Fill::bg_xml).unwrap_or_default();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
             xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
             <p:cSld>{bg}<p:spTree>\
             <p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
             <p:grpSpPr/>{shapes}{pics}{tbls}</p:spTree></p:cSld>\
             <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
        )
        .into_bytes()
    }
}

/// A mutable handle to one slide, aware of the deck's slide size for placeholder
/// geometry.
pub struct Slide<'a> {
    pub(crate) data: &'a mut SlideData,
    pub(crate) slide_cx: i64,
    pub(crate) slide_cy: i64,
}

impl Slide<'_> {
    const MARGIN: i64 = 457200; // 0.5"
    const TITLE_H: i64 = 1143000; // 1.25"

    fn title_box(&self) -> (i64, i64, i64, i64) {
        (Self::MARGIN, Self::MARGIN, self.slide_cx - 2 * Self::MARGIN, Self::TITLE_H)
    }

    fn body_box(&self) -> (i64, i64, i64, i64) {
        let y = 2 * Self::MARGIN + Self::TITLE_H;
        (Self::MARGIN, y, self.slide_cx - 2 * Self::MARGIN, self.slide_cy - y - Self::MARGIN)
    }

    /// Set the slide title (creates or replaces the title placeholder).
    pub fn set_title(&mut self, text: &str) -> Result<()> {
        let body = TextBody { paragraphs: vec![Paragraph { runs: vec![Run::new(text)], ..Default::default() }] };
        if let Some(sp) = self.data.find_ph("title") {
            sp.body = body;
        } else {
            let id = self.data.alloc_id();
            let mut sp = Shape::placeholder(id, "Title 1", "title", None, body);
            sp.xfrm = Some(self.title_box());
            self.data.shapes.push(sp);
        }
        Ok(())
    }

    /// Populate the body placeholder with one paragraph per bullet.
    pub fn add_bullets(&mut self, items: &[Bullet]) -> Result<()> {
        let paragraphs = items
            .iter()
            .map(|b| Paragraph {
                runs: vec![Run { text: b.text.clone(), props: RunProps { bold: b.bold.then_some(true), ..Default::default() } }],
                level: Some(b.level),
                ..Default::default()
            })
            .collect();
        let body = TextBody { paragraphs };
        if let Some(sp) = self.data.find_ph("body") {
            sp.body = body;
        } else {
            let id = self.data.alloc_id();
            let mut sp = Shape::placeholder(id, "Content 1", "body", Some(1), body);
            sp.xfrm = Some(self.body_box());
            self.data.shapes.push(sp);
        }
        Ok(())
    }

    /// Add a positioned text box. Returns a mutable reference to the shape so
    /// callers can adjust run formatting.
    pub fn add_text_box(&mut self, text: &str, x: Emu, y: Emu, w: Emu, h: Emu) -> &mut Shape {
        let id = self.data.alloc_id();
        let body = TextBody { paragraphs: vec![Paragraph { runs: vec![Run::new(text)], ..Default::default() }] };
        let sp = Shape::text_box(id, x.0, y.0, w.0, h.0, body);
        self.data.shapes.push(sp);
        self.data.shapes.last_mut().unwrap()
    }

    /// Add an auto-shape with the given preset geometry. Returns a mutable
    /// reference so callers can set fill/outline.
    pub fn add_shape(&mut self, preset: crate::units::ShapePreset, x: Emu, y: Emu, w: Emu, h: Emu) -> &mut Shape {
        let id = self.data.alloc_id();
        let sp = Shape::auto_shape(id, preset.prst(), x.0, y.0, w.0, h.0);
        self.data.shapes.push(sp);
        self.data.shapes.last_mut().unwrap()
    }

    /// Add a `rows`×`cols` table at the given position/size. Returns its id for
    /// addressing cells via [`Slide::set_table_cell`].
    pub fn add_table(&mut self, rows: usize, cols: usize, x: Emu, y: Emu, w: Emu, h: Emu) -> TableId {
        let id = self.data.alloc_id();
        self.data.tables.push(Table {
            id,
            x: x.0,
            y: y.0,
            cx: w.0,
            cy: h.0,
            rows,
            cols,
            cells: vec![String::new(); rows * cols],
        });
        TableId(self.data.tables.len() - 1)
    }

    /// Set the text of a table cell.
    pub fn set_table_cell(&mut self, table: TableId, row: usize, col: usize, text: &str) -> Result<()> {
        let t = self
            .data
            .tables
            .get_mut(table.0)
            .ok_or_else(|| SlideError::NotFound(format!("table {}", table.0)))?;
        if row >= t.rows || col >= t.cols {
            return Err(SlideError::InvalidInput(format!(
                "cell ({row},{col}) out of bounds for {}x{} table",
                t.rows, t.cols
            )));
        }
        t.cells[row * t.cols + col] = text.to_string();
        Ok(())
    }

    /// Set (or replace) the slide's speaker notes.
    pub fn set_notes(&mut self, text: &str) {
        self.data.notes = Some(text.to_string());
    }

    /// Embed an image at the given EMU position/size. PNG and JPEG supported.
    pub fn add_image(&mut self, src: ImageSrc, x: Emu, y: Emu, w: Emu, h: Emu) -> Result<()> {
        let (data, ext) = match src {
            ImageSrc::Path(p) => {
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .ok_or_else(|| SlideError::InvalidInput("image path has no extension".into()))?;
                let data = std::fs::read(&p)?;
                (data, ext)
            }
            ImageSrc::Bytes { data, ext } => (data, ext.to_ascii_lowercase()),
        };
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
            return Err(SlideError::InvalidInput(format!("unsupported image type '{ext}'")));
        }
        let id = self.data.alloc_id();
        // Embed rel ids start at rId10 to stay clear of layout(rId1)/notes(rId2).
        let embed_rid = format!("rId{}", 10 + self.data.images.len());
        self.data.images.push(ImageMedia {
            ext,
            data,
            embed_rid,
            id,
            x: x.0,
            y: y.0,
            cx: w.0,
            cy: h.0,
        });
        Ok(())
    }

    /// Set the slide background fill.
    pub fn set_background(&mut self, fill: Fill) {
        self.data.background = Some(fill);
    }

    /// Speaker notes text, if any.
    pub fn notes(&self) -> Option<&str> {
        self.data.notes.as_deref()
    }

    /// A summary of the slide's shapes (kind + extracted text).
    pub fn shapes(&self) -> Vec<ShapeInfo> {
        self.data
            .shapes
            .iter()
            .map(|sp| ShapeInfo {
                kind: match &sp.placeholder {
                    Some(ph) => ph.ph_type.clone(),
                    None if sp.text_box => "textbox".to_string(),
                    None => "shape".to_string(),
                },
                text: sp.body.paragraphs.iter().map(|p| p.text()).collect::<Vec<_>>().join("\n"),
            })
            .collect()
    }

    /// Extracted plain text of all shapes (one paragraph per line).
    pub fn text(&self) -> String {
        let mut lines = Vec::new();
        for sp in &self.data.shapes {
            for p in &sp.body.paragraphs {
                lines.push(p.text());
            }
        }
        lines.join("\n")
    }
}

/// A lightweight description of one shape, for read/inspection.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeInfo {
    /// Placeholder type (e.g. "title", "body"), "textbox", or "shape".
    pub kind: String,
    pub text: String,
}

/// Build a canonical notesSlide part for the given notes text. Mirrors the
/// structure PowerPoint emits: a slide-image placeholder and a body
/// placeholder carrying the notes.
pub(crate) fn notes_slide_xml(notes: &str) -> Vec<u8> {
    let esc = notes
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <p:notes xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
         <p:cSld><p:spTree>\
         <p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
         <p:grpSpPr/>\
         <p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Slide Image Placeholder 1\"/>\
         <p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>\
         <p:nvPr><p:ph type=\"sldImg\" idx=\"1\"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>\
         <p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Notes Placeholder 2\"/>\
         <p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>\
         <p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr><p:spPr/>\
         <p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{esc}</a:t></a:r></a:p></p:txBody></p:sp>\
         </p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slide(data: &mut SlideData) -> Slide<'_> {
        Slide { data, slide_cx: 12192000, slide_cy: 6858000 }
    }

    #[test]
    fn title_and_bullets_populate_placeholders() {
        let mut d = SlideData::new();
        {
            let mut s = slide(&mut d);
            s.set_title("Quarterly Review").unwrap();
            s.add_bullets(&[
                Bullet::new("Revenue up 23%"),
                Bullet { text: "EMEA".into(), level: 1, bold: false },
            ])
            .unwrap();
        }
        assert_eq!(d.shapes.len(), 2);
        let xml = String::from_utf8(d.to_xml()).unwrap();
        assert!(xml.contains("<p:ph type=\"title\"/>"));
        assert!(xml.contains("<a:t>Quarterly Review</a:t>"));
        assert!(xml.contains("<p:ph type=\"body\" idx=\"1\"/>"));
        assert!(xml.contains("<a:t>Revenue up 23%</a:t>"));
        assert!(xml.contains("lvl=\"1\""));
    }

    #[test]
    fn set_title_replaces_not_duplicates() {
        let mut d = SlideData::new();
        slide(&mut d).set_title("A").unwrap();
        slide(&mut d).set_title("B").unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(slide(&mut d).text(), "B");
    }

    #[test]
    fn text_box_with_fluent_format() {
        let mut d = SlideData::new();
        {
            let mut s = slide(&mut d);
            s.add_text_box("Hi", Emu::inches(1.0), Emu::inches(1.0), Emu::inches(2.0), Emu::inches(0.5))
                .bold(true)
                .color("#FF0000")
                .size(18.0);
        }
        let xml = String::from_utf8(d.to_xml()).unwrap();
        assert!(xml.contains("txBox=\"1\""));
        assert!(xml.contains("b=\"1\""));
        assert!(xml.contains("<a:srgbClr val=\"FF0000\"/>"));
        assert!(xml.contains("sz=\"1800\""));
    }

    #[test]
    fn notes_and_shape_inventory() {
        let mut d = SlideData::new();
        {
            let mut s = slide(&mut d);
            s.set_title("T").unwrap();
            s.add_bullets(&[Bullet::new("one")]).unwrap();
            s.set_notes("remember this");
            assert_eq!(s.notes(), Some("remember this"));
            let inv = s.shapes();
            assert_eq!(inv.len(), 2);
            assert_eq!(inv[0].kind, "title");
            assert_eq!(inv[1].kind, "body");
            assert_eq!(inv[1].text, "one");
        }
        assert_eq!(d.notes.as_deref(), Some("remember this"));
    }

    #[test]
    fn notes_slide_part_escapes() {
        let xml = String::from_utf8(notes_slide_xml("a < b & c")).unwrap();
        assert!(xml.contains("<p:ph type=\"body\""));
        assert!(xml.contains("a &lt; b &amp; c"));
    }

    #[test]
    fn background_emitted_in_csld() {
        let mut d = SlideData::new();
        slide(&mut d).set_background(Fill::Solid("#102030".into()));
        let xml = String::from_utf8(d.to_xml()).unwrap();
        // bg precedes spTree inside cSld.
        let bg = xml.find("<p:bg>").unwrap();
        let tree = xml.find("<p:spTree>").unwrap();
        assert!(bg < tree);
        assert!(xml.contains("<a:srgbClr val=\"102030\"/>"));
    }

    #[test]
    fn add_image_from_bytes_emits_pic() {
        let mut d = SlideData::new();
        slide(&mut d)
            .add_image(
                ImageSrc::Bytes { data: vec![1, 2, 3], ext: "PNG".into() },
                Emu::inches(1.0),
                Emu::inches(1.0),
                Emu::inches(2.0),
                Emu::inches(2.0),
            )
            .unwrap();
        assert_eq!(d.images.len(), 1);
        assert_eq!(d.images[0].ext, "png");
        assert_eq!(d.images[0].embed_rid, "rId10");
        let xml = String::from_utf8(d.to_xml()).unwrap();
        assert!(xml.contains("<p:pic>"));
        assert!(xml.contains("r:embed=\"rId10\""));
    }

    #[test]
    fn add_image_rejects_unsupported_type() {
        let mut d = SlideData::new();
        let r = slide(&mut d).add_image(
            ImageSrc::Bytes { data: Vec::<u8>::new(), ext: "gif".into() },
            Emu::inches(0.0),
            Emu::inches(0.0),
            Emu::inches(1.0),
            Emu::inches(1.0),
        );
        assert!(r.is_err());
    }

    #[test]
    fn add_shape_with_fill_and_outline() {
        use crate::units::ShapePreset;
        let mut d = SlideData::new();
        {
            let mut s = slide(&mut d);
            let sp = s.add_shape(ShapePreset::Ellipse, Emu::inches(1.0), Emu::inches(1.0), Emu::inches(2.0), Emu::inches(2.0));
            sp.set_fill("#00AA00").set_outline("#000000", 2.0);
        }
        let xml = String::from_utf8(d.to_xml()).unwrap();
        assert!(xml.contains("prst=\"ellipse\""));
        assert!(xml.contains("<a:srgbClr val=\"00AA00\"/>"));
        assert!(xml.contains("<a:ln w=\"25400\">"));
    }

    #[test]
    fn add_table_and_set_cells() {
        let mut d = SlideData::new();
        {
            let mut s = slide(&mut d);
            let t = s.add_table(2, 2, Emu::inches(1.0), Emu::inches(1.0), Emu::inches(4.0), Emu::inches(2.0));
            s.set_table_cell(t, 0, 0, "H1").unwrap();
            s.set_table_cell(t, 1, 1, "v & w").unwrap();
            assert!(s.set_table_cell(t, 5, 0, "x").is_err());
        }
        let xml = String::from_utf8(d.to_xml()).unwrap();
        assert!(xml.contains("graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\""));
        assert!(xml.contains("<a:gridCol"));
        assert!(xml.contains("<a:t>H1</a:t>"));
        assert!(xml.contains("v &amp; w"));
        assert_eq!(xml.matches("<a:tr ").count(), 2);
    }
}
