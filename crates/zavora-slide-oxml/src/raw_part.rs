//! Byte-preserving wrapper for parts the engine does not yet deeply model
//! (`slideMaster`, `slideLayout`, `slide`, `theme`).
//!
//! Phase 0 carries these verbatim so packages round-trip losslessly. Deep typing
//! of the slide body arrives in task 3.1.

/// A presentation part held as raw XML bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct RawPart {
    pub xml: Vec<u8>,
}

impl RawPart {
    pub fn from_xml(xml: &[u8]) -> Self {
        Self { xml: xml.to_vec() }
    }

    pub fn to_xml(&self) -> Vec<u8> {
        self.xml.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_byte_identical() {
        let src = br#"<p:sld xmlns:p="..."><p:cSld/></p:sld>"#;
        let part = RawPart::from_xml(src);
        assert_eq!(part.to_xml(), src.to_vec());
    }
}
