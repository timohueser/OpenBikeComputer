//! Merging the POI section and the hours pool.
//!
//! POI records carry absolute coordinates, so unlike geometry they cannot be relocated. They are
//! re-collected, de-duplicated, re-binned into fresh per-category quadtrees over the assembly bbox,
//! and their hours pool rebuilt with every `HoursRef` remapped.
//!
//! Records are 64 bytes and a whole country holds a few tens of thousands, so this is the cheap
//! rebuild. The expensive one is the nav graph next door.

use std::collections::BTreeMap;

use obc_formats::obcm::{
    poi_directory_category_of, settlement_class_of, CHUNK_END, POI_CATEGORY_COUNT, POI_CAT_ENTRY_LEN, POI_CHUNK_SIZE,
    POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE, POI_RECORD_LEN, SETTLEMENT_CATEGORY_ID, SUMMIT_CATEGORY_ID,
    SUMMIT_SUBTYPE_ID,
};

use crate::emit::{place, scaled, MapWriter};
use crate::grid::UBox;
use crate::input::Cell;
use crate::qtree::{self, Point};
use crate::{Error, Result};

/// Directory length: count byte + shared chunk size + one entry per category + the pool fields.
pub const POI_DIR_LEN: usize = 1 + 2 + POI_CATEGORY_COUNT as usize * POI_CAT_ENTRY_LEN + 4 + 2;

fn directory_len(categories: usize) -> usize {
    1 + 2 + categories * POI_CAT_ENTRY_LEN + 4 + 2
}

/// One merged record, with service hours references remapped to the shared pool.
pub struct MergedPoi {
    pub(crate) metadata: obc_formats::obcm::PoiMetadata,
    pub lat: i32,
    pub lon: i32,
    pub subtype: u8,
    /// Bytes 9..34 of the source record (`Name Len` plus the 24-byte name), copied verbatim so the
    /// assembler never re-folds a name.
    name: [u8; 25],
    /// Service hours reference, or the original signed summit height.
    payload: u16,
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
    /// Records dropped as duplicates. Only operator error can produce one, since each POI has
    /// exactly one cell, so a non-zero count is worth reporting.
    pub duplicates: usize,
}

impl MergedPois {
    /// Add landmark-only schedules and remap the service records before layout.
    pub(crate) fn add_hours(&mut self, schedules: impl Iterator<Item = [u8; POI_HOURS_BLOB_LEN]>) -> Result<()> {
        let mut pool = self.pool.clone();
        pool.extend(schedules);
        pool.sort_unstable();
        pool.dedup();
        if pool.len() >= POI_HOURS_REF_NONE as usize {
            return Err(Error::Capacity("shared hours pool exceeds the format limit".into()));
        }
        for poi in &mut self.pois {
            if has_hours(poi.subtype) && poi.payload != POI_HOURS_REF_NONE {
                poi.payload =
                    pool.binary_search(&self.pool[poi.payload as usize]).expect("existing service schedule") as u16;
            }
        }
        self.pool = pool;
        Ok(())
    }
}

/// Only a service place references the hours pool. A summit's payload holds an elevation and a
/// settlement's holds a population, so neither is remapped.
fn has_hours(subtype: u8) -> bool {
    subtype != SUMMIT_SUBTYPE_ID && settlement_class_of(subtype).is_none()
}

/// Collect every POI record from `cells`, deduplicate by `(lat, lon, subtype)`, and rebuild the
/// hours pool with `HoursRef` remapped.
///
/// Pool order is content-derived, by blob bytes ascending, rather than first-seen, so two
/// assemblies of the same cells produce the same bytes whatever order the cells arrived in.
pub fn merge(cells: &[&Cell<'_>]) -> Result<MergedPois> {
    /// One deduplicated record's payload: its name bytes and the hours blob it referenced.
    type Payload = ([u8; 25], Option<[u8; POI_HOURS_BLOB_LEN]>, u16, i32, i32, u8, obc_formats::obcm::PoiMetadata);
    // (lat, lon, subtype) → payload. A BTreeMap keeps the output ordered by the record key without
    // a separate sort.
    let mut by_key: BTreeMap<obc_formats::obcm::SourceId, Payload> = BTreeMap::new();
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
                let chunk = cell.read(data_start + (k * dir.chunk_size) as u64, dir.chunk_size)?;
                for rec in chunk.as_chunks::<POI_RECORD_LEN>().0 {
                    let subtype = rec[8];
                    if subtype == CHUNK_END {
                        break; // the §7.3 end-of-records sentinel
                    }
                    if poi_directory_category_of(subtype) != Some(entry.category_id) {
                        return Err(Error::Format(format!(
                            "cell {}: POI subtype {subtype} does not match its §7.4 category",
                            cell.id
                        )));
                    }
                    let lat = i32::from_le_bytes(rec[0..4].try_into().expect("4 bytes"));
                    let lon = i32::from_le_bytes(rec[4..8].try_into().expect("4 bytes"));
                    let hours_ref = u16::from_le_bytes(rec[34..36].try_into().expect("2 bytes"));
                    let blob = if !has_hours(subtype) || hours_ref == POI_HOURS_REF_NONE {
                        None
                    } else {
                        Some(*pool.get(hours_ref as usize).ok_or_else(|| {
                            Error::Format(format!("cell {}: HoursRef {hours_ref} is past its pool", cell.id))
                        })?)
                    };
                    let mut name = [0u8; 25];
                    name.copy_from_slice(&rec[9..34]);
                    let metadata = obc_formats::obcm::PoiMetadata::decode(&rec[36..64])
                        .ok_or_else(|| Error::Format(format!("cell {}: invalid POI metadata", cell.id)))?;
                    if by_key.insert(metadata.source, (name, blob, hours_ref, lat, lon, subtype, metadata)).is_some() {
                        duplicates += 1;
                    }
                }
            }
        }
    }

    // Rebuild the pool: distinct blobs, ordered by content.
    let mut pool_index: BTreeMap<[u8; POI_HOURS_BLOB_LEN], u16> = BTreeMap::new();
    for (_, blob, ..) in by_key.values() {
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
        .map(|(_, (name, blob, raw_payload, lat, lon, subtype, metadata))| MergedPoi {
            metadata,
            lat,
            lon,
            subtype,
            name,
            payload: if has_hours(subtype) {
                blob.map_or(POI_HOURS_REF_NONE, |b| pool_index[&b])
            } else {
                // A summit's elevation and a settlement's population travel unchanged.
                raw_payload
            },
        })
        .collect();
    Ok(MergedPois { pois, pool, duplicates })
}

/// Read one cell's hours pool.
pub(crate) fn read_hours_pool(cell: &Cell<'_>) -> Result<Vec<[u8; POI_HOURS_BLOB_LEN]>> {
    let count = cell.pois.hours_pool_count;
    if count == 0 {
        return Ok(Vec::new());
    }
    let bytes = cell.read(cell.pois.hours_pool_offset + 2, count * POI_HOURS_BLOB_LEN)?;
    Ok(bytes.as_chunks::<POI_HOURS_BLOB_LEN>().0.to_vec())
}

/// One category's laid-out block: its quadtree index and its padded chunks.
struct Block {
    cat_id: u8,
    index: Vec<u8>,
    node_count: u32,
    chunks: Vec<u8>,
    chunk_count: u32,
}

/// The POI section, already laid out — everything but the absolute offsets, so a shard's size is
/// known before its header is written.
pub struct PoiSection {
    blocks: Vec<Block>,
    pool: Vec<[u8; POI_HOURS_BLOB_LEN]>,
    len: usize,
    /// Records the chunk-capacity guard refused. Only co-located POIs past the quadtree's recursion
    /// floor can produce one, and dedup makes that effectively impossible — but a drop is data loss,
    /// so it is counted and surfaced rather than truncated into the chunk.
    dropped: usize,
}

impl PoiSection {
    /// Bytes this section occupies.
    pub fn section_len(&self) -> u64 {
        self.len as u64
    }

    /// Bytes added by the optional summit directory entry, index and chunks, including filler.
    pub fn summit_bytes(&self) -> u64 {
        let Some(block) = self.blocks.iter().find(|b| b.cat_id == SUMMIT_CATEGORY_ID) else { return 0 };
        let aligned = crate::emit::align_up;
        aligned(directory_len(self.blocks.len()) as u64) - aligned(directory_len(self.blocks.len() - 1) as u64)
            + aligned(block.index.len() as u64)
            + block.chunks.len() as u64
    }

    /// POI records the chunk-capacity guard dropped.
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

/// Bin the merged POIs into fresh per-category quadtrees over the assembly bbox and chunk them at
/// the directory's shared `Chunk Size`. Records inside a chunk come out ordered by the merge's own
/// `(lat, lon, subtype)` key, so the output is deterministic.
pub fn layout(merged: &MergedPois, global_bbox: UBox) -> Result<PoiSection> {
    let mut category_ids: Vec<_> = obc_formats::obcm::PoiCategory::ALL.iter().map(|c| c.id()).collect();
    if merged.pois.iter().any(|p| p.subtype == SUMMIT_SUBTYPE_ID) {
        category_ids.push(SUMMIT_CATEGORY_ID);
    }
    if merged.pois.iter().any(|p| settlement_class_of(p.subtype).is_some()) {
        category_ids.push(SETTLEMENT_CATEGORY_ID);
    }
    category_ids.sort_unstable();
    let category_count = category_ids.len();
    let mut by_cat: Vec<Vec<&MergedPoi>> = (0..=SETTLEMENT_CATEGORY_ID).map(|_| Vec::new()).collect();
    for p in &merged.pois {
        // Validated at merge time, so the category is known.
        let cat = poi_directory_category_of(p.subtype).expect("subtype validated at merge") as usize;
        by_cat[cat].push(p);
    }

    let capacity = POI_CHUNK_SIZE / POI_RECORD_LEN * POI_RECORD_LEN; // 8 records
    let mut blocks = Vec::with_capacity(category_count);
    let mut dropped = 0usize;
    for cat_id in category_ids {
        let pts = std::mem::take(&mut by_cat[cat_id as usize]);
        if pts.is_empty() {
            blocks.push(Block { cat_id, index: Vec::new(), node_count: 0, chunks: Vec::new(), chunk_count: 0 });
            continue;
        }
        let tree = qtree::build(pts, global_bbox, capacity);
        let (index, node_count, chunks, chunk_count, lost) =
            qtree::flatten(&tree, POI_CHUNK_SIZE, false, &|p, out| out.extend_from_slice(&pack_record(p)));
        dropped += lost;
        blocks.push(Block { cat_id, index, node_count, chunks, chunk_count });
    }
    if dropped != 0 {
        return Err(Error::Capacity(format!("{dropped} POIs exceed the spatial leaf capacity")));
    }
    let mut section = PoiSection { blocks, pool: merged.pool.clone(), len: 0, dropped };
    // The section's own length, measured by laying it out over a cursor that discards its bytes —
    // the same walk the write runs, so the size a shard is planned against and the size it turns
    // out to be cannot be two numbers.
    //
    // Byte `0` is as good a base as any: the section begins on a unit boundary wherever it lands,
    // every structure inside it is placed at the next boundary past the one before, and 512 is a
    // multiple of `U` at every legal scale — so no gap here depends on the absolute offset.
    section.len =
        place(0, |w| walk(&section, &vec![0u8; directory_len(section.blocks.len())], w).map(|_| w.at()))? as usize;
    Ok(section)
}

/// Write the section through `w`, wherever the cursor already is: directory, then each category's
/// index and chunks, then the hours pool, with `0xFF` filler wherever a scaled offset has to name
/// what comes next.
pub fn emit(section: &PoiSection, w: &mut MapWriter<'_>) -> Result<()> {
    let start = w.at();
    let dir = place(start, |p| walk(section, &vec![0u8; directory_len(section.blocks.len())], p))?.encode()?;
    walk(section, &dir, w)?;
    debug_assert_eq!(w.at() - start, section.section_len(), "the projection is the write");
    Ok(())
}

/// The directory's contents, as a walk of the payload behind it resolved them.
struct Directory {
    entries: Vec<(u8, u32, u32, u32)>,
    hours_pool_offset: u64,
    pool_blobs: usize,
}

impl Directory {
    fn encode(&self) -> Result<Vec<u8>> {
        let mut dir = Vec::with_capacity(directory_len(self.entries.len()));
        dir.push(self.entries.len() as u8);
        dir.extend_from_slice(&(POI_CHUNK_SIZE as u16).to_le_bytes());
        for &(cat_id, index_offset, node_count, chunk_count) in &self.entries {
            dir.push(cat_id);
            dir.extend_from_slice(&index_offset.to_le_bytes());
            dir.extend_from_slice(&node_count.to_le_bytes());
            dir.extend_from_slice(&chunk_count.to_le_bytes());
        }
        dir.extend_from_slice(&scaled(self.hours_pool_offset)?.to_le_bytes());
        dir.extend_from_slice(&(self.pool_blobs as u16).to_le_bytes());
        debug_assert_eq!(dir.len(), directory_len(self.entries.len()));
        Ok(dir)
    }
}

/// The one layout the POI section has, and the one place its boundaries are.
///
/// The directory states offsets into the payload behind it, so this runs twice: once over a cursor
/// that keeps nothing but its position, with a placeholder directory, to resolve them, and once
/// with the real directory to write. Two runs of one walk, so nothing is staged and no projection
/// can disagree with an emission.
///
/// Every category gets a directory entry, empty or not, and never a zero offset. An empty
/// category's `Index Offset` still points at where its zero-length index would start, so it is a
/// boundary too.
fn walk(section: &PoiSection, directory: &[u8], w: &mut MapWriter<'_>) -> Result<Directory> {
    debug_assert_eq!(directory.len(), directory_len(section.blocks.len()));
    debug_assert_eq!(w.at(), crate::emit::align_up(w.at()), "the POI section starts on a boundary");
    // The first category index starts on a unit boundary after the directory.
    w.put(directory)?;
    let mut entries = Vec::with_capacity(section.blocks.len());
    for b in &section.blocks {
        entries.push((b.cat_id, scaled(w.begin_section()?)?, b.node_count, b.chunk_count));
        w.put(&b.index)?;
        // The one rounding step: a category's chunks begin at
        // `align_up(Index Offset * U + Index Node Count * 4, U)`.
        w.begin_section()?;
        w.put(&b.chunks)?;
    }
    // The chunk runs are whole 512-byte strides from an aligned start, so the cursor is already a
    // boundary here and this gap is empty in practice. Written from the rule rather than from that
    // observation, because the rule is what the reader resolves.
    let hours_pool_offset = w.begin_section()?;
    w.put(&(section.pool.len() as u16).to_le_bytes())?;
    for blob in &section.pool {
        w.put(blob)?;
    }
    // The section ends on a unit boundary so the nav directory behind it can be named.
    w.begin_section()?;
    Ok(Directory { entries, hours_pool_offset, pool_blobs: section.pool.len() })
}

/// The 64-byte POI record. Name bytes travel verbatim from the source record; only `HoursRef` is
/// new.
fn pack_record(p: &MergedPoi) -> [u8; POI_RECORD_LEN] {
    let mut rec = [CHUNK_END; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&p.lat.to_le_bytes());
    rec[4..8].copy_from_slice(&p.lon.to_le_bytes());
    rec[8] = p.subtype;
    rec[9..34].copy_from_slice(&p.name);
    rec[34..36].copy_from_slice(&p.payload.to_le_bytes());
    rec[36..64].copy_from_slice(&p.metadata.encode());
    rec
}

/// The section a shard with no POIs writes: seven empty categories and an empty pool.
pub fn empty_layout(global_bbox: UBox) -> Result<PoiSection> {
    layout(&MergedPois { pois: Vec::new(), pool: Vec::new(), duplicates: 0 }, global_bbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The section as one `Vec` — the shape these pins want, which the writer itself does not have,
    /// because it streams into whatever cursor the map's write hands it.
    fn serialize(section: &PoiSection, at: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut sink = |b: &[u8]| -> Result<()> {
                bytes.extend_from_slice(b);
                Ok(())
            };
            emit(section, &mut MapWriter::new(crate::emit::SCALE, at as u64, &mut sink))
                .expect("the section serialises");
        }
        bytes
    }

    #[test]
    fn landmark_schedules_remap_services_without_changing_summit_heights() {
        let mut open = [0; POI_HOURS_BLOB_LEN];
        open[2] = 96;
        let poi = |subtype, payload| MergedPoi {
            metadata: Default::default(),
            lat: 100,
            lon: 100,
            subtype,
            name: [0; 25],
            payload,
        };
        let mut merged = MergedPois {
            pois: vec![poi(1, 0), poi(1, POI_HOURS_REF_NONE), poi(SUMMIT_SUBTYPE_ID, 1700)],
            pool: vec![open],
            duplicates: 0,
        };
        merged.add_hours([[0; POI_HOURS_BLOB_LEN], open].into_iter()).unwrap();
        assert_eq!(merged.pool.len(), 2);
        assert_eq!(merged.pois.iter().map(|p| p.payload).collect::<Vec<_>>(), vec![1, POI_HOURS_REF_NONE, 1700]);
        assert_eq!(merged.pool[merged.pois[0].payload as usize], open);
    }

    #[test]
    fn summit_overhead_equals_the_emitted_section_difference() {
        let mut pois: Vec<_> = (0..70)
            .map(|i| MergedPoi {
                metadata: Default::default(),
                lat: 10_000 + i * 10_000,
                lon: 20_000 + i * 11_000,
                subtype: SUMMIT_SUBTYPE_ID,
                name: [0; 25],
                payload: i as u16,
            })
            .collect();
        pois.push(MergedPoi {
            metadata: Default::default(),
            lat: 100,
            lon: 100,
            subtype: 1,
            name: [0; 25],
            payload: 0,
        });
        let mut merged = MergedPois { pois, pool: vec![[0; POI_HOURS_BLOB_LEN]], duplicates: 0 };
        let with_summits = layout(&merged, (0, 0, 1_000_000, 1_000_000)).unwrap();
        merged.pois.retain(|p| p.subtype != SUMMIT_SUBTYPE_ID);
        let services = layout(&merged, (0, 0, 1_000_000, 1_000_000)).unwrap();
        for at in [0, 96, 512] {
            assert_eq!(
                with_summits.summit_bytes(),
                (serialize(&with_summits, at).len() - serialize(&services, at).len()) as u64
            );
        }
        assert!(with_summits.summit_bytes() > POI_CHUNK_SIZE as u64, "the index must actually split");
        assert_eq!(services.summit_bytes(), 0);
    }

    /// The section a shard with no POIs writes, at a unit-aligned offset: the seven empty entries
    /// and the filler.
    ///
    /// The gap assertions are the point of the second half. Every directory field here would read
    /// correctly with the `0xFF` run written as zeros, or written one byte short and the hours pool
    /// one byte early — the offsets are self-consistent either way — so the pin names the fill byte
    /// and the run's exact length.
    #[test]
    fn an_empty_section_is_a_full_directory_with_its_filler() {
        const AT: usize = 96; // a unit boundary, which is all a section offset may be
        let bytes = serialize(&empty_layout((0, 0, 1_000_000, 1_000_000)).expect("an empty section lays out"), AT);
        assert_eq!(bytes[0], POI_CATEGORY_COUNT);
        assert_eq!(u16::from_le_bytes([bytes[1], bytes[2]]) as usize, POI_CHUNK_SIZE);
        let unit = crate::emit::SCALE.unit() as usize;
        // Every category present, every one empty, and every `Index Offset` on a unit boundary just
        // past the directory's own filler — an empty category still has to be nameable.
        let dir_end = crate::emit::align_up((AT + POI_DIR_LEN) as u64) as usize;
        for c in 0..POI_CATEGORY_COUNT as usize {
            let at = 3 + c * POI_CAT_ENTRY_LEN;
            assert_eq!(bytes[at], obc_formats::obcm::PoiCategory::ALL[c].id());
            let index_offset = u32::from_le_bytes(bytes[at + 1..at + 5].try_into().unwrap()) as usize;
            assert_eq!(index_offset * unit, dir_end, "category {} names the first boundary past the directory", c + 1);
            assert_eq!(u32::from_le_bytes(bytes[at + 5..at + 9].try_into().unwrap()), 0, "node count");
        }
        let pool_off = u32::from_le_bytes(bytes[POI_DIR_LEN - 6..POI_DIR_LEN - 2].try_into().unwrap()) as usize * unit;
        assert_eq!(pool_off, dir_end);
        assert_eq!(u16::from_le_bytes(bytes[POI_DIR_LEN - 2..POI_DIR_LEN].try_into().unwrap()), 0);

        // 87 bytes of directory, then filler to the boundary the offsets above name.
        assert_eq!(POI_DIR_LEN, 100, "the §7.1 directory is the width every gap here is measured from");
        let dir_gap = dir_end - AT - POI_DIR_LEN;
        assert_eq!(dir_gap, 12, "87 → 96 at U = 16");
        assert_eq!(&bytes[POI_DIR_LEN..POI_DIR_LEN + dir_gap], &[obc_formats::obcm::FILLER; 12], "§1.2's fill byte");
        // The pool's own two `count` bytes, then the run that leaves the nav directory nameable.
        assert_eq!(&bytes[dir_gap + POI_DIR_LEN..][..2], &0u16.to_le_bytes(), "an empty pool is a bare count");
        assert_eq!(&bytes[dir_gap + POI_DIR_LEN + 2..], &[obc_formats::obcm::FILLER; 14], "the tail run");
        assert_eq!(bytes.len(), 128, "96 (directory + filler) + 16 (the pool, rounded up)");
        assert_eq!(
            bytes.len() as u64,
            empty_layout((0, 0, 1_000_000, 1_000_000)).expect("an empty section lays out").section_len()
        );
    }
}
