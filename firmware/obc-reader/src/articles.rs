//! Bounded selection from a shared article bundle. Text and credits are read only after selection.
use crate::Error;
use obc_formats::{
    articles::*,
    io::{rd_u16, ByteSource},
    obcm::landmarks::{MAX_ATTRIBUTION_BYTES, MAX_TEXT_BYTES},
};

/// Returned references are relative to the self-contained bundle.
pub fn select(source: &dyn ByteSource, preferred: [u8; 2]) -> Result<ArticleVariant, Error> {
    let mut header = [0; HEADER_LEN];
    source.read_at(0, &mut header).map_err(Error::Source)?;
    let default = [header[0], header[1]];
    let count = rd_u16(&header, 2) as usize;
    if count == 0 || count > LANGUAGES.len() || source.len() > u64::from(MAX_BYTES) {
        return Err(Error::BadOffset);
    }
    let payload = (HEADER_LEN + count * VARIANT_LEN) as u32;
    let mut seen = [false; LANGUAGES.len()];
    let mut selected = None;
    let mut rank = 3;
    let mut has_default = false;
    for i in 0..count {
        let mut bytes = [0; VARIANT_LEN];
        source.read_at((HEADER_LEN + i * VARIANT_LEN) as u64, &mut bytes).map_err(Error::Source)?;
        let variant = ArticleVariant::decode(&bytes).ok_or(Error::BadOffset)?;
        let language = LANGUAGES.iter().position(|l| *l == variant.language).ok_or(Error::BadOffset)?;
        if core::mem::replace(&mut seen[language], true) {
            return Err(Error::BadOffset);
        }
        for (reference, limit) in [(variant.text, MAX_TEXT_BYTES), (variant.attribution, MAX_ATTRIBUTION_BYTES)] {
            reference.range(payload, source.len() as u32, limit).ok_or(Error::BadOffset)?;
        }
        has_default |= variant.language == default;
        let candidate_rank = if variant.language == preferred {
            0
        } else if variant.language == *b"en" {
            1
        } else if variant.language == default {
            2
        } else {
            3
        };
        if candidate_rank < rank {
            selected = Some(variant);
            rank = candidate_rank;
        }
    }
    if !has_default {
        return Err(Error::BadOffset);
    }
    selected.ok_or(Error::BadOffset)
}
