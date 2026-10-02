//! Factory-only demo ride writer. The factory image halts after this step.

use obc_formats::io::ByteSource;
use obc_formats::ride::{checked_object_len, decode_footer, FOOTER_LEN, SAMPLE_LEN};
use obc_formats::track::{decode_record, FLAG_SEGMENT_START};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Mode, Mutation, ObjectId, ObjectKind, PutSource,
    Revision, Store, StoreError,
};

/// Add one finished ride from validated bytes. An existing name must hold the same payload.
pub fn seed<D: BlockDevice>(store: &FlatStore<D>, bytes: &[u8]) -> Result<ObjectId, StoreError> {
    check_store(store)?;
    let footer = decode_footer(bytes.last_chunk().ok_or(StoreError::Invalid)?).map_err(|_| StoreError::Invalid)?;
    if !footer.is_demo
        || footer.start_time == 0
        || footer.point_count < 2
        || checked_object_len(footer.point_count).map_err(|_| StoreError::Invalid)? != bytes.len() as u64
    {
        return Err(StoreError::Invalid);
    }
    let mut last_time = None;
    for sample in bytes[..bytes.len() - FOOTER_LEN].as_chunks::<SAMPLE_LEN>().0 {
        let flags = u16::from_le_bytes([sample[10], sample[11]]);
        let point = decode_record(sample);
        if flags & !FLAG_SEGMENT_START != 0
            || !(-180_000_000..=180_000_000).contains(&point.lon)
            || !(-90_000_000..=90_000_000).contains(&point.lat)
            || last_time.is_some_and(|time| point.t_ms < time)
        {
            return Err(StoreError::Invalid);
        }
        last_time = Some(point.t_ms);
    }
    let name = DisplayName::new(footer.name()).ok_or(StoreError::Invalid)?;
    let mut rides = 0;
    let mut existing = None;
    for entry in store.entries() {
        if entry.flags == EntryFlags::RECORDING {
            return Err(StoreError::Busy);
        }
        if entry.kind == ObjectKind::Ride && entry.flags == EntryFlags::NONE {
            rides += 1;
            if entry.name == name {
                if existing.is_some() {
                    return Err(StoreError::Invalid);
                }
                existing = Some(entry);
            }
        }
    }
    if !store.entries_ok() {
        return Err(StoreError::Media);
    }
    if let Some(entry) = existing {
        return if payload_matches(store, entry.id, bytes)? { Ok(entry.id) } else { Err(StoreError::Invalid) };
    }
    if rides >= obc_app::MAX_RIDES {
        return Err(StoreError::CatalogFull);
    }

    let mut allocation = store.allocate(bytes.len() as u64)?;
    for chunk in bytes.chunks(512) {
        store.write(&mut allocation, chunk)?;
    }
    let id = store.next_object_id();
    store.commit(&[Mutation::Put {
        meta: EntryMeta {
            id,
            revision: Revision(1),
            kind: ObjectKind::Ride,
            flags: EntryFlags::NONE,
            payload_len: bytes.len() as u64,
            payload_crc: obc_crc::crc32(bytes),
            name,
            added_at_utc: footer.start_time,
        },
        source: PutSource::Fresh(allocation),
    }])?;
    if !payload_matches(store, id, bytes)? {
        return Err(StoreError::Media);
    }
    Ok(id)
}

fn check_store<D: BlockDevice>(store: &FlatStore<D>) -> Result<(), StoreError> {
    if store.mode() != Mode::ReadWrite {
        return Err(StoreError::ReadOnly);
    }
    if store.recovered_ride().is_some() {
        return Err(StoreError::Busy);
    }
    Ok(())
}

fn payload_matches<D: BlockDevice>(store: &FlatStore<D>, id: ObjectId, bytes: &[u8]) -> Result<bool, StoreError> {
    store
        .with_source(id, None, |source| -> Result<bool, obc_formats::io::Error> {
            if source.len() != bytes.len() as u64 {
                return Ok(false);
            }
            let mut buffer = [0; 512];
            for (index, chunk) in bytes.chunks(512).enumerate() {
                source.read_at((index * 512) as u64, &mut buffer[..chunk.len()])?;
                if &buffer[..chunk.len()] != chunk {
                    return Ok(false);
                }
            }
            Ok(true)
        })?
        .map_err(|_| StoreError::Media)
}
