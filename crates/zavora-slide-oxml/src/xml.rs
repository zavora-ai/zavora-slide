//! A lossless, editable XML DOM.
//!
//! Every node captures its **exact original bytes** during parsing, so an
//! unedited tree serializes byte-for-byte identical to its input — including the
//! XML declaration, attribute quoting/spacing, self-closing style, whitespace,
//! comments, and CDATA. Editing mutates individual nodes (marking them `dirty`);
//! a dirty element regenerates only its own start tag, while clean siblings and
//! subtrees still emit verbatim. This is the foundation for surgical,
//! PowerPoint-grade slide editing — no re-authoring from extracted text.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::{OxmlError, Result};

/// One node in the tree: either an element or a verbatim leaf (text, CDATA,
/// comment, processing instruction, XML declaration, or doctype).
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Element(Element),
    /// Any non-element event, stored as its exact original bytes.
    Raw(Vec<u8>),
}

/// An XML element with its children, preserving original tag bytes verbatim
/// until edited.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// Qualified name, e.g. `b"a:t"` (as written, including any prefix).
    pub name: Vec<u8>,
    /// Exact original start-tag bytes, e.g. `b"<a:t foo=\"1\">"` or `b"<a:t/>"`
    /// for a self-closing element. Emitted verbatim unless `dirty`.
    pub raw_start: Vec<u8>,
    /// Exact original end-tag bytes, e.g. `b"</a:t>"`; empty when self-closing.
    pub raw_end: Vec<u8>,
    /// Whether the source wrote this as a single self-closing tag.
    pub self_closing: bool,
    /// When set, `raw_start` is regenerated from `name` + `attrs` on write.
    pub dirty: bool,
    /// Parsed attributes (name, value) in source order. Authoritative only when
    /// `dirty`; otherwise `raw_start` is the source of truth.
    pub attrs: Vec<(Vec<u8>, Vec<u8>)>,
    pub children: Vec<Node>,
}

impl Element {
    /// Local name with any namespace prefix stripped (`a:t` → `t`).
    pub fn local_name(&self) -> &[u8] {
        match self.name.iter().position(|&b| b == b':') {
            Some(i) => &self.name[i + 1..],
            None => &self.name,
        }
    }

    /// Direct child elements with the given local name.
    pub fn children_named<'a>(&'a self, local: &'a [u8]) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter_map(move |n| match n {
            Node::Element(e) if e.local_name() == local => Some(e),
            _ => None,
        })
    }

    /// Mutable direct child elements with the given local name.
    pub fn children_named_mut<'a>(
        &'a mut self,
        local: &'a [u8],
    ) -> impl Iterator<Item = &'a mut Element> {
        self.children.iter_mut().filter_map(move |n| match n {
            Node::Element(e) if e.local_name() == local => Some(e),
            _ => None,
        })
    }

    /// First descendant element (depth-first, self excluded) matching `local`.
    pub fn find_descendant(&self, local: &[u8]) -> Option<&Element> {
        for child in &self.children {
            if let Node::Element(e) = child {
                if e.local_name() == local {
                    return Some(e);
                }
                if let Some(found) = e.find_descendant(local) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Value of an attribute by its (qualified) name, if present.
    pub fn attr(&self, name: &[u8]) -> Option<&[u8]> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_slice())
    }

    /// Set (or add) an attribute, marking the start tag for regeneration.
    pub fn set_attr(&mut self, name: &[u8], value: &[u8]) {
        self.dirty = true;
        if let Some(slot) = self.attrs.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value.to_vec();
        } else {
            self.attrs.push((name.to_vec(), value.to_vec()));
        }
    }

    /// Concatenated text of this element's immediate text children, XML-unescaped
    /// (for a leaf like `<a:t>`, this is its text content).
    pub fn text_content(&self) -> String {
        let mut s = String::new();
        for c in &self.children {
            if let Node::Raw(b) = c {
                s.push_str(&unescape(b));
            }
        }
        s
    }

    /// Replace this element's content with a single XML-escaped text node,
    /// preserving the element's own tags. Converts a self-closing tag to an
    /// open/close pair so the text has somewhere to live.
    pub fn set_text_content(&mut self, text: &str) {
        self.children = vec![Node::Raw(escape(text).into_bytes())];
        if self.self_closing {
            self.self_closing = false;
            self.dirty = true;
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        if self.dirty {
            out.push(b'<');
            out.extend_from_slice(&self.name);
            for (k, v) in &self.attrs {
                out.push(b' ');
                out.extend_from_slice(k);
                out.extend_from_slice(b"=\"");
                out.extend_from_slice(v);
                out.push(b'"');
            }
            if self.self_closing {
                out.extend_from_slice(b"/>");
                return;
            }
            out.push(b'>');
        } else {
            out.extend_from_slice(&self.raw_start);
            if self.self_closing {
                return;
            }
        }
        for child in &self.children {
            match child {
                Node::Element(e) => e.write(out),
                Node::Raw(b) => out.extend_from_slice(b),
            }
        }
        if self.dirty {
            out.extend_from_slice(b"</");
            out.extend_from_slice(&self.name);
            out.push(b'>');
        } else {
            out.extend_from_slice(&self.raw_end);
        }
    }
}

/// A parsed XML document: a sequence of top-level nodes (declaration, root
/// element, trailing whitespace, …), each byte-faithful until edited.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub nodes: Vec<Node>,
}

impl Document {
    /// Parse XML into the lossless tree.
    pub fn parse(src: &[u8]) -> Result<Document> {
        let mut reader = Reader::from_reader(src);
        // Preserve everything verbatim — never trim or expand.
        reader.config_mut().trim_text(false);

        // Stack of (element, its accumulated children); roots collected at base.
        let mut stack: Vec<Element> = Vec::new();
        let mut roots: Vec<Node> = Vec::new();
        let mut buf = Vec::new();

        loop {
            let start = reader.buffer_position() as usize;
            let ev = reader.read_event_into(&mut buf)?;
            let end = reader.buffer_position() as usize;
            let raw = src[start..end].to_vec();

            match ev {
                Event::Eof => break,
                Event::Start(e) => {
                    stack.push(Element {
                        name: e.name().as_ref().to_vec(),
                        raw_start: raw,
                        raw_end: Vec::new(),
                        self_closing: false,
                        dirty: false,
                        attrs: parse_attrs(&e)?,
                        children: Vec::new(),
                    });
                }
                Event::End(_) => {
                    let mut el = stack
                        .pop()
                        .ok_or_else(|| OxmlError::Parse("unbalanced end tag".into()))?;
                    el.raw_end = raw;
                    push_node(&mut stack, &mut roots, Node::Element(el));
                }
                Event::Empty(e) => {
                    let el = Element {
                        name: e.name().as_ref().to_vec(),
                        raw_start: raw,
                        raw_end: Vec::new(),
                        self_closing: true,
                        dirty: false,
                        attrs: parse_attrs(&e)?,
                        children: Vec::new(),
                    };
                    push_node(&mut stack, &mut roots, Node::Element(el));
                }
                _ => push_node(&mut stack, &mut roots, Node::Raw(raw)),
            }
            buf.clear();
        }

        if !stack.is_empty() {
            return Err(OxmlError::Parse("unclosed element".into()));
        }
        Ok(Document { nodes: roots })
    }

    /// Serialize back to bytes (byte-identical to the source when unedited).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for node in &self.nodes {
            match node {
                Node::Element(e) => e.write(&mut out),
                Node::Raw(b) => out.extend_from_slice(b),
            }
        }
        out
    }

    /// The document's root element, if any.
    pub fn root(&self) -> Option<&Element> {
        self.nodes.iter().find_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// Mutable access to the root element, if any.
    pub fn root_mut(&mut self) -> Option<&mut Element> {
        self.nodes.iter_mut().find_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }
}

fn push_node(stack: &mut [Element], roots: &mut Vec<Node>, node: Node) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => roots.push(node),
    }
}

fn parse_attrs(e: &quick_xml::events::BytesStart) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut out = Vec::new();
    for attr in e.attributes() {
        let attr = attr?;
        out.push((attr.key.as_ref().to_vec(), attr.value.as_ref().to_vec()));
    }
    Ok(out)
}

/// XML-escape text content (the five predefined entities).
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Reverse the predefined entities (sufficient for `<a:t>` text content).
fn unescape(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trips(src: &[u8]) {
        let doc = Document::parse(src).unwrap();
        assert_eq!(doc.to_bytes(), src, "byte-faithful round-trip");
    }

    #[test]
    fn round_trips_declaration_and_nesting() {
        round_trips(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a><b x="1">hi</b><c/></a>"#);
    }

    #[test]
    fn round_trips_single_quotes_and_whitespace() {
        // python-pptx-style single-quoted decl + indentation must survive verbatim.
        round_trips(b"<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<a>\n  <b/>\n</a>\n");
    }

    #[test]
    fn round_trips_comments_cdata_entities() {
        round_trips(br#"<a><!-- c --><b><![CDATA[x<y]]></b><d>a &amp; b &lt; c</d></a>"#);
    }

    #[test]
    fn round_trips_self_closing_with_spacing() {
        round_trips(br#"<a><b   x="1"  y="2" /><c></c></a>"#);
    }

    #[test]
    fn navigation_and_attrs() {
        let doc = Document::parse(br#"<p:sp><p:nvSpPr><p:ph type="title" idx="1"/></p:nvSpPr></p:sp>"#).unwrap();
        let root = doc.root().unwrap();
        assert_eq!(root.local_name(), b"sp");
        let ph = root.find_descendant(b"ph").unwrap();
        assert_eq!(ph.attr(b"type").unwrap(), b"title");
        assert_eq!(ph.attr(b"idx").unwrap(), b"1");
    }
    #[test]
    fn set_text_preserves_siblings_and_escapes() {
        // Editing one <a:t> must leave the sibling run byte-identical.
        let src = br#"<a:p><a:r><a:rPr b="1"/><a:t>old</a:t></a:r><a:r><a:t>keep</a:t></a:r></a:p>"#;
        let mut doc = Document::parse(src).unwrap();
        let root = doc.root_mut().unwrap();
        let first_run = root.children_named_mut(b"r").next().unwrap();
        let t = first_run.children_named_mut(b"t").next().unwrap();
        assert_eq!(t.text_content(), "old");
        t.set_text_content("a < b & c");
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.contains("<a:t>a &lt; b &amp; c</a:t>"), "got {s}");
        assert!(s.contains(r#"<a:rPr b="1"/>"#), "rPr verbatim: {s}");
        assert!(s.contains("<a:r><a:t>keep</a:t></a:r>"), "2nd run verbatim: {s}");
    }

    #[test]
    fn set_text_on_self_closing_opens_the_tag() {
        let mut doc = Document::parse(br#"<a:t/>"#).unwrap();
        doc.root_mut().unwrap().set_text_content("hi");
        assert_eq!(doc.to_bytes(), b"<a:t>hi</a:t>");
    }

    #[test]
    fn set_attr_regenerates_only_that_tag() {
        let mut doc = Document::parse(br#"<a><b x="1"/><c y="2"/></a>"#).unwrap();
        let b = doc.root_mut().unwrap().children_named_mut(b"b").next().unwrap();
        b.set_attr(b"x", b"9");
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.contains(r#"<b x="9"/>"#), "got {s}");
        assert!(s.contains(r#"<c y="2"/>"#), "sibling verbatim: {s}");
    }


    #[test]
    fn dirty_element_regenerates_clean_siblings_verbatim() {
        let mut doc = Document::parse(br#"<a><b x="1">keep</b><c>edit</c></a>"#).unwrap();
        // Mark <c> dirty and change its name to prove regeneration is localized.
        if let Some(root) = doc.root_mut() {
            for e in root.children_named_mut(b"c") {
                e.dirty = true;
                e.name = b"c".to_vec();
            }
        }
        // <b> still emits verbatim (with its original spacing/quotes).
        let out = doc.to_bytes();
        assert!(out.windows(11).any(|w| w == br#"<b x="1">ke"#.as_slice() || w == br#"<b x="1">keep"#[..11].as_ref()));
        assert_eq!(out, br#"<a><b x="1">keep</b><c>edit</c></a>"#);
    }
}
