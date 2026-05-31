//! High-level PowerPoint (.pptx) presentation API.

use std::io::{Seek, Write};
use std::path::Path;

use zavora_slide_opc::{OpcPackage, rel_types};
use zavora_slide_oxml::{Presentation as PresXml, RawPart, SlideIdEntry};

use crate::error::{Result, SlideError};
use crate::slide::{Slide, SlideData};
use crate::template;
use crate::units::{Layout, SlideSize};

/// An in-memory presentation. The typed `presentation.xml` model owns the
/// slide/master id lists and size; master/layout/theme are carried verbatim;
/// slides hold structured, authorable content.
pub struct Presentation {
    pres: PresXml,
    master: RawPart,
    layout: RawPart,
    theme: RawPart,
    slides: Vec<SlideData>,
}

impl Presentation {
    /// Create a blank 16:9 deck: one master, one layout, one theme, zero slides.
    pub fn new() -> Self {
        let mut pres = PresXml::new();
        pres.master_ids.push(SlideIdEntry { id: 2147483648, r_id: "rId1".into() });
        Self {
            pres,
            master: RawPart::from_xml(template::SLIDE_MASTER_XML.as_bytes()),
            layout: RawPart::from_xml(template::SLIDE_LAYOUT_XML.as_bytes()),
            theme: RawPart::from_xml(template::THEME_XML.as_bytes()),
            slides: Vec::new(),
        }
    }

    /// Number of slides in the deck.
    pub fn slide_count(&self) -> usize {
        self.slides.len()
    }

    /// Set the deck slide size.
    pub fn set_slide_size(&mut self, size: SlideSize) {
        let (cx, cy, ty) = size.dims();
        self.pres.slide_size.cx = cx;
        self.pres.slide_size.cy = cy;
        self.pres.slide_size.ty = Some(ty.to_string());
    }

    /// Apply a theme (color scheme + fonts) to the deck.
    pub fn apply_theme(&mut self, theme: &crate::theme::ThemeSpec) {
        self.theme = RawPart { xml: theme.build_theme_xml() };
    }

    /// Rebuild `sldIdLst` from the current slide order: slide N (0-based) gets
    /// numeric id 256+N and presentation rel id rId(N+2) (rId1 is the master).
    /// Called after every structural mutation so ids/rels stay consistent.
    fn resync_slide_ids(&mut self) {
        self.pres.slide_ids = (0..self.slides.len())
            .map(|i| SlideIdEntry { id: 256 + i as u32, r_id: format!("rId{}", i + 2) })
            .collect();
    }

    /// Append a blank slide bound to the (single, Phase 0) layout. Returns the
    /// new slide's 0-based index. `_layout` is accepted for API stability;
    /// per-layout placeholder geometry lands in later phases.
    pub fn add_slide(&mut self, _layout: Layout) -> usize {
        self.slides.push(SlideData::new());
        self.resync_slide_ids();
        self.slides.len() - 1
    }

    /// Deep-copy the slide at `idx`, inserting the copy immediately after it.
    pub fn duplicate_slide(&mut self, idx: usize) -> Result<usize> {
        let copy = self
            .slides
            .get(idx)
            .ok_or_else(|| SlideError::NotFound(format!("slide index {idx}")))?
            .clone();
        self.slides.insert(idx + 1, copy);
        self.resync_slide_ids();
        Ok(idx + 1)
    }

    /// Remove the slide at `idx`.
    pub fn delete_slide(&mut self, idx: usize) -> Result<()> {
        if idx >= self.slides.len() {
            return Err(SlideError::NotFound(format!("slide index {idx}")));
        }
        self.slides.remove(idx);
        self.resync_slide_ids();
        Ok(())
    }

    /// Move the slide at `from` to position `to`.
    pub fn move_slide(&mut self, from: usize, to: usize) -> Result<()> {
        let n = self.slides.len();
        if from >= n || to >= n {
            return Err(SlideError::NotFound(format!("slide index {}", from.max(to))));
        }
        let s = self.slides.remove(from);
        self.slides.insert(to, s);
        self.resync_slide_ids();
        Ok(())
    }

    /// Borrow a slide for editing (title, bullets, text boxes).
    pub fn slide_mut(&mut self, idx: usize) -> Result<Slide<'_>> {
        let (cx, cy) = (self.pres.slide_size.cx, self.pres.slide_size.cy);
        let data = self
            .slides
            .get_mut(idx)
            .ok_or_else(|| SlideError::NotFound(format!("slide index {idx}")))?;
        Ok(Slide { data, slide_cx: cx, slide_cy: cy })
    }

    /// A text outline of the deck: per slide, its shape text and any notes.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for (i, s) in self.slides.iter().enumerate() {
            out.push_str(&format!("# Slide {}\n", i + 1));
            for sp in &s.shapes {
                for p in &sp.body.paragraphs {
                    let t = p.text();
                    if t.is_empty() {
                        continue;
                    }
                    let lvl = p.level.unwrap_or(0) as usize;
                    out.push_str(&format!("{}- {}\n", "  ".repeat(lvl), t));
                }
            }
            if let Some(n) = &s.notes {
                out.push_str(&format!("> notes: {n}\n"));
            }
        }
        out
    }

    /// Save to a file path.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let pkg = self.build_package()?;
        pkg.save(path)?;
        Ok(())
    }

    /// Serialize the package to bytes.
    pub fn save_to_buffer(&self) -> Result<Vec<u8>> {
        let mut buf = std::io::Cursor::new(Vec::new());
        self.write_to(&mut buf)?;
        Ok(buf.into_inner())
    }

    fn write_to<W: Write + Seek>(&self, w: W) -> Result<()> {
        self.build_package()?.write_to(w)?;
        Ok(())
    }

    /// Assemble the full OPC package: parts, content types, and relationships.
    fn build_package(&self) -> Result<OpcPackage> {
        let mut pkg = OpcPackage::new_pptx();
        let ct = &mut pkg.content_types;
        ct.add_override("/ppt/slideMasters/slideMaster1.xml", template::CT_SLIDE_MASTER);
        ct.add_override("/ppt/slideLayouts/slideLayout1.xml", template::CT_SLIDE_LAYOUT);
        ct.add_override("/ppt/theme/theme1.xml", template::CT_THEME);
        ct.add_override("/ppt/presProps.xml", template::CT_PRES_PROPS);
        ct.add_override("/ppt/viewProps.xml", template::CT_VIEW_PROPS);
        ct.add_override("/ppt/tableStyles.xml", template::CT_TABLE_STYLES);
        ct.add_override("/docProps/core.xml", template::CT_CORE);
        ct.add_override("/docProps/app.xml", template::CT_APP);

        // Core + standard auxiliary parts (every real .pptx ships these).
        pkg.set_part("/ppt/presentation.xml", self.pres.to_xml()?);
        pkg.set_part("/ppt/slideMasters/slideMaster1.xml", self.master.to_xml());
        pkg.set_part("/ppt/slideLayouts/slideLayout1.xml", self.layout.to_xml());
        pkg.set_part("/ppt/theme/theme1.xml", self.theme.to_xml());
        pkg.set_part("/ppt/presProps.xml", template::PRES_PROPS_XML.as_bytes().to_vec());
        pkg.set_part("/ppt/viewProps.xml", template::VIEW_PROPS_XML.as_bytes().to_vec());
        pkg.set_part("/ppt/tableStyles.xml", template::TABLE_STYLES_XML.as_bytes().to_vec());
        pkg.set_part("/docProps/core.xml", template::CORE_XML.as_bytes().to_vec());
        pkg.set_part("/docProps/app.xml", template::APP_XML.as_bytes().to_vec());

        // Package-level rels add docProps (presentation is already rId1 here).
        pkg.package_rels.add_if_absent(template::RT_CORE, "docProps/core.xml");
        pkg.package_rels.add_if_absent(template::RT_EXTENDED, "docProps/app.xml");

        // presentation.xml rels: master (rId1), slides (rId2..), then aux parts.
        {
            let aux = self.slides.len() + 2;
            let rels = pkg.get_or_create_part_rels("/ppt/presentation.xml");
            rels.add_with_id("rId1", rel_types::SLIDE_MASTER, "slideMasters/slideMaster1.xml");
            for idx in 0..self.slides.len() {
                rels.add_with_id(
                    &format!("rId{}", idx + 2),
                    rel_types::SLIDE,
                    &format!("slides/slide{}.xml", idx + 1),
                );
            }
            rels.add_with_id(&format!("rId{aux}"), template::RT_PRES_PROPS, "presProps.xml");
            rels.add_with_id(&format!("rId{}", aux + 1), template::RT_VIEW_PROPS, "viewProps.xml");
            rels.add_with_id(&format!("rId{}", aux + 2), rel_types::THEME, "theme/theme1.xml");
            rels.add_with_id(&format!("rId{}", aux + 3), template::RT_TABLE_STYLES, "tableStyles.xml");
        }

        // Master → layout (rId1) + theme (rId2).
        {
            let rels = pkg.get_or_create_part_rels("/ppt/slideMasters/slideMaster1.xml");
            rels.add_with_id("rId1", rel_types::SLIDE_LAYOUT, "../slideLayouts/slideLayout1.xml");
            rels.add_with_id("rId2", rel_types::THEME, "../theme/theme1.xml");
        }

        // Layout → master (rId1).
        {
            let rels = pkg.get_or_create_part_rels("/ppt/slideLayouts/slideLayout1.xml");
            rels.add_with_id("rId1", rel_types::SLIDE_MASTER, "../slideMasters/slideMaster1.xml");
        }

        // Each slide part + its layout rel (+ notes rel when present).
        let has_notes = self.slides.iter().any(|s| s.notes.is_some());
        for (idx, slide) in self.slides.iter().enumerate() {
            let part = format!("/ppt/slides/slide{}.xml", idx + 1);
            pkg.content_types.add_override(&part, template::CT_SLIDE);
            pkg.set_part(&part, slide.to_xml());
            {
                let rels = pkg.get_or_create_part_rels(&part);
                rels.add_with_id("rId1", rel_types::SLIDE_LAYOUT, "../slideLayouts/slideLayout1.xml");
                if slide.notes.is_some() {
                    rels.add_with_id(
                        "rId2",
                        template::RT_NOTES_SLIDE,
                        &format!("../notesSlides/notesSlide{}.xml", idx + 1),
                    );
                }
                for img in &slide.images {
                    rels.add_with_id(
                        &img.embed_rid,
                        rel_types::IMAGE,
                        &format!("../media/image{}_{}.{}", idx + 1, img.id, img.ext),
                    );
                }
            }
            // Image media parts (default content types for png/jpeg).
            for img in &slide.images {
                let ct_ext = if img.ext == "jpg" { "jpeg" } else { img.ext.as_str() };
                pkg.content_types.add_default(ct_ext, &format!("image/{ct_ext}"));
                pkg.set_part(
                    &format!("/ppt/media/image{}_{}.{}", idx + 1, img.id, img.ext),
                    img.data.clone(),
                );
            }
            // notesSlide part: links to the notes master and back to its slide.
            if let Some(notes) = &slide.notes {
                let np = format!("/ppt/notesSlides/notesSlide{}.xml", idx + 1);
                pkg.content_types.add_override(&np, template::CT_NOTES_SLIDE);
                pkg.set_part(&np, crate::slide::notes_slide_xml(notes));
                let nrels = pkg.get_or_create_part_rels(&np);
                nrels.add_with_id("rId1", template::RT_NOTES_MASTER, "../notesMasters/notesMaster1.xml");
                nrels.add_with_id("rId2", template::RT_SLIDE, &format!("../slides/slide{}.xml", idx + 1));
            }
        }

        // Shared notes master (theme-linked) when any slide has notes.
        if has_notes {
            pkg.content_types.add_override("/ppt/notesMasters/notesMaster1.xml", template::CT_NOTES_MASTER);
            pkg.set_part("/ppt/notesMasters/notesMaster1.xml", template::NOTES_MASTER_XML.as_bytes().to_vec());
            pkg.get_or_create_part_rels("/ppt/notesMasters/notesMaster1.xml")
                .add_with_id("rId1", rel_types::THEME, "../theme/theme1.xml");
        }

        Ok(pkg)
    }
}

impl Default for Presentation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_blank() {
        let p = Presentation::new();
        assert_eq!(p.slide_count(), 0);
    }

    #[test]
    fn add_slides_increments() {
        let mut p = Presentation::new();
        assert_eq!(p.add_slide(Layout::TitleContent), 0);
        assert_eq!(p.add_slide(Layout::Blank), 1);
        assert_eq!(p.slide_count(), 2);
        assert_eq!(p.pres.slide_ids.len(), 2);
    }

    #[test]
    fn duplicate_delete_move_keep_ids_consistent() {
        let mut p = Presentation::new();
        p.add_slide(Layout::Blank); // 0
        p.add_slide(Layout::Blank); // 1
        p.slide_mut(0).unwrap().set_title("A").unwrap();
        p.slide_mut(1).unwrap().set_title("B").unwrap();

        // Duplicate slide 0 → copy at index 1 (content deep-copied).
        assert_eq!(p.duplicate_slide(0).unwrap(), 1);
        assert_eq!(p.slide_count(), 3);
        assert_eq!(p.slide_mut(1).unwrap().text(), "A");

        // Move the copy (1) to the end (2).
        p.move_slide(1, 2).unwrap();
        assert_eq!(p.slide_mut(2).unwrap().text(), "A");

        // Delete first slide.
        p.delete_slide(0).unwrap();
        assert_eq!(p.slide_count(), 2);

        // Slide ids are contiguous and rel-ids match index.
        assert_eq!(p.pres.slide_ids.len(), 2);
        assert_eq!(p.pres.slide_ids[0].r_id, "rId2");
        assert_eq!(p.pres.slide_ids[1].r_id, "rId3");

        // Out-of-range ops error.
        assert!(p.delete_slide(9).is_err());
        assert!(p.duplicate_slide(9).is_err());
        assert!(p.move_slide(0, 9).is_err());
    }

    #[test]
    fn buffer_is_valid_zip_with_parts() {
        let mut p = Presentation::new();
        p.add_slide(Layout::TitleContent);
        let bytes = p.save_to_buffer().unwrap();
        // Re-open via the OPC layer and assert required parts exist.
        let pkg = OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(pkg.main_presentation_part().as_deref(), Some("/ppt/presentation.xml"));
        assert!(pkg.get_part("/ppt/slideMasters/slideMaster1.xml").is_some());
        assert!(pkg.get_part("/ppt/slideLayouts/slideLayout1.xml").is_some());
        assert!(pkg.get_part("/ppt/theme/theme1.xml").is_some());
        assert!(pkg.get_part("/ppt/slides/slide1.xml").is_some());
    }
}
