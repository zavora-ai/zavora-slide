//! PresentationML + shared DrawingML typed models (parse/serialize).
//!
//! Phase 0 models `ppt/presentation.xml` (slide-id list, master-id list, slide
//! size) with verbatim capture of unmodeled children. The heavyweight parts
//! (`slideMaster`, `slideLayout`, `slide`, `theme`) are carried byte-for-byte via
//! [`RawPart`] until deeper typing lands in task 3.1.

mod error;
pub mod drawing;
pub mod presentation;
mod raw_part;

pub use drawing::{Align, Paragraph, Placeholder, Run, RunProps, Shape, TextBody};
pub use error::{OxmlError, Result};
pub use presentation::{Presentation, SlideIdEntry, SlideSize};
pub use raw_part::RawPart;
