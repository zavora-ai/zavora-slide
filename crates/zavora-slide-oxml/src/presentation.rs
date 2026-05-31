//! Typed model for `ppt/presentation.xml` (CT_Presentation, subset).
//!
//! The engine edits the slide-id list, master-id list, and slide size; every
//! other child element is captured verbatim into `extra_children` and re-emitted
//! on save (round-trip rule — no silent drops).

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer};

use crate::error::Result;

/// Default namespace declarations for `<p:presentation>`.
pub fn default_root_attrs() -> Vec<(String, String)> {
    [
        ("xmlns:a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
        ("xmlns:r", "http://schemas.openxmlformats.org/officeDocument/2006/relationships"),
        ("xmlns:p", "http://schemas.openxmlformats.org/presentationml/2006/main"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// An entry in `sldIdLst` or `sldMasterIdLst`: a numeric id + relationship id.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideIdEntry {
    pub id: u32,
    pub r_id: String,
}

/// `<p:sldSz>` — slide dimensions in EMU plus an optional `type` hint.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideSize {
    pub cx: i64,
    pub cy: i64,
    pub ty: Option<String>,
}

impl SlideSize {
    /// 16:9 widescreen (default in modern PowerPoint).
    pub fn widescreen() -> Self {
        Self { cx: 12192000, cy: 6858000, ty: Some("screen16x9".into()) }
    }

    /// Default notes-page size: 7.5"×10" portrait (required `<p:notesSz>`).
    pub fn notes_default() -> Self {
        Self { cx: 6858000, cy: 9144000, ty: None }
    }
}

/// Typed subset of `ppt/presentation.xml`.
#[derive(Debug, Clone)]
pub struct Presentation {
    pub root_attrs: Vec<(String, String)>,
    pub master_ids: Vec<SlideIdEntry>,
    pub slide_ids: Vec<SlideIdEntry>,
    pub slide_size: SlideSize,
    /// `<p:notesSz>` — required by the schema; default 7.5"×10" portrait.
    pub notes_size: SlideSize,
    /// Verbatim top-level children we don't model (defaultTextStyle, extLst, ...).
    pub extra_children: Vec<Vec<u8>>,
}

impl Presentation {
    /// A blank 16:9 presentation with default namespaces and no slides.
    pub fn new() -> Self {
        Self {
            root_attrs: default_root_attrs(),
            master_ids: Vec::new(),
            slide_ids: Vec::new(),
            slide_size: SlideSize::widescreen(),
            notes_size: SlideSize::notes_default(),
            extra_children: Vec::new(),
        }
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();

        let mut pres = Presentation {
            root_attrs: Vec::new(),
            master_ids: Vec::new(),
            slide_ids: Vec::new(),
            slide_size: SlideSize { cx: 0, cy: 0, ty: None },
            notes_size: SlideSize::notes_default(),
            extra_children: Vec::new(),
        };

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) if local(e.name().as_ref()) == b"presentation" => {
                    pres.root_attrs = collect_attrs(&e)?;
                }
                Event::Start(e) => match local(e.name().as_ref()) {
                    b"sldMasterIdLst" => pres.master_ids = parse_id_list(&mut reader, b"sldMasterId")?,
                    b"sldIdLst" => pres.slide_ids = parse_id_list(&mut reader, b"sldId")?,
                    _ => pres.extra_children.push(capture_subtree(&mut reader, &e)?),
                },
                Event::Empty(e) => match local(e.name().as_ref()) {
                    b"sldSz" => pres.slide_size = parse_sld_sz(&e)?,
                    b"notesSz" => pres.notes_size = parse_sld_sz(&e)?,
                    b"sldMasterIdLst" | b"sldIdLst" => {}
                    _ => {
                        let mut w = Writer::new(Vec::new());
                        w.write_event(Event::Empty(e.borrow()))?;
                        pres.extra_children.push(w.into_inner());
                    }
                },
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }

        if pres.root_attrs.is_empty() {
            pres.root_attrs = default_root_attrs();
        }
        Ok(pres)
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut w = Writer::new(Vec::new());
        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), Some("yes"))))?;

        let mut root = BytesStart::new("p:presentation");
        for (k, v) in &self.root_attrs {
            root.push_attribute((k.as_str(), v.as_str()));
        }
        w.write_event(Event::Start(root))?;

        write_id_list(&mut w, "p:sldMasterIdLst", "p:sldMasterId", &self.master_ids)?;
        write_id_list(&mut w, "p:sldIdLst", "p:sldId", &self.slide_ids)?;

        let mut sz = BytesStart::new("p:sldSz");
        sz.push_attribute(("cx", self.slide_size.cx.to_string().as_str()));
        sz.push_attribute(("cy", self.slide_size.cy.to_string().as_str()));
        if let Some(ref t) = self.slide_size.ty {
            sz.push_attribute(("type", t.as_str()));
        }
        w.write_event(Event::Empty(sz))?;

        // notesSz is required by CT_Presentation and must follow sldSz.
        let mut nsz = BytesStart::new("p:notesSz");
        nsz.push_attribute(("cx", self.notes_size.cx.to_string().as_str()));
        nsz.push_attribute(("cy", self.notes_size.cy.to_string().as_str()));
        w.write_event(Event::Empty(nsz))?;

        for raw in &self.extra_children {
            w.get_mut().extend_from_slice(raw);
        }

        w.write_event(Event::End(BytesEnd::new("p:presentation")))?;
        Ok(w.into_inner())
    }
}

impl Default for Presentation {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip a namespace prefix from a tag name (`p:sldSz` → `sldSz`).
fn local(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn collect_attrs(e: &BytesStart) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for attr in e.attributes() {
        let attr = attr?;
        let k = std::str::from_utf8(attr.key.as_ref())?.to_string();
        let v = std::str::from_utf8(&attr.value)?.to_string();
        out.push((k, v));
    }
    Ok(out)
}

/// Parse `sldId`/`sldMasterId` entries until the enclosing list's End tag.
fn parse_id_list<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    entry: &[u8],
) -> Result<Vec<SlideIdEntry>> {
    let mut out = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Empty(e) | Event::Start(e) if local(e.name().as_ref()) == entry => {
                let mut id = 0u32;
                let mut r_id = String::new();
                for attr in e.attributes() {
                    let attr = attr?;
                    match attr.key.as_ref() {
                        b"id" => id = std::str::from_utf8(&attr.value)?.parse().unwrap_or(0),
                        k if local(k) == b"id" => {
                            // r:id
                            r_id = std::str::from_utf8(&attr.value)?.to_string();
                        }
                        _ => {}
                    }
                }
                out.push(SlideIdEntry { id, r_id });
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn parse_sld_sz(e: &BytesStart) -> Result<SlideSize> {
    let mut cx = 0i64;
    let mut cy = 0i64;
    let mut ty = None;
    for attr in e.attributes() {
        let attr = attr?;
        match attr.key.as_ref() {
            b"cx" => cx = std::str::from_utf8(&attr.value)?.parse().unwrap_or(0),
            b"cy" => cy = std::str::from_utf8(&attr.value)?.parse().unwrap_or(0),
            b"type" => ty = Some(std::str::from_utf8(&attr.value)?.to_string()),
            _ => {}
        }
    }
    Ok(SlideSize { cx, cy, ty })
}

/// Capture a Start element and its entire subtree as raw bytes.
fn capture_subtree<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    start: &BytesStart,
) -> Result<Vec<u8>> {
    let mut w = Writer::new(Vec::new());
    w.write_event(Event::Start(start.borrow()))?;
    let mut depth = 1usize;
    let mut buf = Vec::new();
    loop {
        let ev = reader.read_event_into(&mut buf)?;
        match ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            Event::Eof => break,
            _ => {}
        }
        w.write_event(ev.borrow())?;
        if depth == 0 {
            break;
        }
        buf.clear();
    }
    Ok(w.into_inner())
}

fn write_id_list<W: std::io::Write>(
    w: &mut Writer<W>,
    list_tag: &str,
    entry_tag: &str,
    entries: &[SlideIdEntry],
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    w.write_event(Event::Start(BytesStart::new(list_tag)))?;
    for e in entries {
        let mut el = BytesStart::new(entry_tag);
        el.push_attribute(("id", e.id.to_string().as_str()));
        el.push_attribute(("r:id", e.r_id.as_str()));
        w.write_event(Event::Empty(el))?;
    }
    w.write_event(Event::End(BytesEnd::new(list_tag)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>
<p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst>
<p:sldSz cx="12192000" cy="6858000" type="screen16x9"/>
<p:notesSz cx="6858000" cy="9144000"/>
<p:defaultTextStyle><a:defPPr/></p:defaultTextStyle>
</p:presentation>"#;

    #[test]
    fn parse_fields() {
        let p = Presentation::from_xml(SAMPLE).unwrap();
        assert_eq!(p.master_ids.len(), 1);
        assert_eq!(p.master_ids[0].id, 2147483648);
        assert_eq!(p.master_ids[0].r_id, "rId1");
        assert_eq!(p.slide_ids.len(), 2);
        assert_eq!(p.slide_ids[1].id, 257);
        assert_eq!(p.slide_size.cx, 12192000);
        assert_eq!(p.slide_size.ty.as_deref(), Some("screen16x9"));
        // notesSz is now typed.
        assert_eq!(p.notes_size.cx, 6858000);
        assert_eq!(p.notes_size.cy, 9144000);
        // defaultTextStyle is unmodeled → captured.
        assert_eq!(p.extra_children.len(), 1);
    }

    #[test]
    fn round_trip_preserves_fields_and_extras() {
        let p1 = Presentation::from_xml(SAMPLE).unwrap();
        let xml = p1.to_xml().unwrap();
        let p2 = Presentation::from_xml(&xml).unwrap();
        assert_eq!(p1.master_ids, p2.master_ids);
        assert_eq!(p1.slide_ids, p2.slide_ids);
        assert_eq!(p1.slide_size, p2.slide_size);
        assert_eq!(p1.notes_size, p2.notes_size);
        // notesSz is emitted, and the captured defaultTextStyle survives.
        assert!(String::from_utf8_lossy(&xml).contains("notesSz"));
        assert!(String::from_utf8_lossy(&xml).contains("defaultTextStyle"));
        assert_eq!(p2.extra_children.len(), 1);
    }

    #[test]
    fn new_is_blank_16x9() {
        let p = Presentation::new();
        assert!(p.slide_ids.is_empty());
        assert_eq!(p.slide_size, SlideSize::widescreen());
        let xml = String::from_utf8(p.to_xml().unwrap()).unwrap();
        assert!(xml.contains("screen16x9"));
        assert!(xml.contains("notesSz"));
        assert!(xml.contains("p:presentation"));
    }
}
