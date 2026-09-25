//! Development-only finished rides. Called before the board starts its storage writer.

use obc_crc::Crc32;
use obc_formats::io::{ByteSource, SliceSource};
use obc_formats::ride::{encode_footer, EffortLimits, Footer, FOOTER_LEN, SAMPLE_LEN};
use obc_formats::track::{decode_record, encode_record, FLAG_SEGMENT_START};
use obc_ports::TrackPoint;
use obc_route::RideInfo;
use obc_storage::flat::{
    Allocation, BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectId,
    ObjectKind, PutSource, Revision, Store, StoreError,
};

pub const NAMES: [&str; 3] = ["Demo GPS", "Demo Heart Rate", "Demo Heart Rate + Power"];
const INTERVALS: u32 = 360;
const STEP_SECONDS: u32 = 5;
const PAYLOAD_LEN: u64 = (INTERVALS as u64 + 1) * SAMPLE_LEN as u64 + FOOTER_LEN as u64;

// A synthetic loop near Freiburg, in microdegrees and metres. It is not a navigable route.
const LOOP: [(i32, i32, i16); 12] = [
    (7_820_000, 48_000_000, 230),
    (7_834_000, 48_008_000, 242),
    (7_850_000, 48_009_000, 265),
    (7_868_000, 48_005_000, 310),
    (7_880_000, 47_998_000, 285),
    (7_879_000, 47_990_000, 270),
    (7_864_000, 47_987_000, 295),
    (7_850_000, 47_985_000, 265),
    (7_833_000, 47_987_000, 250),
    (7_820_000, 47_990_000, 238),
    (7_815_000, 47_994_000, 225),
    (7_820_000, 48_000_000, 230),
];

fn point(index: u32, sensors: usize) -> TrackPoint {
    let scaled = index * (LOOP.len() as u32 - 1);
    let segment = (scaled / INTERVALS).min(LOOP.len() as u32 - 2);
    let fraction = (scaled - segment * INTERVALS) as i32;
    let a = LOOP[segment as usize];
    let b = LOOP[segment as usize + 1];
    let lerp = |a, b| a + (b - a) * fraction / INTERVALS as i32;
    let effort = (index % 80).min(80 - index % 80);
    TrackPoint {
        lon: lerp(a.0, b.0),
        lat: lerp(a.1, b.1),
        ele: lerp(i32::from(a.2), i32::from(b.2)) as i16,
        t_ms: index * STEP_SECONDS * 1000,
        segment_start: index == 0,
        hr: (sensors >= 1).then_some(120 + effort as u8),
        cadence: None,
        power: (sensors == 2).then_some(if index % 90 < 5 { 0 } else { 130 + effort as u16 * 3 }),
    }
}

/// Add missing demo names only. Never initialize a card, replace an object, or touch a live ride.
pub fn seed<D: BlockDevice>(store: &FlatStore<D>, first_start: u32) -> Result<[ObjectId; 3], StoreError> {
    check_store(store)?;
    if first_start == 0 || first_start.checked_add(2 * 86_400).is_none() {
        return Err(StoreError::Invalid);
    }
    let mut ids = [ObjectId(0); 3];
    let mut rides = 0;
    for entry in store.entries() {
        if entry.flags == EntryFlags::RECORDING {
            return Err(StoreError::Busy);
        }
        if entry.kind != ObjectKind::Ride || entry.flags != EntryFlags::NONE {
            continue;
        }
        rides += 1;
        if let Some(index) = NAMES.iter().position(|name| Some(*name) == entry.name.as_str()) {
            ids[index] = entry.id;
        }
    }
    if !store.entries_ok() {
        return Err(StoreError::Media);
    }
    upgrade_v5_rides(store)?;
    if rides + ids.iter().filter(|id| id.0 == 0).count() > obc_app::MAX_RIDES {
        return Err(StoreError::CatalogFull);
    }
    for (index, id) in ids.iter_mut().enumerate() {
        if id.0 == 0 {
            *id = write_ride(store, index, first_start + index as u32 * 86_400)?;
        }
    }
    Ok(ids)
}

/// Add one finished ride from validated bytes. An existing name must hold the same payload.
pub fn seed_file<D: BlockDevice>(store: &FlatStore<D>, bytes: &[u8]) -> Result<ObjectId, StoreError> {
    check_store(store)?;
    let info = RideInfo::read(&SliceSource(bytes)).map_err(|_| StoreError::Invalid)?;
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
    let name = DisplayName::new(info.name.as_str()).ok_or(StoreError::Invalid)?;
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
        // A v5 copy of this fixture is replaced in place; any other different payload is refused.
        if !payload_matches(store, entry.id, bytes)? {
            if !is_v5(store, &entry)? {
                return Err(StoreError::Invalid);
            }
            replace(store, &entry, |allocation| store.write(allocation, bytes).map(|_| ()), bytes.len() as u64)?;
        }
        upgrade_v5_rides(store)?;
        return if payload_matches(store, entry.id, bytes)? { Ok(entry.id) } else { Err(StoreError::Media) };
    }
    upgrade_v5_rides(store)?;
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
            added_at_utc: info.start_time,
        },
        source: PutSource::Fresh(allocation),
    }])?;
    if !payload_matches(store, id, bytes)? {
        return Err(StoreError::Media);
    }
    Ok(id)
}

/// The footer length of the previous ride-object version.
const V5_FOOTER_LEN: usize = 150;

/// Rewrite every finished v5 ride on the card as a v6 ride with no effort limits: the same object,
/// one revision on, with its samples unchanged. The v6 board rejects a v5 ride, and one unreadable
/// ride keeps the whole Rides menu from loading, so a development card is upgraded in place.
fn upgrade_v5_rides<D: BlockDevice>(store: &FlatStore<D>) -> Result<(), StoreError> {
    loop {
        let mut found = None;
        for entry in store.entries().filter(|e| e.kind == ObjectKind::Ride && e.flags == EntryFlags::NONE) {
            if is_v5(store, &entry)? {
                found = Some(entry);
                break;
            }
        }
        if !store.entries_ok() {
            return Err(StoreError::Media);
        }
        let Some(entry) = found else { return Ok(()) };
        let samples = entry.payload_len - V5_FOOTER_LEN as u64;
        let mut footer = [0; FOOTER_LEN];
        let handle = store.open(entry.id, Some(entry.revision))?;
        read_exact(store, &handle, samples, &mut footer[..V5_FOOTER_LEN])?;
        footer[4] = obc_formats::ride::VERSION;
        footer[6..8].copy_from_slice(&(FOOTER_LEN as u16).to_le_bytes());
        obc_formats::ride::decode_footer(&footer).map_err(|_| StoreError::Invalid)?;
        let copy = |allocation: &mut Allocation| {
            let mut buffer = [0; 500];
            let mut offset = 0;
            while offset < samples {
                let count = (samples - offset).min(buffer.len() as u64) as usize;
                read_exact(store, &handle, offset, &mut buffer[..count])?;
                store.write(allocation, &buffer[..count])?;
                offset += count as u64;
            }
            store.write(allocation, &footer).map(|_| ())
        };
        let replaced = replace(store, &entry, copy, samples + FOOTER_LEN as u64);
        store.close(handle);
        replaced?;
    }
}

/// Whether a finished ride ends in a v5 footer over whole samples.
fn is_v5<D: BlockDevice>(store: &FlatStore<D>, entry: &EntryMeta) -> Result<bool, StoreError> {
    let len = entry.payload_len;
    if len < V5_FOOTER_LEN as u64 || !(len - V5_FOOTER_LEN as u64).is_multiple_of(SAMPLE_LEN as u64) {
        return Ok(false);
    }
    let mut head = [0; 8];
    let handle = store.open(entry.id, Some(entry.revision))?;
    let read = read_exact(store, &handle, len - V5_FOOTER_LEN as u64, &mut head);
    store.close(handle);
    read?;
    Ok(head[..5] == *b"OBRF\x05" && u16::from_le_bytes([head[6], head[7]]) as usize == V5_FOOTER_LEN)
}

/// Replace `entry` with `len` bytes that `fill` writes: the same object and name, one revision on.
fn replace<D: BlockDevice>(
    store: &FlatStore<D>,
    entry: &EntryMeta,
    fill: impl FnOnce(&mut Allocation) -> Result<(), StoreError>,
    len: u64,
) -> Result<(), StoreError> {
    let mut allocation = store.allocate(len)?;
    fill(&mut allocation)?;
    let revision = Revision(entry.revision.0.checked_add(1).ok_or(StoreError::Invalid)?);
    let meta = EntryMeta { revision, payload_len: len, payload_crc: store.allocation_crc(&allocation)?, ..*entry };
    store
        .commit(&[
            Mutation::Remove { id: entry.id, revision: entry.revision },
            Mutation::Put { meta, source: PutSource::Fresh(allocation) },
        ])
        .map(|_| ())
}

fn read_exact<D: BlockDevice>(
    store: &FlatStore<D>,
    handle: &Handle,
    offset: u64,
    out: &mut [u8],
) -> Result<(), StoreError> {
    let mut done = 0;
    while done < out.len() {
        let count = store.read(handle, offset + done as u64, &mut out[done..])?;
        if count == 0 {
            return Err(StoreError::Media);
        }
        done += count;
    }
    Ok(())
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

fn write_ride<D: BlockDevice>(store: &FlatStore<D>, sensors: usize, start: u32) -> Result<ObjectId, StoreError> {
    let mut allocation = store.allocate(PAYLOAD_LEN)?;
    let mut crc = Crc32::new();
    let mut distance = 0.0;
    let (mut climb, mut descent, mut hr_sum, mut power_sum) = (0, 0, 0, 0);
    let (mut max_hr, mut max_power) = (0, 0);
    let mut previous = point(0, sensors);
    for index in 0..=INTERVALS {
        let sample = point(index, sensors);
        if index > 0 {
            distance += obc_map_scene::ground_dist_m((previous.lon, previous.lat), (sample.lon, sample.lat));
            let rise = i32::from(sample.ele) - i32::from(previous.ele);
            climb += rise.max(0) as u16;
            descent += (-rise).max(0) as u16;
        }
        if index < INTERVALS {
            hr_sum += u32::from(sample.hr.unwrap_or(0));
            power_sum += u32::from(sample.power.unwrap_or(0));
        }
        max_hr = max_hr.max(sample.hr.unwrap_or(0));
        max_power = max_power.max(sample.power.unwrap_or(0));
        previous = sample;
        let bytes = encode_record(&sample);
        store.write(&mut allocation, &bytes)?;
        crc.update(&bytes);
    }
    let duration = INTERVALS * STEP_SECONDS;
    let mut footer = Footer::new(
        NAMES[sensors],
        start,
        distance as u32,
        duration,
        (distance * 100.0 / duration as f32) as u16,
        climb,
        INTERVALS + 1,
        (sensors >= 1).then_some((hr_sum / INTERVALS) as u8),
        (sensors >= 1).then_some(max_hr),
        None,
        (sensors == 2).then_some((power_sum / INTERVALS) as u16),
        (sensors == 2).then_some(max_power),
    );
    footer.descent_m = descent;
    footer.energy_kj = (sensors == 2).then_some(power_sum * STEP_SECONDS / 1000);
    if sensors >= 1 {
        footer.limits = EffortLimits { max_hr: 185, ftp_w: 250 };
    }
    let bytes = encode_footer(&footer);
    store.write(&mut allocation, &bytes)?;
    crc.update(&bytes);
    let id = store.next_object_id();
    let payload_crc = crc.finalize();
    store.commit(&[Mutation::Put {
        meta: EntryMeta {
            id,
            revision: Revision(1),
            kind: ObjectKind::Ride,
            flags: EntryFlags::NONE,
            payload_len: PAYLOAD_LEN,
            payload_crc,
            name: DisplayName::new(NAMES[sensors]).ok_or(StoreError::Invalid)?,
            added_at_utc: start,
        },
        source: PutSource::Fresh(allocation),
    }])?;
    let handle = store.open(id, None)?;
    let mut check = Crc32::new();
    let mut buffer = [0; 512];
    let mut offset = 0;
    while offset < PAYLOAD_LEN {
        let count = store.read(&handle, offset, &mut buffer)?;
        if count == 0 {
            return Err(StoreError::Media);
        }
        check.update(&buffer[..count]);
        offset += count as u64;
    }
    store.close(handle);
    if check.finalize() != payload_crc {
        return Err(StoreError::Media);
    }
    Ok(id)
}
