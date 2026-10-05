use super::{BlockDevice, EntryFlags, FlatStore, ObjectId, ObjectKind, Revision, Store, StoreError};
use obc_formats::io::{ByteSource, Error};

/// The immutable revision selected for one catalog slot.
#[derive(Clone, Copy)]
pub struct Head {
    pub id: ObjectId,
    pub revision: Revision,
}

type Decode<'a> = dyn FnMut(Head, bool, &dyn ByteSource) -> Result<(), Error> + 'a;

/// Read the newest bounded catalog into caller-owned staging. Route entries include only heads.
/// The accepted flag belongs to the selected revision. Malformed objects are omitted; media and
/// open failures abort the scan. Callback values are tentative until the scan succeeds.
#[inline(never)]
pub fn scan<D: BlockDevice, const N: usize>(
    store: &FlatStore<D>,
    kind: ObjectKind,
    heads: &mut heapless::Vec<Head, N>,
    decode: &mut Decode<'_>,
) -> Result<(), StoreError> {
    const { assert!(N <= 64) };
    heads.clear();
    for entry in
        store.entries().filter(|entry| entry.kind == kind && (kind != ObjectKind::Route || entry.flags.is_route_head()))
    {
        let at = heads.iter().position(|head| entry.id > head.id).unwrap_or(heads.len());
        if at < N {
            if heads.is_full() {
                let _ = heads.pop();
            }
            let _ = heads.insert(at, Head { id: entry.id, revision: entry.revision });
        }
    }
    if !store.entries_ok() {
        return Err(StoreError::Media);
    }
    let mut accepted = 0u64;
    if kind == ObjectKind::Route {
        for meta in store.entries().filter(|meta| meta.flags.has(EntryFlags::ASSISTANT_ACCEPTED)) {
            if let Some(index) = heads.iter().position(|head| head.id == meta.id && head.revision == meta.revision) {
                accepted |= 1 << index;
            }
        }
        if !store.entries_ok() {
            return Err(StoreError::Media);
        }
    }
    for (index, head) in heads.iter().copied().enumerate() {
        match store
            .with_source(head.id, Some(head.revision), |source| decode(head, accepted & (1 << index) != 0, source))
        {
            Ok(Err(Error::Io)) => return Err(StoreError::Media),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
