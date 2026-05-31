//! High-level PowerPoint (.pptx) presentation API.
//!
//! ```no_run
//! use zavora_slide::{Presentation, Layout};
//! let mut p = Presentation::new();
//! p.add_slide(Layout::TitleContent);
//! p.save("deck.pptx").unwrap();
//! ```

mod error;
mod presentation;
mod template;
mod units;

pub use error::{Result, SlideError};
pub use presentation::Presentation;
pub use units::{Emu, Layout, RenderFormat, ShapePreset, SlideSize};
