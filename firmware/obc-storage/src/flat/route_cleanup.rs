//! Explicit age-based route cleanup. The caller supplies a confirmed cutoff and protects navigation.
use super::{
    store::MAX_BATCH, BlockDevice, EntryFlags, FlatStore, Mutation, ObjectId, ObjectKind, Revision, Store, StoreError,
};

/// The removals for the candidate route heads in `heads` that are still safe to remove: the exact
/// revision named is still the head, it is not flagged accepted, and the durable checkpoint does
/// not name it. The protected routes are loaded once and the catalog is walked once.
pub fn candidate_removals<D: BlockDevice>(
    store: &FlatStore<D>,
    heads: &[(ObjectId, Revision)],
) -> Result<heapless::Vec<Mutation, MAX_BATCH>, StoreError> {
    let protected = super::metadata::protected_routes(store).map_err(|error| match error {
        super::metadata::Error::Store(error) => error,
        super::metadata::Error::RemountRequired => StoreError::ReadOnly,
        _ => StoreError::Invalid,
    })?;
    let mut batch = heapless::Vec::new();
    for entry in store.entries().filter(|e| e.kind == ObjectKind::Route && e.flags.is_route_head()) {
        if heads.contains(&(entry.id, entry.revision))
            && !entry.flags.has(EntryFlags::ASSISTANT_ACCEPTED)
            && !protected.contains(&Some(entry.id))
        {
            batch.push(Mutation::Remove { id: entry.id, revision: entry.revision }).map_err(|_| StoreError::Invalid)?;
        }
    }
    if !store.entries_ok() {
        return Err(StoreError::Media);
    }
    Ok(batch)
}

pub fn next<D: BlockDevice>(
    store: &FlatStore<D>,
    before_utc: u32,
    active: Option<ObjectId>,
) -> Result<Option<(ObjectId, heapless::Vec<Mutation, 2>)>, StoreError> {
    let protected = super::metadata::protected_routes(store).map_err(|error| match error {
        super::metadata::Error::Store(error) => error,
        super::metadata::Error::RemountRequired => StoreError::ReadOnly,
        _ => StoreError::Invalid,
    })?;
    let head = store.entries().find(|e| {
        e.kind == ObjectKind::Route
            && e.flags.is_route_head()
            && !protected.contains(&Some(e.id))
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
    use crate::flat::{sim::SparseDisk, EntryFlags, EntryMeta, PutSource, Revision, StoreId};

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
    fn candidate_removals_skip_accepted_named_and_moved_heads() {
        use crate::flat::metadata::{fingerprint, write_checkpoint};
        let disk = SparseDisk::blank(200_000, 3);
        let card = StoreId([5; 16]);
        let store = FlatStore::initialize(&disk, card).unwrap();
        let plain = route(&store, None);
        let accepted = route(&store, None);
        let named = route(&store, None);
        let moved = route(&store, None);
        let checkpoint = obc_formats::assistant::NavigatorCheckpoint {
            route: fingerprint(accepted),
            original: Some(fingerprint(named)),
            progress_m: 0,
            occurrence: 0,
            lon: 0,
            lat: 0,
            phase: obc_formats::assistant::JourneyPhase::Following,
            unresolved_avoidance: false,
            selection: false,
            lower_m: 0,
            upper_m: 1,
        };
        write_checkpoint(&store, card, store.sequence(), None, Some(checkpoint)).unwrap();
        let heads =
            [(plain.id, Revision(1)), (accepted.id, Revision(1)), (named.id, Revision(1)), (moved.id, Revision(2))];
        let batch = candidate_removals(&store, &heads).unwrap();
        assert_eq!(batch.len(), 1);
        assert!(matches!(batch[0], Mutation::Remove { id, revision: Revision(1) } if id == plain.id));
        store.commit(&batch).unwrap();
        let heads = [(accepted.id, Revision(1)), (named.id, Revision(1)), (moved.id, Revision(1))];
        assert_eq!(candidate_removals(&store, &heads).unwrap().len(), 1, "the moved revision alone was the block");
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
