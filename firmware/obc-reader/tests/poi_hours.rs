//! Contract tests for the device-side POI hours lookup (`Reader::poi_hours`, epic #439 P3 #443).
//!
//! Each test builds a synthetic v8 `.obcm` whose POI section carries a real hours pool (via
//! `obcm-testkit`'s pool + record builders, mirroring the packer's `serialize.rs`), then asserts the
//! reader resolves a POI's `hours_ref` back to the pooled [`WeeklySchedule`] — and that every
//! out-of-range / corrupt case yields `None`, never a panic. The `WeeklySchedule` eval semantics
//! (today/open-now, overnight/24h/closed) are unit-tested in `obc_reader::hours`; here we pin the
//! read + decode path end to end through the real file layout.

use obc_formats::obcm::{POI_HOURS_BLOB_LEN, POI_HOURS_FLAG_TRUNCATED as HOURS_FLAG_TRUNCATED};
use obc_reader::{Interval, MapCache, MapTables, PoiCategory, Reader, SliceSource, WeeklySchedule};
use obcm_testkit::{
    align_up, build_file, empty_nav_directory, filler_len, hours_pool, pack_poi_chunk, pack_poi_record, poi_dir_len,
    poi_directory, resolve_offset, scaled, seal, LodSpec, PoiCat, Style, FILLER,
};

const CS: usize = 64;
const GLOBAL: (i32, i32, i32, i32) = (0, 0, 100_000_000, 100_000_000);
const STYLES: &[Style] = &[(1, 3, 0xF800, 2, 3, false, None)];

/// A 29-byte pool blob from `flags` + per-day `(open_q, close_q)` slot pairs (Mon..Sun).
fn blob(flags: u8, days: [[(u8, u8); 2]; 7]) -> [u8; POI_HOURS_BLOB_LEN] {
    let mut b = [0u8; POI_HOURS_BLOB_LEN];
    b[0] = flags;
    let mut i = 1;
    for day in &days {
        for &(o, c) in day {
            b[i] = o;
            b[i + 1] = c;
            i += 2;
        }
    }
    b
}

fn iv(open_q: u8, close_q: u8) -> Interval {
    Interval { open_q, close_q }
}

/// Assemble a full v8 `.obcm` with one accommodation category (id 3) holding two POI records and a
/// hours pool of `blobs`. Records A/B reference `hours_ref` `ref_a`/`ref_b`. Mirrors the format
/// suite's `populated_poi_category_round_trips_with_record_layout` assembly.
fn build_map_with_pool(blobs: &[[u8; POI_HOURS_BLOB_LEN]], ref_a: u16, ref_b: u16) -> Vec<u8> {
    // Everything up to the POI section from build_file; then a populated directory + pool.
    let base = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![seal(vec![], CS)], chunk_size: CS }],
    );
    let poi_off = resolve_offset(&base, 32);

    // Two hotels (subtype 7) near the map centre so nearest_pois returns both.
    let rec_a = pack_poi_record(50_000_000, 50_000_000, 7, "Hotel A", ref_a);
    let rec_b = pack_poi_record(50_001_000, 50_001_000, 7, "Hotel B", ref_b);
    let chunk = pack_poi_chunk(&[rec_a, rec_b], 512);
    let pool = hours_pool(blobs);

    // v14: every offset the directory names is a unit boundary (§1.1), so the index sits at the
    // first one past the 87-byte directory and its chunks at `align_up(index + node_count * 4, U)`
    // — §7.1's one rounding step. The 512-byte chunk then leaves the cursor aligned for the pool.
    let dir_gap = filler_len(poi_off + poi_dir_len());
    let cat3_index_off = poi_off + poi_dir_len() + dir_gap;
    let cat3_chunk_off = align_up(cat3_index_off + 4); // one u32 node
    let index_gap = cat3_chunk_off - (cat3_index_off + 4);
    let pool_off = cat3_chunk_off + chunk.len();
    let cats: Vec<PoiCat> = (1..=6u8)
        .map(|id| {
            if id == 3 {
                PoiCat { category_id: 3, index_offset: cat3_index_off, node_count: 1, chunk_count: 1 }
            } else {
                PoiCat { category_id: id, index_offset: pool_off, node_count: 0, chunk_count: 0 }
            }
        })
        .collect();

    let mut bytes = base[..poi_off].to_vec();
    bytes.extend_from_slice(&poi_directory(512, &cats, pool_off, blobs.len() as u16));
    bytes.resize(bytes.len() + dir_gap, FILLER);
    bytes.extend_from_slice(&0u32.to_le_bytes()); // cat 3's single leaf → chunk 0
    bytes.resize(bytes.len() + index_gap, FILLER);
    bytes.extend_from_slice(&chunk);
    bytes.extend_from_slice(&pool);
    // The populated POI section displaced base's tail nav section: re-append an empty one at the
    // next unit boundary and patch the header's (scaled) nav offset at byte 36.
    bytes.resize(align_up(bytes.len()), FILLER);
    let nav_off = bytes.len();
    bytes[36..40].copy_from_slice(&scaled(nav_off).to_le_bytes());
    bytes.extend_from_slice(&empty_nav_directory(nav_off));
    bytes
}

/// Run a query + `poi_hours` over a built map; returns (the two POIs' hours_refs, the decoded
/// schedules by hours_ref lookup).
fn query_hotels(bytes: &[u8]) -> Vec<(u16, Option<WeeklySchedule>)> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = heapless::Vec::new();
    r.nearest_pois(PoiCategory::Accommodation, (50_000_500, 50_000_500), &mut out).unwrap();
    out.iter().map(|p| (p.hours_ref, r.poi_hours(p.hours_ref))).collect()
}

#[test]
fn poi_hours_resolves_pooled_schedules_end_to_end() {
    // Blob 0: Mon 08:00-18:00 (32,72), truncated flag. Blob 1: 24h Mon (0,96).
    let mut d0 = [[(0u8, 0u8); 2]; 7];
    d0[0][0] = (32, 72);
    let blob0 = blob(HOURS_FLAG_TRUNCATED, d0);
    let mut d1 = [[(0u8, 0u8); 2]; 7];
    d1[0][0] = (0, 96);
    let blob1 = blob(0, d1);

    // Record A → blob 0, record B → blob 1.
    let bytes = build_map_with_pool(&[blob0, blob1], 0, 1);
    let results = query_hotels(&bytes);
    assert_eq!(results.len(), 2, "both hotels returned");

    // Find each by its hours_ref (the query orders by distance, but the refs are distinct).
    let sched0 = results.iter().find(|(hr, _)| *hr == 0).and_then(|(_, s)| *s).expect("blob 0 resolves");
    assert!(sched0.is_truncated(), "blob 0 truncated flag");
    assert_eq!(sched0.today_intervals(0), &[iv(32, 72)], "Mon 08:00-18:00");
    assert!(sched0.is_open(0, 600) && !sched0.is_open(0, 1200), "open 10:00, closed 20:00");

    let sched1 = results.iter().find(|(hr, _)| *hr == 1).and_then(|(_, s)| *s).expect("blob 1 resolves");
    assert!(!sched1.is_truncated());
    assert_eq!(sched1.today_intervals(0), &[iv(0, 96)], "Mon 24h");
    assert!(sched1.is_open(0, 0) && sched1.is_open(0, 1439), "24h always open");
}

#[test]
fn poi_hours_sentinel_and_out_of_range_are_none() {
    let blob0 = blob(0, [[(32, 72), (0, 0)]; 7]);
    let bytes = build_map_with_pool(&[blob0], 0, 0xFFFF); // record B has no hours (0xFFFF)
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);

    // The pool has exactly one blob (count 1).
    assert_eq!(r.poi_directory().hours_pool_count, 1);
    // Valid index.
    assert!(r.poi_hours(0).is_some(), "index 0 resolves");
    // The no-hours sentinel.
    assert_eq!(r.poi_hours(0xFFFF), None, "0xFFFF ⇒ no hours");
    // One past the end (== count).
    assert_eq!(r.poi_hours(1), None, "hours_ref == count ⇒ None");
    // A huge index well past the pool.
    assert_eq!(r.poi_hours(50_000), None, "huge index ⇒ None");

    // Record B carried 0xFFFF into its Poi; poi_hours on it is None.
    let mut out = heapless::Vec::new();
    r.nearest_pois(PoiCategory::Accommodation, (50_000_500, 50_000_500), &mut out).unwrap();
    let no_hours = out.iter().find(|p| p.hours_ref == 0xFFFF).expect("record B has no hours");
    assert_eq!(r.poi_hours(no_hours.hours_ref), None, "0xFFFF Poi ⇒ None");
}

/// A `ByteSource` over an in-memory map that reports a *larger* `len()` than the real bytes and
/// fails any `read_at` reaching past the real end. This lets a corrupt-but-parseable map exist:
/// `parse_poi_directory` bounds the pool against the (inflated) `len()`, so parse succeeds, but the
/// per-blob read in `poi_hours` still hits the real end and must fail cleanly (`None`, never UB).
struct ShortBackedSource {
    bytes: Vec<u8>,
    reported_len: u32,
}

impl obc_reader::ByteSource for ShortBackedSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        let start = offset as usize;
        let end = start.checked_add(buf.len()).ok_or(obc_formats::io::Error::BadOffset)?;
        if end > self.bytes.len() {
            return Err(obc_formats::io::Error::BadOffset);
        }
        buf.copy_from_slice(&self.bytes[start..end]);
        Ok(())
    }
    fn len(&self) -> u32 {
        self.reported_len
    }
}

#[test]
fn poi_hours_corrupt_count_past_eof_is_none() {
    // A directory whose declared hours_pool_count runs past the real bytes but fits the source's
    // (inflated) reported len — so parse accepts it, yet the read for the missing blob must fail
    // cleanly. This exercises the `poi_hours` read bounds guard directly (parse's own pool-region
    // check can't catch a source that lies about its length).
    let blob0 = blob(0, [[(32, 72), (0, 0)]; 7]);
    let mut bytes = build_map_with_pool(&[blob0], 0, 0);

    let real_len = bytes.len();
    let poi_off = resolve_offset(&bytes, 32);
    let count_field = poi_off + 3 + 6 * 13 + 4;
    let off_field = poi_off + 3 + 6 * 13; // hours_pool_offset u32 (scaled)
    let pool_off = resolve_offset(&bytes, off_field);
    // Forge just enough blobs that the LAST one's read runs one blob past the real bytes (blob 0 is
    // still fully present). Derived from the real length so it's robust to the trailing nav section's
    // size (the §8.6 profile table pushed the file out in v9).
    let real_blobs = (real_len - pool_off - 2) / POI_HOURS_BLOB_LEN;
    let forged_count = (real_blobs + 2) as u16; // one more blob than the bytes actually hold
    bytes[count_field..count_field + 2].copy_from_slice(&forged_count.to_le_bytes());
    bytes[pool_off..pool_off + 2].copy_from_slice(&forged_count.to_le_bytes());

    // Report a len big enough that parse's pool-region bound (pool_off + 2 + count*29) fits, so parse
    // succeeds; the real bytes still stop before the last blob.
    let reported_len = (pool_off + 2 + forged_count as usize * POI_HOURS_BLOB_LEN + 16) as u32;
    let src = ShortBackedSource { bytes, reported_len };

    let tables = MapTables::parse(&src).expect("inflated len lets the corrupt pool parse");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    assert_eq!(r.poi_directory().hours_pool_count, forged_count as usize, "directory declares the forged blob count");
    // Blob 0 is fully present and resolves.
    assert!(r.poi_hours(0).is_some(), "in-file blob 0 resolves");
    // The last blob's read reaches past the real bytes ⇒ None, never a panic/UB.
    assert_eq!(r.poi_hours(forged_count - 1), None, "missing blob ⇒ read fails ⇒ None");
}

#[test]
fn weekly_schedule_decode_short_slice_is_none() {
    // A hand-made short slice (a truncated pool buffer) decodes to None — the corrupt-pool guard at
    // the decode boundary, independent of any file.
    let short = [0u8; POI_HOURS_BLOB_LEN - 1];
    assert_eq!(WeeklySchedule::decode(&short), None);
    // Exactly 29 bytes decodes.
    let ok = blob(0, [[(32, 72), (0, 0)]; 7]);
    assert!(WeeklySchedule::decode(&ok).is_some());
}
