//! WASM bindings for the web slides client.
//!
//! Exposes opening a `.pptx` from bytes (text-only model), inspecting it, and
//! rendering slides to SVG. SVG is the web client's target; PNG/PDF (which pull
//! in heavy rasterization deps) are intentionally not exposed here.

use wasm_bindgen::prelude::*;
use zavora_slide::{Presentation, RenderFormat};

/// An opened presentation, owned by JS.
#[wasm_bindgen]
pub struct WasmDeck {
    pres: Presentation,
}

#[wasm_bindgen]
impl WasmDeck {
    /// Open a `.pptx` from its raw bytes (text-only extraction).
    #[wasm_bindgen(js_name = open)]
    pub fn open(bytes: &[u8]) -> Result<WasmDeck, JsValue> {
        Presentation::open_from_bytes(bytes)
            .map(|pres| WasmDeck { pres })
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Number of slides.
    #[wasm_bindgen(js_name = slideCount)]
    pub fn slide_count(&self) -> usize {
        self.pres.slide_count()
    }

    /// Markdown outline of the deck (titles, bullets, notes).
    #[wasm_bindgen(js_name = toMarkdown)]
    pub fn to_markdown(&self) -> String {
        self.pres.to_markdown()
    }

    /// Render one slide to an SVG document string.
    #[wasm_bindgen(js_name = renderSvg)]
    pub fn render_svg(&self, slide: usize) -> Result<String, JsValue> {
        let bytes = self
            .pres
            .render_slide(slide, RenderFormat::Svg)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        String::from_utf8(bytes).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}
