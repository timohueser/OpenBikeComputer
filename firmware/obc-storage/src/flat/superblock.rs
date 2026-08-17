//! The superblock (`FLAT_Store_Format.md` §4): written once by initialization and never again.
//!
//! Nothing in normal operation updates it, which is why it carries no gate and no sequence. Two
//! copies of identical bytes exist so that one bad block does not make the card unreadable.

use super::error::{DecodeError, Reason, Record, Result};
use super::layout::BLOCK;
use super::raw::{bytes16_at, crc32, is_zero, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use super::seam::StoreId;
use super::FORMAT_VERSION;

/// `FSSB`.
pub const MAGIC: [u8; 4] = *b"FSSB";
/// The CRC covers bytes `0..504`.
const CRC_OFFSET: usize = 504;

/// The superblock body: block 0 of the copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub store: StoreId,
    /// Total card blocks observed at initialization.
    pub total_blocks: u64,
}

impl Superblock {
    /// The exact 512 bytes.
    pub fn encode(&self) -> [u8; BLOCK] {
        let mut out = [0u8; BLOCK];
        put_bytes(&mut out, 0, &MAGIC);
        put_u16(&mut out, 4, FORMAT_VERSION);
        put_bytes(&mut out, 8, &self.store.0);
        put_u64(&mut out, 24, self.total_blocks);
        let crc = crc32(&out[..CRC_OFFSET]);
        put_u32(&mut out, CRC_OFFSET, crc);
        out
    }

    /// Decodes block 0 of a copy. Valid when its magic, version and CRC all check.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let err = |reason| DecodeError::new(Record::Superblock, reason);
        if bytes.len() < BLOCK {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u32_at(bytes, CRC_OFFSET) != crc32(&bytes[..CRC_OFFSET]) {
            return Err(err(Reason::Crc));
        }
        if !is_zero(bytes, 6, 2) || !is_zero(bytes, 32, 472) || !is_zero(bytes, 508, 4) {
            return Err(err(Reason::Reserved));
        }
        Ok(Superblock { store: StoreId(bytes16_at(bytes, 8)), total_blocks: u64_at(bytes, 24) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let superblock = Superblock { store: StoreId([0x5A; 16]), total_blocks: 1_234_567 };
        assert_eq!(Superblock::decode(&superblock.encode()).unwrap(), superblock);
    }

    /// §2: "Block 0 is deliberately not an MBR." Its bytes `510..511` are zero, so a host sees an
    /// unformatted device rather than a partition table it might try to repair.
    #[test]
    fn block_zero_never_looks_like_a_partition_table() {
        let bytes = Superblock { store: StoreId([0xFF; 16]), total_blocks: u64::MAX }.encode();
        assert_eq!(bytes[510..512], [0, 0]);
    }

    #[test]
    fn every_single_byte_flip_is_rejected() {
        let bytes = Superblock { store: StoreId([0x11; 16]), total_blocks: 62_914_560 }.encode();
        for index in 0..BLOCK {
            let mut torn = bytes;
            torn[index] ^= 0xFF;
            assert!(Superblock::decode(&torn).is_err(), "byte {index} flip accepted");
        }
    }

    #[test]
    fn a_zeroed_or_short_block_is_not_a_superblock() {
        assert_eq!(Superblock::decode(&[0u8; BLOCK]).unwrap_err().reason, Reason::Magic);
        assert_eq!(Superblock::decode(&[0u8; 511]).unwrap_err().reason, Reason::Length);
    }
}
