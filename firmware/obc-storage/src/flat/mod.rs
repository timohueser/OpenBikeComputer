// The card's own byte layout, and nothing above the seam may name it. `seam`, `error`, `device` and
// `store` are the public face: a board crate has to be able to name `BlockDevice`, and a consumer
// `Store`.
pub(crate) mod bitmap;
pub(crate) mod catalog;
pub mod device;
pub mod error;
pub(crate) mod journal;
pub(crate) mod layout;
pub mod metadata;
pub(crate) mod raw;
pub mod seam;
pub mod source;
pub mod store;
pub(crate) mod superblock;
pub mod wire;

#[cfg(any(test, feature = "std"))]
pub mod model;
#[cfg(any(test, feature = "std"))]
pub mod sim;

#[cfg(test)]
mod block_runs;
#[cfg(test)]
mod cost;
#[cfg(test)]
mod crash;
#[cfg(test)]
mod fence;
#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod granularity;
#[cfg(test)]
mod map_read;
#[cfg(test)]
mod read_cost;
#[cfg(test)]
mod sealed;
#[cfg(test)]
mod vectors;

pub use device::BlockDevice;
pub use error::{DecodeError, Reason, Record, StoreError};
pub use layout::MAX_RANGES;
pub use seam::{
    Allocation, DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision,
    RideCheckpoint, Store, StoreId, RIDE_RESUME_LEN,
};
pub use source::{SealedSource, StoreSource};
pub use store::{BlockRun, FlatStore, Handle, Mode, RideRecovery, SealedAllocation};
/// The four bytes at block 0 of a formatted card: what names a card image.
pub use superblock::MAGIC as SUPERBLOCK_MAGIC;

/// The two blocks a media-identity probe reads, in mount preference order.
pub const SUPERBLOCK_BLOCKS: [u64; 2] = layout::SUPERBLOCK;
/// The two catalog gates, in copy order.
pub const CATALOG_GATE_BLOCKS: [u64; 2] = [layout::catalog_gate(0), layout::catalog_gate(1)];

/// The identity and recorded size from one fully validated superblock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaIdentity {
    pub store: StoreId,
    pub total_blocks: u64,
}

/// The retained mount facts a hot-reinsert probe must find again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountedMediaState {
    pub store: StoreId,
    pub sequence: u64,
    pub high_water: u64,
}

/// Decode one superblock with the same checks a mount uses.
///
/// `None` also covers a card smaller than the size recorded at initialization. Such media cannot
/// safely serve addresses derived from the mounted store.
pub fn decode_media_identity(bytes: &[u8], observed_blocks: u64) -> Option<MediaIdentity> {
    let superblock = superblock::Superblock::decode(bytes).ok()?;
    (observed_blocks >= superblock.total_blocks)
        .then_some(MediaIdentity { store: superblock.store, total_blocks: superblock.total_blocks })
}

/// Decode one catalog gate with the same CRC, version, position and StoreId checks as mount.
pub fn decode_catalog_sequence(bytes: &[u8], copy: usize, store: StoreId) -> Option<u64> {
    catalog::Gate::decode(bytes, copy, &store).ok().map(|gate| gate.sequence)
}

/// Whether two decoded gate slots still describe the retained mount.
///
/// The served sequence must remain present and no well-formed gate may advance or rewind the
/// high-water mark. Invalid gates are `None`, as they are during mount selection.
pub fn catalog_state_matches(state: MountedMediaState, gates: [Option<u64>; 2]) -> bool {
    gates.contains(&Some(state.sequence)) && gates.into_iter().flatten().max() == Some(state.high_water)
}

#[cfg(test)]
mod media_probe_tests {
    use super::*;

    const STORE: StoreId = StoreId([0x35; 16]);
    const BLOCKS: u64 = 62_914_560;

    fn gate(copy: usize, sequence: u64) -> [u8; 512] {
        catalog::Gate { copy: copy as u8, store: STORE, sequence, entry_count: 4, body_crc: 0x1234_5678 }.encode()
    }

    #[test]
    fn media_probe_uses_the_format_decoders() {
        let bytes = superblock::Superblock::for_card(STORE, BLOCKS).unwrap().encode();
        assert_eq!(decode_media_identity(&bytes, BLOCKS), Some(MediaIdentity { store: STORE, total_blocks: BLOCKS }));
        assert_eq!(decode_media_identity(&bytes, BLOCKS - 1), None, "a smaller replacement cannot serve old extents");

        let mut corrupt = bytes;
        corrupt[8] ^= 1;
        assert_eq!(decode_media_identity(&corrupt, BLOCKS), None, "the superblock CRC stays authoritative");

        assert_eq!(decode_catalog_sequence(&gate(1, 9), 1, STORE), Some(9));
        assert_eq!(decode_catalog_sequence(&gate(1, 9), 0, STORE), None, "the gate must occupy its encoded copy");
    }

    #[test]
    fn retained_catalog_rejects_a_newer_commit_on_the_same_store() {
        let state = MountedMediaState { store: STORE, sequence: 8, high_water: 8 };
        assert!(catalog_state_matches(state, [Some(7), Some(8)]));
        assert!(!catalog_state_matches(state, [Some(8), Some(9)]), "an older matching gate cannot hide a new commit");
        assert!(!catalog_state_matches(state, [Some(7), None]), "the served catalog must still be present");
    }
}

/// The format version this store implements.
pub const FORMAT_VERSION: u16 = 1;

pub mod route_cleanup;
