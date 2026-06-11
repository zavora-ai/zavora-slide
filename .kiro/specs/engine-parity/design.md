# Design Document — engine-parity (`zavora-slide`)

## Overview

This design extends the existing engine architecture; it introduces no new layers. The
lossless DOM is the substrate for every parity feature.

```
zavora-slide-opc     OPC package; byte-preserving rels + content-types
zavora-slide-oxml    xml::{Document,Element,Node}  ← lossless DOM
                     slide_dom::SlideDom           ← semantic editing surface
                     drawing/presentation          ← build-model authoring
zavora-slide         Presentation/Slide high-level API; overlay save
zavora-slide-render  Scene → SVG/PNG (resvg)
zavora-slide-pdf     Scene → PDF
zavora-slide-cli / -wasm
```

**Design principle (unchanged):** all editing of opened decks goes through the lossless
DOM — locate node(s), mutate in place, mark dirty; overlay save serializes the DOM so
every untouched byte (incl. rels/content-types) is preserved. New-deck authoring keeps
using the build model. No re-authoring from extracted text.

Most parity work lands in **`slide_dom.rs`** (new editing methods) and thin **`Slide`/
`Presentation`** wrappers; charts add a new high-level module + part graph.

## Cross-cutting building blocks (implement once, reuse everywhere)

1. **DOM mutation toolkit** on `xml::Element`: `insert_child_at`,
   `remove_children_where`, `replace_child`, and ordered-insert honoring a `child_order`
   table for `a:rPr`/`a:pPr`/`p:spPr`/`p:txBody` so authored children land in
   schema-valid positions. (Centralizes today's ad-hoc per-method logic.)
2. **Fragment authoring**: build a small XML string, `Document::parse` it, graft the
   node — guarantees well-formed output via the byte-faithful serializer. (Proven in
   `add_text_box`/color/font.)
3. **Theme color**: `SchemeColor` enum → `<a:schemeClr val=.../>`; a render-side
   resolver (scheme→RGB) reading the deck theme.
4. **Id/part allocation**: reuse `next_shape_id` (max `cNvPr@id`+1); a package-level
   part/rId allocator mirroring `apply_reorder`'s slide-rel allocation for new
   chart/media parts.
5. **Surgical-edit test util**: promote `package_entries` (parts + all rels +
   content-types) to a shared util with `assert_only_changed(&[parts])`.

## Part A — Granular text model (Req 1–3)

`slide_dom.rs`: `ParagraphRef`/`RunRef` cursors borrowing into the DOM.
- `paragraphs_mut()` / `runs_mut()` expose `&mut Element` per `<a:p>` / `<a:r>`.
- Paragraph props write into `<a:pPr>` (create if absent, correct order): `algn`,
  `lvl`, `<a:spcBef>/<a:spcAft>`, `<a:lnSpc>`, bullet (`buChar`/`buNone`/`buAutoNum`).
- Run props extend `apply_run_format`: `<a:strike>`, `baseline` (sub/super), `lang`,
  underline-style enum, theme color via `<a:schemeClr>`.
- Auto-fit: `<a:normAutofit fontScale=.../>` or `<a:spAutoFit/>` on `<a:bodyPr>`;
  render reads `fontScale`.

## Part B — Charts (Req 4–5)

New `zavora-slide::chart` module + overlay/build-package wiring; data workbook via a
new dependency on local **`zavora-xlsx`**.
- `ChartSpec{kind,categories,series,title,legend,data_labels}` → `ppt/charts/chartN.xml`
  (`c:chartSpace`); types map to `c:barChart`/`lineChart`/`pieChart`/`areaChart`/
  `scatterChart` with `barDir`/`grouping`.
- Embedded workbook built with `zavora-xlsx` → `ppt/embeddings/...xlsx`; chart rel
  (`package`) → workbook; `c:externalData` references it.
- Slide `p:graphicFrame` (`a:graphicData` chart URI) + rel → chart part; content types:
  chart override + `xlsx` default.
- Edit: locate chart part via slide rel, rewrite `c:ser` + workbook cells; preserve
  unmodeled `c:*` via the DOM.

**Risk:** highest — new part graph + xlsx coupling. Land bar/column first.

## Part C — Tables (Req 6)

`slide_dom.rs` methods on the `a:tbl` inside a `graphicFrame`:
- Parse `a:tblGrid` (`gridCol@w`) and `a:tr` (`@h`, `a:tc`) as refs.
- Add/remove row = insert/remove `<a:tr>` (+ matching `tc` count); add/remove column =
  insert/remove a `gridCol` + one `tc` per row.
- Merge = `gridSpan`/`rowSpan` on origin + `hMerge`/`vMerge` on covered; split clears.
- Cell text reuses Part A on the cell `a:txBody`.

## Part D — Shapes (Req 7–9)

`slide_dom.rs` shape methods + a `ShapeStyle` builder:
- Geometry: edit `<a:xfrm>` `off`/`ext`/`@rot` on a shape by id/index.
- Delete/reorder: remove/reposition `<p:sp>`/`<p:pic>`/`<p:graphicFrame>` in `spTree`.
- Fill/line: upsert `<a:solidFill|gradFill|pattFill|blipFill|noFill>` and `<a:ln>`
  (with `<a:prstDash>`) in `spPr`, correct order, theme-color aware.
- Presets: generated `ShapePreset` table (~239 `MSO_SHAPE` → `prstGeom@prst`).
- Connectors: `<p:cxnSp>` + `stCxn`/`endCxn`. Freeform: `<a:custGeom>` path builder.
  Groups: traverse/add into `<p:grpSp>`.

## Part E — Images (Req 10)

- Crop: `<a:srcRect l/r/t/b>` on the `pic` `blipFill`. Rotation: `xfrm@rot`.
- Surgical insert on opened deck: author `<p:pic>` into `spTree`, add media part keyed
  by **content hash** (dedupe), add slide rel — mirrors `add_text_box` DOM path + media.

## Part F — Hyperlinks, metadata, notes (Req 11–13)

- Run hyperlink: `<a:hlinkClick r:id=.../>` in `a:rPr` + external relationship
  (`add_external` exists). Shape action / jump: `<a:hlinkClick>` in `cNvPr` (internal
  slide rel or `ppaction://`).
- Core properties: typed `docProps/core.xml` (`cp:coreProperties`) with byte-preserving
  raw fallback (mirror `ContentTypes`/`Relationships` raw-bytes pattern); read + write.
- Notes via DOM: parse the notes-slide part as a body, edit placeholder text in place;
  overlay writes only that notes part — replaces today's notes→rebuild fallback.

## Part H — Render fidelity (Req 16–20)

`zavora-slide-layout` + `zavora-slide-render`. Today the Scene IR uses hardcoded boxes
and estimated wrapping; this part makes the Scene faithful.

- **Geometry inheritance** (`-layout`): resolve placeholder boxes by walking
  slide → layout → master, matching on `ph@type`/`@idx`; cache the master/layout
  geometry per deck. Non-placeholder shapes use explicit `xfrm`; apply `@rot`.
- **Text shaping** (`-render`): replace the char-width estimate with rustybuzz +
  fontdb (reuse the `zavora-docx-layout` stack) for measured wrapping; emit per-line
  runs to SVG with alignment, indent, line-spacing, space-before/after, bullets,
  vertical anchor.
- **Theme resolution** (`-layout`/`-render`): a resolver reading `theme1.xml`
  `clrScheme`/`fontScheme` + the slide `clrMap`/`clrMapOvr`; `schemeClr` → RGB with
  tint/shade/lumMod/lumOff; `+mn-lt`/`+mj-lt` → faces. Shares the `SchemeColor`
  emitter from §cross-cutting.
- **Shape/fill/image fidelity** (`-render`): preset geometry paths for common presets
  (table mapping `prst` → path commands; uncommon → bbox); gradient/picture fills;
  outline dash; image `srcRect` crop + rotation; slide/shape backgrounds.
- **Similarity check** (test-only): render our PNG and a LibreOffice PNG of the same
  slide; compare via a perceptual/structural metric; threshold-gated, env-guarded.

**Risk:** high. Geometry inheritance and shaping are the load-bearing pieces; land them
first and measure with the similarity check before the rest.

## Part I — Visual QA (Req 21–23)

New module `zavora-slide` `qa.rs` operating on the **resolved Scene** from Part H (so
QA is only as trustworthy as fidelity — hence the Part H dependency).

- **Layout report**: from the Scene's positioned items, emit per-element bbox (EMU +
  fraction), kind, z-order; compute overlaps (rectangle intersection area over
  threshold, with text-vs-text / text-vs-shape classification), off-canvas, frame
  overflow (shaped text height/width vs frame), and margin violations. Deterministic.
- **Contrast**: for each text run, resolve fg color (run/theme) and the effective bg
  (nearest underlapping fill or slide background); compute WCAG ratio; flag < ratio
  and < min size.
- **Render-diff**: rasterize before/after (reuse render), tile-diff to report changed
  regions.

Output is a typed `QaReport { elements, findings: Vec<Finding{severity,kind,refs,msg}> }`.

## Part J — Design system (Req 24–26)

New module `zavora-slide` `design.rs`, data-driven.

- **Palettes / font pairings**: a static catalog (`Palette{id,primary,secondary,accent,
  weights,tone}`, `FontPairing{id,heading,body}`) seeded from the curated sets; serde
  so it can be enumerated. `apply_theme` gains palette/pairing-by-name → writes the
  theme `clrScheme`/`fontScheme`.
- **Layout patterns**: parameterized builders (two-column, icon-rows, stat-callout,
  quote, section-divider, image-caption) emitting positioned shapes via the build
  model + active palette/fonts, enforcing margins/size-hierarchy and avoiding the
  anti-patterns.
- **Design lint**: rules over the Scene + theme (text-only slide, centered body, accent
  line under title, font-count, equal-weight palette use, undersized title) → reuses
  the `Finding` type from Part I.

## Part K — Extraction (Req 27)

Extend `to_markdown` and add `to_outline` in `zavora-slide`.

- Walk each slide's DOM in reading order; collect title/body(+level)/tables(grid)/
  notes/alt-text/shape text. Markdown emits tables as Markdown tables, notes as
  blockquotes, slide separators. `to_outline` returns a serde `DeckOutline` (slides →
  elements) for programmatic use. Benchmark richness against `markitdown` on the corpus.

## Part G — Verification

- Promote `package_entries` + `assert_only_changed`.
- Grow the corpus per feature (charts/tables/groups/hyperlinks); commit a curated small
  set, keep large real decks out of git.
- `libreoffice --headless --convert-to pdf` smoke gate (env-guarded) asserting exit-0.
- Regression tests pinning the three leads (render non-empty + dark pixels; delete/
  move/duplicate part-set assertions; unedited round-trip byte-identical).
- Update `docs/comparison-python-pptx.md` per landed requirement.

## Sequencing rationale

Order by user value × reuse × ascending risk, interleaving the experience parts (H–K)
where they unblock the most value:

1. Cross-cutting toolkit + theme-color emitter (unblocks A, D, H).
2. Granular text (A) — highest everyday value, pure DOM.
3. **Render fidelity (H)** — geometry inheritance + text shaping + theme resolution.
   High value and the prerequisite for trustworthy QA; do it early, measure with the
   similarity check.
4. Shape geometry/style/lifecycle (D7–8) — pure DOM.
5. **Visual QA (I)** — built on the now-faithful Scene.
6. Tables (C); Images (E).
7. **Design system (J)** and **Extraction (K)** — reuse build model + Scene.
8. Hyperlinks + metadata + notes (F).
9. Shape vocabulary + connectors + freeform (D9) — data-heavy.
10. Charts (B) — highest risk, last.

Rationale for moving render fidelity (H) early despite its risk: the design system (J)
and visual QA (I) are only credible on a faithful renderer, and fidelity is the single
biggest differentiator over both python-pptx (no render) and the skill (borrows
LibreOffice). Authoring parts that are pure-DOM (A, C, E) can proceed in parallel
conceptually but are sequenced after to keep one-task-at-a-time discipline.

## Non-regression invariants (after every task)

- Unedited open→save byte-identical for every entry.
- A surgical edit changes only the intended part(s).
- Render (PNG/SVG) + PDF still produce valid non-empty output.
- Slide delete/move/duplicate remain faithful.
- python-pptx structural validation + (when available) LibreOffice headless load pass.
