//! Bounded factory-reset deletion. Maps and firmware packages are system data.
use super::{store::MAX_BATCH, BlockDevice, FlatStore, Mutation, ObjectKind, Store, StoreError};

/// Remove metadata first so no checkpoint or archive proof outlives its personal objects.
pub fn next<D: BlockDevice>(store: &FlatStore<D>) -> Result<heapless::Vec<Mutation, MAX_BATCH>, StoreError> {
    let mut batch = heapless::Vec::new();
    for metadata in [true, false] {
        for entry in store.entries().filter(|entry| {
            if metadata {
                entry.kind == ObjectKind::Metadata
            } else {
                matches!(entry.kind, ObjectKind::Route | ObjectKind::Trip | ObjectKind::Ride)
            }
        }) {
            batch.push(Mutation::Remove { id: entry.id, revision: entry.revision }).map_err(|_| StoreError::Invalid)?;
            if batch.is_full() {
                break;
            }
        }
        if !store.entries_ok() {
            return Err(StoreError::Media);
        }
        if !batch.is_empty() {
            break;
        }
    }
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat::{sim::SparseDisk, EntryFlags, EntryMeta, PutSource, Revision, Store, StoreId};

    fn put(store: &FlatStore<&SparseDisk>, kind: ObjectKind, bytes: &[u8]) -> EntryMeta {
        let mut allocation = store.allocate(bytes.len() as u64).unwrap();
        store.write(&mut allocation, bytes).unwrap();
        let meta = EntryMeta {
            id: store.next_object_id(),
            revision: Revision(1),
            kind,
            flags: EntryFlags::NONE,
            payload_len: bytes.len() as u64,
            payload_crc: obc_crc::crc32(bytes),
            name: Default::default(),
            added_at_utc: 0,
        };
        store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).unwrap();
        meta
    }

    #[test]
    fn reset_removes_metadata_first_and_all_personal_objects_across_reboots() {
        let disk = SparseDisk::blank(2_000_000, 5);
        let identity = StoreId([7; 16]);
        let mut store = FlatStore::initialize(&disk, identity).unwrap();
        let system =
            [ObjectKind::MapShard, ObjectKind::MapSetManifest, ObjectKind::UpdatePackage, ObjectKind::RollbackReserve]
                .map(|kind| put(&store, kind, b"system data"));
        for i in 0..75 {
            put(&store, [ObjectKind::Route, ObjectKind::Trip, ObjectKind::Ride][i % 3], b"personal data");
        }
        let metadata = put(&store, ObjectKind::Metadata, b"personal metadata");
        let first = next(&store).unwrap();
        assert_eq!(first.len(), 1);
        assert!(matches!(first[0], Mutation::Remove { id, .. } if id == metadata.id));
        store.commit(&first).unwrap();
        loop {
            store = FlatStore::mount(&disk);
            let batch = next(&store).unwrap();
            if batch.is_empty() {
                break;
            }
            assert!(batch.len() <= MAX_BATCH);
            store.commit(&batch).unwrap();
        }
        assert_eq!(store.store_id(), identity);
        assert_eq!(store.entries().collect::<std::vec::Vec<_>>(), system);
        for meta in system {
            let handle = store.open(meta.id, Some(meta.revision)).unwrap();
            let mut bytes = [0; 11];
            assert_eq!(store.read(&handle, 0, &mut bytes).unwrap(), bytes.len());
            assert_eq!(&bytes, b"system data");
            store.close(handle);
        }
    }
}
