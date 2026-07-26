//! A lossless, editable XML DOM.
//!
//! Every node captures its **exact original bytes** during parsing, so an
//! unedited tree serializes byte-for-byte identical to its input — including the
//! XML declaration, attribute quoting/spacing, self-closing style, whitespace,
//! comments, and CDATA. Editing mutates individual nodes (marking them `dirty`);
//! a dirty element regenerates only its own start tag, while clean siblings and
//! subtrees still emit verbatim. This is the foundation for surgical,
//! PowerPoint-grade slide editing — no re-authoring from extracted text.

use quick_xml::Reader;
use quick_xml::events::Event;

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
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_slice())
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

    // ─── DOM mutation toolkit ───────────────────────────────────────────

    /// Insert a child node at `index`. If `index >= len`, appends.
    /// Converts a self-closing element to open/close so children can exist.
    pub fn insert_child_at(&mut self, index: usize, node: Node) {
        if self.self_closing {
            self.self_closing = false;
            self.dirty = true;
        }
        let len = self.children.len();
        if index >= len {
            self.children.push(node);
        } else {
            self.children.insert(index, node);
        }
    }

    /// Remove all children for which `predicate` returns `true`.
    /// Returns the removed nodes. If all children are removed and the element
    /// was not self-closing, it remains open (empty content).
    pub fn remove_children_where<F>(&mut self, predicate: F) -> Vec<Node>
    where
        F: Fn(&Node) -> bool,
    {
        let mut removed = Vec::new();
        let mut i = 0;
        while i < self.children.len() {
            if predicate(&self.children[i]) {
                removed.push(self.children.remove(i));
            } else {
                i += 1;
            }
        }
        removed
    }

    /// Replace the child at `index` with `node`. Returns the old child.
    /// Panics if `index` is out of bounds.
    pub fn replace_child(&mut self, index: usize, node: Node) -> Node {
        assert!(
            index < self.children.len(),
            "replace_child: index {} out of bounds (len {})",
            index,
            self.children.len()
        );
        std::mem::replace(&mut self.children[index], node)
    }

    /// Insert a child element in schema-valid order according to the
    /// `child_order` table for this element's local name. If the element's
    /// local name has no ordering table, or the child's local name is not in
    /// the table, the child is appended.
    ///
    /// The ordering is determined by looking up the child's local name in the
    /// table for this parent. The child is inserted after the last existing
    /// sibling whose order rank is ≤ the new child's rank.
    pub fn insert_child_ordered(&mut self, node: Node) {
        if self.self_closing {
            self.self_closing = false;
            self.dirty = true;
        }

        let child_local = match &node {
            Node::Element(e) => e.local_name().to_vec(),
            _ => {
                self.children.push(node);
                return;
            }
        };

        let parent_local = self.local_name().to_vec();
        let table = match child_order_table(&parent_local) {
            Some(t) => t,
            None => {
                self.children.push(node);
                return;
            }
        };

        let child_rank = rank_of(&child_local, table);

        // Find the insertion point: after the last sibling with rank <= child_rank.
        let mut insert_pos = 0;
        for (i, existing) in self.children.iter().enumerate() {
            if let Node::Element(e) = existing {
                let existing_rank = rank_of(e.local_name(), table);
                if existing_rank <= child_rank {
                    insert_pos = i + 1;
                }
            }
        }

        self.children.insert(insert_pos, node);
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

// ─── Child-order tables (ECMA-376 schema-valid ordering) ────────────────────

/// Schema-valid child ordering for `a:rPr` (CT_TextCharacterProperties).
/// Children must appear in this order per ECMA-376 §21.1.2.3.9.
const RPR_ORDER: &[&[u8]] = &[
    b"ln",             // a:ln
    b"noFill",         // a:noFill
    b"solidFill",      // a:solidFill
    b"gradFill",       // a:gradFill
    b"blipFill",       // a:blipFill
    b"pattFill",       // a:pattFill
    b"grpFill",        // a:grpFill
    b"effectLst",      // a:effectLst
    b"effectDag",      // a:effectDag
    b"highlight",      // a:highlight
    b"uLnTx",          // a:uLnTx
    b"uLn",            // a:uLn
    b"uFillTx",        // a:uFillTx
    b"uFill",          // a:uFill
    b"latin",          // a:latin
    b"ea",             // a:ea
    b"cs",             // a:cs
    b"sym",            // a:sym
    b"hlinkClick",     // a:hlinkClick
    b"hlinkMouseOver", // a:hlinkMouseOver
    b"rtl",            // a:rtl
    b"extLst",         // a:extLst
];

/// Schema-valid child ordering for `a:pPr` (CT_TextParagraphProperties).
/// Children must appear in this order per ECMA-376 §21.1.2.2.7.
const PPR_ORDER: &[&[u8]] = &[
    b"lnSpc",     // a:lnSpc
    b"spcBef",    // a:spcBef
    b"spcAft",    // a:spcAft
    b"buClrTx",   // a:buClrTx
    b"buClr",     // a:buClr
    b"buSzTx",    // a:buSzTx
    b"buSzPct",   // a:buSzPct
    b"buSzPts",   // a:buSzPts
    b"buFontTx",  // a:buFontTx
    b"buFont",    // a:buFont
    b"buNone",    // a:buNone
    b"buAutoNum", // a:buAutoNum
    b"buChar",    // a:buChar
    b"buBlip",    // a:buBlip
    b"tabLst",    // a:tabLst
    b"defRPr",    // a:defRPr
    b"extLst",    // a:extLst
];

/// Schema-valid child ordering for `p:spPr` / `a:spPr` (CT_ShapeProperties).
/// Children must appear in this order per ECMA-376 §19.3.1.44 / §21.1.2.1.1.
const SPPR_ORDER: &[&[u8]] = &[
    b"xfrm",      // a:xfrm
    b"custGeom",  // a:custGeom
    b"prstGeom",  // a:prstGeom
    b"noFill",    // a:noFill
    b"solidFill", // a:solidFill
    b"gradFill",  // a:gradFill
    b"blipFill",  // a:blipFill
    b"pattFill",  // a:pattFill
    b"grpFill",   // a:grpFill
    b"ln",        // a:ln
    b"effectLst", // a:effectLst
    b"effectDag", // a:effectDag
    b"scene3d",   // a:scene3d
    b"sp3d",      // a:sp3d
    b"extLst",    // a:extLst
];

/// Schema-valid child ordering for `p:txBody` / `a:txBody` (CT_TextBody).
/// Children must appear in this order per ECMA-376 §21.1.2.1.1.
const TXBODY_ORDER: &[&[u8]] = &[
    b"bodyPr",   // a:bodyPr
    b"lstStyle", // a:lstStyle
    b"p",        // a:p (repeating)
];

/// Look up the child-order table for a given parent local name.
fn child_order_table(parent_local: &[u8]) -> Option<&'static [&'static [u8]]> {
    match parent_local {
        b"rPr" => Some(RPR_ORDER),
        b"pPr" => Some(PPR_ORDER),
        b"spPr" => Some(SPPR_ORDER),
        b"txBody" => Some(TXBODY_ORDER),
        _ => None,
    }
}

/// Return the rank (position) of a child local name in the order table.
/// Unknown names get a rank past the end (appended).
fn rank_of(local: &[u8], table: &[&[u8]]) -> usize {
    table
        .iter()
        .position(|&entry| entry == local)
        .unwrap_or(table.len())
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

/// Public XML-escape for sibling modules authoring element text.
pub fn escape_public(s: &str) -> String {
    escape(s)
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
        round_trips(
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a><b x="1">hi</b><c/></a>"#,
        );
    }

    #[test]
    fn round_trips_single_quotes_and_whitespace() {
        // python-pptx-style single-quoted decl + indentation must survive verbatim.
        round_trips(
            b"<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<a>\n  <b/>\n</a>\n",
        );
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
        let doc =
            Document::parse(br#"<p:sp><p:nvSpPr><p:ph type="title" idx="1"/></p:nvSpPr></p:sp>"#)
                .unwrap();
        let root = doc.root().unwrap();
        assert_eq!(root.local_name(), b"sp");
        let ph = root.find_descendant(b"ph").unwrap();
        assert_eq!(ph.attr(b"type").unwrap(), b"title");
        assert_eq!(ph.attr(b"idx").unwrap(), b"1");
    }
    #[test]
    fn set_text_preserves_siblings_and_escapes() {
        // Editing one <a:t> must leave the sibling run byte-identical.
        let src =
            br#"<a:p><a:r><a:rPr b="1"/><a:t>old</a:t></a:r><a:r><a:t>keep</a:t></a:r></a:p>"#;
        let mut doc = Document::parse(src).unwrap();
        let root = doc.root_mut().unwrap();
        let first_run = root.children_named_mut(b"r").next().unwrap();
        let t = first_run.children_named_mut(b"t").next().unwrap();
        assert_eq!(t.text_content(), "old");
        t.set_text_content("a < b & c");
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.contains("<a:t>a &lt; b &amp; c</a:t>"), "got {s}");
        assert!(s.contains(r#"<a:rPr b="1"/>"#), "rPr verbatim: {s}");
        assert!(
            s.contains("<a:r><a:t>keep</a:t></a:r>"),
            "2nd run verbatim: {s}"
        );
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
        let b = doc
            .root_mut()
            .unwrap()
            .children_named_mut(b"b")
            .next()
            .unwrap();
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
        assert!(
            out.windows(11)
                .any(|w| w == br#"<b x="1">ke"#.as_slice()
                    || w == br#"<b x="1">keep"#[..11].as_ref())
        );
        assert_eq!(out, br#"<a><b x="1">keep</b><c>edit</c></a>"#);
    }

    // ─── DOM mutation toolkit tests ─────────────────────────────────────

    #[test]
    fn insert_child_at_beginning() {
        let mut doc = Document::parse(b"<a><b/><c/></a>").unwrap();
        let new_node = Node::Element(Element {
            name: b"z".to_vec(),
            raw_start: b"<z/>".to_vec(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: false,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        doc.root_mut().unwrap().insert_child_at(0, new_node);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.contains("<a><z/><b/><c/></a>"), "got {s}");
    }

    #[test]
    fn insert_child_at_middle() {
        let mut doc = Document::parse(b"<a><b/><c/></a>").unwrap();
        let new_node = Node::Element(Element {
            name: b"z".to_vec(),
            raw_start: b"<z/>".to_vec(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: false,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        doc.root_mut().unwrap().insert_child_at(1, new_node);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.contains("<a><b/><z/><c/></a>"), "got {s}");
    }

    #[test]
    fn insert_child_at_end_when_index_exceeds_len() {
        let mut doc = Document::parse(b"<a><b/></a>").unwrap();
        let new_node = Node::Element(Element {
            name: b"z".to_vec(),
            raw_start: b"<z/>".to_vec(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: false,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        doc.root_mut().unwrap().insert_child_at(999, new_node);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.contains("<a><b/><z/></a>"), "got {s}");
    }

    #[test]
    fn insert_child_at_opens_self_closing() {
        let mut doc = Document::parse(b"<a/>").unwrap();
        let new_node = Node::Element(Element {
            name: b"b".to_vec(),
            raw_start: b"<b/>".to_vec(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: false,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        doc.root_mut().unwrap().insert_child_at(0, new_node);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert_eq!(s, "<a><b/></a>");
    }

    #[test]
    fn remove_children_where_removes_matching() {
        let mut doc = Document::parse(b"<a><b/><c/><b/><d/></a>").unwrap();
        let removed = doc
            .root_mut()
            .unwrap()
            .remove_children_where(|n| matches!(n, Node::Element(e) if e.local_name() == b"b"));
        assert_eq!(removed.len(), 2);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert_eq!(s, "<a><c/><d/></a>");
    }

    #[test]
    fn remove_children_where_no_match() {
        let mut doc = Document::parse(b"<a><b/><c/></a>").unwrap();
        let removed = doc
            .root_mut()
            .unwrap()
            .remove_children_where(|n| matches!(n, Node::Element(e) if e.local_name() == b"z"));
        assert_eq!(removed.len(), 0);
        // Unchanged
        assert_eq!(doc.to_bytes(), b"<a><b/><c/></a>");
    }

    #[test]
    fn replace_child_swaps_node() {
        let mut doc = Document::parse(b"<a><b/><c/><d/></a>").unwrap();
        let new_node = Node::Element(Element {
            name: b"z".to_vec(),
            raw_start: b"<z/>".to_vec(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: false,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        let old = doc.root_mut().unwrap().replace_child(1, new_node);
        // Old node was <c/>
        if let Node::Element(e) = old {
            assert_eq!(e.local_name(), b"c");
        } else {
            panic!("expected element");
        }
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert_eq!(s, "<a><b/><z/><d/></a>");
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn replace_child_panics_on_out_of_bounds() {
        let mut doc = Document::parse(b"<a><b/></a>").unwrap();
        let new_node = Node::Element(Element {
            name: b"z".to_vec(),
            raw_start: b"<z/>".to_vec(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: false,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        doc.root_mut().unwrap().replace_child(5, new_node);
    }

    #[test]
    fn insert_child_ordered_rpr_latin_before_hlinkclick() {
        // a:rPr with hlinkClick already present; inserting latin should go before it.
        let src = br#"<a:rPr><a:solidFill/><a:hlinkClick r:id="rId1"/></a:rPr>"#;
        let mut doc = Document::parse(src).unwrap();
        let rpr = doc.root_mut().unwrap();
        let latin = Node::Element(Element {
            name: b"a:latin".to_vec(),
            raw_start: Vec::new(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: true,
            attrs: vec![(b"typeface".to_vec(), b"Arial".to_vec())],
            children: Vec::new(),
        });
        rpr.insert_child_ordered(latin);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        // latin (rank 14) should be after solidFill (rank 2) and before hlinkClick (rank 18)
        let latin_pos = s.find("a:latin").unwrap();
        let solid_pos = s.find("a:solidFill").unwrap();
        let hlink_pos = s.find("a:hlinkClick").unwrap();
        assert!(solid_pos < latin_pos, "solidFill before latin: {s}");
        assert!(latin_pos < hlink_pos, "latin before hlinkClick: {s}");
    }

    #[test]
    fn insert_child_ordered_ppr_spcbef_before_buchar() {
        // a:pPr with buChar; inserting spcBef should go before it.
        let src = b"<a:pPr><a:buChar char=\"-\"/></a:pPr>";
        let mut doc = Document::parse(src).unwrap();
        let ppr = doc.root_mut().unwrap();
        let spc_bef = Node::Element(Element {
            name: b"a:spcBef".to_vec(),
            raw_start: Vec::new(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: true,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        ppr.insert_child_ordered(spc_bef);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        let spc_pos = s.find("a:spcBef").unwrap();
        let bu_pos = s.find("a:buChar").unwrap();
        assert!(spc_pos < bu_pos, "spcBef before buChar: {s}");
    }

    #[test]
    fn insert_child_ordered_sppr_solidfill_after_xfrm() {
        // p:spPr with xfrm; inserting solidFill should go after xfrm.
        let src = b"<p:spPr><a:xfrm/></p:spPr>";
        let mut doc = Document::parse(src).unwrap();
        let sppr = doc.root_mut().unwrap();
        let fill = Node::Element(Element {
            name: b"a:solidFill".to_vec(),
            raw_start: Vec::new(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: true,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        sppr.insert_child_ordered(fill);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        let xfrm_pos = s.find("a:xfrm").unwrap();
        let fill_pos = s.find("a:solidFill").unwrap();
        assert!(xfrm_pos < fill_pos, "xfrm before solidFill: {s}");
    }

    #[test]
    fn insert_child_ordered_txbody_p_after_bodypr() {
        // p:txBody with bodyPr; inserting p should go after bodyPr.
        let src = b"<p:txBody><a:bodyPr/></p:txBody>";
        let mut doc = Document::parse(src).unwrap();
        let txbody = doc.root_mut().unwrap();
        let p = Node::Element(Element {
            name: b"a:p".to_vec(),
            raw_start: Vec::new(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: true,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        txbody.insert_child_ordered(p);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        let body_pos = s.find("a:bodyPr").unwrap();
        let p_pos = s.find("a:p").unwrap();
        assert!(body_pos < p_pos, "bodyPr before p: {s}");
    }

    #[test]
    fn insert_child_ordered_unknown_parent_appends() {
        // An element with no ordering table just appends.
        let src = b"<foo><bar/></foo>";
        let mut doc = Document::parse(src).unwrap();
        let foo = doc.root_mut().unwrap();
        let baz = Node::Element(Element {
            name: b"baz".to_vec(),
            raw_start: b"<baz/>".to_vec(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: false,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        foo.insert_child_ordered(baz);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert_eq!(s, "<foo><bar/><baz/></foo>");
    }

    #[test]
    fn insert_child_ordered_unknown_child_appends() {
        // A child not in the table for a known parent appends.
        let src = b"<a:rPr><a:solidFill/></a:rPr>";
        let mut doc = Document::parse(src).unwrap();
        let rpr = doc.root_mut().unwrap();
        let unknown = Node::Element(Element {
            name: b"a:unknownExt".to_vec(),
            raw_start: Vec::new(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: true,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        rpr.insert_child_ordered(unknown);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        // Unknown goes after all known elements (appended)
        assert!(s.ends_with("a:unknownExt/></a:rPr>"), "got {s}");
    }

    #[test]
    fn insert_child_ordered_opens_self_closing_parent() {
        let mut doc = Document::parse(b"<a:pPr/>").unwrap();
        let ppr = doc.root_mut().unwrap();
        let spc = Node::Element(Element {
            name: b"a:spcBef".to_vec(),
            raw_start: Vec::new(),
            raw_end: Vec::new(),
            self_closing: true,
            dirty: true,
            attrs: Vec::new(),
            children: Vec::new(),
        });
        ppr.insert_child_ordered(spc);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.contains("a:spcBef"), "got {s}");
        assert!(s.starts_with("<a:pPr>"), "should open: {s}");
        assert!(s.ends_with("</a:pPr>"), "should close: {s}");
    }

    #[test]
    fn insert_child_ordered_raw_node_appends() {
        // Raw (non-element) nodes are always appended.
        let src = b"<a:rPr><a:solidFill/></a:rPr>";
        let mut doc = Document::parse(src).unwrap();
        let rpr = doc.root_mut().unwrap();
        let raw = Node::Raw(b"<!-- comment -->".to_vec());
        rpr.insert_child_ordered(raw);
        let s = String::from_utf8(doc.to_bytes()).unwrap();
        assert!(s.ends_with("<!-- comment --></a:rPr>"), "got {s}");
    }
}
