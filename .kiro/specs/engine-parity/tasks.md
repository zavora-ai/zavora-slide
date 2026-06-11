# Implementation Plan — engine-parity (`zavora-slide`)

## Overview

Bring the engine to python-pptx v1.0.2 authoring parity (Parts A–F) and beyond into
capabilities python-pptx lacks: render fidelity (H), deterministic visual QA (I),
design system (J), and rich extraction (K). One task at a time; every task ends green
on `cargo build`/`test`/`clippy` for the engine workspace and holds the non-regression
invariants. Tasks reference requirements as `_Requirements: N.M_`.

Each task's definition of done includes the full quality bar (byte-faithful, surgical,
opens clean in PowerPoint/LibreOffice/Slides, real-deck tested) and a scorecard update.
Server tools exposing these capabilities are tracked in the `slides-mcp` repo's
`server-parity` spec.

Sequencing note: render fidelity (Phase 3) is pulled early because visual QA (Phase 5)
and the design system (Phase 8) are only trustworthy on a faithful renderer.

Requirement map: A=1–3, B=4–5, C=6, D=7–9, E=10, F=11–13, H=14–18, I=19–21, J=22–24,
K=25, Verification=26, Scorecard=27.

## Tasks

- [x] 1. Cross-cutting foundation
  - [x] 1.1 DOM mutation toolkit on `xml::Element` (`insert_child_at`,
    `remove_children_where`, `replace_child`, ordered-insert + `child_order` table) +
    unit tests — _Requirements: 1.2, 2.2_
  - [x] 1.2 `SchemeColor` emitter + render-side scheme→RGB resolver — _Requirements: 2.4, 8.3, 16.1_
  - [x] 1.3 Shared test util: promote `package_entries`, add `assert_only_changed`;
    env-guarded LibreOffice-headless load gate — _Requirements: 26.1, 26.2, 26.3_

- [x] 2. Granular text model (Part A)
  - [x] 2.1 Paragraph add/insert/delete/reorder + pPr props — _Requirements: 1.1, 1.2, 1.3, 1.4_
  - [x] 2.2 Run add/insert/delete/edit + `<a:br>`; sibling preservation — _Requirements: 2.1, 2.2_
  - [x] 2.3 Run-format breadth: strike, sub/superscript, lang, underline styles, theme color — _Requirements: 2.3, 2.4_
  - [x] 2.4 Auto-fit (`normAutofit`/`spAutoFit`) emission — _Requirements: 3.1_
  - [x] 2.5 Corpus tests + scorecard — _Requirements: 26.2, 27.1_

- [x] 3. Render fidelity (Part H) — pulled early; prerequisite for QA & design
  - [x] 3.1 Placeholder geometry inheritance slide→layout→master — _Requirements: 14.1, 14.2, 14.3_
  - [x] 3.2 Real text shaping (rustybuzz/fontdb): measured wrapping, alignment, indent,
    line/para spacing, bullets, vertical anchor, run formatting, autofit scale — _Requirements: 15.1, 15.2, 15.3, 15.4, 15.5_
  - [x] 3.3 Theme + color resolution (schemeClr→RGB, theme fonts, tint/shade/lumMod) — _Requirements: 16.1, 16.2, 16.3_
  - [x] 3.4 Shape/fill/image fidelity (preset geometry, gradient/picture fills, outline
    dash, image crop/rotation, backgrounds) — _Requirements: 17.1, 17.2, 17.3, 17.4_
  - [x] 3.5 LibreOffice similarity check (env-guarded) + threshold gate — _Requirements: 18.1, 18.2_

- [x] 4. Shape geometry, lifecycle, styling (Part D core)
  - [x] 4.1 Geometry: off/ext/rot in place — _Requirements: 7.1, 7.4_
  - [x] 4.2 Lifecycle: delete + reorder; richer inventory — _Requirements: 7.2, 7.3, 7.4_
  - [x] 4.3 Fill: solid/gradient/pattern/picture/none (theme-aware) — _Requirements: 8.1, 8.3_
  - [x] 4.4 Line: color/width/dash/none — _Requirements: 8.2, 8.3_
  - [x] 4.5 Corpus tests + scorecard — _Requirements: 26.2, 27.1_

- [x] 5. Visual QA (Part I) — on the now-faithful Scene
  - [x] 5.1 Structured layout report (bboxes, overlap, off-canvas, frame overflow,
    margins) — _Requirements: 19.1, 19.2, 19.3_
  - [x] 5.2 Contrast + min-size checks (WCAG) — _Requirements: 20.1, 20.2_
  - [x] 5.3 Render-diff QA (before/after changed regions) — _Requirements: 21.1_
  - [x] 5.4 Fixture tests with known defects + scorecard — _Requirements: 26.5, 27.1_

- [x] 6. Tables, full (Part C)
  - [x] 6.1 Add/remove rows & columns — _Requirements: 6.1_
  - [x] 6.2 Merge/split cells — _Requirements: 6.2_
  - [x] 6.3 Column widths / row heights; cell text/align/fill/margins — _Requirements: 6.3, 6.4, 6.5_
  - [x] 6.4 Corpus tests + scorecard — _Requirements: 26.2, 27.1_

- [x] 7. Images (Part E)
  - [x] 7.1 Crop (`srcRect`) + rotation on existing pictures — _Requirements: 10.1, 10.2_
  - [x] 7.2 Surgical insert on opened deck + content-hash dedupe; GIF — _Requirements: 10.3, 10.4_
  - [x] 7.3 Corpus tests + scorecard — _Requirements: 26.2, 27.1_

- [x] 8. Design system (Part J)
  - [x] 8.1 Curated palettes + font pairings catalog; `apply_theme` by name — _Requirements: 22.1, 22.2, 22.3_
  - [x] 8.2 Layout patterns (two-column, icon-rows, stat, quote, divider, image-caption) — _Requirements: 23.1, 23.2_
  - [x] 8.3 Design lint (anti-patterns) reusing the QA `Finding` type — _Requirements: 24.1, 24.2_
  - [x] 8.4 Fixture tests + scorecard — _Requirements: 26.5, 27.1_

- [x] 9. Rich extraction (Part K)
  - [x] 9.1 Structured `to_outline` (titles/body+level/tables/notes/alt-text, reading order) — _Requirements: 25.1, 25.3_
  - [x] 9.2 Markdown ≥ markitdown (tables, notes, slide boundaries); benchmark on corpus — _Requirements: 25.2_
  - [x] 9.3 Tests + scorecard — _Requirements: 26.5, 27.1_

- [x] 10. Hyperlinks, metadata, notes (Part F)
  - [x] 10.1 Run hyperlink (external URL) — _Requirements: 11.1, 11.3_
  - [x] 10.2 Shape click action / jump-to-slide — _Requirements: 11.2, 11.3_
  - [x] 10.3 Core/document properties read/write (byte-preserving) — _Requirements: 12.1, 12.2_
  - [x] 10.4 Notes editing via DOM (no rebuild); footer/slide-number/date — _Requirements: 13.1, 13.2_
  - [x] 10.5 Corpus tests + scorecard — _Requirements: 26.2, 27.1_

- [x] 11. Shape vocabulary breadth (Part D extended)
  - [x] 11.1 Full ~239 `MSO_SHAPE` preset table → `prstGeom` — _Requirements: 9.1_
  - [x] 11.2 Connectors — _Requirements: 9.2_
  - [x] 11.3 Freeform `custGeom` path builder — _Requirements: 9.3_
  - [x] 11.4 Group traversal + add-to-group — _Requirements: 9.4_
  - [x] 11.5 Corpus tests + scorecard — _Requirements: 26.2, 27.1_

- [x] 12. Charts (Part B) — highest quality, last
  - [x] 12.1 Wire local `zavora-xlsx` dep; minimal embedded-workbook builder — _Requirements: 4.5_
  - [x] 12.2 Bar/column chart part + graphicFrame + rels + content types; round-trip — _Requirements: 4.1, 4.2, 4.3, 4.6_
  - [x] 12.3 Line, pie/doughnut, area, scatter — _Requirements: 4.2_
  - [x] 12.4 Title, legend, data labels — _Requirements: 4.4_
  - [x] 12.5 Chart data editing (XML + workbook in sync) — _Requirements: 5.1, 5.2_
  - [x] 12.6 Render charts in the Scene (reuse Part H) — _Requirements: 17.1_
  - [x] 12.7 Corpus tests + scorecard — _Requirements: 26.2, 27.1_

- [x] 13. Close-out
  - [x] 13.1 Regression suite for the three lead capabilities — _Requirements: 26.4_
  - [x] 13.2 Corpus breadth + LibreOffice gate + render-similarity across features — _Requirements: 26.1, 26.3, 26.5_
  - [x] 13.3 Final scorecards (`comparison-python-pptx.md` + `capabilities.md`), honest deltas — _Requirements: 27.1, 27.2_
  - [x] 13.4 Full engine gate — _Requirements: 26.6_
