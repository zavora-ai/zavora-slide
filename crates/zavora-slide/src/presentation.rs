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
    /// Next numeric slide id for `sldIdLst` (PowerPoint starts at 256).
    next_slide_id: u32,
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
            next_slide_id: 256,
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

    /// Append a blank slide bound to the (single, Phase 0) layout. Returns the
    /// new slide's 0-based index. `_layout` is accepted for API stability;
    /// per-layout placeholder geometry lands in later phases.
    pub fn add_slide(&mut self, _layout: Layout) -> usize {
        let idx = self.slides.len();
        self.slides.push(SlideData::new());
        let id = self.next_slide_id;
        self.next_slide_id += 1;
        // Presentation-part rel id for this slide: rId(2 + idx) — rId1 is the master.
        self.pres.slide_ids.push(SlideIdEntry { id, r_id: format!("rId{}", idx + 2) });
        idx
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

        // Each slide part + its layout rel.
        for (idx, slide) in self.slides.iter().enumerate() {
            let part = format!("/ppt/slides/slide{}.xml", idx + 1);
            pkg.content_types.add_override(&part, template::CT_SLIDE);
            pkg.set_part(&part, slide.to_xml());
            pkg.get_or_create_part_rels(&part).add_with_id(
                "rId1",
                rel_types::SLIDE_LAYOUT,
                "../slideLayouts/slideLayout1.xml",
            );
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
