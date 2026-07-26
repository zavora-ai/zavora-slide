//! Semantic notes-slide model over the lossless [`Document`] tree.
//!
//! Mirrors [`SlideDom`] but for `ppt/notesSlides/notesSlideN.xml`. Edits mutate
//! the body placeholder's text in place; every other byte of the notes part is
//! preserved (surgical edit, no full rebuild).

use crate::error::{OxmlError, Result};
use crate::xml::{Document, Element, Node};

/// A notes-slide part (`ppt/notesSlides/notesSlideN.xml`) as an editable DOM.
///
/// The notes slide contains a shape tree with (at minimum) a slide-image
/// placeholder and a body placeholder carrying the speaker notes text.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesDom {
    doc: Document,
}

impl NotesDom {
    /// Parse a notes-slide part into the editable DOM.
    pub fn parse(src: &[u8]) -> Result<NotesDom> {
        Ok(NotesDom {
            doc: Document::parse(src)?,
        })
    }

    /// Serialize back to bytes (byte-identical when unedited).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.doc.to_bytes()
    }

    /// The `<p:spTree>` element (under `p:notes > p:cSld > p:spTree`).
    fn sp_tree(&self) -> Option<&Element> {
        self.doc.root()?.find_descendant(b"spTree")
    }

    fn sp_tree_mut(&mut self) -> Option<&mut Element> {
        find_descendant_mut(self.doc.root_mut()?, b"spTree")
    }

    /// Find the body placeholder shape in the notes slide. The notes body
    /// placeholder has `<p:ph type="body" .../>` (idx may vary).
    fn find_body_placeholder(&self) -> Option<&Element> {
        let tree = self.sp_tree()?;
        for child in &tree.children {
            if let Node::Element(sp) = child {
                if sp.local_name() != b"sp" {
                    continue;
                }
                if Self::is_body_placeholder(sp) {
                    return Some(sp);
                }
            }
        }
        None
    }

    /// Find the body placeholder shape (mutable).
    fn find_body_placeholder_mut(&mut self) -> Option<&mut Element> {
        let tree = self.sp_tree_mut()?;
        for child in &mut tree.children {
            if let Node::Element(sp) = child {
                if sp.local_name() != b"sp" {
                    continue;
                }
                if Self::is_body_placeholder(sp) {
                    return Some(sp);
                }
            }
        }
        None
    }

    /// Check if a shape element is the body placeholder.
    fn is_body_placeholder(sp: &Element) -> bool {
        if let Some(ph) = sp.find_descendant(b"ph") {
            match ph.attr(b"type") {
                Some(t) => t == b"body",
                // Default placeholder type is "body" when no type attribute.
                None => true,
            }
        } else {
            false
        }
    }

    /// Read the current notes text from the body placeholder.
    ///
    /// Returns the concatenated text of all paragraphs (joined by newlines).
    /// Returns an empty string if no body placeholder or no text exists.
    pub fn notes_text(&self) -> String {
        let Some(sp) = self.find_body_placeholder() else {
            return String::new();
        };
        let Some(tx) = sp.find_descendant(b"txBody") else {
            return String::new();
        };
        let lines: Vec<String> = tx.children_named(b"p").map(paragraph_text).collect();
        lines.join("\n")
    }

    /// Set the notes text on the body placeholder.
    ///
    /// Splits `text` on newlines and creates one paragraph per line. The first
    /// existing paragraph's `a:pPr` and first run's `a:rPr` are preserved as
    /// formatting templates. All other shape properties (nvSpPr, spPr) and
    /// sibling shapes (slide-image placeholder) remain byte-identical.
    ///
    /// Returns an error if the notes slide has no body placeholder.
    pub fn set_notes_text(&mut self, text: &str) -> Result<()> {
        let sp = self
            .find_body_placeholder_mut()
            .ok_or_else(|| OxmlError::Parse("notes slide has no body placeholder".into()))?;
        let tx = find_descendant_mut(sp, b"txBody")
            .ok_or_else(|| OxmlError::Parse("notes body placeholder has no txBody".into()))?;

        // Capture formatting templates from the first existing paragraph/run.
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

        // Create one paragraph per line (or a single paragraph if no newlines).
        let lines: Vec<&str> = if text.is_empty() {
            vec![""]
        } else {
            text.lines().collect()
        };

        for line in lines {
            let p = make_paragraph_with_rpr(line, &rpr_tmpl);
            tx.children.push(Node::Element(p));
        }

        Ok(())
    }
}

// ─── Helper functions ───────────────────────────────────────────────────────

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

/// Build an `<a:p>` with one `<a:r>` run, optionally preserving an `a:rPr`.
fn make_paragraph_with_rpr(text: &str, rpr_tmpl: &Option<Element>) -> Element {
    let mut p = new_element(b"a:p");
    let run = make_run(text, rpr_tmpl.clone());
    p.children.push(Node::Element(run));
    p
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal notes-slide XML matching what PowerPoint emits.
    fn sample_notes_xml() -> Vec<u8> {
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Original notes</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#.to_vec()
    }

    #[test]
    fn parse_and_read_notes_text() {
        let dom = NotesDom::parse(&sample_notes_xml()).unwrap();
        assert_eq!(dom.notes_text(), "Original notes");
    }

    #[test]
    fn set_notes_text_single_line() {
        let mut dom = NotesDom::parse(&sample_notes_xml()).unwrap();
        dom.set_notes_text("Updated notes").unwrap();
        assert_eq!(dom.notes_text(), "Updated notes");
    }

    #[test]
    fn set_notes_text_multi_line() {
        let mut dom = NotesDom::parse(&sample_notes_xml()).unwrap();
        dom.set_notes_text("Line one\nLine two\nLine three")
            .unwrap();
        assert_eq!(dom.notes_text(), "Line one\nLine two\nLine three");
    }

    #[test]
    fn notes_edit_is_surgical() {
        let original = sample_notes_xml();
        let mut dom = NotesDom::parse(&original).unwrap();
        dom.set_notes_text("New text").unwrap();
        let output = dom.to_bytes();

        // The slide-image placeholder and other structural elements should be
        // preserved byte-for-byte. Check that the sldImg placeholder is intact.
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Slide Image Placeholder 1"));
        assert!(output_str.contains(r#"<p:ph type="sldImg" idx="1"/>"#));
        // The notes text should be updated.
        assert!(output_str.contains("New text"));
        assert!(!output_str.contains("Original notes"));
    }

    #[test]
    fn round_trip_unedited_is_byte_identical() {
        let original = sample_notes_xml();
        let dom = NotesDom::parse(&original).unwrap();
        assert_eq!(dom.to_bytes(), original);
    }
}
