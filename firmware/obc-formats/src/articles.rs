//! Shared, self-contained multilingual article bundles. Photos belong to the enclosing place.
use crate::{
    io::rd_u32,
    obcm::landmarks::{ContentRef, MAX_ATTRIBUTION_BYTES, MAX_TEXT_BYTES, MAX_TEXT_PAGES},
};

/// The device UI order is checked against this mapping by obc-app.
pub const LANGUAGES: [[u8; 2]; 4] = [*b"en", *b"de", *b"fr", *b"es"];
pub const HEADER_LEN: usize = 4;
pub const VARIANT_LEN: usize = 20;
pub const MAX_BYTES: u32 =
    HEADER_LEN as u32 + LANGUAGES.len() as u32 * (VARIANT_LEN as u32 + MAX_TEXT_BYTES + MAX_ATTRIBUTION_BYTES);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArticleVariant {
    pub language: [u8; 2],
    pub text_pages: u8,
    pub text: ContentRef,
    pub attribution: ContentRef,
}
impl ArticleVariant {
    pub fn decode(bytes: &[u8; VARIANT_LEN]) -> Option<Self> {
        let language = [bytes[0], bytes[1]];
        (LANGUAGES.contains(&language) && (1..=MAX_TEXT_PAGES).contains(&bytes[2]) && bytes[3] == 0).then(|| Self {
            language,
            text_pages: bytes[2],
            text: ContentRef { offset: rd_u32(bytes, 4), len: rd_u32(bytes, 8) },
            attribution: ContentRef { offset: rd_u32(bytes, 12), len: rd_u32(bytes, 16) },
        })
    }
    pub fn encode(&self) -> [u8; VARIANT_LEN] {
        let mut bytes = [0; VARIANT_LEN];
        bytes[..2].copy_from_slice(&self.language);
        bytes[2] = self.text_pages;
        for (at, reference) in [(4, self.text), (12, self.attribution)] {
            bytes[at..at + 4].copy_from_slice(&reference.offset.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&reference.len.to_le_bytes());
        }
        bytes
    }
}
