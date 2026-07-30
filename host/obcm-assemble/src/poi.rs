//! Merging the POI section and the hours pool
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §4.5).
//!
//! POI records carry **absolute** coordinates (`OBCM_Spec.md` §7.3), so unlike geometry they cannot
//! be relocated — the theorem says nothing about them (§2.4). They are therefore re-collected,
//! de-duplicated, re-binned into fresh per-category quadtrees over the assembly bbox, and their
//! hours pool rebuilt with every `HoursRef` remapped.
//!
//! Records are 36 bytes and a whole country holds a few tens of thousands, so this is the cheap
//! rebuild. The expensive one is the nav graph next door.

use std::collections::BTreeMap;

use obc_formats::obcm::{
    poi_category_of, CHUNK_END, POI_CATEGORY_COUNT, POI_CAT_ENTRY_LEN, POI_CHUNK_SIZE, POI_HOURS_BLOB_LEN,
    POI_HOURS_REF_NONE, POI_RECORD_LEN,
};

use crate::grid::UBox;
use crate::input::Cell;
use crate::qtree::{self, Point};
use crate::{Error, Result};

/// Directory length: count byte + shared chunk size + one entry per category + the v7 pool fields.
pub const POI_DIR_LEN: usize = 1 + 2 + POI_CATEGORY_COUNT as usize * POI_CAT_ENTRY_LEN + 4 + 2;

/// One merged POI: the record's own bytes minus its `HoursRef`, which is remapped at write time.
pub struct MergedPoi {
    pub lat: i32,
    pub lon: i32,
    pub subtype: u8,
    /// Bytes 9..34 of the source record (`Name Len` + the 24-byte name), copied verbatim so the
    /// assembler never re-folds a name.
    name: [u8; 25],
    /// Index into the rebuilt pool, or [`POI_HOURS_REF_NONE`].
    hours_ref: u16,
}

impl Point for MergedPoi {
    fn lat(&self) -> i32 {
        self.lat
    }
    fn lon(&self) -> i32 {
        self.lon
    }
    fn record_len(&self) -> usize {
        POI_RECORD_LEN
    }
}

/// What the merge produced, before it is laid out.
pub struct MergedPois {
    pub pois: Vec<MergedPoi>,
    pub pool: Vec<[u8; POI_HOURS_BLOB_LEN]>,
    /// Records dropped as duplicates. Only operator error can produce one (§3.6 gives each POI
    /// exactly one cell), so a non-zero count is worth reporting.
    pub duplicates: usize,
}

/// Collect every POI record from `cells`, deduplicate by `(lat, lon, subtype)`, and rebuild the
/// hours pool with `HoursRef` remapped (§4.5.1–§4.5.3).
///
/// Pool order is content-derived (blob bytes ascending), not first-seen, so two assemblies of the
/// same cells produce the same bytes whatever order the cells arrived in.
pub fn merge(cells: &[&Cell<'_>]) -> Result<MergedPois> {
    /// One deduplicated record's payload: its name bytes and the hours blob it referenced.
    type Payload = ([u8; 25], Option<[u8; POI_HOURS_BLOB_LEN]>);
    // (lat, lon, subtype) → payload. A BTreeMap keeps the output ordered by the §4.5.5 key without a
    // separate sort.
    let mut by_key: BTreeMap<(i32, i32, u8), Payload> = BTreeMap::new();
    let mut duplicates = 0usize;

    for cell in cells {
        let dir = &cell.pois;
        let pool = read_hours_pool(cell)?;
        for entry in &dir.entries {
            if entry.chunk_count == 0 {
                continue;
            }
            let data_start = entry
                .data_start()
                .ok_or_else(|| Error::Format(format!("cell {}: POI directory overflows", cell.id)))?;
            for k in 0..entry.chunk_count {
                let chunk = cell.read(data_start + k * dir.chunk_size, dir.chunk_size)?;
                for rec in chunk.chunks_exact(POI_RECORD_LEN) {
                    let subtype = rec[8];
                    if subtype == CHUNK_END {
                        break; // the §7.3 end-of-records sentinel
                    }
                    if poi_category_of(subtype).is_none() {
                        return Err(Error::Format(format!(
                            "cell {}: POI subtype {subtype} is not in the §7.4 table",
                            cell.id
                        )));
                    }
                    let lat = i32::from_le_bytes(rec[0..4].try_into().expect("4 bytes"));
                    let lon = i32::from_le_bytes(rec[4..8].try_into().expect("4 bytes"));
                    let hours_ref = u16::from_le_bytes(rec[34..36].try_into().expect("2 bytes"));
                    let blob = if hours_ref == POI_HOURS_REF_NONE {
                        None
                    } else {
                        Some(*pool.get(hours_ref as usize).ok_or_else(|| {
                            Error::Format(format!("cell {}: HoursRef {hours_ref} is past its pool", cell.id))
                        })?)
                    };
                    let mut name = [0u8; 25];
                    name.copy_from_slice(&rec[9..34]);
                    if by_key.insert((lat, lon, subtype), (name, blob)).is_some() {
                        duplicates += 1;
                    }
                }
            }
        }
    }

    // Rebuild the pool: distinct blobs, ordered by content.
    let mut pool_index: BTreeMap<[u8; POI_HOURS_BLOB_LEN], u16> = BTreeMap::new();
    for (_, blob) in by_key.values() {
        if let Some(b) = blob {
            pool_index.entry(*b).or_insert(0);
        }
    }
    if pool_index.len() >= POI_HOURS_REF_NONE as usize {
        return Err(Error::Capacity(format!(
            "the merged hours pool holds {} distinct schedules; `HoursRef` is a uint16 with 0xFFFF reserved (§4.5)",
            pool_index.len()
        )));
    }
    let mut pool = Vec::with_capacity(pool_index.len());
    for (i, (blob, slot)) in pool_index.iter_mut().enumerate() {
        *slot = i as u16;
        pool.push(*blob);
    }

    let pois = by_key
        .into_iter()
        .map(|((lat, lon, subtype), (name, blob))| MergedPoi {
            lat,
            lon,
            subtype,
            name,
            hours_ref: blob.map_or(POI_HOURS_REF_NONE, |b| pool_index[&b]),
        })
        .collect();
    Ok(MergedPois { pois, pool, duplicates })
}

/// Read one cell's hours pool (`OBCM_Spec.md` §7.5).
fn read_hours_pool(cell: &Cell<'_>) -> Result<Vec<[u8; POI_HOURS_BLOB_LEN]>> {
    let count = cell.pois.hours_pool_count;
    if count == 0 {
        return Ok(Vec::new());
    }
    let bytes = cell.read(cell.pois.hours_pool_offset + 2, count * POI_HOURS_BLOB_LEN)?;
    Ok(bytes.chunks_exact(POI_HOURS_BLOB_LEN).map(|b| b.try_into().expect("blob width")).collect())
}

/// One category's laid-out block: its quadtree index and its padded chunks.
struct Block {
    cat_id: u8,
    index: Vec<u8>,
    node_count: u32,
    chunks: Vec<u8>,
    chunk_count: u32,
}

/// The §7 section, **already laid out** — everything but the absolute offsets, so a shard's size is
/// known before its header is written.
pub struct PoiSection {
    blocks: Vec<Block>,
    pool: Vec<[u8; POI_HOURS_BLOB_LEN]>,
    len: usize,
}

impl PoiSection {
    /// Bytes this section occupies.
    pub fn section_len(&self) -> usize {
        self.len
    }
}

/// Bin the merged POIs into fresh per-category quadtrees over the **assembly** bbox and chunk them
/// at the directory's shared `Chunk Size` (§4.5.4). Records inside a chunk come out ordered by
/// `(lat, lon, subtype)` — the merge's own key — so the output is deterministic (§4.5.5).
pub fn layout(merged: &MergedPois, global_bbox: UBox) -> PoiSection {
    let mut by_cat: Vec<Vec<&MergedPoi>> = (0..=POI_CATEGORY_COUNT as usize).map(|_| Vec::new()).collect();
    for p in &merged.pois {
        // Validated at merge time, so the category is known.
        let cat = poi_category_of(p.subtype).expect("subtype validated at merge").id() as usize;
        by_cat[cat].push(p);
    }

    let capacity = POI_CHUNK_SIZE / POI_RECORD_LEN * POI_RECORD_LEN; // 14 records
    let mut blocks = Vec::with_capacity(POI_CATEGORY_COUNT as usize);
    for cat_id in 1..=POI_CATEGORY_COUNT {
        let pts = std::mem::take(&mut by_cat[cat_id as usize]);
        if pts.is_empty() {
            blocks.push(Block { cat_id, index: Vec::new(), node_count: 0, chunks: Vec::new(), chunk_count: 0 });
            continue;
        }
        let tree = qtree::build(pts, global_bbox, capacity);
        let (index, node_count, chunks, chunk_count) = qtree::flatten(&tree, POI_CHUNK_SIZE, false, &|pts, out| {
            for p in pts {
                out.extend_from_slice(&pack_record(p));
            }
        });
        blocks.push(Block { cat_id, index, node_count, chunks, chunk_count });
    }
    let len = POI_DIR_LEN
        + blocks.iter().map(|b| b.index.len() + b.chunks.len()).sum::<usize>()
        + 2
        + merged.pool.len() * POI_HOURS_BLOB_LEN;
    PoiSection { blocks, pool: merged.pool.clone(), len }
}

/// Write the section at absolute byte `section_offset`: directory, then each category's index +
/// chunks, then the hours pool.
///
/// Every category gets a directory entry, empty or not — a map with no POIs writes six empty entries
/// and never a zero offset, which is also what a non-core shard writes (§5.1).
pub fn serialize(section: &PoiSection, section_offset: usize) -> Vec<u8> {
    let mut cursor = section_offset + POI_DIR_LEN;
    let mut payload = Vec::new();
    let mut entries = Vec::with_capacity(POI_CATEGORY_COUNT as usize);
    for b in &section.blocks {
        entries.push((b.cat_id, cursor as u32, b.node_count, b.chunk_count));
        payload.extend_from_slice(&b.index);
        payload.extend_from_slice(&b.chunks);
        cursor += b.index.len() + b.chunks.len();
    }
    let hours_pool_offset = cursor;
    payload.extend_from_slice(&(section.pool.len() as u16).to_le_bytes());
    for blob in &section.pool {
        payload.extend_from_slice(blob);
    }

    let mut out = Vec::with_capacity(section.len);
    out.push(POI_CATEGORY_COUNT);
    out.extend_from_slice(&(POI_CHUNK_SIZE as u16).to_le_bytes());
    for (cat_id, index_offset, node_count, chunk_count) in entries {
        out.push(cat_id);
        out.extend_from_slice(&index_offset.to_le_bytes());
        out.extend_from_slice(&node_count.to_le_bytes());
        out.extend_from_slice(&chunk_count.to_le_bytes());
    }
    out.extend_from_slice(&(hours_pool_offset as u32).to_le_bytes());
    out.extend_from_slice(&(section.pool.len() as u16).to_le_bytes());
    debug_assert_eq!(out.len(), POI_DIR_LEN);
    out.extend_from_slice(&payload);
    debug_assert_eq!(out.len(), section.len);
    out
}

/// The 36-byte §7.3 record. Name bytes travel verbatim from the source record; only `HoursRef` is
/// new.
fn pack_record(p: &MergedPoi) -> [u8; POI_RECORD_LEN] {
    let mut rec = [CHUNK_END; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&p.lat.to_le_bytes());
    rec[4..8].copy_from_slice(&p.lon.to_le_bytes());
    rec[8] = p.subtype;
    rec[9..34].copy_from_slice(&p.name);
    rec[34..36].copy_from_slice(&p.hours_ref.to_le_bytes());
    rec
}

/// The section a shard with no POIs writes: six empty categories and an empty pool (§5.1/§7.1).
pub fn empty_layout(global_bbox: UBox) -> PoiSection {
    layout(&MergedPois { pois: Vec::new(), pool: Vec::new(), duplicates: 0 }, global_bbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_section_is_a_full_directory() {
        let bytes = serialize(&empty_layout((0, 0, 1_000_000, 1_000_000)), 100);
        assert_eq!(bytes[0], POI_CATEGORY_COUNT);
        assert_eq!(u16::from_le_bytes([bytes[1], bytes[2]]) as usize, POI_CHUNK_SIZE);
        // Every category present, every one empty, and the pool offset just past the directory.
        for c in 0..POI_CATEGORY_COUNT as usize {
            let at = 3 + c * POI_CAT_ENTRY_LEN;
            assert_eq!(bytes[at], c as u8 + 1);
            assert_eq!(u32::from_le_bytes(bytes[at + 5..at + 9].try_into().unwrap()), 0, "node count");
        }
        let pool_off = u32::from_le_bytes(bytes[POI_DIR_LEN - 6..POI_DIR_LEN - 2].try_into().unwrap()) as usize;
        assert_eq!(pool_off, 100 + POI_DIR_LEN);
        assert_eq!(u16::from_le_bytes(bytes[POI_DIR_LEN - 2..POI_DIR_LEN].try_into().unwrap()), 0);
    }
}
