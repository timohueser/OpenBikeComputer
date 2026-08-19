//! The binder: [`FlatStore`] as the protocol-v4 engine names it.
//!
//! `FLAT_Store_Protocol.md` §2 is one seam written twice — here as the trait the store implements
//! ([`super::seam::Store`]), and in `obc-link` as the trait the engine declares
//! ([`obc_link::flat::Store`]) — because the dependency runs downward: `obc-link` is a
//! foundation crate and this one is a platform adapter, so the engine may not name the store and the
//! store therefore names the engine. This module is the whole of that cost: one `impl` block and two
//! total conversions, with the two definitions pinned to each other by the tests below.
//!
//! Nothing here decides anything. Every refusal is the store's own, every value is carried across
//! unchanged, and the one thing it adds is [`MAX_BATCH`](super::store::MAX_BATCH): a batch larger
//! than the store's plan arrays is refused rather than truncated.

use obc_link::flat as v4;

use super::device::BlockDevice;
use super::error::StoreError;
use super::seam::{
    Allocation, DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision, Store, StoreId,
};
use super::store::{FlatStore, Handle, Mode, MAX_BATCH};

/// The kind, as the wire names it. An exhaustive match rather than a decode: the two tables are the
/// same table (`FLAT_Store_Format.md` §3.1), and a match that stops compiling is how a divergence
/// announces itself.
fn kind_out(kind: ObjectKind) -> v4::ObjectKind {
    match kind {
        ObjectKind::Route => v4::ObjectKind::Route,
        ObjectKind::Trip => v4::ObjectKind::Trip,
        ObjectKind::Ride => v4::ObjectKind::Ride,
        ObjectKind::WeatherBundle => v4::ObjectKind::WeatherBundle,
        ObjectKind::MapShard => v4::ObjectKind::MapShard,
        ObjectKind::MapSetManifest => v4::ObjectKind::MapSetManifest,
        ObjectKind::UpdatePackage => v4::ObjectKind::UpdatePackage,
        ObjectKind::RollbackReserve => v4::ObjectKind::RollbackReserve,
    }
}

fn kind_in(kind: v4::ObjectKind) -> ObjectKind {
    match kind {
        v4::ObjectKind::Route => ObjectKind::Route,
        v4::ObjectKind::Trip => ObjectKind::Trip,
        v4::ObjectKind::Ride => ObjectKind::Ride,
        v4::ObjectKind::WeatherBundle => ObjectKind::WeatherBundle,
        v4::ObjectKind::MapShard => ObjectKind::MapShard,
        v4::ObjectKind::MapSetManifest => ObjectKind::MapSetManifest,
        v4::ObjectKind::UpdatePackage => ObjectKind::UpdatePackage,
        v4::ObjectKind::RollbackReserve => ObjectKind::RollbackReserve,
    }
}

fn flags_out(flags: EntryFlags) -> v4::EntryFlags {
    let mut out = v4::EntryFlags::NONE;
    for (mine, theirs) in [
        (EntryFlags::RECORDING, v4::EntryFlags::RECORDING),
        (EntryFlags::RETAINED, v4::EntryFlags::RETAINED),
        (EntryFlags::RESERVED, v4::EntryFlags::RESERVED),
    ] {
        if flags.has(mine) {
            out = out.with(theirs);
        }
    }
    out
}

fn flags_in(flags: v4::EntryFlags) -> EntryFlags {
    let mut out = EntryFlags::NONE;
    for (mine, theirs) in [
        (EntryFlags::RECORDING, v4::EntryFlags::RECORDING),
        (EntryFlags::RETAINED, v4::EntryFlags::RETAINED),
        (EntryFlags::RESERVED, v4::EntryFlags::RESERVED),
    ] {
        if flags.has(theirs) {
            out = EntryFlags::decode(out.bits() | mine.bits()).unwrap_or(out);
        }
    }
    out
}

fn meta_out(meta: EntryMeta) -> v4::EntryMeta {
    v4::EntryMeta {
        id: v4::ObjectId(meta.id.0),
        revision: v4::Revision(meta.revision.0),
        kind: kind_out(meta.kind),
        flags: flags_out(meta.flags),
        payload_len: meta.payload_len,
        payload_crc: meta.payload_crc,
        // The wire name takes any 48 bytes, and the card may hold bytes that are not UTF-8: the
        // store does not normalise a name and neither does this.
        name: v4::DisplayName::from_bytes(meta.name.as_bytes()).unwrap_or_default(),
    }
}

fn meta_in(meta: v4::EntryMeta) -> EntryMeta {
    EntryMeta {
        id: ObjectId(meta.id.0),
        revision: Revision(meta.revision.0),
        kind: kind_in(meta.kind),
        flags: flags_in(meta.flags),
        payload_len: meta.payload_len,
        payload_crc: meta.payload_crc,
        // A name reaching the store from the wire has already been checked: at most 48 bytes, pad
        // zero, and the UTF-8 the field says it is. The fallback is unreachable and is an empty
        // name rather than a panic, because this runs on the device.
        name: DisplayName::decode(meta.name.len() as u8, meta.name.padded()).unwrap_or_default(),
    }
}

fn mutation_in(mutation: &v4::Mutation<Allocation>) -> Mutation {
    match mutation {
        v4::Mutation::Put { meta, source } => Mutation::Put {
            meta: meta_in(*meta),
            source: match source {
                v4::PutSource::Fresh(allocation) => PutSource::Fresh(*allocation),
                v4::PutSource::Amend => PutSource::Amend,
            },
        },
        v4::Mutation::Remove { id, revision } => {
            Mutation::Remove { id: ObjectId(id.0), revision: Revision(revision.0) }
        }
    }
}

fn error_out(error: StoreError) -> v4::StoreError {
    match error {
        StoreError::NotFound => v4::StoreError::NotFound,
        StoreError::RevisionConflict { current } => {
            v4::StoreError::RevisionConflict { current: v4::Revision(current.0) }
        }
        StoreError::NoSpace { required } => v4::StoreError::NoSpace { required },
        StoreError::TooFragmented => v4::StoreError::TooFragmented,
        StoreError::CatalogFull => v4::StoreError::CatalogFull,
        StoreError::Invalid => v4::StoreError::Invalid,
        StoreError::Media => v4::StoreError::Media,
        StoreError::ReadOnly => v4::StoreError::ReadOnly,
        StoreError::Busy => v4::StoreError::Busy,
    }
}

fn mode_out(mode: Mode) -> v4::Mode {
    match mode {
        Mode::ReadWrite => v4::Mode::ReadWrite,
        Mode::RevisionSpaceExhausted => v4::Mode::RevisionSpaceExhausted,
        Mode::SequenceSpaceExhausted => v4::Mode::SequenceSpaceExhausted,
        Mode::CatalogUnreadable => v4::Mode::CatalogUnreadable,
        Mode::Unformatted => v4::Mode::Unformatted,
        Mode::CardTooSmall => v4::Mode::CardTooSmall,
    }
}

impl<D: BlockDevice> v4::Store for FlatStore<D> {
    type Allocation = Allocation;
    type Handle = Handle;

    fn mode(&self) -> v4::Mode {
        mode_out(FlatStore::mode(self))
    }

    fn store_id(&self) -> v4::StoreId {
        let StoreId(bytes) = FlatStore::store_id(self);
        v4::StoreId(bytes)
    }

    fn commit_sequence(&self) -> u64 {
        self.sequence()
    }

    fn next_object_id(&self) -> v4::ObjectId {
        v4::ObjectId(FlatStore::next_object_id(self).0)
    }

    fn allocate(&self, bytes: u64) -> Result<Allocation, v4::StoreError> {
        Store::allocate(self, bytes).map_err(error_out)
    }

    fn write(&self, allocation: &mut Allocation, bytes: &[u8]) -> Result<(), v4::StoreError> {
        Store::write(self, allocation, bytes).map_err(error_out)
    }

    fn cancel(&self, allocation: Allocation) {
        FlatStore::cancel(self, allocation);
    }

    fn commit(&self, mutations: &[v4::Mutation<Allocation>]) -> Result<u64, v4::StoreError> {
        if mutations.len() > MAX_BATCH {
            return Err(v4::StoreError::Invalid);
        }
        let mut batch: heapless::Vec<Mutation, MAX_BATCH> = heapless::Vec::new();
        for mutation in mutations {
            if batch.push(mutation_in(mutation)).is_err() {
                return Err(v4::StoreError::Invalid);
            }
        }
        Store::commit(self, &batch).map_err(error_out)
    }

    fn open(&self, id: v4::ObjectId, revision: Option<v4::Revision>) -> Result<Handle, v4::StoreError> {
        Store::open(self, ObjectId(id.0), revision.map(|revision| Revision(revision.0))).map_err(error_out)
    }

    fn read(&self, handle: &Handle, offset: u64, buf: &mut [u8]) -> Result<usize, v4::StoreError> {
        Store::read(self, handle, offset, buf).map_err(error_out)
    }

    fn close(&self, handle: Handle) {
        FlatStore::close(self, handle);
    }

    fn entries(&self) -> impl Iterator<Item = v4::EntryMeta> + '_ {
        Store::entries(self).map(meta_out)
    }

    fn entries_ok(&self) -> bool {
        FlatStore::entries_ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two documents carry the same numbers or one of them is wrong. This is where that is a
    /// fact: every kind and every flag bit, across the seam and back.
    #[test]
    fn the_wire_and_the_card_register_the_same_kinds_and_flags() {
        for value in 1..=8u16 {
            let mine = ObjectKind::decode(value).unwrap();
            let theirs = v4::ObjectKind::decode(value).unwrap();
            assert_eq!(kind_out(mine), theirs, "kind {value} crosses the seam unchanged");
            assert_eq!(kind_in(theirs), mine);
            assert_eq!(theirs.value(), mine as u16);
        }
        for bits in 0..=0b111u16 {
            let mine = EntryFlags::decode(bits).unwrap();
            let theirs = v4::EntryFlags::decode(bits).unwrap();
            assert_eq!(flags_out(mine).bits(), bits);
            assert_eq!(flags_in(theirs).bits(), bits);
        }
    }

    #[test]
    fn an_entry_crosses_the_seam_unchanged() {
        let meta = EntryMeta {
            id: ObjectId(7),
            revision: Revision(3),
            kind: ObjectKind::WeatherBundle,
            flags: EntryFlags::RETAINED,
            payload_len: 42_137,
            payload_crc: 0x9C4A_7E21,
            name: DisplayName::new("Grimsel Loop").unwrap(),
        };
        assert_eq!(meta_in(meta_out(meta)), meta);
        // A name the card holds that is not UTF-8 still lists: the store never normalised it and
        // this does not either.
        let mut field = [0u8; 48];
        field[0] = 0xFF;
        let raw = EntryMeta { name: DisplayName::decode(1, &field).unwrap(), ..meta };
        assert_eq!(meta_out(raw).name.as_bytes(), &[0xFF]);
    }

    #[test]
    fn every_seam_refusal_and_every_mode_has_one_wire_face() {
        assert_eq!(
            error_out(StoreError::RevisionConflict { current: Revision(5) }),
            v4::StoreError::RevisionConflict { current: v4::Revision(5) }
        );
        assert_eq!(error_out(StoreError::NoSpace { required: 9 }), v4::StoreError::NoSpace { required: 9 });
        assert_eq!(error_out(StoreError::Invalid), v4::StoreError::Invalid);
        // The one mapping the protocol names explicitly: a card smaller than its superblock is not
        // a flat store, and the wire says so with `readOnly`/`unformatted`.
        assert_eq!(mode_out(Mode::CardTooSmall), v4::Mode::CardTooSmall);
        assert!(!v4::Mode::CardTooSmall.readable());
        assert_eq!(mode_out(Mode::ReadWrite), v4::Mode::ReadWrite);
        assert!(v4::Mode::ReadWrite.writable());
    }
}
