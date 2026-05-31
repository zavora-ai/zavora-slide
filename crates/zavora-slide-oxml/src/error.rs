//! Error types for OOXML model operations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OxmlError {
    #[error("XML error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XML attribute error: {0}")]
    XmlAttr(#[from] quick_xml::events::attributes::AttrError),

    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("parse error: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, OxmlError>;
