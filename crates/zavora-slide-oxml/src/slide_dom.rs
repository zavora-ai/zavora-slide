//! Semantic slide model over the lossless [`Document`] tree.
//!
//! This is the production editing surface: it locates PresentationML structures
//! (the shape tree, placeholders, text bodies, paragraphs, runs) *as references
//! into the DOM*, so edits mutate exactly those nodes and every other byte of the
//! slide is preserved. Nothing is copied into a parallel model — the DOM is the
//! single source of truth.

use crate::error::{OxmlError, Result};
use crate::xml::{Document, Element, Node};

/// A slide part (`ppt/slides/slideN.xml`) as an editable DOM.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideDom {
    doc: Document,
}

impl SlideDom {
    /// Parse a slide part into the editable DOM.
    pub fn parse(src: &[u8]) -> Result<SlideDom> {
        Ok(SlideDom { doc: Document::parse(src)? })
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
        self.sp_tree().into_iter().flat_map(|t| t.children_named(b"sp"))
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
                SlideDom::shape_ph_type(sp).filter(|t| t == "body").map(|_| sp)
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
            tx.children
                .push(Node::Element(make_paragraph(text, *level, &ppr_tmpl, &rpr_tmpl)));
        }
        Ok(())
    }
}

/// Build an `<a:p>` with optional pPr template (level applied) and one run.
fn make_paragraph(text: &str, level: u8, ppr_tmpl: &Option<Element>, rpr_tmpl: &Option<Element>) -> Element {
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
    p.children.push(Node::Element(make_run(text, rpr_tmpl.clone())));
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
    first_p.children.push(Node::Element(make_run(text, preserved_rpr)));
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
        assert!(s.contains(r#"<a:rPr lang="en-US" b="1"/>"#), "rPr preserved: {s}");
        assert!(s.contains("<a:t>Bullet one</a:t>"), "body untouched: {s}");
        assert!(s.contains("<a:t>Bullet two</a:t>"));
        // Title shape's spPr/placeholder binding intact.
        assert!(s.contains(r#"<p:ph type="title"/>"#));
    }

    #[test]
    fn set_body_bullets_preserves_title_and_formatting() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_body_bullets(&[("New A".into(), 0), ("New B".into(), 1)]).unwrap();
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
    fn set_title_preserves_body_shape_byte_for_byte() {
        let mut dom = SlideDom::parse(SLIDE).unwrap();
        dom.set_title("X").unwrap();
        let out = String::from_utf8(dom.to_bytes()).unwrap();
        let src = String::from_utf8(SLIDE.to_vec()).unwrap();
        // Everything from the body shape onward is byte-identical to the source.
        let anchor = "<p:sp><p:nvSpPr><p:cNvPr id=\"3\"";
        assert_eq!(&out[out.find(anchor).unwrap()..], &src[src.find(anchor).unwrap()..]);
    }
}
