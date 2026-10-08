use super::{BlockDevice, EntryFlags, FlatStore, ObjectId, ObjectKind, Revision, Store, StoreError};
use obc_formats::io::{ByteSource, Error};

/// The immutable revision selected for one catalog slot.
#[derive(Clone, Copy)]
pub struct Head {
    pub id: ObjectId,
    pub revision: Revision,
}

impl Head {
    pub const EMPTY: Self = Self { id: ObjectId::NONE, revision: Revision(0) };
}

type Decode<'a> = dyn FnMut(Head, bool, &dyn ByteSource) -> Result<(), Error> + 'a;

/// Read the newest bounded catalog into caller-owned staging. Route entries include only heads.
/// The accepted flag belongs to the selected revision. Decoder errors other than `Io` are omitted;
/// an `Io` or open failure aborts the scan. Callback values are tentative until the scan succeeds.
#[inline(never)]
pub fn scan<D: BlockDevice>(
    store: &FlatStore<D>,
    kind: ObjectKind,
    heads: &mut [Head],
    decode: &mut Decode<'_>,
) -> Result<(), StoreError> {
    if heads.len() > u64::BITS as usize {
        return Err(StoreError::Invalid);
    }
    let capacity = heads.len();
    let mut len = 0;
    for entry in store.entries() {
        let entry = entry?;
        if entry.kind != kind || (kind == ObjectKind::Route && !entry.flags.is_route_head()) {
            continue;
        }
        let at = heads[..len].iter().position(|head| entry.id > head.id).unwrap_or(len);
        if at < capacity {
            heads.copy_within(at..len.min(capacity - 1), at + 1);
            heads[at] = Head { id: entry.id, revision: entry.revision };
            len = (len + 1).min(capacity);
        }
    }
    let heads = &heads[..len];
    let mut accepted = 0u64;
    if kind == ObjectKind::Route {
        for meta in store.entries() {
            let meta = meta?;
            if !meta.flags.has(EntryFlags::ASSISTANT_ACCEPTED) {
                continue;
            }
            if let Some(index) = heads.iter().position(|head| head.id == meta.id && head.revision == meta.revision) {
                accepted |= 1 << index;
            }
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
