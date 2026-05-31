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
    /// Original package when opened from an existing `.pptx`. Re-emitted verbatim
    /// on save for a faithful round-trip; cleared by any edit (after which save
    /// rebuilds from the text-only model — see [`Presentation::open`]).
    source: Option<OpcPackage>,
    /// Set when slides of a source-backed deck were reordered/deleted (but not
    /// added/duplicated): the overlay save rebuilds `sldIdLst`/presentation rels
    /// from the surviving slides' preserved identities and prunes removed parts,
    /// keeping master/layouts/theme/other parts byte-identical.
    reordered: bool,
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
            source: None,
            reordered: false,
        }
    }

    /// Open an existing `.pptx`. The original package is preserved; on
    /// [`save`](Self::save) it is re-emitted with all parts byte-identical except
    /// the slides you actually edited, which are re-authored in place (an
    /// **overlay save**). Master, layouts, theme, untouched slides, and media are
    /// preserved verbatim — so editing one slide no longer rebuilds the whole deck.
    ///
    /// Each opened slide's in-memory model is **text-only** (paragraph text +
    /// level); an edited slide is re-authored from that model, so its own
    /// original unmodeled detail (custom shapes, exact run formatting) is not
    /// preserved. Structural changes (add/delete/move/duplicate slide, theme,
    /// slide size) fall back to a full rebuild from the engine's model.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_from_package(OpcPackage::open(path)?)
    }

    /// Open from in-memory `.pptx` bytes (see [`Presentation::open`]).
    pub fn open_from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::open_from_package(OpcPackage::from_reader(std::io::Cursor::new(bytes.to_vec()))?)
    }

    fn open_from_package(pkg: OpcPackage) -> Result<Self> {
        use zavora_slide_oxml::TextBody;
        let pres_xml = pkg
            .get_part("/ppt/presentation.xml")
            .ok_or_else(|| SlideError::NotFound("presentation.xml".into()))?;
        let parsed = PresXml::from_xml(pres_xml)?;

        let mut p = Presentation::new();
        p.pres.slide_size = parsed.slide_size.clone();

        // Slides in id-list order; resolve each rel target to a part.
        let rels = pkg.get_part_rels("/ppt/presentation.xml");
        for entry in &parsed.slide_ids {
            let target = rels
                .and_then(|r| r.get_by_id(&entry.r_id))
                .map(|rel| OpcPackage::resolve_rel_target("/ppt/presentation.xml", &rel.target));
            let mut data = SlideData::new();
            data.sld_id = Some((entry.id, entry.r_id.clone()));
            if let Some(part_path) = target.as_deref() {
                data.source_part = Some(part_path.to_string());
                if let Some(part) = pkg.get_part(part_path) {
                    // Authoritative editable DOM (lossless, surgical edits).
                    data.dom = Some(zavora_slide_oxml::SlideDom::parse(part)?);
                    // Text extraction populates ONLY the build model (render/
                    // markdown read paths); must not touch the DOM we just parsed.
                    let body = TextBody::from_xml(part)?;
                    if !body.paragraphs.is_empty() {
                        let bullets: Vec<crate::slide::Bullet> = body
                            .paragraphs
                            .iter()
                            .map(|para| crate::slide::Bullet {
                                text: para.text(),
                                level: para.level.unwrap_or(0),
                                bold: false,
                            })
                            .collect();
                        let (cx, cy) = (p.pres.slide_size.cx, p.pres.slide_size.cy);
                        let mut slide = Slide { data: &mut data, slide_cx: cx, slide_cy: cy };
                        slide.sync_build_bullets_public(&bullets);
                    }
                }
            }
            data.dirty = false;
            p.slides.push(data);
        }
        p.resync_slide_ids();
        // Preserve the original package for a faithful round-trip on save.
        p.source = Some(pkg);
        Ok(p)
    }

    /// Discard the preserved source package (called by any structural/content
    /// edit, so a subsequent save reflects the edited model rather than the
    /// original bytes).
    fn invalidate_source(&mut self) {
        self.source = None;
    }

    /// Number of slides in the deck.
    pub fn slide_count(&self) -> usize {
        self.slides.len()
    }

    /// Set the deck slide size.
    pub fn set_slide_size(&mut self, size: SlideSize) {
        self.invalidate_source();
        let (cx, cy, ty) = size.dims();
        self.pres.slide_size.cx = cx;
        self.pres.slide_size.cy = cy;
        self.pres.slide_size.ty = Some(ty.to_string());
    }

    /// Apply a theme (color scheme + fonts) to the deck.
    pub fn apply_theme(&mut self, theme: &crate::theme::ThemeSpec) {
        self.invalidate_source();
        self.theme = RawPart { xml: theme.build_theme_xml() };
    }

    /// Rebuild `sldIdLst` from the current slide order: slide N (0-based) gets
    /// numeric id 256+N and presentation rel id rId(N+2) (rId1 is the master).
    /// Called after every structural mutation so ids/rels stay consistent.
    fn resync_slide_ids(&mut self) {
        self.invalidate_source();
        self.pres.slide_ids = (0..self.slides.len())
            .map(|i| SlideIdEntry { id: 256 + i as u32, r_id: format!("rId{}", i + 2) })
            .collect();
    }

    /// Append a blank slide bound to the (single, Phase 0) layout. Returns the
    /// new slide's 0-based index. `_layout` is accepted for API stability;
    /// per-layout placeholder geometry lands in later phases.
    pub fn add_slide(&mut self, layout: Layout) -> usize {
        if self.source.is_some() && self.slides.iter().all(|s| s.sld_id.is_some()) {
            // Faithful add: a new blank slide bound to one of the deck's existing
            // layouts (resolved by type during save), inheriting its master/theme.
            let new_part = self.fresh_slide_part_path();
            let new_id = self.fresh_sld_id();
            let mut data = SlideData::new();
            data.source_part = Some(new_part);
            data.sld_id = Some((new_id, String::new()));
            data.new_blank_layout_type = Some(layout.layout_type().to_string());
            data.dom = zavora_slide_oxml::SlideDom::parse(&blank_slide_xml()).ok();
            self.slides.push(data);
            self.reordered = true;
            self.resync_build_ids_only();
        } else {
            self.slides.push(SlideData::new());
            self.invalidate_source();
            self.resync_slide_ids();
        }
        self.slides.len() - 1
    }

    /// Deep-copy the slide at `idx`, inserting the copy immediately after it.
    /// On a source-backed deck the copy is materialized faithfully: its part is a
    /// clone of the original (relationships included), so it references the same
    /// layout/media; master/layouts/theme and all other slides stay byte-identical.
    pub fn duplicate_slide(&mut self, idx: usize) -> Result<usize> {
        let mut copy = self
            .slides
            .get(idx)
            .ok_or_else(|| SlideError::NotFound(format!("slide index {idx}")))?
            .clone();

        if self.source.is_some() && copy.source_part.is_some() {
            // Faithful duplication: assign a fresh part path + sldId; clone rels
            // from the original; new part bytes come from the (cloned) DOM. The
            // presentation rel id is allocated during save (apply_reorder).
            let new_part = self.fresh_slide_part_path();
            let new_id = self.fresh_sld_id();
            copy.clone_rels_from = copy.source_part.clone();
            copy.source_part = Some(new_part);
            copy.sld_id = Some((new_id, String::new()));
            self.slides.insert(idx + 1, copy);
            self.reordered = true;
            self.resync_build_ids_only();
        } else {
            self.slides.insert(idx + 1, copy);
            self.invalidate_source();
            self.resync_slide_ids();
        }
        Ok(idx + 1)
    }

    /// A slide part path not already used by any slide (e.g. /ppt/slides/slideN.xml).
    fn fresh_slide_part_path(&self) -> String {
        let used: std::collections::HashSet<&str> =
            self.slides.iter().filter_map(|s| s.source_part.as_deref()).collect();
        (1..)
            .map(|n| format!("/ppt/slides/slide{n}.xml"))
            .find(|p| !used.contains(p.as_str()))
            .unwrap()
    }

    /// A numeric sldId greater than any in use (ids must be unique; PowerPoint
    /// uses values >= 256).
    fn fresh_sld_id(&self) -> u32 {
        self.slides.iter().filter_map(|s| s.sld_id.as_ref().map(|(id, _)| *id)).max().unwrap_or(255) + 1
    }

    /// Remove the slide at `idx`. On a source-backed deck this is faithful: the
    /// slide is dropped from `sldIdLst` and its part pruned, while master/
    /// layouts/theme/other slides stay byte-identical.
    pub fn delete_slide(&mut self, idx: usize) -> Result<()> {
        if idx >= self.slides.len() {
            return Err(SlideError::NotFound(format!("slide index {idx}")));
        }
        self.slides.remove(idx);
        self.mark_reorder_or_rebuild();
        Ok(())
    }

    /// Move the slide at `from` to position `to`. Faithful on a source-backed
    /// deck (only `sldIdLst` order changes).
    pub fn move_slide(&mut self, from: usize, to: usize) -> Result<()> {
        let n = self.slides.len();
        if from >= n || to >= n {
            return Err(SlideError::NotFound(format!("slide index {}", from.max(to))));
        }
        let s = self.slides.remove(from);
        self.slides.insert(to, s);
        self.mark_reorder_or_rebuild();
        Ok(())
    }

    /// After a delete/move: if every surviving slide came from the source (each
    /// has a preserved `sld_id`), keep the source and flag a faithful reorder;
    /// otherwise fall back to a full rebuild.
    fn mark_reorder_or_rebuild(&mut self) {
        if self.source.is_some() && self.slides.iter().all(|s| s.sld_id.is_some()) {
            self.reordered = true;
            self.resync_build_ids_only();
        } else {
            self.invalidate_source();
            self.resync_slide_ids();
        }
    }

    /// Resync only the build-model `sldIdLst` (used by the rebuild path).
    fn resync_build_ids_only(&mut self) {
        self.pres.slide_ids = (0..self.slides.len())
            .map(|i| SlideIdEntry { id: 256 + i as u32, r_id: format!("rId{}", i + 2) })
            .collect();
    }

    /// Borrow a slide for editing (title, bullets, text boxes). When the deck was
    /// opened from a source package, the edited slide is marked dirty and
    /// re-authored on save while every other part stays byte-identical (overlay
    /// save). The edited slide's own unmodeled detail is not preserved.
    pub fn slide_mut(&mut self, idx: usize) -> Result<Slide<'_>> {
        if idx >= self.slides.len() {
            return Err(SlideError::NotFound(format!("slide index {idx}")));
        }
        let (cx, cy) = (self.pres.slide_size.cx, self.pres.slide_size.cy);
        let data = &mut self.slides[idx];
        data.dirty = true;
        Ok(Slide { data, slide_cx: cx, slide_cy: cy })
    }

    /// Read-only borrow of a slide (does not invalidate the source package).
    pub fn slide(&self, idx: usize) -> Result<crate::slide::SlideRef<'_>> {
        let (cx, cy) = (self.pres.slide_size.cx, self.pres.slide_size.cy);
        let data = self
            .slides
            .get(idx)
            .ok_or_else(|| SlideError::NotFound(format!("slide index {idx}")))?;
        Ok(crate::slide::SlideRef { data, slide_cx: cx, slide_cy: cy })
    }

    /// Test/inspection helper: a slide's source part path and current DOM bytes,
    /// if it was opened from an existing deck.
    #[doc(hidden)]
    pub fn slide_dom_debug(&self, idx: usize) -> Option<(String, Vec<u8>)> {
        let s = self.slides.get(idx)?;
        Some((s.source_part.clone()?, s.dom.as_ref()?.to_bytes()))
    }

    /// Render a slide to PNG or SVG bytes at a default width (1280px).
    pub fn render_slide(&self, idx: usize, format: crate::units::RenderFormat) -> Result<Vec<u8>> {
        let data = self
            .slides
            .get(idx)
            .ok_or_else(|| SlideError::NotFound(format!("slide index {idx}")))?;
        let scene = data.to_scene(self.pres.slide_size.cx, self.pres.slide_size.cy);
        const WIDTH: u32 = 1280;
        match format {
            crate::units::RenderFormat::Svg => {
                Ok(zavora_slide_render::scene_to_svg(&scene, WIDTH).into_bytes())
            }
            crate::units::RenderFormat::Png => zavora_slide_render::scene_to_png(&scene, WIDTH)
                .map_err(|e| SlideError::Unsupported(format!("render: {e}"))),
        }
    }

    /// Render the whole deck to a PDF (one page per slide) and return the bytes.
    pub fn to_pdf_bytes(&self) -> Result<Vec<u8>> {
        let scenes: Vec<_> = self
            .slides
            .iter()
            .map(|s| s.to_scene(self.pres.slide_size.cx, self.pres.slide_size.cy))
            .collect();
        zavora_slide_pdf::scenes_to_pdf(&scenes).map_err(|e| SlideError::Unsupported(format!("pdf: {e}")))
    }

    /// Save the whole deck as a PDF (one page per slide).
    pub fn save_pdf<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        std::fs::write(path, self.to_pdf_bytes()?)?;
        Ok(())
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
        if let Some(pkg) = self.overlay_package()? {
            pkg.save(path)?;
            return Ok(());
        }
        self.build_package()?.save(path)?;
        Ok(())
    }

    /// Serialize the package to bytes.
    pub fn save_to_buffer(&self) -> Result<Vec<u8>> {
        let mut buf = std::io::Cursor::new(Vec::new());
        self.write_to(&mut buf)?;
        Ok(buf.into_inner())
    }

    fn write_to<W: Write + Seek>(&self, w: W) -> Result<()> {
        if let Some(pkg) = self.overlay_package()? {
            pkg.write_to(w)?;
            return Ok(());
        }
        self.build_package()?.write_to(w)?;
        Ok(())
    }

    /// When the deck was opened from a source package, return a faithful package:
    /// the original parts byte-for-byte, with only the **edited** slides updated.
    ///
    /// A slide edited purely through its DOM (text/title/bullets) is serialized
    /// from that DOM — every untouched byte of the slide is preserved and its
    /// relationships are left exactly as opened (fully surgical). A slide that
    /// gained engine-authored media (image/picture background) is re-authored
    /// with rebuilt media rels. Returns `None` (→ full rebuild) when there is no
    /// source, a structural change cleared it, or an edited slide gained notes
    /// (injecting a notesMaster into a foreign deck risks a repair prompt).
    fn overlay_package(&self) -> Result<Option<OpcPackage>> {
        let Some(src) = &self.source else { return Ok(None) };
        if self.slides.iter().any(|s| s.dirty && s.notes.is_some()) {
            return Ok(None);
        }
        let mut pkg = src.clone();
        for slide in self.slides.iter().filter(|s| s.dirty) {
            let Some(part) = &slide.source_part else { continue };
            let has_new_media = slide.background.is_some() || !slide.images.is_empty();

            // Surgical DOM path: serialize the mutated tree, leave rels untouched.
            if let (Some(dom), false) = (&slide.dom, has_new_media) {
                pkg.set_part(part, dom.to_bytes());
                continue;
            }

            // Media path: re-author the slide and rebuild its media rels, keeping
            // the original layout link.
            let layout_target = pkg
                .get_part_rels(part)
                .and_then(|r| r.get_by_type(rel_types::SLIDE_LAYOUT))
                .map(|r| r.target.clone());
            let mut rels = zavora_slide_opc::Relationships::new();
            if let Some(t) = &layout_target {
                rels.add_with_id("rId1", rel_types::SLIDE_LAYOUT, t);
            }
            let stem = part.rsplit('/').next().unwrap_or("slide").trim_end_matches(".xml");
            if let Some(crate::slide::Fill::Picture { data, ext }) = &slide.background {
                rels.add_with_id(crate::slide::BG_EMBED_RID, rel_types::IMAGE, &format!("../media/bg_{stem}.{ext}"));
                let ct_ext = if ext == "jpg" { "jpeg" } else { ext.as_str() };
                pkg.content_types.add_default(ct_ext, &format!("image/{ct_ext}"));
                pkg.set_part(&format!("/ppt/media/bg_{stem}.{ext}"), data.clone());
            }
            for img in &slide.images {
                let name = format!("img_{stem}_{}.{}", img.id, img.ext);
                rels.add_with_id(&img.embed_rid, rel_types::IMAGE, &format!("../media/{name}"));
                let ct_ext = if img.ext == "jpg" { "jpeg" } else { img.ext.as_str() };
                pkg.content_types.add_default(ct_ext, &format!("image/{ct_ext}"));
                pkg.set_part(&format!("/ppt/media/{name}"), img.data.clone());
            }
            pkg.part_rels.insert(part.clone(), rels);
            pkg.set_part(part, slide.to_xml());
        }

        if self.reordered {
            self.apply_reorder(&mut pkg)?;
        }
        Ok(Some(pkg))
    }

    /// Rewrite the source `presentation.xml`'s `<p:sldIdLst>` to the current
    /// slides (in order), materialize any duplicated slides (new part = clone of
    /// the original, rels cloned), prune deleted slide parts/rels, and keep
    /// everything else byte-identical.
    fn apply_reorder(&self, pkg: &mut OpcPackage) -> Result<()> {
        use zavora_slide_oxml::{Document, Node};

        // Highest presentation rel id in use → base for allocating new slide rels.
        let mut next_rid = pkg
            .get_part_rels("/ppt/presentation.xml")
            .map(|r| {
                r.items
                    .iter()
                    .filter_map(|x| x.id.strip_prefix("rId").and_then(|s| s.parse::<u32>().ok()))
                    .max()
                    .unwrap_or(1)
            })
            .unwrap_or(1)
            + 1;

        // Effective presentation rel id per slide (new/cloned slides get a fresh one).
        let mut eff_rids: Vec<String> = Vec::with_capacity(self.slides.len());
        for s in &self.slides {
            if let Some(orig_part) = &s.clone_rels_from {
                let new_part = s.source_part.as_ref().expect("duplicated slide has a part path");
                let rid = format!("rId{next_rid}");
                next_rid += 1;
                // New slide part bytes from the cloned DOM.
                if let Some(dom) = &s.dom {
                    pkg.set_part(new_part, dom.to_bytes());
                }
                pkg.content_types.add_override(new_part, template::CT_SLIDE);
                // Clone the original's part rels to the new part.
                if let Some(orig_rels) = pkg.get_part_rels(orig_part).cloned() {
                    pkg.part_rels.insert(new_part.clone(), orig_rels);
                }
                // Add presentation → new slide rel (target relative to ppt/).
                let target = new_part.strip_prefix("/ppt/").unwrap_or(new_part);
                pkg.get_or_create_part_rels("/ppt/presentation.xml")
                    .add_with_id(&rid, rel_types::SLIDE, target);
                eff_rids.push(rid);
            } else if let Some(lt) = &s.new_blank_layout_type {
                let new_part = s.source_part.as_ref().expect("new slide has a part path");
                let rid = format!("rId{next_rid}");
                next_rid += 1;
                // Bind to an existing layout of the requested type (fallback: any).
                let layout = find_layout_by_type(pkg, lt)
                    .or_else(|| find_layout_by_type(pkg, ""))
                    .ok_or_else(|| SlideError::Unsupported("deck has no slide layout to bind".into()))?;
                let bytes = s.dom.as_ref().map(|d| d.to_bytes()).unwrap_or_else(blank_slide_xml);
                pkg.set_part(new_part, bytes);
                pkg.content_types.add_override(new_part, template::CT_SLIDE);
                let layout_target = format!("../slideLayouts/{}", layout.rsplit('/').next().unwrap());
                let mut srels = zavora_slide_opc::Relationships::new();
                srels.add_with_id("rId1", rel_types::SLIDE_LAYOUT, &layout_target);
                pkg.part_rels.insert(new_part.clone(), srels);
                let target = new_part.strip_prefix("/ppt/").unwrap_or(new_part);
                pkg.get_or_create_part_rels("/ppt/presentation.xml")
                    .add_with_id(&rid, rel_types::SLIDE, target);
                eff_rids.push(rid);
            } else {
                eff_rids.push(s.sld_id.as_ref().expect("reorder requires sld_id").1.clone());
            }
        }

        let pres_xml = pkg
            .get_part("/ppt/presentation.xml")
            .ok_or_else(|| SlideError::NotFound("presentation.xml".into()))?;
        let mut doc = Document::parse(pres_xml).map_err(|e| SlideError::Unsupported(e.to_string()))?;

        // Build the new <p:sldId> children from current slides, in order.
        let mut new_ids: Vec<Node> = Vec::new();
        let mut kept_rids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (s, rid) in self.slides.iter().zip(&eff_rids) {
            let id = s.sld_id.as_ref().expect("reorder requires sld_id").0;
            kept_rids.insert(rid.clone());
            let xml = format!("<p:sldId id=\"{id}\" r:id=\"{rid}\"/>");
            let frag = Document::parse(xml.as_bytes()).map_err(|e| SlideError::Unsupported(e.to_string()))?;
            new_ids.extend(frag.nodes.into_iter().filter(|n| matches!(n, Node::Element(_))));
        }

        // Replace the sldIdLst's children in place (preserving the list element).
        if let Some(root) = doc.root_mut()
            && let Some(lst) = find_child_named_mut(root, b"sldIdLst")
        {
            lst.children = new_ids;
        }
        pkg.set_part("/ppt/presentation.xml", doc.to_bytes());

        // Drop presentation→slide rels not referenced by the new sldIdLst.
        if let Some(rels) = pkg.part_rels.get_mut("/ppt/presentation.xml") {
            let before = rels.items.len();
            rels.items.retain(|r| r.rel_type != rel_types::SLIDE || kept_rids.contains(&r.id));
            if rels.items.len() != before {
                rels.touch();
            }
        }

        // Prune slide parts (and rels/content-type) no longer referenced.
        let kept: std::collections::HashSet<&str> =
            self.slides.iter().filter_map(|s| s.source_part.as_deref()).collect();
        let all_slide_parts: Vec<String> = pkg
            .part_names()
            .filter(|n| n.starts_with("/ppt/slides/slide") && n.ends_with(".xml"))
            .map(|s| s.to_string())
            .collect();
        for part in all_slide_parts {
            if !kept.contains(part.as_str()) {
                pkg.remove_part(&part);
                pkg.part_rels.remove(&part);
                pkg.content_types.remove_override(&part);
            }
        }
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
                if let Some(crate::slide::Fill::Picture { ext, .. }) = &slide.background {
                    rels.add_with_id(
                        crate::slide::BG_EMBED_RID,
                        rel_types::IMAGE,
                        &format!("../media/bg{}.{}", idx + 1, ext),
                    );
                }
            }
            // Background picture media part (default content type for png/jpeg).
            if let Some(crate::slide::Fill::Picture { data, ext }) = &slide.background {
                let ct_ext = if ext == "jpg" { "jpeg" } else { ext.as_str() };
                pkg.content_types.add_default(ct_ext, &format!("image/{ct_ext}"));
                pkg.set_part(&format!("/ppt/media/bg{}.{}", idx + 1, ext), data.clone());
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

/// First direct child element of `el` with the given local name (mutable).
fn find_child_named_mut<'a>(
    el: &'a mut zavora_slide_oxml::Element,
    local: &[u8],
) -> Option<&'a mut zavora_slide_oxml::Element> {
    el.children.iter_mut().find_map(|n| match n {
        zavora_slide_oxml::Node::Element(e) if e.local_name() == local => Some(e),
        _ => None,
    })
}

/// Find a slideLayout part whose `sldLayout@type` equals `ty` (or any layout
/// when `ty` is empty). Returns the part path.
fn find_layout_by_type(pkg: &OpcPackage, ty: &str) -> Option<String> {
    let mut parts: Vec<&str> = pkg
        .part_names()
        .filter(|n| n.starts_with("/ppt/slideLayouts/slideLayout") && n.ends_with(".xml"))
        .collect();
    parts.sort();
    for part in parts {
        let bytes = pkg.get_part(part)?;
        let head = &bytes[..bytes.len().min(400)];
        let s = String::from_utf8_lossy(head);
        if ty.is_empty() {
            return Some(part.to_string());
        }
        if let Some(i) = s.find("<p:sldLayout")
            && s[i..].split('>').next().is_some_and(|tag| tag.contains(&format!("type=\"{ty}\"")))
        {
            return Some(part.to_string());
        }
    }
    None
}

/// A minimal valid blank slide part (empty shape tree).
fn blank_slide_xml() -> Vec<u8> {
    concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
        "<p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ",
        "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",
        "xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">",
        "<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/>",
        "</p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld>",
        "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
    )
    .as_bytes()
    .to_vec()
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
