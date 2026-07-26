//! DrawingML text bodies, runs, and shapes (the `a:`/`p:sp` vocabulary shared
//! by slides). These are build-oriented: the high-level API constructs them and
//! serializes to XML that is spliced into a slide's `spTree`.
//!
//! `TextBody::from_xml` additionally extracts paragraph text + level, which
//! powers both the round-trip test here and slide text extraction (task 3.3).

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::Result;

/// Escape XML text/attribute content.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn local(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Paragraph alignment (`a:pPr@algn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    fn attr(self) -> &'static str {
        match self {
            Align::Left => "l",
            Align::Center => "ctr",
            Align::Right => "r",
            Align::Justify => "just",
        }
    }
    fn parse(s: &str) -> Option<Align> {
        match s {
            "l" => Some(Align::Left),
            "ctr" => Some(Align::Center),
            "r" => Some(Align::Right),
            "just" => Some(Align::Justify),
            _ => None,
        }
    }
}

/// Character formatting for a run (`a:rPr`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunProps {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub size_pt: Option<f64>,
    pub color: Option<String>,
    pub font: Option<String>,
}

impl RunProps {
    fn to_xml(&self) -> String {
        let mut a = String::from(" lang=\"en-US\"");
        if self.bold == Some(true) {
            a.push_str(" b=\"1\"");
        }
        if self.italic == Some(true) {
            a.push_str(" i=\"1\"");
        }
        if self.underline == Some(true) {
            a.push_str(" u=\"sng\"");
        }
        if let Some(sz) = self.size_pt {
            a.push_str(&format!(" sz=\"{}\"", (sz * 100.0) as i64));
        }
        let mut children = String::new();
        if let Some(c) = &self.color {
            children.push_str(&format!(
                "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
                esc(c)
            ));
        }
        if let Some(f) = &self.font {
            children.push_str(&format!("<a:latin typeface=\"{}\"/>", esc(f)));
        }
        if children.is_empty() {
            format!("<a:rPr{a}/>")
        } else {
            format!("<a:rPr{a}>{children}</a:rPr>")
        }
    }
}

/// A run of text with formatting (`a:r`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub props: RunProps,
}

impl Run {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            props: RunProps::default(),
        }
    }
    fn to_xml(&self) -> String {
        format!(
            "<a:r>{}<a:t>{}</a:t></a:r>",
            self.props.to_xml(),
            esc(&self.text)
        )
    }
}

/// A paragraph (`a:p`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    /// Outline level; `None` or `Some(0)` = top level (no `lvl` attribute).
    pub level: Option<u8>,
    pub align: Option<Align>,
    /// `Some(false)` emits `<a:buNone/>`; `None` inherits the layout bullet.
    pub bullet: Option<bool>,
}

impl Paragraph {
    /// Plain-text content of the paragraph (runs concatenated).
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn to_xml(&self) -> String {
        let mut attrs = String::new();
        if let Some(l) = self.level
            && l > 0
        {
            attrs.push_str(&format!(" lvl=\"{l}\""));
        }
        if let Some(al) = self.align {
            attrs.push_str(&format!(" algn=\"{}\"", al.attr()));
        }
        let children = if self.bullet == Some(false) {
            "<a:buNone/>"
        } else {
            ""
        };
        let ppr = if attrs.is_empty() && children.is_empty() {
            String::new()
        } else if children.is_empty() {
            format!("<a:pPr{attrs}/>")
        } else {
            format!("<a:pPr{attrs}>{children}</a:pPr>")
        };
        let runs: String = self.runs.iter().map(Run::to_xml).collect();
        format!("<a:p>{ppr}{runs}</a:p>")
    }
}

/// A shape text body (`p:txBody`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextBody {
    pub paragraphs: Vec<Paragraph>,
}

impl TextBody {
    pub fn to_xml(&self) -> String {
        // CT_TextBody requires at least one paragraph; emit an empty one when
        // the body has no content (e.g. an auto-shape with no text).
        let ps: String = if self.paragraphs.is_empty() {
            "<a:p/>".to_string()
        } else {
            self.paragraphs.iter().map(Paragraph::to_xml).collect()
        };
        format!("<p:txBody><a:bodyPr/><a:lstStyle/>{ps}</p:txBody>")
    }

    /// Extract paragraphs (text, level, align, run bold/italic/size) from XML.
    /// Color/font are not parsed back (build-only); unmodeled detail is ignored.
    pub fn from_xml(xml: &[u8]) -> Result<TextBody> {
        let mut reader = Reader::from_reader(xml);
        let mut buf = Vec::new();
        let mut paras: Vec<Paragraph> = Vec::new();
        let mut cur: Option<Paragraph> = None;
        let mut rprops = RunProps::default();
        let mut text = String::new();
        let mut in_t = false;

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => match local(e.name().as_ref()) {
                    b"p" => cur = Some(Paragraph::default()),
                    b"pPr" => {
                        if let Some(p) = cur.as_mut() {
                            read_ppr(&e, p)?;
                        }
                    }
                    b"rPr" => rprops = read_rpr(&e)?,
                    b"t" => {
                        in_t = true;
                        text.clear();
                    }
                    _ => {}
                },
                Event::Empty(e) => match local(e.name().as_ref()) {
                    b"pPr" => {
                        if let Some(p) = cur.as_mut() {
                            read_ppr(&e, p)?;
                        }
                    }
                    b"rPr" => rprops = read_rpr(&e)?,
                    _ => {}
                },
                Event::Text(t) if in_t => {
                    let s = t.unescape()?;
                    text.push_str(&s);
                }
                Event::End(e) => match local(e.name().as_ref()) {
                    b"t" => in_t = false,
                    b"r" => {
                        if let Some(p) = cur.as_mut() {
                            p.runs.push(Run {
                                text: std::mem::take(&mut text),
                                props: std::mem::take(&mut rprops),
                            });
                        }
                    }
                    b"p" => {
                        if let Some(p) = cur.take() {
                            paras.push(p);
                        }
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(TextBody { paragraphs: paras })
    }
}

fn read_ppr(e: &quick_xml::events::BytesStart, p: &mut Paragraph) -> Result<()> {
    for attr in e.attributes() {
        let attr = attr?;
        match attr.key.as_ref() {
            b"lvl" => p.level = std::str::from_utf8(&attr.value)?.parse().ok(),
            b"algn" => p.align = Align::parse(std::str::from_utf8(&attr.value)?),
            _ => {}
        }
    }
    Ok(())
}

fn read_rpr(e: &quick_xml::events::BytesStart) -> Result<RunProps> {
    let mut rp = RunProps::default();
    for attr in e.attributes() {
        let attr = attr?;
        match attr.key.as_ref() {
            b"b" => rp.bold = Some(&*attr.value == b"1"),
            b"i" => rp.italic = Some(&*attr.value == b"1"),
            b"u" => rp.underline = Some(&*attr.value != b"none"),
            b"sz" => {
                rp.size_pt = std::str::from_utf8(&attr.value)?
                    .parse::<f64>()
                    .ok()
                    .map(|v| v / 100.0)
            }
            _ => {}
        }
    }
    Ok(rp)
}

/// A placeholder reference (`p:ph`).
#[derive(Debug, Clone, PartialEq)]
pub struct Placeholder {
    pub ph_type: String,
    pub idx: Option<u32>,
}

/// A shape (`p:sp`) — placeholder, text box, or auto-shape.
#[derive(Debug, Clone)]
pub struct Shape {
    pub id: u32,
    pub name: String,
    pub placeholder: Option<Placeholder>,
    /// Explicit geometry (x, y, cx, cy) in EMU, for text boxes / auto-shapes.
    pub xfrm: Option<(i64, i64, i64, i64)>,
    pub text_box: bool,
    /// Preset geometry name (`a:prstGeom@prst`), e.g. "rect", "ellipse".
    pub geom: String,
    /// Optional solid fill color (hex, no `#`).
    pub fill: Option<String>,
    /// Optional outline: (hex color, width in EMU).
    pub line: Option<(String, i64)>,
    pub body: TextBody,
}

impl Shape {
    /// Build a placeholder shape (e.g. title or body).
    pub fn placeholder(
        id: u32,
        name: &str,
        ph_type: &str,
        idx: Option<u32>,
        body: TextBody,
    ) -> Self {
        Self {
            id,
            name: name.to_string(),
            placeholder: Some(Placeholder {
                ph_type: ph_type.to_string(),
                idx,
            }),
            xfrm: None,
            text_box: false,
            geom: "rect".to_string(),
            fill: None,
            line: None,
            body,
        }
    }

    /// Build a positioned text box.
    pub fn text_box(id: u32, x: i64, y: i64, cx: i64, cy: i64, body: TextBody) -> Self {
        Self {
            id,
            name: format!("TextBox {id}"),
            placeholder: None,
            xfrm: Some((x, y, cx, cy)),
            text_box: true,
            geom: "rect".to_string(),
            fill: None,
            line: None,
            body,
        }
    }

    /// Build an auto-shape with the given preset geometry.
    pub fn auto_shape(id: u32, geom: &str, x: i64, y: i64, cx: i64, cy: i64) -> Self {
        Self {
            id,
            name: format!("Shape {id}"),
            placeholder: None,
            xfrm: Some((x, y, cx, cy)),
            text_box: false,
            geom: geom.to_string(),
            fill: None,
            line: None,
            body: TextBody::default(),
        }
    }

    /// Set a solid fill color (hex, with or without `#`).
    pub fn set_fill(&mut self, hex: &str) -> &mut Self {
        self.fill = Some(hex.trim_start_matches('#').to_uppercase());
        self
    }

    /// Set an outline color (hex) and width in points.
    pub fn set_outline(&mut self, hex: &str, width_pt: f64) -> &mut Self {
        self.line = Some((
            hex.trim_start_matches('#').to_uppercase(),
            (width_pt * 12700.0) as i64,
        ));
        self
    }

    /// Apply a mutation to every run's properties (fluent formatting helpers).
    fn map_runs(&mut self, f: impl Fn(&mut crate::drawing::RunProps)) -> &mut Self {
        for p in &mut self.body.paragraphs {
            for r in &mut p.runs {
                f(&mut r.props);
            }
        }
        self
    }

    pub fn bold(&mut self, on: bool) -> &mut Self {
        self.map_runs(|rp| rp.bold = Some(on))
    }
    pub fn italic(&mut self, on: bool) -> &mut Self {
        self.map_runs(|rp| rp.italic = Some(on))
    }
    pub fn underline(&mut self, on: bool) -> &mut Self {
        self.map_runs(|rp| rp.underline = Some(on))
    }
    pub fn size(&mut self, pt: f64) -> &mut Self {
        self.map_runs(move |rp| rp.size_pt = Some(pt))
    }
    pub fn color(&mut self, hex: &str) -> &mut Self {
        let hex = hex.trim_start_matches('#').to_string();
        self.map_runs(move |rp| rp.color = Some(hex.clone()))
    }
    pub fn font(&mut self, family: &str) -> &mut Self {
        let family = family.to_string();
        self.map_runs(move |rp| rp.font = Some(family.clone()))
    }
    /// Set alignment on every paragraph.
    pub fn align(&mut self, align: crate::drawing::Align) -> &mut Self {
        for p in &mut self.body.paragraphs {
            p.align = Some(align);
        }
        self
    }

    pub fn to_xml(&self) -> String {
        let nv_pr = match &self.placeholder {
            Some(ph) => {
                let idx = ph.idx.map(|i| format!(" idx=\"{i}\"")).unwrap_or_default();
                format!(
                    "<p:nvPr><p:ph type=\"{}\"{idx}/></p:nvPr>",
                    esc(&ph.ph_type)
                )
            }
            None => "<p:nvPr/>".to_string(),
        };
        let cnv_sp = if self.text_box {
            "<p:cNvSpPr txBox=\"1\"/>"
        } else if self.placeholder.is_some() {
            "<p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>"
        } else {
            "<p:cNvSpPr/>"
        };
        let sp_pr = match self.xfrm {
            Some((x, y, cx, cy)) => {
                let fill = self
                    .fill
                    .as_ref()
                    .map(|h| format!("<a:solidFill><a:srgbClr val=\"{h}\"/></a:solidFill>"))
                    .unwrap_or_default();
                let line = self
                    .line
                    .as_ref()
                    .map(|(h, w)| format!("<a:ln w=\"{w}\"><a:solidFill><a:srgbClr val=\"{h}\"/></a:solidFill></a:ln>"))
                    .unwrap_or_default();
                format!(
                    "<p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/>\
                     </a:xfrm><a:prstGeom prst=\"{}\"><a:avLst/></a:prstGeom>{fill}{line}</p:spPr>",
                    esc(&self.geom)
                )
            }
            None => "<p:spPr/>".to_string(),
        };
        format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{}\" name=\"{}\"/>{cnv_sp}{nv_pr}</p:nvSpPr>{sp_pr}{}</p:sp>",
            self.id,
            esc(&self.name),
            self.body.to_xml()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_props_serialize() {
        let rp = RunProps {
            bold: Some(true),
            size_pt: Some(18.0),
            color: Some("FF0000".into()),
            ..Default::default()
        };
        let xml = rp.to_xml();
        assert!(xml.contains("b=\"1\""));
        assert!(xml.contains("sz=\"1800\""));
        assert!(xml.contains("<a:srgbClr val=\"FF0000\"/>"));
    }

    #[test]
    fn text_is_escaped() {
        let r = Run::new("a < b & \"c\"");
        assert!(r.to_xml().contains("a &lt; b &amp; &quot;c&quot;"));
    }

    #[test]
    fn round_trip_text_level_and_props() {
        let body = TextBody {
            paragraphs: vec![
                Paragraph {
                    runs: vec![Run {
                        text: "Hello".into(),
                        props: RunProps {
                            bold: Some(true),
                            size_pt: Some(24.0),
                            ..Default::default()
                        },
                    }],
                    align: Some(Align::Center),
                    ..Default::default()
                },
                Paragraph {
                    runs: vec![Run::new("World")],
                    level: Some(2),
                    ..Default::default()
                },
            ],
        };
        let parsed = TextBody::from_xml(body.to_xml().as_bytes()).unwrap();
        assert_eq!(parsed.paragraphs.len(), 2);
        assert_eq!(parsed.paragraphs[0].text(), "Hello");
        assert_eq!(parsed.paragraphs[0].align, Some(Align::Center));
        assert_eq!(parsed.paragraphs[0].runs[0].props.bold, Some(true));
        assert_eq!(parsed.paragraphs[0].runs[0].props.size_pt, Some(24.0));
        assert_eq!(parsed.paragraphs[1].text(), "World");
        assert_eq!(parsed.paragraphs[1].level, Some(2));
    }

    #[test]
    fn empty_textbody_still_has_paragraph() {
        // CT_TextBody requires >=1 a:p; an auto-shape with no text must still emit one.
        let sp = Shape::auto_shape(2, "ellipse", 0, 0, 100, 100);
        let xml = sp.to_xml();
        assert!(xml.contains("<p:txBody>"));
        assert!(xml.contains("<a:p/>"));
    }

    #[test]
    fn placeholder_and_textbox_shapes() {
        let title = Shape::placeholder(
            2,
            "Title 1",
            "title",
            None,
            TextBody {
                paragraphs: vec![Paragraph {
                    runs: vec![Run::new("T")],
                    ..Default::default()
                }],
            },
        );
        let tx = title.to_xml();
        assert!(tx.contains("<p:ph type=\"title\"/>"));
        assert!(tx.contains("<a:t>T</a:t>"));

        let tb = Shape::text_box(5, 914400, 914400, 1828800, 457200, TextBody::default());
        let bx = tb.to_xml();
        assert!(bx.contains("txBox=\"1\""));
        assert!(bx.contains("<a:off x=\"914400\" y=\"914400\"/>"));
    }
}
