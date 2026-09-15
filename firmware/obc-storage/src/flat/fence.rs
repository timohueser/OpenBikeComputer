//! A live card can report failure after the catalog publication became durable.

use core::cell::Cell;

use super::layout::{Geometry, EXTENT_AREA};
use super::sim::{DiskError, FaultOnce, MediaOp, SparseDisk};
use super::{
    Allocation, BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Mode, Mutation, ObjectId, ObjectKind,
    PutSource, Revision, RideCheckpoint, Store, StoreError, StoreId, RIDE_RESUME_LEN,
};

const STORE: StoreId = StoreId([0x71; 16]);

#[derive(Clone, Copy)]
enum Failure {
    GateWrite,
    GateSync,
}

struct PublishedError<'a> {
    disk: &'a SparseDisk,
    armed: Cell<Option<Failure>>,
    gate_written: Cell<bool>,
    fired: Cell<bool>,
}

impl BlockDevice for &PublishedError<'_> {
    type Error = DiskError;

    fn block_count(&self) -> Result<u64, DiskError> {
        self.disk.block_count()
    }
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        self.disk.read(lba, buf)
    }
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        self.disk.write(lba, buf)?;
        if self.armed.get().is_some() && buf.starts_with(b"FSCG") {
            self.gate_written.set(true);
            if matches!(self.armed.get(), Some(Failure::GateWrite)) {
                // A write is allowed to reach durable media before it reports failure.
                self.disk.sync()?;
                self.armed.set(None);
                self.fired.set(true);
                return Err(DiskError);
            }
        }
        Ok(())
    }
    fn sync(&self) -> Result<(), DiskError> {
        self.disk.sync()?;
        if self.gate_written.get() && matches!(self.armed.get(), Some(Failure::GateSync)) {
            self.armed.set(None);
            self.fired.set(true);
            return Err(DiskError);
        }
        Ok(())
    }
}

fn disk() -> SparseDisk {
    SparseDisk::blank(EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * 64, 7)
}

fn put<D: BlockDevice>(store: &FlatStore<D>, revision: u64, bytes: &[u8]) -> (Allocation, Mutation) {
    let mut allocation = store.allocate(bytes.len() as u64).unwrap();
    store.write(&mut allocation, bytes).unwrap();
    let meta = EntryMeta {
        added_at_utc: 0,
        id: ObjectId(1),
        revision: Revision(revision),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: bytes.len() as u64,
        payload_crc: obc_crc::crc32(bytes),
        name: DisplayName::default(),
    };
    (allocation, Mutation::Put { meta, source: PutSource::Fresh(allocation) })
}

#[test]
fn uncertain_publication_fences_every_mutation_and_preserves_pinned_reads() {
    for failure in [Failure::GateWrite, Failure::GateSync] {
        let disk = disk();
        let faulty = PublishedError {
            disk: &disk,
            armed: Cell::new(None),
            gate_written: Cell::new(false),
            fired: Cell::new(false),
        };
        let sequence = {
            let store = FlatStore::initialize(&faulty, STORE).unwrap();
            let (_, initial) = put(&store, 1, b"old");
            store.commit(&[initial]).unwrap();
            let old = store.open(ObjectId(1), Some(Revision(1))).unwrap();
            let clone = store.open(ObjectId(1), Some(Revision(1))).unwrap();
            let (mut allocation, replacement) = put(&store, 2, b"new");
            let mut pending_list = store.entries();
            let mut exhausted_list = store.entries();
            assert!(exhausted_list.next().is_some());
            assert!(exhausted_list.next().is_none());
            let sequence = store.sequence();
            let free = store.free_extents();
            faulty.armed.set(Some(failure));
            assert_eq!(
                store.commit(&[Mutation::Remove { id: ObjectId(1), revision: Revision(1) }, replacement]),
                Err(StoreError::Media)
            );
            assert!(faulty.fired.get());
            assert_eq!(store.mode(), Mode::RemountRequired);
            assert_eq!(store.sequence(), sequence);
            assert!(!store.has_commit_capacity(1));
            let ops = disk.ops();
            store.cancel(allocation);
            assert!(matches!(store.allocate(3), Err(StoreError::ReadOnly)));
            assert_eq!(store.write(&mut allocation, b"x"), Err(StoreError::ReadOnly));
            assert_eq!(store.patch_allocation(&allocation, 0, b"bad"), Err(StoreError::ReadOnly));
            assert_eq!(store.commit(&[]), Err(StoreError::ReadOnly));
            assert_eq!(
                store.journal(RideCheckpoint {
                    id: ObjectId(1),
                    revision: Revision(1),
                    append: b"x",
                    payload_crc: 0,
                    resume: &[0; RIDE_RESUME_LEN],
                }),
                Err(StoreError::ReadOnly)
            );
            assert_eq!(store.format_media(StoreId([0x99; 16])), Err(StoreError::ReadOnly));
            assert!(matches!(store.open(ObjectId(1), Some(Revision(1))), Err(StoreError::ReadOnly)));
            assert!(pending_list.next().is_none());
            assert!(exhausted_list.next().is_none());
            assert_eq!(store.entries().count(), 0);
            assert!(!store.entries_ok());
            store.close(clone);
            assert_eq!(disk.ops(), ops, "the fenced paths must not access media");
            assert_eq!(store.free_extents(), free);
            assert_eq!(store.allocation_crc(&allocation), Ok(obc_crc::crc32(b"new")), "cancel kept the reservation");
            let mut bytes = [0; 3];
            assert_eq!(store.read(&old, 0, &mut bytes), Ok(3));
            assert_eq!(&bytes, b"old");
            let ops = disk.ops();
            store.close(old);
            assert_eq!(disk.ops(), ops, "the last close must not reclaim from uncertain catalog authority");
            assert_eq!(store.free_extents(), free);
            sequence
        };
        disk.reboot();
        let mounted = FlatStore::mount(&disk);
        assert_eq!(mounted.mode(), Mode::ReadWrite);
        assert_eq!(mounted.store_id(), STORE);
        assert_eq!(mounted.sequence(), sequence + 1);
        let new = mounted.open(ObjectId(1), Some(Revision(2))).unwrap();
        let mut bytes = [0; 3];
        assert_eq!(mounted.read(&new, 0, &mut bytes), Ok(3));
        assert_eq!(&bytes, b"new");
        mounted.close(new);
    }
}

#[test]
fn uncertain_empty_catalog_cannot_authorize_default_metadata() {
    let disk = disk();
    let faulty =
        PublishedError { disk: &disk, armed: Cell::new(None), gate_written: Cell::new(false), fired: Cell::new(false) };
    let store = FlatStore::initialize(&faulty, STORE).unwrap();
    let mut empty = store.entries();
    assert!(empty.next().is_none());
    let (_, mutation) = put(&store, 1, b"new");
    faulty.armed.set(Some(Failure::GateSync));
    assert_eq!(store.commit(&[mutation]), Err(StoreError::Media));
    assert!(faulty.fired.get());
    assert!(empty.next().is_none());
    assert!(!store.entries_ok());
    assert_eq!(store.entries().count(), 0);
    assert!(!store.entries_ok());
    let mut metadata = super::metadata::Metadata::new(&store);
    let mut buffer = [0; super::metadata::MAX_LEN];
    assert!(metadata.load(&store, &mut buffer).is_err());
}

#[test]
fn prepublication_failure_allows_safe_retry_and_cancel() {
    let disk = disk();
    let faulty = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&faulty, STORE).unwrap();
    let (allocation, mutation) = put(&store, 1, b"new");
    faulty.fault_next(MediaOp::Sync);
    assert_eq!(store.commit(&[mutation]), Err(StoreError::Media));
    assert!(faulty.fired());
    assert_eq!(store.mode(), Mode::ReadWrite);
    store.cancel(allocation);
    let (_, mutation) = put(&store, 1, b"retry");
    store.commit(&[mutation]).unwrap();
    let handle = store.open(ObjectId(1), Some(Revision(1))).unwrap();
    let mut bytes = [0; 5];
    assert_eq!(store.read(&handle, 0, &mut bytes), Ok(5));
    assert_eq!(&bytes, b"retry");
    store.close(handle);
}
