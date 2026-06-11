# Requirements Document

## Introduction

This spec defines the work to bring the **`zavora-slide` engine** to **feature parity
with python-pptx v1.0.2** as a PresentationML content-authoring library, then beyond —
into capabilities python-pptx does not have at all: high-fidelity rendering,
deterministic visual QA, a design system, and rich extraction.

It is the engine half of a two-spec effort. A companion spec in the `slides-mcp` repo
(`server-parity`) exposes these engine capabilities as MCP tools; this document is
strictly about the engine crates and makes no reference to the server beyond a stable
public API.

Parity targets are derived from a **source-verified audit** (python-pptx v1.0.2 cloned
and inspected, captured in `docs/comparison-python-pptx.md`). The render-fidelity,
visual-QA, design-system, and extraction targets are derived from a review of a
production "PPTX skill" (a procedural agent playbook) whose real strengths over a raw
engine are **rendering fidelity (it leans on LibreOffice), a render→inspect→fix QA
loop, and encoded design taste**. This spec turns those into structured, deterministic
engine capabilities rather than prose recipes.

The engine already **leads** python-pptx in three areas that remain non-negotiable
invariants here: **rendering (PNG/SVG) + PDF export**, **slide delete/move/duplicate**,
and **byte-faithful round-trip**. No requirement may regress them.

### Scope map

- **Parts A–F** — authoring parity with python-pptx (text, charts, tables, shapes,
  images, hyperlinks/metadata/notes).
- **Part H** — render fidelity (geometry inheritance, real text shaping, theme
  resolution, shape/fill/image fidelity, LibreOffice similarity check).
- **Part I** — deterministic visual QA (structured layout report, contrast checks,
  render-diff).
- **Part J** — design system (curated palettes/font pairings, layout patterns, design
  lint).
- **Part K** — rich extraction (structured + Markdown, ≥ markitdown).
- **Part G** — verification & scorecard across all of the above.

### Parity gaps this spec closes (engine-side, from the audit)

1. Granular paragraph/run text model (add/insert/delete/reorder paragraphs & runs;
   mixed formatting; spacing/line-spacing/alignment; auto-fit).
2. Run-format breadth: theme colors, strikethrough, sub/superscript, language,
   underline styles.
3. Charts (author bar/line/pie/area/scatter + embedded data workbook; edit data).
4. Tables: merge/split cells, add/remove rows & columns, sizing — on opened decks.
5. Shape manipulation: move/resize/rotate/delete/reorder existing shapes; full
   fill/line styling; ~239 autoshape presets; connectors; group traversal; freeform.
6. Images: crop, rotation, surgical insert on opened decks, media de-duplication.
7. Hyperlinks (run-level + shape click actions / jump-to-slide).
8. Core/document properties read/write.
9. Notes editing without rebuild; footers/slide numbers/date placeholders.

### Non-negotiable quality bar (applies to EVERY requirement)

A capability is **done** only when:

1. **Byte-faithful**: untouched content is byte-for-byte preserved (lossless-DOM
   invariant), asserted per-part.
2. **Opens clean**: output opens without a repair prompt in PowerPoint, LibreOffice
   Impress, and Google Slides.
3. **Real-deck tested**: verified against genuine PowerPoint-authored decks.
4. **Surgical**: an edit changes only the parts it must.
5. **Green**: `cargo build`/`test`/`clippy` pass on the engine workspace.

## Glossary

- **Engine**: the `zavora-slide` crate family.
- **DOM**: the lossless editable XML tree (`zavora-slide-oxml::xml`) where every node
  preserves its exact source bytes until edited.
- **SlideDom**: the semantic editing model over the DOM (`zavora-slide-oxml::slide_dom`).
- **Surgical edit**: a mutation regenerating only the affected node(s); all other bytes
  (including relationships and `[Content_Types].xml`) are preserved.
- **Build model**: the from-scratch authoring path (`drawing`/`presentation` typed
  models) used for new decks.
- **Embedded workbook**: the `.xlsx` part backing a chart's data (`ppt/embeddings/`).
- **Parity**: an authoring capability present in python-pptx v1.0.2, matched at
  equal-or-better fidelity.

## Requirements

---

### PART A — Granular Text Model

### Requirement 1: Paragraph-level editing

**User Story:** As a caller editing an opened deck, I want to add, insert, delete, and
reorder individual paragraphs in any text body.

#### Acceptance Criteria

1. THE SlideDom SHALL expose, for any text body (placeholder, text box, table cell,
   shape), its paragraphs as addressable editable references.
2. WHEN a paragraph is inserted/deleted/reordered, THE Engine SHALL mutate only the
   affected `<a:p>` nodes; sibling paragraphs and the shape's other children SHALL
   remain byte-identical.
3. THE Engine SHALL set paragraph properties — alignment, indent level, space-before,
   space-after, line-spacing, bullet on/off/character/auto-number — on an existing
   paragraph in place, preserving unspecified properties.
4. WHERE a target text body does not exist, THE Engine SHALL return a typed error.

### Requirement 2: Run-level editing and mixed formatting

**User Story:** As a caller, I want multiple differently-formatted runs in one
paragraph, and to edit a single run without disturbing the others.

#### Acceptance Criteria

1. THE Engine SHALL add, insert, delete, and edit individual `<a:r>` runs and `<a:br>`
   line breaks within a paragraph.
2. WHEN one run is edited, THE Engine SHALL preserve every other run and its `a:rPr`
   byte-for-byte.
3. THE Engine SHALL support per-run: bold, italic, underline (all underline styles),
   size, font family, RGB color, theme-color reference, strikethrough,
   subscript/superscript, and language tag.
4. THE Engine SHALL emit a theme-color reference as `<a:schemeClr>` (not a baked RGB)
   so it tracks theme changes.

### Requirement 3: Text auto-fit

**User Story:** As a caller, I want text frames that auto-size to fit.

#### Acceptance Criteria

1. THE Engine SHALL support `none`, `shrink-to-fit` (`normAutofit`), and `resize-shape`
   (`spAutoFit`) on a text body, emitting the correct `bodyPr` child.
2. WHERE shrink-to-fit is requested, THE render layer SHALL apply a font scale
   consistent with the emitted `fontScale`.

---

### PART B — Charts

### Requirement 4: Chart authoring

**User Story:** As a caller, I want to add charts with data.

#### Acceptance Criteria

1. THE Engine SHALL add a chart as a `p:graphicFrame` with a chart part
   (`ppt/charts/chartN.xml`) and an embedded `.xlsx` workbook, wired with correct
   relationships and content types.
2. THE Engine SHALL support at minimum: clustered/stacked bar & column, line,
   pie/doughnut, area, and XY-scatter.
3. THE Engine SHALL set categories and one or more named numeric series.
4. THE Engine SHALL set chart title, legend position, and per-series data labels.
5. THE embedded workbook SHALL be a valid `.xlsx` (reuse the `zavora-xlsx` engine).
6. WHEN a deck containing charts is opened and re-saved unmodified, THE chart parts
   SHALL be byte-preserved.

### Requirement 5: Chart editing

**User Story:** As a caller, I want to update a chart's data on an opened deck.

#### Acceptance Criteria

1. THE Engine SHALL replace a chart's categories/series values, updating both the
   chart XML and its embedded workbook consistently.
2. Unmodeled chart features (styling, effects) SHALL be preserved on edit.

---

### PART C — Tables (full)

### Requirement 6: Table structure editing

**User Story:** As a caller, I want to add/remove rows and columns and merge cells.

#### Acceptance Criteria

1. THE Engine SHALL add and remove rows and columns of an existing table, preserving
   other cells' content and formatting.
2. THE Engine SHALL merge a rectangular cell range (`gridSpan`/`rowSpan` + `hMerge`/
   `vMerge`) and split a merged cell.
3. THE Engine SHALL set column widths and row heights.
4. THE Engine SHALL set per-cell text, alignment, fill, and margins.
5. ALL table edits on an opened deck SHALL be surgical (only the table's
   `graphicFrame` changes).

---

### PART D — Shapes (full)

### Requirement 7: Shape geometry and lifecycle

**User Story:** As a caller, I want to move, resize, rotate, delete, and reorder any
shape on an opened slide.

#### Acceptance Criteria

1. THE Engine SHALL set position (`off`), size (`ext`), and rotation (`rot`) of an
   existing shape in place.
2. THE Engine SHALL delete a shape and reorder shapes within the `spTree`.
3. THE Engine SHALL expose a shape inventory with id, name, type, geometry, and text.
4. ALL operations SHALL preserve sibling shapes byte-for-byte.

### Requirement 8: Shape styling

**User Story:** As a caller, I want full fill and outline control.

#### Acceptance Criteria

1. THE Engine SHALL set shape fill: solid, gradient (stops + angle), pattern, picture,
   and no-fill.
2. THE Engine SHALL set outline: color, width, dash style, and no-line.
3. THE Engine SHALL support theme-color references for fills and lines.

### Requirement 9: Shape vocabulary breadth

**User Story:** As a caller, I want the autoshapes, connectors, and freeform shapes
PowerPoint offers.

#### Acceptance Criteria

1. THE Engine SHALL support the full `MSO_SHAPE` autoshape preset set (≈239 presets)
   by name, emitting the correct `prstGeom`.
2. THE Engine SHALL add connectors (straight/elbow/curved) between anchor points.
3. THE Engine SHALL build freeform shapes from a path of move/line/curve segments
   (`custGeom`).
4. THE Engine SHALL traverse group shapes and add shapes to a group.

---

### PART E — Images

### Requirement 10: Image operations

**User Story:** As a caller, I want to crop, rotate, and insert images on opened decks.

#### Acceptance Criteria

1. THE Engine SHALL set image crop (l/r/t/b fractions) via `a:srcRect`.
2. THE Engine SHALL set image rotation.
3. THE Engine SHALL insert an image into an opened slide surgically (DOM `p:pic` +
   media part + rel), de-duplicating identical media by content hash.
4. THE Engine SHALL accept PNG, JPEG, GIF, and preserve other embedded formats.

---

### PART F — Hyperlinks, Metadata, Notes

### Requirement 11: Hyperlinks and actions

#### Acceptance Criteria

1. THE Engine SHALL set a run-level hyperlink to an external URL (`a:hlinkClick` +
   external relationship).
2. THE Engine SHALL set a shape click action: external URL, or jump to another slide.
3. Hyperlink edits SHALL be surgical with the external relationship correctly scoped.

### Requirement 12: Document properties

#### Acceptance Criteria

1. THE Engine SHALL read and write core properties: title, author, subject, keywords,
   comments, category, created/modified timestamps, last-modified-by.
2. WHEN core properties are unset by the caller, THE Engine SHALL preserve existing
   `docProps/core.xml` values on an opened deck.

### Requirement 13: Notes and slide furniture

#### Acceptance Criteria

1. THE Engine SHALL create/update a slide's notes text via the DOM, preserving the
   rest of the deck byte-for-byte (no full rebuild).
2. THE Engine SHALL set slide-number, date, and footer placeholders' visibility and
   text where the layout provides them.

---

### PART H — Render Fidelity

> Rationale: the engine already leads python-pptx by rendering at all, but rendering
> is approximate (estimated wrapping, hardcoded placeholder boxes, no theme
> resolution). Fidelity is the prerequisite for trustworthy visual QA (Part I) and a
> credible UI. This part closes the gap with a faithful, layout-aware render path.

### Requirement 14: Layout/master geometry inheritance

**User Story:** As a render consumer, I want shapes and placeholders positioned where
PowerPoint would put them, so previews match the saved deck.

#### Acceptance Criteria

1. THE render layer SHALL resolve a placeholder's geometry by inheritance:
   slide → slide-layout → slide-master, using the first explicit `a:xfrm` found.
2. THE render layer SHALL position non-placeholder shapes by their explicit `a:xfrm`,
   and apply shape rotation (`@rot`).
3. WHERE no geometry resolves, THE render layer SHALL fall back to a documented
   default box (today's behavior) rather than failing.

### Requirement 15: Faithful text shaping

**User Story:** As a render consumer, I want accurate text wrapping, sizing, and
placement.

#### Acceptance Criteria

1. THE render layer SHALL shape text with real font metrics (e.g. rustybuzz/fontdb),
   computing line breaks from measured glyph advances, not character estimates.
2. THE render layer SHALL apply paragraph alignment, indent level, line-spacing,
   space-before/after, and bullet markers/indents.
3. THE render layer SHALL apply vertical anchoring (top/middle/bottom) from `bodyPr`.
4. THE render layer SHALL honor run formatting: bold, italic, underline, size, font
   family (with bundled fallback), and color.
5. WHERE `normAutofit fontScale` is present, THE render layer SHALL scale text
   accordingly.

### Requirement 16: Theme and color resolution

**User Story:** As a render consumer, I want theme colors and fonts resolved exactly.

#### Acceptance Criteria

1. THE render layer SHALL resolve `<a:schemeClr>` references (`accent1..6`, `dk1/2`,
   `lt1/2`, `hlink`) to RGB via the deck theme + the slide's color map.
2. THE render layer SHALL resolve theme font references (`+mn-lt`/`+mj-lt`) to the
   theme's actual typefaces.
3. THE render layer SHALL apply tint/shade/lumMod/lumOff modifiers on colors.

### Requirement 17: Shape, fill, and image rendering fidelity

**User Story:** As a render consumer, I want shapes, fills, and images to look right.

#### Acceptance Criteria

1. THE render layer SHALL render preset autoshape geometry (not rectangle
   approximation) for the common presets; uncommon presets MAY fall back to bounding
   box.
2. THE render layer SHALL render solid, gradient, and picture fills, and outline
   color/width/dash.
3. THE render layer SHALL render images with crop (`srcRect`) and rotation, and basic
   table gridlines/fills.
4. THE render layer SHALL render slide and shape backgrounds (solid + picture).

### Requirement 18: Render parity check

**User Story:** As the maintainer, I want objective evidence that our render matches a
reference.

#### Acceptance Criteria

1. THE Engine SHALL provide a test mode comparing our PNG of a slide against a
   LibreOffice-rendered PNG of the same slide (env-guarded), reporting a similarity
   score.
2. A curated set of slides SHALL meet a documented minimum similarity threshold;
   regressions below threshold SHALL fail the gate.

---

### PART I — Visual QA (deterministic inspection)

> Rationale: the highest-leverage idea from the PPTX skill is a render→inspect→fix
> loop. We can make it **deterministic** (we own the renderer): instead of asking a
> model to eyeball an image, the engine computes a structured layout report. This is a
> leapfrog over prose-based QA. Gated on Part H fidelity.

### Requirement 19: Structured layout report

**User Story:** As an author/agent, I want a machine-readable report of layout problems
on a slide, so I can fix them before delivering.

#### Acceptance Criteria

1. THE Engine SHALL produce, for a slide, a report listing every visible element with
   its resolved bounding box (in EMU and as fraction of slide), kind, and z-order.
2. THE report SHALL flag, with element references and severities: elements overflowing
   the slide bounds; elements overlapping (text-over-text, text-over-shape) beyond a
   threshold; text overflowing its frame; gaps below a minimum margin from slide
   edges; and elements with effectively empty/zero-area boxes.
3. THE report SHALL be deterministic for a given deck (no model in the loop).

### Requirement 20: Contrast and readability checks

**User Story:** As an author/agent, I want low-contrast text/elements flagged.

#### Acceptance Criteria

1. THE Engine SHALL compute the WCAG contrast ratio between resolved text color and
   its resolved background (slide/shape fill) for each text run.
2. THE report SHALL flag runs below a configurable ratio (default 4.5:1) and font
   sizes below a configurable minimum.

### Requirement 21: Render-difference QA

**User Story:** As an author/agent, I want to know if an edit changed a slide's
appearance unexpectedly.

#### Acceptance Criteria

1. THE Engine SHALL render before/after PNGs of an edited slide and report a
   structural-difference summary (changed regions) so unintended visual changes are
   surfaced.

---

### PART J — Design System

> Rationale: the skill encodes taste (palettes, font pairings, layout patterns,
> anti-patterns). Our engine is design-neutral and will faithfully produce ugly decks.
> A design-system layer makes "not boring" the default, as structured data — not prose.

### Requirement 22: Curated palettes and font pairings

**User Story:** As an author, I want professionally designed color palettes and font
pairings I can apply by name.

#### Acceptance Criteria

1. THE Engine SHALL ship a catalog of named palettes (primary/secondary/accent with
   defined dominance weighting) and named heading/body font pairings.
2. `apply_theme` SHALL accept a palette name and font-pairing name, mapping them onto
   the deck theme's scheme colors and font scheme.
3. THE catalog SHALL be data-driven and enumerable (id, swatches, intended tone).

### Requirement 23: Layout patterns

**User Story:** As an author, I want ready-made slide layout patterns (two-column,
icon-rows, stat callout, quote, section divider, image-with-caption) so slides have
structure and a visual element by default.

#### Acceptance Criteria

1. THE Engine SHALL provide parameterized layout patterns that emit correctly
   positioned shapes/placeholders for the chosen pattern, using the active palette/
   fonts and consistent spacing/margins.
2. Patterns SHALL avoid the documented anti-patterns (no centered body text, no
   accent underline beneath titles, enforced size hierarchy, minimum margins).

### Requirement 24: Design lint

**User Story:** As an author/agent, I want the engine to flag design anti-patterns.

#### Acceptance Criteria

1. THE Engine SHALL flag, per slide: text-only slides (no visual element), centered
   body text, an accent line directly under a title, more than N fonts, palette colors
   used at equal weight, and titles below the size-hierarchy threshold.
2. THE design lint output SHALL be structured and reference the offending elements.

---

### PART K — Extraction & Content I/O

> Rationale: the skill's most common trigger is "read a .pptx for use elsewhere"
> (markitdown). Our extraction must be at least as rich.

### Requirement 25: Rich extraction

**User Story:** As a consumer, I want complete, structured extraction of a deck's
content.

#### Acceptance Criteria

1. THE Engine SHALL extract, per slide: title, body paragraphs with level, table
   contents (as a grid), speaker notes, alt-text, and shape text — in reading order.
2. THE Engine SHALL produce a Markdown rendering equal-or-richer than `markitdown`
   (tables as Markdown tables, notes called out, slide boundaries marked).
3. THE Engine SHALL optionally emit a structured (JSON) deck outline for programmatic
   consumers.

---

### PART G — Verification

### Requirement 26: Parity & quality verification harness

#### Acceptance Criteria

1. THE Engine SHALL maintain a corpus of genuine PowerPoint-authored decks exercising
   each parity feature; round-trip tests SHALL assert byte-fidelity per part.
2. FOR each authoring feature, a test SHALL assert the emitted XML matches the
   ECMA-376 shape PowerPoint expects AND that a surgical edit changes only the
   intended part.
3. THE verification SHALL include an automated "opens without error" check via
   LibreOffice headless (env-guarded), in addition to python-pptx structural checks.
4. THE three leading capabilities (render/PDF, slide delete/move/duplicate, byte-
   faithful round-trip) SHALL have regression tests that fail if fidelity drops.
5. RENDER fidelity (Part H) SHALL be guarded by the similarity check (Req 18); VISUAL
   QA (Part I) and DESIGN LINT (Part J) SHALL have unit tests on fixtures with known
   defects; EXTRACTION (Part K) SHALL be tested against the corpus.
6. `cargo build`/`test`/`clippy` SHALL pass on the engine workspace for every task.

### Requirement 27: Capability scorecard

#### Acceptance Criteria

1. `docs/comparison-python-pptx.md` (parity rows) and a new
   `docs/capabilities.md` (render fidelity, visual QA, design system, extraction)
   SHALL be updated as each requirement lands, with affected rows moved to ✅ and dated.
2. A capability SHALL be marked ✅ only when its tests meet the full quality bar above.
