//! Explicit age-based route cleanup. The caller supplies a confirmed cutoff and protects navigation.
use super::{BlockDevice, EntryFlags, FlatStore, Mutation, ObjectId, ObjectKind, Store, StoreError};

pub fn next<D: BlockDevice>(
    store: &FlatStore<D>,
    before_utc: u32,
    active: Option<ObjectId>,
) -> Result<Option<(ObjectId, heapless::Vec<Mutation, 2>)>, StoreError> {
    let head = store.entries().find(|e| {
        e.kind == ObjectKind::Route
            && e.flags == EntryFlags::NONE
            && Some(e.id) != active
            && e.added_at_utc != 0
            && e.added_at_utc < before_utc
    });
    if !store.entries_ok() {
        return Err(StoreError::Media);
    }
    let Some(head) = head else { return Ok(None) };
    let mut batch = heapless::Vec::new();
    for entry in store.entries().filter(|e| e.id == head.id) {
        batch.push(Mutation::Remove { id: entry.id, revision: entry.revision }).map_err(|_| StoreError::Invalid)?;
    }
    if !store.entries_ok() {
        return Err(StoreError::Media);
    }
    Ok(Some((head.id, batch)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat::{sim::SparseDisk, EntryMeta, PutSource, Revision, StoreId};

    fn route(store: &FlatStore<&SparseDisk>, utc: Option<u32>) -> EntryMeta {
        store.set_route_added_at(utc);
        let mut allocation = store.allocate(1).unwrap();
        store.write(&mut allocation, b"r").unwrap();
        let meta = EntryMeta {
            id: store.next_object_id(),
            revision: Revision(1),
            kind: ObjectKind::Route,
            flags: EntryFlags::NONE,
            payload_len: 1,
            payload_crc: obc_crc::crc32(b"r"),
            name: Default::default(),
            added_at_utc: 0,
        };
        store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).unwrap();
        store.entries().find(|e| e.id == meta.id).unwrap()
    }

    #[test]
    fn cleanup_scans_beyond_the_menu_and_keeps_active_unknown_and_recent_routes() {
        let disk = SparseDisk::blank(2_000_000, 1);
        let store = FlatStore::initialize(&disk, StoreId([3; 16])).unwrap();
        let active = route(&store, Some(10));
        for _ in 0..64 {
            route(&store, None);
        }
        let old = route(&store, Some(20));
        let recent = route(&store, Some(100));
        let future = route(&store, Some(200));
        let (id, batch) = next(&store, 100, Some(active.id)).unwrap().unwrap();
        assert_eq!(id, old.id);
        store.commit(&batch).unwrap();
        assert!(next(&store, 100, Some(active.id)).unwrap().is_none());
        assert!(store.entries().any(|e| e.id == recent.id));
        assert!(store.entries().any(|e| e.id == future.id));
    }

    #[test]
    fn upload_age_survives_remount_and_amendment() {
        let disk = SparseDisk::blank(200_000, 2);
        let store = FlatStore::initialize(&disk, StoreId([4; 16])).unwrap();
        let old = route(&store, Some(123));
        assert_eq!(old.added_at_utc, 123);
        store.set_route_added_at(Some(456));
        store
            .commit(&[Mutation::Put { meta: EntryMeta { added_at_utc: 0, ..old }, source: PutSource::Amend }])
            .unwrap();
        let reopened = FlatStore::mount(&disk);
        assert_eq!(reopened.entries().find(|e| e.id == old.id).unwrap().added_at_utc, 123);
    }
}
