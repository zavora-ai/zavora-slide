//! High-level slide model and editing handle.
//!
//! A created slide owns a list of DrawingML shapes; serialization wraps them in
//! the canonical blank-slide shell (matching the part PowerPoint accepts). Title
//! and body placeholders are placed with explicit geometry derived from the
//! slide size, so they render regardless of the (single, Phase 0) layout —
//! per-layout placeholder geometry resolution is a later (layout) phase.

use zavora_slide_oxml::{Paragraph, Run, RunProps, Shape, TextBody};

use crate::error::Result;
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

/// Stored slide content: the shapes injected into the slide's `spTree`.
#[derive(Debug, Clone, Default)]
pub struct SlideData {
    pub shapes: Vec<Shape>,
    /// Next shape id (group shape is id 1, so authored shapes start at 2).
    next_id: u32,
}

impl SlideData {
    pub fn new() -> Self {
        Self { shapes: Vec::new(), next_id: 2 }
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
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
             xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
             xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
             <p:cSld><p:spTree>\
             <p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
             <p:grpSpPr/>{shapes}</p:spTree></p:cSld>\
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
}
