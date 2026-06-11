# zavora-slide — Capabilities

Tracks capabilities beyond python-pptx parity: render fidelity, visual QA, design
system, and extraction. Updated as each requirement lands.

**Legend:** ✅ done (tested, quality bar met) · 🟡 partial · ❌ not started

---

## Text Model (Part A)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Paragraph add/insert/delete/reorder | ✅ | 2025-01-15 | Surgical on opened decks; corpus-tested |
| Paragraph properties (alignment, indent, spacing, bullets) | ✅ | 2025-01-15 | All pPr props; schema-ordered insertion |
| Run add/insert/delete/edit | ✅ | 2025-01-15 | Sibling preservation verified byte-for-byte |
| Run formatting (bold, italic, underline styles, size, font, color, theme color, strikethrough, baseline, lang) | ✅ | 2025-01-15 | Full RunFormat breadth; theme color as `<a:schemeClr>` |
| Auto-fit (normAutofit, spAutoFit) | ✅ | 2025-01-15 | Shrink-to-fit with fontScale, resize-shape, none |

---

## Shapes (Part D)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Move/resize (left/top/width/height) | ✅ | 2025-01-15 | `set_shape_position` / `set_shape_size` — surgical on opened decks |
| Rotation | ✅ | 2025-01-15 | `set_shape_rotation` — surgical, corpus-tested |
| Edit/delete existing shapes on opened deck | ✅ | 2025-01-15 | `delete_shape` / `reorder_shape` — surgical, sibling-preserving |
| Shape fill (solid/gradient/pattern/picture/none) | ✅ | 2025-01-15 | All fill types; theme-color aware; schema-ordered |
| Shape line (color/width/dash) | ✅ | 2025-01-15 | Styled + no-line; dash presets; theme-color aware |
| Shape inventory (id, name, type, geometry, text) | ✅ | 2025-01-15 | `shape_inventory` returns structured metadata |

---

## Shape Vocabulary (Part D extended)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Autoshape preset coverage (191 presets) | ✅ | 2025-01-15 | `add_autoshape` — full `prstGeom` table; corpus round-trip tested |
| Connectors (straight/elbow/curved) | ✅ | 2025-01-15 | `add_connector` — `stCxn`/`endCxn` anchors; corpus round-trip tested |
| Freeform/custom geometry builder | ✅ | 2025-01-15 | `add_freeform` — `FreeformPath` move/line/cubic/close → `custGeom`; corpus round-trip tested |
| Group shapes (traverse/add) | ✅ | 2025-01-15 | `group_shapes` / `add_shape_to_group` — traverse + add into `grpSp`; corpus-tested |

---

## Charts (Part B)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Author charts (bar/line/pie/…) | ✅ | 2025-01-15 | `add_chart` — `p:graphicFrame` + chart part + embedded workbook + rels/content-types |
| Chart types (9 supported) | ✅ | 2025-01-15 | Clustered/stacked bar, clustered/stacked column, line, pie, doughnut, area, scatter |
| Embedded data workbook | ✅ | 2025-01-15 | `zavora-xlsx` builds valid `.xlsx`; chart XML ↔ workbook kept in sync |
| Title / legend / data labels | ✅ | 2025-01-15 | Optional title, legend position (b/t/l/r/tr), per-series `dLbls` |
| Axes (catAx / valAx) | ✅ | 2025-01-15 | Category + value axes for bar/column/line/area; two value axes for scatter; none for pie/doughnut |
| Chart data editing on opened deck | ✅ | 2025-01-15 | `update_chart_data` — replaces categories/series in DOM + rebuilds workbook; preserves styling |
| Round-trip byte preservation | ✅ | 2025-01-15 | Unmodified open→save preserves chart parts byte-for-byte |

---

## Tables (Part C)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Add/remove rows & columns | ✅ | 2025-01-15 | `add_table_row` / `insert_table_row` / `remove_table_row` / `add_table_column` / `remove_table_column` — surgical |
| Merge/split cells | ✅ | 2025-01-15 | `merge_table_cells` / `split_table_cell` — gridSpan/rowSpan/hMerge/vMerge |
| Column width / row height | ✅ | 2025-01-15 | `set_column_width` / `set_row_height` — surgical on opened decks |
| Cell text/alignment/fill/margins | ✅ | 2025-01-15 | `set_cell_text` / `set_cell_alignment` / `set_cell_fill` / `set_cell_margins` — surgical |
| Edit cells on opened deck | ✅ | 2025-01-15 | All table edits are surgical (only graphicFrame changes); corpus-tested |

---

## Images (Part E)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Crop (l/r/t/b) | ✅ | 2025-01-15 | `set_image_crop` — `a:srcRect` on blipFill; surgical on opened decks |
| Rotation | ✅ | 2025-01-15 | `set_shape_rotation` on picture shapes — surgical, corpus-tested |
| Insert on opened deck (surgical) | ✅ | 2025-01-15 | `insert_image_bytes` — DOM `p:pic` + media part + rel; content-hash dedupe |
| Content-hash deduplication | ✅ | 2025-01-15 | Same image bytes reuse existing media part (SHA-256 keyed) |
| GIF format support | ✅ | 2025-01-15 | PNG, JPEG, GIF accepted via magic-byte detection |

---

## Hyperlinks, Metadata, Notes (Part F)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Run-level hyperlink (external URL) | ✅ | 2025-01-15 | `set_run_hyperlink` — `a:hlinkClick` + external rel; surgical on opened decks |
| Shape click action (external URL / jump-to-slide) | ✅ | 2025-01-15 | `set_shape_click_action` — `ExternalUrl` / `JumpToSlide`; surgical, sibling-preserving |
| Core properties read/write | ✅ | 2025-01-15 | `core_properties()` / `set_core_properties()` — title/author/subject/keywords/comments/category/created/modified/last-modified-by; byte-preserving on opened decks |
| Speaker notes (DOM-based, no rebuild) | ✅ | 2025-01-15 | `set_notes` — edits notes-slide part in place via `NotesDom`; only notes part changes |
| Footer/slide-number/date placeholders | ✅ | 2025-01-15 | `set_footer_text` / `set_slide_number_text` / `set_date_text` / visibility toggle; surgical on opened decks |

---

## Render Fidelity (Part H)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Placeholder geometry inheritance (slide→layout→master) | ✅ | 2025-01-15 | Resolves `a:xfrm` by walking slide→layout→master; caches per deck; fallback to default box |
| Real text shaping (rustybuzz/fontdb) | ✅ | 2025-01-15 | Measured wrapping via rustybuzz + fontdb; alignment, indent, line-spacing, bullets, vertical anchor |
| Theme + color resolution (schemeClr→RGB, tint/shade/lumMod) | ✅ | 2025-01-15 | Resolves scheme colors via deck theme + slide color map; tint/shade/lumMod/lumOff modifiers; theme font refs |
| Shape/fill/image fidelity (preset geometry, gradients, crop) | ✅ | 2025-01-15 | Preset geometry paths for common presets; gradient/picture fills; outline dash; image srcRect crop + rotation |
| LibreOffice similarity check + threshold gate | ✅ | 2025-01-15 | Env-guarded PNG comparison; perceptual MAE metric; threshold-gated (0.5 baseline, tighten as fidelity improves) |

---

## Visual QA (Part I)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Structured layout report (bboxes, overlap, overflow) | ✅ | 2025-01-15 | Deterministic; off-canvas, overlap, zero-area, margin detection; fixture-tested |
| Contrast + readability checks (WCAG) | ✅ | 2025-01-15 | WCAG ratio + min font size; effective bg resolution; fixture-tested |
| Render-diff QA (before/after) | ✅ | 2025-01-15 | Tile-based PNG diff with noise threshold; fixture-tested |

---

## Design System (Part J)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Curated palettes + font pairings | ✅ | 2025-01-15 | 8 palettes + 6 font pairings; `apply_design_theme` by name; data-driven catalog |
| Layout patterns (two-column, icon-rows, stat, quote, etc.) | ✅ | 2025-01-15 | 6 patterns; enforced margins/hierarchy; anti-pattern avoidance; fixture-tested |
| Design lint (anti-patterns) | ✅ | 2025-01-15 | TextOnlySlide, CenteredBody, TooManyFonts, UndersizedTitle; structured findings; fixture-tested |

---

## Rich Extraction (Part K)

| Capability | Status | Date | Notes |
|---|---|---|---|
| Structured `to_outline` (JSON) | ✅ | 2025-01-15 | Serde-serializable `DeckOutline`; titles/body+level/tables/notes/alt-text/shape text; reading order; corpus-tested |
| Markdown ≥ markitdown | ✅ | 2025-01-15 | Tables as Markdown tables, notes as blockquotes, slide boundaries, bullet formatting; richness benchmark on corpus |
