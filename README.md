# zavora-slide

A pure-Rust PowerPoint (`.pptx`) engine — create, read, and (in later phases)
render presentations programmatically. No LibreOffice, no COM automation, no C
dependencies. It is the foundation for the [`slides-mcp`](https://github.com/zavora-ai/slides-mcp)
MCP server and a future web slides client.

Sibling to [`zavora-docx`](https://github.com/zavora-ai/zavora-docx) and
[`zavora-xlsx`](https://github.com/zavora-ai/zavora-xlsx), with the same layered
architecture.

## Status

Phase 0 (authoring core) — create a valid 16:9 deck, add slides, and save a
`.pptx` that opens in PowerPoint, LibreOffice Impress, and Google Slides without
a repair prompt. Text, images, shapes, theming, and rendering land in later
phases (see the spec in `slides-mcp`).

## Quick Start

```rust
use zavora_slide::{Presentation, Layout};

let mut pres = Presentation::new();          // blank 16:9 deck
pres.add_slide(Layout::TitleContent);
pres.add_slide(Layout::Blank);
pres.save("deck.pptx").unwrap();
```

```toml
[dependencies]
zavora-slide = "0.1"
```

## Crate Architecture

| Crate | Purpose |
|---|---|
| `zavora-slide` | High-level `Presentation` API |
| `zavora-slide-opc` | OPC/ZIP package I/O, `[Content_Types].xml`, relationships |
| `zavora-slide-oxml` | PresentationML typed models (parse/serialize) |
| `zavora-slide-layout` | Layout/EMU positioning (Phase 3) |
| `zavora-slide-render` | Slide → PNG/SVG (Phase 3) |
| `zavora-slide-pdf` | Presentation → PDF (Phase 3) |
| `zavora-slide-cli` | `zslide` CLI binary |
| `zavora-slide-wasm` | WASM bindings for the web client (Phase 4, excluded from workspace) |

## License

Licensed under either of MIT or Apache-2.0 at your option.
