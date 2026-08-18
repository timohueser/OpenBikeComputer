//! The superblock (`FLAT_Store_Format.md` §4): written once by initialization and never again.
//!
//! Nothing in normal operation updates it, which is why it carries no gate and no sequence. Two
//! copies of identical bytes exist so that one bad block does not make the card unreadable.

use super::error::{DecodeError, Reason, Record, Result};
use super::layout::{Geometry, BLOCK, MAX_EXTENTS};
use super::raw::{bytes16_at, crc32, is_zero, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use super::seam::StoreId;
use super::FORMAT_VERSION;

/// `FSSB`.
pub const MAGIC: [u8; 4] = *b"FSSB";
/// The CRC covers bytes `0..504`.
const CRC_OFFSET: usize = 504;
/// The recorded extent size, as a base-2 logarithm of bytes (§4). It sits right after the block count
/// because the two together are the card's geometry, and inside the CRC's range because it is the one
/// field an address depends on.
const EXTENT_LOG2_OFFSET: usize = 32;

/// The superblock body: block 0 of the copy.
///
/// The fields are public and [`encode`](Self::encode) checks nothing, so a caller can build and
/// encode a record that [`decode`](Self::decode) will refuse — a size that does not cover its own
/// card is the interesting one. That is deliberate and test-only: the tests that pin both geometry
/// refusals need a way to write the card §8 never writes. **Production has one constructor**,
/// [`for_card`](Self::for_card), which cannot produce either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub store: StoreId,
    /// Total card blocks observed at initialization.
    pub total_blocks: u64,
    /// The extent size §8 chose for this card, which every address on it is derived from.
    pub geometry: Geometry,
}

impl Superblock {
    /// The superblock §8 writes for a card of `total_blocks`, geometry included. `None` for a card
    /// this format cannot express — see [`Geometry::for_card`].
    pub fn for_card(store: StoreId, total_blocks: u64) -> Option<Self> {
        Some(Superblock { store, total_blocks, geometry: Geometry::for_card(total_blocks)? })
    }

    /// §6's extent count for this card at this card's own extent size. Within [`MAX_EXTENTS`] by
    /// construction: a superblock that would exceed it does not decode.
    pub fn extent_count(&self) -> u32 {
        self.geometry.extent_count(self.total_blocks) as u32
    }

    /// The exact 512 bytes.
    pub fn encode(&self) -> [u8; BLOCK] {
        let mut out = [0u8; BLOCK];
        put_bytes(&mut out, 0, &MAGIC);
        put_u16(&mut out, 4, FORMAT_VERSION);
        put_bytes(&mut out, 8, &self.store.0);
        put_u64(&mut out, 24, self.total_blocks);
        out[EXTENT_LOG2_OFFSET] = self.geometry.log2();
        let crc = crc32(&out[..CRC_OFFSET]);
        put_u32(&mut out, CRC_OFFSET, crc);
        out
    }

    /// Decodes block 0 of a copy. Valid when its magic, version and CRC all check, its recorded
    /// extent size is one §6 admits, and that size covers the recorded card in extents the entry's
    /// `u16` index can name.
    ///
    /// Both geometry rules are checked **here**, before any address is derived from the record — and a
    /// superblock that fails either is not a superblock, so §5.6 step 1 falls back to the other copy
    /// and then classifies the card as unformatted. That is the same face a bad CRC gets, and the right
    /// one: the card's geometry is unreadable, so there is no store here to mount read-only.
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
        if !is_zero(bytes, 6, 2) || !is_zero(bytes, 33, 471) || !is_zero(bytes, 508, 4) {
            return Err(err(Reason::Reserved));
        }
        let geometry = Geometry::from_log2(bytes[EXTENT_LOG2_OFFSET]).ok_or(err(Reason::Geometry))?;
        let total_blocks = u64_at(bytes, 24);
        if geometry.extent_count(total_blocks) > MAX_EXTENTS as u64 {
            return Err(err(Reason::Count));
        }
        Ok(Superblock { store: StoreId(bytes16_at(bytes, 8)), total_blocks, geometry })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: StoreId = StoreId([0x5A; 16]);

    #[test]
    fn round_trips() {
        for log2 in 20..=31u8 {
            let superblock =
                Superblock { store: STORE, total_blocks: 1_234_567, geometry: Geometry::from_log2(log2).unwrap() };
            assert_eq!(Superblock::decode(&superblock.encode()).unwrap(), superblock);
        }
    }

    /// §2: "Block 0 is deliberately not an MBR." Its bytes `510..511` are zero, so a host sees an
    /// unformatted device rather than a partition table it might try to repair.
    #[test]
    fn block_zero_never_looks_like_a_partition_table() {
        let bytes = Superblock::for_card(StoreId([0xFF; 16]), (128 << 40) / 512).unwrap().encode();
        assert_eq!(bytes[510..512], [0, 0]);
    }

    #[test]
    fn every_single_byte_flip_is_rejected() {
        let bytes = Superblock::for_card(StoreId([0x11; 16]), 62_914_560).unwrap().encode();
        for index in 0..BLOCK {
            let mut torn = bytes;
            torn[index] ^= 0xFF;
            assert!(Superblock::decode(&torn).is_err(), "byte {index} flip accepted");
        }
    }

    /// §8's rule, recorded and read back: the size scales with the card and the count stays inside the
    /// index either way.
    #[test]
    fn the_geometry_is_the_cards_own_and_survives_the_round_trip() {
        for (blocks, size, extents) in [
            (62_914_560u64, 1u64 << 20, 30_718u32), // §4.1's 32 GB card
            ((64 << 30) / 512, 1 << 20, 65_534),    // 64 GiB, the old ceiling, at the old size
            ((128 << 30) / 512, 2 << 20, 65_535),   // 128 GiB, which the old format could not reach
            ((2u64 << 40) / 512, 32 << 20, 65_535), // 2 TiB, SDXC's ceiling
        ] {
            let superblock = Superblock::for_card(STORE, blocks).expect("an expressible card");
            let decoded = Superblock::decode(&superblock.encode()).unwrap();
            assert_eq!(decoded.geometry.extent_size(), size, "the wrong extent size at {blocks} blocks");
            assert_eq!(decoded.extent_count(), extents, "the wrong extent count at {blocks} blocks");
        }
        assert_eq!(Superblock::for_card(STORE, u64::MAX), None, "a card past 128 TiB is not expressible");
    }

    /// The two geometry refusals. Neither is a store that mounts read-only: a card whose geometry does
    /// not decode has no addresses at all.
    #[test]
    fn an_inadmissible_geometry_is_not_a_superblock() {
        let mut bytes = Superblock::for_card(STORE, 62_914_560).unwrap().encode();
        for log2 in [0u8, 19, 32, 255] {
            bytes[32] = log2;
            let crc = crc32(&bytes[..CRC_OFFSET]);
            put_u32(&mut bytes, CRC_OFFSET, crc);
            assert_eq!(Superblock::decode(&bytes).unwrap_err().reason, Reason::Geometry, "2^{log2} was accepted");
        }

        // A 128 GiB card recorded with 1 MiB extents needs 131,070 of them, which the entry's `u16`
        // index cannot name. Refused rather than silently capped: the tail would be unreachable space
        // the superblock claims.
        let over = Superblock { store: STORE, total_blocks: (128 << 30) / 512, geometry: Geometry::DEFAULT };
        assert_eq!(Superblock::decode(&over.encode()).unwrap_err().reason, Reason::Count);
    }

    #[test]
    fn a_zeroed_or_short_block_is_not_a_superblock() {
        assert_eq!(Superblock::decode(&[0u8; BLOCK]).unwrap_err().reason, Reason::Magic);
        assert_eq!(Superblock::decode(&[0u8; 511]).unwrap_err().reason, Reason::Length);
    }
}
