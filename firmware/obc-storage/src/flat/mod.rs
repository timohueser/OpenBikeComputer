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
    pub extent_size: u64,
    pub extent_count: u32,
}

/// The fields of one well-formed catalog gate that identify the commit it certifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogGateIdentity {
    pub copy: u8,
    pub sequence: u64,
    pub entry_count: u16,
    pub body_crc: u32,
}

/// The retained mount facts a hot-reinsert probe must find again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountedMediaState {
    pub store: StoreId,
    pub extent_size: u64,
    pub extent_count: u32,
    pub served: CatalogGateIdentity,
    pub high_water: u64,
}

/// Decode one superblock with the same checks a mount uses.
///
/// `None` also covers a card smaller than the size recorded at initialization. Such media cannot
/// safely serve addresses derived from the mounted store.
pub fn decode_media_identity(bytes: &[u8], observed_blocks: u64) -> Option<MediaIdentity> {
    let superblock = superblock::Superblock::decode(bytes).ok()?;
    (observed_blocks >= superblock.total_blocks).then_some(MediaIdentity {
        store: superblock.store,
        total_blocks: superblock.total_blocks,
        extent_size: superblock.geometry.extent_size(),
        extent_count: superblock.extent_count(),
    })
}

/// Decode one catalog gate with the same CRC, version, position and StoreId checks as mount.
pub fn decode_catalog_identity(bytes: &[u8], copy: usize, store: StoreId) -> Option<CatalogGateIdentity> {
    let gate = catalog::Gate::decode(bytes, copy, &store).ok()?;
    Some(CatalogGateIdentity {
        copy: gate.copy,
        sequence: gate.sequence,
        entry_count: gate.entry_count,
        body_crc: gate.body_crc,
    })
}

/// Whether a decoded superblock describes the retained address space.
pub fn media_identity_matches(state: MountedMediaState, identity: MediaIdentity) -> bool {
    identity.store == state.store
        && identity.extent_size == state.extent_size
        && identity.extent_count == state.extent_count
}

/// Whether two decoded gate slots still describe the retained mount.
///
/// A fallback mount with a higher well-formed gate is refused: recovery does not reread the full
/// candidate body, so it cannot prove that candidate stayed invalid while the card was absent.
/// Equal-sequence gates are also refused, as they are at mount. Otherwise the exact served gate
/// must remain the unique highest gate.
pub fn catalog_state_matches(state: MountedMediaState, gates: [Option<CatalogGateIdentity>; 2]) -> bool {
    if state.high_water != state.served.sequence
        || gates[0].zip(gates[1]).is_some_and(|(a, b)| a.sequence == b.sequence)
    {
        return false;
    }
    gates.get(state.served.copy as usize).copied().flatten() == Some(state.served)
        && gates.into_iter().flatten().map(|gate| gate.sequence).max() == Some(state.high_water)
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
        assert_eq!(
            decode_media_identity(&bytes, BLOCKS),
            Some(MediaIdentity { store: STORE, total_blocks: BLOCKS, extent_size: 1 << 20, extent_count: 30_718 })
        );
        assert_eq!(decode_media_identity(&bytes, BLOCKS - 1), None, "a smaller replacement cannot serve old extents");

        let mut corrupt = bytes;
        corrupt[8] ^= 1;
        assert_eq!(decode_media_identity(&corrupt, BLOCKS), None, "the superblock CRC stays authoritative");

        assert_eq!(decode_catalog_identity(&gate(1, 9), 1, STORE).unwrap().sequence, 9);
        assert_eq!(decode_catalog_identity(&gate(1, 9), 0, STORE), None, "the gate must occupy its encoded copy");
    }

    #[test]
    fn retained_media_rejects_changed_geometry_with_the_same_store_id() {
        let state = mounted_state(1, 8);
        let changed = MediaIdentity {
            store: STORE,
            total_blocks: BLOCKS,
            extent_size: state.extent_size * 2,
            extent_count: state.extent_count / 2,
        };
        assert!(!media_identity_matches(state, changed));
    }

    fn marker(copy: usize, sequence: u64) -> CatalogGateIdentity {
        decode_catalog_identity(&gate(copy, sequence), copy, STORE).unwrap()
    }

    fn mounted_state(copy: usize, sequence: u64) -> MountedMediaState {
        MountedMediaState {
            store: STORE,
            extent_size: 1 << 20,
            extent_count: 30_718,
            served: marker(copy, sequence),
            high_water: sequence,
        }
    }

    #[test]
    fn retained_catalog_requires_the_exact_served_gate_and_unique_sequence() {
        let state = mounted_state(1, 8);
        assert!(catalog_state_matches(state, [Some(marker(0, 7)), Some(marker(1, 8))]));
        assert!(!catalog_state_matches(state, [Some(marker(0, 8)), Some(marker(1, 8))]), "equal gates do not mount");
        assert!(!catalog_state_matches(state, [Some(marker(0, 8)), Some(marker(1, 7))]), "the served copy cannot move");

        let mut changed = marker(1, 8);
        changed.body_crc ^= 1;
        assert!(!catalog_state_matches(state, [Some(marker(0, 7)), Some(changed)]));
        changed = marker(1, 8);
        changed.entry_count += 1;
        assert!(!catalog_state_matches(state, [Some(marker(0, 7)), Some(changed)]));
    }

    #[test]
    fn retained_catalog_refuses_a_fallback_mount() {
        let mut state = mounted_state(0, 8);
        state.high_water = 9;
        assert!(!catalog_state_matches(state, [Some(marker(0, 8)), Some(marker(1, 9))]));
    }
}

/// The format version this store implements.
pub const FORMAT_VERSION: u16 = 1;

pub mod route_cleanup;
