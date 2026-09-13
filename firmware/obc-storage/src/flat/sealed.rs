//! Sealed temporary bytes remain immutable while the other reservation writes.
use super::layout::{Geometry, EXTENT_AREA};
use super::sim::{DiskError, FaultOnce, MediaOp, SparseDisk};
use super::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision,
    SealedAllocation, Store, StoreError, StoreId,
};
use core::{cell::Cell, ptr::NonNull};
use obc_formats::io::{ByteSource, Error};
const ID: StoreId = StoreId([0x54; 16]);
fn disk() -> SparseDisk {
    SparseDisk::blank(EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * 64, 7)
}

#[test]
fn sealing_revokes_every_writable_copy_without_publishing_or_spending_a_hold() {
    let disk = disk();
    let store = FlatStore::initialize(&disk, ID).unwrap();
    let free = store.free_extents();
    let seq = store.sequence();
    let mut stale = store.allocate(2048).unwrap();
    let bytes = [0x73; 777];
    store.write(&mut stale, &bytes).unwrap();
    store.patch_allocation(&stale, 0, b"head").unwrap();
    let sealed = store.seal(stale).unwrap();
    let mut expected = bytes;
    expected[..4].copy_from_slice(b"head");
    let source = store.sealed_source(&sealed);
    let mut actual = [0; 777];
    source.read_at(0, &mut actual).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(source.read_at(777, &mut [0]), Err(Error::BadOffset));
    assert_eq!(store.write(&mut stale, b"bad"), Err(StoreError::Invalid));
    assert_eq!(store.patch_allocation(&stale, 0, b"bad"), Err(StoreError::Invalid));
    assert!(matches!(store.seal(stale), Err(StoreError::Invalid)));
    store.cancel(stale);
    let meta = EntryMeta {
        id: ObjectId(1),
        revision: Revision(1),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: stale.written_bytes(),
        payload_crc: 0,
        name: DisplayName::default(),
    };
    assert_eq!(store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(stale) }]), Err(StoreError::Invalid));
    let other = store.allocate(2048).unwrap();
    assert!(matches!(store.allocate(1), Err(StoreError::Invalid)));
    assert_eq!(store.sequence(), seq);
    assert_eq!(store.entries().count(), 0);
    source.read_at(0, &mut actual).unwrap();
    assert_eq!(actual, expected);
    store.release_sealed(sealed).unwrap();
    let reused = store.allocate(2048).unwrap();
    store.cancel(stale);
    assert!(matches!(store.allocate(1), Err(StoreError::Invalid)));
    store.cancel(reused);
    store.cancel(other);
    assert_eq!(store.free_extents(), free);
}

#[test]
fn failed_seal_keeps_cleanup_and_fenced_seals_cannot_free_uncertain_space() {
    let disk = disk();
    let fault = FaultOnce::new(&disk);
    let store = FlatStore::initialize(&fault, ID).unwrap();
    let free = store.free_extents();
    let mut allocation = store.allocate(1024).unwrap();
    store.write(&mut allocation, b"tail").unwrap();
    fault.fault_next(MediaOp::Write);
    assert!(matches!(store.seal(allocation), Err(StoreError::Media)));
    assert!(fault.fired());
    store.cancel(allocation);
    assert_eq!(store.free_extents(), free);
    let mut allocation = store.allocate(1024).unwrap();
    store.write(&mut allocation, b"tail").unwrap();
    let sealed = store.seal(allocation).unwrap();
    let other_disk = self::disk();
    let other = FlatStore::initialize(&other_disk, ID).unwrap();
    assert_eq!(other.sealed_source(&sealed).read_at(0, &mut [0; 4]), Err(Error::Io));
    let sealed = other.release_sealed(sealed).expect_err("wrong mount returns the cleanup owner");
    store.release_sealed(sealed).unwrap();
    let mut allocation = store.allocate(1024).unwrap();
    store.write(&mut allocation, b"tail").unwrap();
    fault.fault_next(MediaOp::Write);
    assert!(matches!(store.seal(allocation), Err(StoreError::Media)));
    let sealed = store.seal(allocation).unwrap();
    let mut retried = [0; 4];
    store.sealed_source(&sealed).read_at(0, &mut retried).unwrap();
    assert_eq!(&retried, b"tail");
    store.require_remount();
    let before = disk.ops();
    assert!(matches!(store.seal(allocation), Err(StoreError::ReadOnly)));
    let occupied = store.free_extents();
    store.release_sealed(sealed).unwrap();
    assert_eq!(store.free_extents(), occupied);
    assert_eq!(disk.ops(), before);
}

struct ReadDuringWrite {
    disk: SparseDisk,
    store: Cell<Option<NonNull<FlatStore<&'static ReadDuringWrite>>>>,
    sealed: Cell<Option<NonNull<SealedAllocation<'static>>>>,
    reads: Cell<usize>,
}
impl BlockDevice for &ReadDuringWrite {
    type Error = DiskError;
    fn block_count(&self) -> Result<u64, DiskError> {
        (&self.disk).block_count()
    }
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        (&self.disk).read(lba, buf)
    }
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        if let (Some(store), Some(sealed)) = (self.store.get(), self.sealed.get()) {
            // Test-only reentry: armed after both owners are stationary and disarmed before either drops.
            let (store, sealed) = unsafe { (store.as_ref(), sealed.as_ref()) };
            let mut bytes = [0; 777];
            store.sealed_source(sealed).read_at(0, &mut bytes).unwrap();
            assert_eq!(bytes, [0x62; 777]);
            self.reads.set(self.reads.get() + 1);
        }
        (&self.disk).write(lba, buf)
    }
    fn sync(&self) -> Result<(), DiskError> {
        (&self.disk).sync()
    }
}
#[test]
fn sealed_reads_need_no_reservation_borrow_while_other_output_flushes() {
    let card = ReadDuringWrite { disk: disk(), store: Cell::new(None), sealed: Cell::new(None), reads: Cell::new(0) };
    let store = FlatStore::initialize(&card, ID).unwrap();
    let mut a = store.allocate(2048).unwrap();
    store.write(&mut a, &[0x62; 777]).unwrap();
    let sealed = store.seal(a).unwrap();
    // Raw test probes erase only lifetimes; no owner moves while the card is armed.
    card.store.set(Some(NonNull::from(&store).cast()));
    card.sealed.set(Some(NonNull::from(&sealed).cast()));
    let mut b = store.allocate(2048).unwrap();
    store.write(&mut b, &[0x31; 1001]).unwrap();
    let sealed_b = store.seal(b).unwrap();
    assert_eq!(card.reads.get(), 2);
    card.store.set(None);
    card.sealed.set(None);
    store.release_sealed(sealed_b).unwrap();
    store.release_sealed(sealed).unwrap();
}
