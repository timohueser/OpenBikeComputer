//! Separate peak article collection. All interior offsets are section-relative bytes.
use super::{landmarks::ContentRef, SourceId};
use crate::io::rd_u32;

pub const HEADER_LEN: usize = 24;
pub const ASSOCIATION_LEN: usize = 44;
pub const RECORD_LEN: usize = 64;
/// Each payload starts with its article identity and its content slot (0..3).
pub const CONTENT_GUARD_LEN: u32 = 33;
pub const VERSION: u16 = 1;
pub const MAX_RECORDS: u32 = 65_535;
pub const MAX_ASSOCIATIONS: u32 = 262_144;
/// SHA-256 of the compiler's canonical UTF-8 article identity.
pub type ArticleId = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Association {
    pub source: SourceId,
    pub article: ArticleId,
    pub index: u32,
}
impl Association {
    pub fn decode(bytes: &[u8; ASSOCIATION_LEN]) -> Option<Self> {
        let source = SourceId(u64::from_le_bytes(bytes[..8].try_into().ok()?));
        (source.is_valid() && source.0 >> 62 == 1).then(|| Self {
            source,
            article: bytes[8..40].try_into().unwrap(),
            index: rd_u32(bytes, 40),
        })
    }
    pub fn encode(self) -> [u8; ASSOCIATION_LEN] {
        let mut bytes = [0; ASSOCIATION_LEN];
        bytes[..8].copy_from_slice(&self.source.0.to_le_bytes());
        bytes[8..40].copy_from_slice(&self.article);
        bytes[40..].copy_from_slice(&self.index.to_le_bytes());
        bytes
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: ArticleId,
    /// Name, multilingual article bundle, optional photo, optional photo attribution.
    pub content: [ContentRef; 4],
}
impl Record {
    pub fn decode(bytes: &[u8; RECORD_LEN]) -> Self {
        Self {
            id: bytes[..32].try_into().unwrap(),
            content: core::array::from_fn(|i| ContentRef {
                offset: rd_u32(bytes, 32 + i * 8),
                len: rd_u32(bytes, 36 + i * 8),
            }),
        }
    }
    pub fn encode(self) -> [u8; RECORD_LEN] {
        let mut bytes = [0; RECORD_LEN];
        bytes[..32].copy_from_slice(&self.id);
        for (i, r) in self.content.iter().enumerate() {
            bytes[32 + i * 8..36 + i * 8].copy_from_slice(&r.offset.to_le_bytes());
            bytes[36 + i * 8..40 + i * 8].copy_from_slice(&r.len.to_le_bytes());
        }
        bytes
    }
}
