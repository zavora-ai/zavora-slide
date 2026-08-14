# zavora-slide

[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![MSRV: 1.94.1](https://img.shields.io/badge/MSRV-1.94.1-blue.svg)](https://www.rust-lang.org/tools/install)

A native Rust PowerPoint (`.pptx`) engine for creating, reading, surgically
editing, rendering, reviewing, and exporting presentations. It requires no
LibreOffice, COM automation, Microsoft Office, or C runtime dependency.

It powers the
[`slides-mcp-server`](https://github.com/zavora-ai/mcp_slides), while remaining
usable as an ordinary Rust library, CLI, or WebAssembly package.

## Capabilities

- **Faithful package handling:** read and write OPC/PresentationML packages;
  an unedited open/save round trip preserves package content byte-for-byte.
- **Slide lifecycle:** add, duplicate, delete, move, resize, and inspect slides.
- **Text model:** text boxes, paragraphs, runs, line breaks, bullets, alignment,
  spacing, rich formatting, autofit, and surgical DOM edits.
- **Shapes:** more than 187 preset geometries, freeforms, connectors, groups,
  fills, gradients, picture fills, outlines, rotation, and z-order.
- **Tables:** create and edit cells, rows, columns, sizing, margins, fills,
  alignment, merges, and splits.
- **Media and charts:** PNG/JPEG/GIF images, cropping, rotation, deduplication,
  nine chart kinds, editable chart data, and embedded XLSX workbooks.
- **Presentation features:** themes, palettes, font pairings, layout patterns,
  notes, footers, core properties, hyperlinks, and click actions.
- **Rendering and export:** SVG and PNG slide rendering, PDF export, Markdown
  extraction, and structured JSON outlines.
- **Quality analysis:** contrast checks, design linting, geometry diagnostics,
  render diffs, and similarity gates.
- **Portable surfaces:** `zslide` CLI plus standalone WebAssembly bindings.

## Quick start

```rust
use zavora_slide::{Layout, Presentation, RenderFormat};

let mut deck = Presentation::new();
let slide_index = deck.add_slide(Layout::TitleContent);
deck.slide_mut(slide_index)?.set_title("Quarterly review")?;
deck.save("review.pptx")?;

let png = deck.render_slide(slide_index, RenderFormat::Png)?;
std::fs::write("review.png", png)?;
# Ok::<(), zavora_slide::SlideError>(())
```

```toml
[dependencies]
zavora-slide = "0.1"
```

## Crate architecture

| Crate | Purpose |
|---|---|
| `zavora-slide` | High-level `Presentation` API, editing, extraction, design, and QA |
| `zavora-slide-opc` | OPC/ZIP package I/O, content types, and relationships |
| `zavora-slide-oxml` | Byte-faithful PresentationML DOM and typed mutations |
| `zavora-slide-layout` | EMU geometry, layout resolution, and preset shape paths |
| `zavora-slide-render` | SVG/PNG rendering and theme/font resolution |
| `zavora-slide-pdf` | Multi-slide PDF export |
| `zavora-slide-xlsx` | Embedded chart-workbook generation |
| `zavora-slide-cli` | `zslide` command-line interface |
| `zavora-slide-wasm` | Standalone WebAssembly bindings |

The workspace crates are intentionally modular. Publishing them separately
keeps small consumers lightweight while the façade crate enables the complete
engine; consolidation does not remove or rename any public capability.

## Verification

The test suite covers authored and real-world decks, byte-faithful round trips,
surgical edits, LibreOffice load gates, chart workbooks, rendering, PDF output,
design QA, and package validity. CI tests all native targets with Rust 1.94.1
and checks the standalone WASM package for `wasm32-unknown-unknown`.

## Minimum supported Rust version

Rust 1.94.1 (edition 2024).

## License

Licensed under either MIT or Apache-2.0 at your option.
