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
mod slide;
mod template;
mod theme;
mod units;

pub use error::{Result, SlideError};
pub use presentation::Presentation;
pub use slide::{Bullet, Fill, ImageSrc, ShapeInfo, Slide, SlideData, Table, TableId};
pub use theme::ThemeSpec;
pub use units::{Emu, Layout, RenderFormat, ShapePreset, SlideSize};

pub use zavora_slide_oxml::Align;
pub use zavora_slide_layout::{Color, Item, Scene, TextLine};
