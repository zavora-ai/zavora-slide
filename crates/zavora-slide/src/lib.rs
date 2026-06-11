//! High-level PowerPoint (.pptx) presentation API.
//!
//! ```no_run
//! use zavora_slide::{Presentation, Layout};
//! let mut p = Presentation::new();
//! p.add_slide(Layout::TitleContent);
//! p.save("deck.pptx").unwrap();
//! ```

mod core_properties;
pub mod chart;
pub mod design;
mod error;
pub mod extraction;
mod presentation;
pub mod qa;
mod slide;
mod template;
mod theme;
mod units;

pub use error::{Result, SlideError};
pub use presentation::Presentation;
pub use slide::{Bullet, Fill, ImageFormat, ImageSrc, MediaEntry, ShapeInfo, Slide, SlideData, SlideRef, Table, TableId};
pub use theme::ThemeSpec;
pub use units::{Emu, Layout, RenderFormat, ShapePreset, SlideSize};
pub use chart::{ChartKind, ChartSpec, ChartDataUpdate};
pub use design::{
    apply_design_theme, apply_layout_pattern, font_pairing_by_id, font_pairings,
    palette_by_id, palettes, FontPairing, LayoutPattern, Palette, PatternParams,
};

pub use zavora_slide_oxml::Align;
pub use zavora_slide_oxml::RunFormat;
pub use zavora_slide_oxml::{AutoFit, SpacingValue, BulletKind};
pub use qa::{design_lint, design_lint_with_config};
pub use zavora_slide_layout::{Color, Item, Scene, TextLine};

pub use core_properties::CoreProperties;
pub use extraction::{to_markdown, to_outline, DeckOutline, OutlineElement, SlideOutline};
