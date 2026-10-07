use std::{cell::Cell, time::Instant};

use obc_formats::io::{ByteSink, ByteSource};
use obc_reader::{
    CorridorPoi, MapCache, MapTables, PoiCategorySet, Reader, RoutePath, SliceSource, MAX_CORRIDOR_RESULTS,
};

//
// The Up-ahead list's data source (`Reader::corridor_pois`) is an on-demand snapshot, not a frame
// stage, so it doesn't belong in the scene matrix — but it is the epic's named cost risk and its
// budget conversation has to happen on numbers. This mode reports the **deterministic** half exactly
// (source `read_at` calls + bytes: the device does one card read per call, so this is the SD-read
// count) plus host wall time. On-target wall clock is a different machine and stays a hardware
// measurement; the read count is the number that transfers.

/// A read-counting [`ByteSource`] wrapper — the SD-read proxy the corridor bench reports.
struct CountingSource<'a> {
    inner: SliceSource<'a>,
    reads: Cell<u32>,
    bytes: Cell<u64>,
}

impl<'a> CountingSource<'a> {
    fn new(bytes: &'a [u8]) -> CountingSource<'a> {
        CountingSource { inner: SliceSource(bytes), reads: Cell::new(0), bytes: Cell::new(0) }
    }
    fn take(&self) -> (u32, u64) {
        let out = (self.reads.get(), self.bytes.get());
        self.reads.set(0);
        self.bytes.set(0);
        out
    }
}

impl ByteSource for CountingSource<'_> {
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        self.reads.set(self.reads.get() + 1);
        self.bytes.set(self.bytes.get() + buf.len() as u64);
        self.inner.read_at(off, buf)
    }
    fn len(&self) -> u64 {
        self.inner.len()
    }
}

/// A `ByteSink` over a `Vec` — the GPX→OBCR conversion target.
#[derive(Default)]
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// The corridor fixture's map bbox and POI chunk size (the packer's §7.1 default).
const CORRIDOR_BBOX: (i32, i32, i32, i32) = (7_000_000, 47_000_000, 9_000_000, 49_000_000);
const CORRIDOR_POI_CHUNK: usize = 512;
/// Latitude the fixture route runs along; at 48° N one µdeg of longitude is ≈0.0745 m.
const CORRIDOR_LAT: i32 = 48_000_000;

/// A generally-eastbound `.obcr` of `points` track points spaced `step_udeg` apart — the "long
/// remaining route" half of the worst case.
///
/// The track **meanders** — a ±120 µdeg (≈±13 m) sine in latitude with a ~10-point (≈75 m) period —
/// rather than running dead straight. This is not decoration: the OBCR converter decimates on a 1 m
/// perpendicular tolerance, so a dead-straight GPX collapses to two points and **one** chunk, which
/// would quietly delete the route-length dimension this bench exists to measure. The chosen
/// curvature puts each point well past the tolerance (so the route spans many chunks, like a real
/// one) while adding only ~10 % to its length.
fn corridor_route(points: usize, step_udeg: i32) -> Vec<u8> {
    let mut gpx = String::from(r#"<?xml version="1.0"?><gpx version="1.1"><trk><trkseg>"#);
    for i in 0..points {
        let lon = 7_800_000 + step_udeg * i as i32;
        let lat = CORRIDOR_LAT + (120.0 * (i as f32 * 0.628).sin()) as i32;
        gpx.push_str(&format!(
            "<trkpt lat=\"{}.{:06}\" lon=\"{}.{:06}\"><ele>200.0</ele></trkpt>",
            lat / 1_000_000,
            lat % 1_000_000,
            lon / 1_000_000,
            lon % 1_000_000
        ));
    }
    gpx.push_str("</trkseg></trk></gpx>");
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "bench", &mut sink).expect("fixture route converts");
    sink.0
}

/// A POI-dense map: `per_cat` POIs of each of the six categories strung along the fixture route's
/// corridor, alternating side, plus the same number again just outside it (so the reject path is
/// exercised, not optimised away).
fn corridor_map(per_cat: usize, span_udeg: i32) -> Vec<u8> {
    // One representative subtype per category id 1..=6 (§7.4).
    const SUBTYPE: [u8; 6] = [1, 5, 7, 13, 17, 18];
    let mut cats: Vec<(u8, Vec<obcm_testkit::PoiSpec>)> = Vec::new();
    for (i, &subtype) in SUBTYPE.iter().enumerate() {
        let mut specs = Vec::with_capacity(per_cat * 2);
        for j in 0..per_cat {
            // Interleave the categories along the span (category `i` sits `i/6` of a slot in), so a
            // *thin* fixture still spreads its handful of POIs over the whole route instead of
            // bunching them at the start where one chunk would find them all.
            let slot = (j * 6 + i) as i32;
            let lon = 7_800_000 + (span_udeg as i64 * slot as i64 / (per_cat.max(1) * 6) as i64) as i32;
            // ±1500 µdeg ≈ ±167 m: inside the 300 m corridor, alternating side.
            let side = if j % 2 == 0 { 1_500 } else { -1_500 };
            specs.push(obcm_testkit::PoiSpec {
                lat: CORRIDOR_LAT + side,
                lon,
                subtype,
                name: format!("in{i}{j}"),
                payload: 0xFFFF,
            });
            // …and one well outside it, in the same quadtree leaves.
            specs.push(obcm_testkit::PoiSpec {
                lat: CORRIDOR_LAT + side * 5,
                lon,
                subtype,
                name: format!("out{i}{j}"),
                payload: 0xFFFF,
            });
        }
        cats.push((i as u8 + 1, specs));
    }
    obcm_testkit::build_poi_map(CORRIDOR_BBOX, CORRIDOR_POI_CHUNK, &cats)
}

/// One corridor-snapshot measurement.
pub(super) struct CorridorResult {
    pub(super) name: String,
    pub(super) results: usize,
    pub(super) map_reads: u32,
    pub(super) map_bytes: u64,
    pub(super) route_reads: u32,
    pub(super) route_bytes: u64,
    pub(super) us: u64,
}

/// Take one corridor snapshot over `(map, obcr)` and measure it. `warm` runs the query once first,
/// so the reported reads are a **warm-cache** snapshot (the realistic mid-ride case: the ride loop
/// has already streamed these route chunks); `warm == false` reports the cold first take.
#[allow(clippy::too_many_arguments)]
fn run_corridor(
    name: &str,
    map: &[u8],
    obcr: &[u8],
    cats: PoiCategorySet,
    progress_m: u32,
    warm: bool,
    iters: usize,
) -> CorridorResult {
    let map_src = CountingSource::new(map);
    let route_src = CountingSource::new(obcr);
    let tables = MapTables::parse(&map_src).expect("fixture map parses");
    let idx = obc_route::RouteIndex::read(&route_src).expect("fixture route parses");
    let cache = MapCache::new();
    let route_cache = obc_route::RouteCache::new();
    let route = obc_route::RouteReader::new_cached(&idx, &route_src, &route_cache);
    let reader = Reader::new(&map_src, &tables, &cache);
    let path: &dyn RoutePath = &route;
    let mut out = heapless::Vec::<CorridorPoi, MAX_CORRIDOR_RESULTS>::new();

    if warm {
        reader.corridor_pois(cats, path, progress_m, &mut out).expect("corridor query");
    }
    // Measure the *next* snapshot: counters zeroed, so parse/warm-up reads are excluded.
    let _ = map_src.take();
    let _ = route_src.take();
    let t0 = Instant::now();
    for _ in 0..iters {
        reader.corridor_pois(cats, path, progress_m, &mut out).expect("corridor query");
    }
    let us = (t0.elapsed().as_nanos() as u64) / (iters as u64 * 1000).max(1);
    let (map_reads, map_bytes) = map_src.take();
    let (route_reads, route_bytes) = route_src.take();
    CorridorResult {
        name: name.to_string(),
        results: out.len(),
        map_reads: map_reads / iters as u32,
        map_bytes: map_bytes / iters as u64,
        route_reads: route_reads / iters as u32,
        route_bytes: route_bytes / iters as u64,
        us,
    }
}

/// The corridor matrix. Two different worst cases matter and both are here:
///
/// - **dense** — Everything, a POI every ~25 m of corridor. The 16 slots fill in the first route
///   chunk and the walk stops there, so the cost is the *quadtree density*, not the route length.
/// - **thin** — one POI per category over the whole 15 km. The set never fills, so nothing can be
///   pruned and the walk pays for **every** remaining chunk × every category. This is the true
///   upper bound on a snapshot, and the number the budget conversation should use.
///
/// Around them, the shapes a rider actually meets: a single-category filter, and a snapshot taken
/// mid-ride (where the chunks already behind are skipped outright).
pub(super) fn run_corridor_matrix() -> (Vec<CorridorResult>, Vec<u8>) {
    let obcr = corridor_route(2_000, 100); // ≈15 km, ~2000 points
    let dense = corridor_map(100, 200_000); // 600 in-corridor POIs over the route's span
    let sparse = corridor_map(4, 200_000); // 24 in-corridor POIs
    let thin = corridor_map(1, 200_000); // 6 in-corridor POIs — the set never fills
    let water = PoiCategorySet::only(obc_reader::PoiCategory::Water);
    let results = vec![
        run_corridor("thin/all/cold", &thin, &obcr, PoiCategorySet::ALL, 0, false, 1),
        run_corridor("thin/all/full-walk", &thin, &obcr, PoiCategorySet::ALL, 0, true, 20),
        run_corridor("thin/water/full-walk", &thin, &obcr, water, 0, true, 20),
        run_corridor("dense/all/cold", &dense, &obcr, PoiCategorySet::ALL, 0, false, 1),
        run_corridor("dense/all/warm", &dense, &obcr, PoiCategorySet::ALL, 0, true, 20),
        run_corridor("dense/water/warm", &dense, &obcr, water, 0, true, 20),
        run_corridor("dense/all/mid-ride", &dense, &obcr, PoiCategorySet::ALL, 10_000, true, 20),
        run_corridor("sparse/all/warm", &sparse, &obcr, PoiCategorySet::ALL, 0, true, 20),
        run_corridor("sparse/water/warm", &sparse, &obcr, water, 0, true, 20),
    ];
    (results, obcr)
}

pub(super) fn print_corridor_table(results: &[CorridorResult], route: &[u8]) {
    let src = SliceSource(route);
    let idx = obc_route::RouteIndex::read(&src).expect("fixture route parses");
    println!("route-corridor POI snapshot (epic #946 U2) — one `Reader::corridor_pois` take");
    println!(
        "fixture route: {} points, {} OBCR chunks, {:.1} km\n",
        idx.point_count,
        idx.chunks().len(),
        idx.total_distance_m as f32 / 1000.0
    );
    println!(
        "{:<20} {:>4}  {:>9} {:>10}  {:>11} {:>12}  {:>8}",
        "case", "rows", "map reads", "map bytes", "route reads", "route bytes", "host"
    );
    for r in results {
        println!(
            "{:<20} {:>4}  {:>9} {:>10}  {:>11} {:>12}  {:>6}us",
            r.name, r.results, r.map_reads, r.map_bytes, r.route_reads, r.route_bytes, r.us
        );
    }
    println!(
        "\n`reads` are `ByteSource::read_at` calls — one card read each on the device. Host time is\n\
         an x86 figure over an in-RAM source; on-target wall clock needs hardware."
    );
}
