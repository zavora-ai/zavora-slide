//! PresentationML + shared DrawingML typed models (parse/serialize).
//!
//! Phase 0 models `ppt/presentation.xml` (slide-id list, master-id list, slide
//! size) with verbatim capture of unmodeled children. The heavyweight parts
//! (`slideMaster`, `slideLayout`, `slide`, `theme`) are carried byte-for-byte via
//! [`RawPart`] until deeper typing lands in task 3.1.

pub mod drawing;
mod error;
pub mod notes_dom;
pub mod presentation;
mod raw_part;
pub mod scheme_color;
pub mod slide_dom;
pub mod xml;

pub use drawing::{Align, Paragraph, Placeholder, Run, RunProps, Shape, TextBody};
pub use error::{OxmlError, Result};
pub use notes_dom::NotesDom;
pub use presentation::{Presentation, SlideIdEntry, SlideSize};
pub use raw_part::RawPart;
pub use scheme_color::SchemeColor;
pub use slide_dom::{
    AutoFit, BulletKind, ClickAction, ColorSpec, ConnectorAnchor, ConnectorType, FillSpec,
    FreeformPath, LineSpec, RunFormat, ShapeInfo, SlideDom, SpacingValue,
};
pub use xml::{Document, Element, Node};
