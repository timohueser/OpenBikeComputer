//! Host render benchmark harness + pixel-hash and read-counter golden gate (issues #327, #1467).
//!
//! Renders a fixed 7-scene matrix through the **real pipeline** — `obcm-testkit`'s deterministic
//! fixture → `SliceSource` → `MapTables`/`MapCache`/`Reader` → `RenderScratch::render_timed` → the
//! device-resolution [`Framebuffer565`] — and prints per-stage timings (min of 10 after a warm-up),
//! the [`RenderStats`] counters, and an FNV-1a 64 hash of the frame's pixels.
//!
//! Two jobs, one binary:
//! - **Benchmark** (the epic's measuring instrument): the timings are the before/after numbers every
//!   #329 optimization lands with. Printed, never gated — shared CI runners are noisy.
//! - **Tripwire** (`--check`): both the frame hashes *and* the map read path's per-case read
//!   counters are deterministic (seeded fixture, integer/`libm` math, fixed cache policy), so CI
//!   compares them against the committed `golden.txt` and fails on any drift. Pixels catch a
//!   rendering change; the counters catch a cache change that halves the hit rate while every pixel
//!   stays identical (epic #1402 §2.5). A pure refactor must touch neither; an intentional change
//!   regenerates the file with `--write-golden` in the same PR — that is the review signal.
//!
//! `--check`/`--write-golden` cover **two** matrices against one file: the 7 render scenes and the
//! 9 route-corridor snapshot cases, the latter under `corridor/` names.
//!
//! Modes: default (print the table), `--repeat <N>` (repeat the whole matrix and report
//! min/median/max), `--write-golden <file>`, `--check <file>` (exit 1 on mismatch), `--corridor`
//! (print the corridor matrix alone), and `--map <path> --mpp <f> --heading <deg>` — a manual
//! escape hatch to run one scene against a real local `.obcm` (never in CI: real maps aren't
//! byte-stable fixtures).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Instant;

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use obc_display::Framebuffer565;
use obc_formats::io::{ByteSink, ByteSource};
use obc_map_scene::{ground_dist_m, BBox};
use obc_reader::{
    CorridorPoi, MapCache, MapTables, PoiCategorySet, Reader, RoutePath, SliceSource, MAX_CORRIDOR_RESULTS,
};
use obc_render::{
    zoom_for_mpp, Clock, OverlayChunk, RenderConfig, RenderScratch, RenderStats, RouteOverlaySource, Viewport,
};

/// Device resolution — the LS021B7DD02 panel the shipping firmware renders at. The single
/// [`obc_display`] frame authority, not a re-declared literal.
const WIDTH: u32 = obc_display::ls021::FRAME_W as u32;
const HEIGHT: u32 = obc_display::ls021::FRAME_H as u32;

/// Timed iterations per scene (after one warm-up render that fills the chunk cache). The report is
/// the **min** of each stage — the noise-floor estimator for a deterministic workload.
const ITERS: usize = 10;

/// The fixed scene matrix: `(name, meters-per-pixel, heading°)`. Rides the fixture's two LODs
/// (riding = fine, mid/overview = coarse) both north-up and rotated; the overview pair must
/// saturate the frame budget (`features_dropped > 0`) or the fixture has gone stale. What fills
/// first there is the **span** buffer: screen-point packing let us raise the ring ceiling above the
/// fixture's one-ring-per-feature span ceiling.
const SCENES: [(&str, f32, f32); 6] = [
    ("riding", 0.5, 0.0),
    ("riding-rot", 0.5, 35.0),
    ("mid", 4.0, 0.0),
    ("mid-rot", 4.0, 35.0),
    ("overview", 30.0, 0.0),
    ("overview-rot", 30.0, 35.0),
];

/// [`Clock`] over [`std::time::Instant`]: µs since construction, threaded through `render_timed`
/// so `collect_us`/`sort_us`/`draw_us` are real host wall time.
struct StdClock(Instant);

impl Clock for StdClock {
    fn now_us(&self) -> u64 {
        self.0.elapsed().as_micros() as u64
    }
}

/// FNV-1a 64-bit over the framebuffer's pixels (each `u16` folded little-endian, row-major) — the
/// stable frame fingerprint the tripwire compares. Inline per the offset-basis/prime constants;
/// byte order is fixed explicitly so the hash never depends on host endianness.
fn frame_hash(buf: &[u16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for px in buf {
        for b in px.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// One scene's report: the min-of-[`ITERS`] stage timings, the last iteration's counters, and the
/// frame hash.
struct SceneResult {
    name: String,
    collect_us: u32,
    sort_us: u32,
    draw_us: u32,
    total_us: u64,
    stats: RenderStats,
    hash: u64,
}

/// Render one scene through the steady-state device flow: parse-once tables, one `MapCache` reused
/// across iterations, camera at the map's bbox center. Warm-up once (fills the chunk cache), then
/// time [`ITERS`] renders and keep the min of each stage; counters come from the last iteration and
/// the hash from the final frame.
fn run_scene(map: &[u8], name: &str, mpp: f32, heading_deg: f32, clock: &StdClock) -> SceneResult {
    let src = SliceSource(map);
    let tables = MapTables::parse(&src).expect("bench map must parse");
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let mut scratch = RenderScratch::new();
    let mut buf = vec![0u16; (WIDTH * HEIGHT) as usize];

    // Clear color + pixel policy: the backdrop style's color, mapped 1:1 to RGB565 — the host
    // true-color path (the device would quantize to its RGB222 gamut instead).
    let bg = Rgb565::from(RawU16::new(reader.backdrop_style().map(|s| s.color).unwrap_or(0xFFFF)));
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

    let cx = (tables.bbox.min_lon + tables.bbox.max_lon) / 2;
    let cy = (tables.bbox.min_lat + tables.bbox.max_lat) / 2;
    let vp = Viewport::new_rotated(WIDTH as f32, HEIGHT as f32, cx, cy, zoom_for_mpp(mpp), heading_deg.to_radians());

    // Warm-up: fills the chunk cache, so the timed iterations measure the steady state the device
    // sees (a slow pan re-hits last frame's chunks), not the cold SD-fill.
    let mut fb = Framebuffer565::new(&mut buf, WIDTH, HEIGHT);
    scratch.render_timed(&mut fb, &reader, &vp, bg, RenderConfig::default(), color_fn, clock);

    let (mut collect_us, mut sort_us, mut draw_us, mut total_us) = (u32::MAX, u32::MAX, u32::MAX, u64::MAX);
    let mut stats = RenderStats::default();
    for _ in 0..ITERS {
        let mut fb = Framebuffer565::new(&mut buf, WIDTH, HEIGHT);
        let t0 = clock.now_us();
        stats = scratch.render_timed(&mut fb, &reader, &vp, bg, RenderConfig::default(), color_fn, clock);
        total_us = total_us.min(clock.now_us() - t0);
        collect_us = collect_us.min(stats.collect_us);
        sort_us = sort_us.min(stats.sort_us);
        draw_us = draw_us.min(stats.draw_us);
    }

    SceneResult { name: name.into(), collect_us, sort_us, draw_us, total_us, stats, hash: frame_hash(&buf) }
}

/// The `route` scene's polyline, as two chunks of `(Δlon, Δlat)` microdegree offsets from the
/// fixture's bbox center (chunk 1 repeats chunk 0's last vertex — the seam, exactly as the OBCR
/// reader hands chunks over). A zigzag sized to cross the mid-zoom view, so the stroke, the view
/// clip and both chevron-window bounds are all exercised.
const ROUTE_DELTAS: [&[(i32, i32)]; 2] =
    [&[(-9000, -8000), (-4000, -2500), (-1500, -4000), (0, 0)], &[(0, 0), (1500, 3000), (4500, 2000), (8000, 8000)]];

/// A static, deterministic [`RouteOverlaySource`] over the bench fixture — the seam is trivially
/// fakeable, so the hash tripwire covers the route overlay (stroke + chevrons) with no OBCR file.
struct StaticRoute {
    /// Absolute `(lon, lat)` microdegree points per chunk.
    chunks: Vec<Vec<(i32, i32)>>,
    /// Cumulative route distance (m) at each chunk's first point.
    cum_m: Vec<u32>,
    total_m: u32,
}

impl StaticRoute {
    /// Anchor [`ROUTE_DELTAS`] at `(cx, cy)` and accumulate the same ground metric the real
    /// route format stores (seam vertices contribute zero between chunks).
    fn at(cx: i32, cy: i32) -> Self {
        let chunks: Vec<Vec<(i32, i32)>> =
            ROUTE_DELTAS.iter().map(|c| c.iter().map(|&(dx, dy)| (cx + dx, cy + dy)).collect()).collect();
        let (mut cum_m, mut s) = (Vec::new(), 0.0f64);
        for c in &chunks {
            cum_m.push(s as u32);
            s += c.windows(2).map(|w| ground_dist_m(w[0], w[1]) as f64).sum::<f64>();
        }
        StaticRoute { chunks, cum_m, total_m: s as u32 }
    }
}

impl RouteOverlaySource for StaticRoute {
    fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    fn chunk(&self, k: usize) -> OverlayChunk {
        let mut bbox = BBox { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN };
        for &(lon, lat) in &self.chunks[k] {
            bbox.min_lon = bbox.min_lon.min(lon);
            bbox.min_lat = bbox.min_lat.min(lat);
            bbox.max_lon = bbox.max_lon.max(lon);
            bbox.max_lat = bbox.max_lat.max(lat);
        }
        OverlayChunk { bbox, cum_distance_m: self.cum_m[k] }
    }
    fn total_distance_m(&self) -> u32 {
        self.total_m
    }
    fn visit_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        visit(&self.chunks[k]);
    }
}

/// The `route` scene: the mid-zoom base map plus the static route overlay — magenta stroke and
/// white chevrons anchored at the chunk seam, so both `draw_route` passes land pixels in the hash.
/// Same warm-up/min-of-[`ITERS`] shape as [`run_scene`]; the per-stage timings are the map's
/// (the overlay shows up in `total`).
fn run_route_scene(map: &[u8], clock: &StdClock) -> SceneResult {
    let src = SliceSource(map);
    let tables = MapTables::parse(&src).expect("bench map must parse");
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let mut scratch = RenderScratch::new();
    let mut buf = vec![0u16; (WIDTH * HEIGHT) as usize];

    let bg = Rgb565::from(RawU16::new(reader.backdrop_style().map(|s| s.color).unwrap_or(0xFFFF)));
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

    let cx = (tables.bbox.min_lon + tables.bbox.max_lon) / 2;
    let cy = (tables.bbox.min_lat + tables.bbox.max_lat) / 2;
    let vp = Viewport::new_rotated(WIDTH as f32, HEIGHT as f32, cx, cy, zoom_for_mpp(4.0), 0.0);
    let route = StaticRoute::at(cx, cy);
    // Rider at the chunk seam: the chevron window spans both chunks' distance ranges.
    let arrows_at = Some(route.cum_m[1]);
    // Route stroke colour + weight imported straight from the Map screen, so the bench can't drift
    // from what the device draws: magenta `palette::ROUTE` (0xF81F), `ROUTE_WEIGHT` (11 px). The
    // chevron colour stays the literal white 0xFFFF (not the app's `ARROW_COLOR` = `PARCHMENT`,
    // 0xF79D): those two RGB565 words quantize to the *same* device-64 white on-glass, but this bench
    // renders into RGB565 and its committed frame hashes pin the literal pixels — importing PARCHMENT
    // would repaint the chevrons 0xF79D and break the hash for zero on-device difference.
    let (route_c, arrow_c) = (color_fn(obc_app::screen::palette::ROUTE), color_fn(0xFFFF));

    let draw = |buf: &mut [u16], scratch: &mut RenderScratch| {
        let mut fb = Framebuffer565::new(buf, WIDTH, HEIGHT);
        let stats = scratch.render_timed(&mut fb, &reader, &vp, bg, RenderConfig::default(), color_fn, clock);
        scratch.draw_route(&mut fb, &vp, &route, route_c, obc_app::screen::ROUTE_WEIGHT, arrow_c, arrows_at);
        stats
    };

    draw(&mut buf, &mut scratch); // warm-up: fills the chunk cache

    let (mut collect_us, mut sort_us, mut draw_us, mut total_us) = (u32::MAX, u32::MAX, u32::MAX, u64::MAX);
    let mut stats = RenderStats::default();
    for _ in 0..ITERS {
        let t0 = clock.now_us();
        stats = draw(&mut buf, &mut scratch);
        total_us = total_us.min(clock.now_us() - t0);
        collect_us = collect_us.min(stats.collect_us);
        sort_us = sort_us.min(stats.sort_us);
        draw_us = draw_us.min(stats.draw_us);
    }

    SceneResult { name: "route".into(), collect_us, sort_us, draw_us, total_us, stats, hash: frame_hash(&buf) }
}

/// Run the full built-in matrix over the testkit fixture, asserting the overview scenes saturate
/// (`features_dropped > 0`) — if they don't, the fixture isn't dense enough and the drop path went
/// unexercised, so fail loudly rather than green-light a hollow benchmark.
fn run_matrix() -> Vec<SceneResult> {
    let map = obcm_testkit::build_bench_map();
    let clock = StdClock(Instant::now());
    let mut results: Vec<SceneResult> = SCENES
        .iter()
        .map(|&(name, mpp, heading)| {
            let r = run_scene(&map, name, mpp, heading, &clock);
            if name.starts_with("overview") {
                assert!(
                    r.stats.features_dropped > 0,
                    "scene `{name}` must saturate the frame budget (features_dropped > 0); \
                     the fixture isn't dense enough — grow obcm_testkit::build_bench_map"
                );
            }
            r
        })
        .collect();
    // The route-overlay scene (issue #332): the map scenes carry no route, so this seventh frame
    // is what puts `draw_route`'s stroke + chevrons under the hash tripwire.
    results.push(run_route_scene(&map, &clock));
    results
}

/// The scene table. `chunks`/`hit/miss`/`sd`/`bytes` are the gated read counters — `sd` is
/// `ByteSource::read_at` calls (one card read each on the device, index blocks included) and
/// `bytes` what they moved.
fn print_table(results: &[SceneResult]) {
    println!(
        "{:<13} {:>3} {:>10} {:>8} {:>9} {:>9}  {:>6} {:>6} {:>7}  {:>6} {:>9} {:>5} {:>8}  hash",
        "scene",
        "lod",
        "collect",
        "sort",
        "draw",
        "total",
        "tried",
        "drawn",
        "dropped",
        "chunks",
        "hit/miss",
        "sd",
        "bytes"
    );
    for r in results {
        let s = &r.stats;
        println!(
            "{:<13} {:>3} {:>8}us {:>6}us {:>7}us {:>7}us  {:>6} {:>6} {:>7}  {:>6} {:>4}/{:<4} {:>5} {:>8}  0x{:016x}",
            r.name,
            s.lod,
            r.collect_us,
            r.sort_us,
            r.draw_us,
            r.total_us,
            s.features_tried,
            s.features_drawn,
            s.features_dropped,
            s.chunks_visited,
            s.map_chunk_hits,
            s.map_chunk_misses,
            s.map_sd_reads,
            s.map_bytes_read,
            r.hash
        );
    }
}

/// Repeat the complete matrix and summarize each scene's end-to-end time. Each matrix result is
/// already the min of [`ITERS`] warmed renders; the outer median rejects process/scheduler noise,
/// while min/max expose the observed envelope used to set a review tolerance. Every gated value —
/// hash and read counters alike — must agree on every repeat, keeping this timing mode covered by
/// the same determinism contract the golden file gates.
fn print_repeat_table(repeats: usize) {
    let runs: Vec<Vec<SceneResult>> = (0..repeats).map(|_| run_matrix()).collect();
    println!("{:13} {:>8} {:>8} {:>8} {:>8}  hash", "scene", "min", "median", "max", "spread");
    for scene in 0..runs[0].len() {
        let name = &runs[0][scene].name;
        let hash = runs[0][scene].hash;
        let gated = scene_values(&runs[0][scene]);
        let mut totals: Vec<u64> = runs.iter().map(|run| run[scene].total_us).collect();
        assert!(
            runs.iter().all(|run| run[scene].name == *name && scene_values(&run[scene]) == gated),
            "scene order, pixel hash or read counters changed between benchmark repeats"
        );
        totals.sort_unstable();
        let min = totals[0];
        let median = totals[totals.len() / 2];
        let max = totals[totals.len() - 1];
        println!("{name:13} {min:>6}us {median:>6}us {max:>6}us {spread:>6}us  0x{hash:016x}", spread = max - min);
    }
}

// ==================== the route-corridor snapshot bench (epic #946, U2) ====================
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
                hours_ref: 0xFFFF,
            });
            // …and one well outside it, in the same quadtree leaves.
            specs.push(obcm_testkit::PoiSpec {
                lat: CORRIDOR_LAT + side * 5,
                lon,
                subtype,
                name: format!("out{i}{j}"),
                hours_ref: 0xFFFF,
            });
        }
        cats.push((i as u8 + 1, specs));
    }
    obcm_testkit::build_poi_map(CORRIDOR_BBOX, CORRIDOR_POI_CHUNK, &cats)
}

/// One corridor-snapshot measurement.
struct CorridorResult {
    name: String,
    results: usize,
    map_reads: u32,
    map_bytes: u64,
    route_reads: u32,
    route_bytes: u64,
    us: u64,
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
fn run_corridor_matrix() -> (Vec<CorridorResult>, Vec<u8>) {
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

fn print_corridor_table(results: &[CorridorResult], route: &[u8]) {
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

// ==================== the golden file: pixels and read counters, one gate (issue #1467) ====================
//
// One record per case, `name key=value …`, keys named exactly as the `RenderStats` / corridor-table
// fields that produced them so a golden line greps straight back to the code. Both matrices share
// one namespace: corridor cases carry a `corridor/` prefix and the name selects the key set, so
// there is no record-type field to keep in sync.
//
// Timings are *not* here. Shared runners are noisy; only values that are bit-for-bit reproducible on
// any host are gated.

/// A scene record's keys, in the order [`golden_lines`] writes them. `hash` is `0x` + 16 hex
/// digits; every counter is decimal. The counters are the last of [`ITERS`] warmed renders — a
/// per-frame steady state, so they do not depend on how many iterations ran.
const SCENE_KEYS: [&str; 6] =
    ["hash", "chunks_visited", "map_chunk_hits", "map_chunk_misses", "map_sd_reads", "map_bytes_read"];

/// A corridor record's keys, named after the columns [`print_corridor_table`] prints.
const CORRIDOR_KEYS: [&str; 5] = ["rows", "map_reads", "map_bytes", "route_reads", "route_bytes"];

/// The corridor matrix's namespace inside the shared golden file.
const CORRIDOR_PREFIX: &str = "corridor/";

/// Which key set a case name carries.
fn keys_for(name: &str) -> &'static [&'static str] {
    if name.starts_with(CORRIDOR_PREFIX) {
        &CORRIDOR_KEYS
    } else {
        &SCENE_KEYS
    }
}

/// One scene's gated values, in [`SCENE_KEYS`] order.
fn scene_values(r: &SceneResult) -> Vec<u64> {
    let s = &r.stats;
    vec![
        r.hash,
        s.chunks_visited as u64,
        u64::from(s.map_chunk_hits),
        u64::from(s.map_chunk_misses),
        u64::from(s.map_sd_reads),
        u64::from(s.map_bytes_read),
    ]
}

/// One corridor case's gated values, in [`CORRIDOR_KEYS`] order.
fn corridor_values(r: &CorridorResult) -> Vec<u64> {
    vec![r.results as u64, u64::from(r.map_reads), r.map_bytes, u64::from(r.route_reads), r.route_bytes]
}

/// Both matrices as `(case name, gated values)` in run order — the order the golden file is written
/// in, so it reads like the tables the bench prints.
fn records(scenes: &[SceneResult], corridor: &[CorridorResult]) -> Vec<(String, Vec<u64>)> {
    scenes
        .iter()
        .map(|r| (r.name.clone(), scene_values(r)))
        .chain(corridor.iter().map(|r| (format!("{CORRIDOR_PREFIX}{}", r.name), corridor_values(r))))
        .collect()
}

/// Render one value the way its key is written and read.
fn format_value(key: &str, value: u64) -> String {
    if key == "hash" {
        format!("0x{value:016x}")
    } else {
        value.to_string()
    }
}

fn parse_value(at: usize, key: &str, value: &str) -> Result<u64, String> {
    if key == "hash" {
        return value
            .strip_prefix("0x")
            .filter(|digits| digits.len() == 16 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|digits| u64::from_str_radix(digits, 16).ok())
            .ok_or_else(|| format!("line {at} has invalid hash `{value}`"));
    }
    value.parse().map_err(|error| format!("line {at} has invalid {key} `{value}`: {error}"))
}

/// Parse the golden file. Every key of the record's key set must appear exactly once and no other
/// key is accepted, so a dropped counter is an error rather than a silent zero that would gate
/// nothing.
fn parse_golden(golden: &str) -> Result<BTreeMap<String, Vec<u64>>, String> {
    let mut expected = BTreeMap::new();
    for (index, raw) in golden.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let at = index + 1;
        let mut fields = line.split_whitespace();
        let name = fields.next().expect("a non-empty trimmed line has a first field");
        let keys = keys_for(name);
        let mut found: Vec<Option<u64>> = vec![None; keys.len()];
        for field in fields {
            let (key, value) =
                field.split_once('=').ok_or_else(|| format!("line {at} field `{field}` is not `key=value`"))?;
            let slot = keys
                .iter()
                .position(|known| *known == key)
                .ok_or_else(|| format!("line {at} has unknown key `{key}` for case `{name}`"))?;
            if found[slot].is_some() {
                return Err(format!("line {at} repeats key `{key}`"));
            }
            found[slot] = Some(parse_value(at, key, value)?);
        }
        let values = keys
            .iter()
            .zip(&found)
            .map(|(key, value)| value.ok_or_else(|| format!("line {at} case `{name}` is missing key `{key}`")))
            .collect::<Result<Vec<u64>, String>>()?;
        if expected.insert(name.to_string(), values).is_some() {
            return Err(format!("line {at} duplicates case `{name}`"));
        }
    }
    Ok(expected)
}

/// Compare a run's records to the golden file. Malformed, duplicate or incomplete golden lines, any
/// changed value, and any difference between the golden/current case-name sets print a focused
/// diagnostic and fail the check. A counter delta fails exactly like a hash delta.
fn check_golden(scenes: &[SceneResult], corridor: &[CorridorResult], golden: &str) -> bool {
    let expected = match parse_golden(golden) {
        Ok(expected) => expected,
        Err(error) => {
            eprintln!("GOLDEN INVALID: {error}");
            return false;
        }
    };
    let mut current = BTreeMap::new();
    let mut ok = true;
    for (name, values) in records(scenes, corridor) {
        if current.insert(name.clone(), values).is_some() {
            eprintln!("CURRENT INVALID: duplicate case `{name}`");
            ok = false;
        }
    }
    for (name, want) in &expected {
        let Some(got) = current.get(name) else {
            eprintln!("STALE {name}: golden entry has no current case");
            ok = false;
            continue;
        };
        for ((key, want), got) in keys_for(name).iter().zip(want).zip(got) {
            if want != got {
                eprintln!(
                    "MISMATCH {name} {key}: golden {} != run {}",
                    format_value(key, *want),
                    format_value(key, *got)
                );
                ok = false;
            }
        }
    }
    for name in current.keys() {
        if !expected.contains_key(name) {
            eprintln!("MISSING {name}: no golden entry for this case");
            ok = false;
        }
    }
    ok
}

/// Run and print everything the golden file gates — the scene matrix and the corridor matrix — so
/// `--check` and `--write-golden` measure and report exactly the same thing.
fn run_gated_matrices() -> (Vec<SceneResult>, Vec<CorridorResult>) {
    let scenes = run_matrix();
    print_table(&scenes);
    let (corridor, route) = run_corridor_matrix();
    println!();
    print_corridor_table(&corridor, &route);
    (scenes, corridor)
}

fn golden_lines(scenes: &[SceneResult], corridor: &[CorridorResult]) -> String {
    let mut out = String::new();
    for (name, values) in records(scenes, corridor) {
        out.push_str(&name);
        for (key, value) in keys_for(&name).iter().zip(&values) {
            out.push_str(&format!(" {key}={}", format_value(key, *value)));
        }
        out.push('\n');
    }
    out
}

/// What the hand-parsed CLI asked for. No CLI framework — five flags, parsed by hand.
enum Mode {
    Table,
    Repeat(usize),
    WriteGolden(String),
    Check(String),
    Custom {
        map: String,
        mpp: f32,
        heading: f32,
    },
    /// The route-corridor snapshot cost matrix (epic #946 U2) — SD reads + host time, not pixels.
    Corridor,
}

fn parse_args() -> Result<Mode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut write, mut check, mut map, mut repeat) = (None, None, None, None);
    let mut corridor = false;
    let (mut mpp, mut heading) = (4.0f32, 0.0f32);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match a.as_str() {
            "--write-golden" => write = Some(val("--write-golden")?),
            "--check" => check = Some(val("--check")?),
            "--repeat" => {
                let n: usize = val("--repeat")?.parse().map_err(|e| format!("--repeat: {e}"))?;
                if n == 0 || n.is_multiple_of(2) {
                    return Err("--repeat must be a positive odd number (so the median is unambiguous)".into());
                }
                repeat = Some(n);
            }
            "--corridor" => corridor = true,
            "--map" => map = Some(val("--map")?),
            "--mpp" => mpp = val("--mpp")?.parse().map_err(|e| format!("--mpp: {e}"))?,
            "--heading" => heading = val("--heading")?.parse().map_err(|e| format!("--heading: {e}"))?,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(match (write, check, map, repeat, corridor) {
        (Some(f), None, None, None, false) => Mode::WriteGolden(f),
        (None, Some(f), None, None, false) => Mode::Check(f),
        (None, None, Some(map), None, false) => Mode::Custom { map, mpp, heading },
        (None, None, None, Some(n), false) => Mode::Repeat(n),
        (None, None, None, None, true) => Mode::Corridor,
        (None, None, None, None, false) => Mode::Table,
        _ => return Err("pick one of --repeat / --write-golden / --check / --corridor / --map".into()),
    })
}

fn main() -> ExitCode {
    let mode = match parse_args() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("obc-bench: {e}");
            eprintln!(
                "usage: obc-bench [--repeat <odd-N> | --write-golden <file> | --check <file> | --corridor | --map <path> [--mpp <f>] [--heading <deg>]]"
            );
            return ExitCode::FAILURE;
        }
    };

    match mode {
        Mode::Table => print_table(&run_matrix()),
        Mode::Corridor => {
            let (results, route) = run_corridor_matrix();
            print_corridor_table(&results, &route);
        }
        Mode::Repeat(n) => print_repeat_table(n),
        Mode::WriteGolden(path) => {
            let (scenes, corridor) = run_gated_matrices();
            if let Err(e) = std::fs::write(&path, golden_lines(&scenes, &corridor)) {
                eprintln!("obc-bench: writing {path}: {e}");
                return ExitCode::FAILURE;
            }
            println!("wrote {path}");
        }
        Mode::Check(path) => {
            let golden = match std::fs::read_to_string(&path) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("obc-bench: reading {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let (scenes, corridor) = run_gated_matrices();
            if !check_golden(&scenes, &corridor, &golden) {
                eprintln!(
                    "pixels or read counters drifted from {path} — intentional change? regenerate with \
                     --write-golden and state the reason in the same PR"
                );
                return ExitCode::FAILURE;
            }
            println!("all {} golden records match {path}", scenes.len() + corridor.len());
        }
        // Manual escape hatch: one scene over a real local `.obcm`. No hash bookkeeping, no
        // saturation assert — real maps aren't fixtures.
        Mode::Custom { map, mpp, heading } => {
            let bytes = match std::fs::read(&map) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("obc-bench: reading {map}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let clock = StdClock(Instant::now());
            print_table(&[run_scene(&bytes, "custom", mpp, heading, &clock)]);
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture generator is byte-deterministic — the foundation the committed hashes stand on.
    #[test]
    fn bench_map_bytes_are_deterministic() {
        assert_eq!(obcm_testkit::build_bench_map(), obcm_testkit::build_bench_map());
    }

    /// A full-map overview view must push past the frame's feature ceiling so the priority-drop
    /// path is exercised — through the real render, exactly as the runtime assert in `run_matrix`
    /// demands.
    #[test]
    fn overview_scene_saturates_frame_budget() {
        let map = obcm_testkit::build_bench_map();
        let clock = StdClock(Instant::now());
        let r = run_scene(&map, "overview", 30.0, 0.0, &clock);
        assert!(r.stats.features_tried > obc_render::MAX_SPANS, "fixture density under the feature ceiling MAX_SPANS");
        assert!(r.stats.features_dropped > 0, "overview must overflow the frame budget");
        // Single-ring features consume one span and one ring each. The rebalanced arena has more
        // rings than spans, so spans are now the real trigger; pin that rather than preserving the
        // pre-compaction bottleneck by accident.
        assert!(r.stats.span_utilization >= 1.0, "the span buffer is the saturated one");
    }

    /// The riding scenes must land on the fine LOD and the overview on the coarse one, or the
    /// matrix isn't exercising `select_lod_for_mpp`'s switch.
    #[test]
    fn scene_matrix_switches_lod() {
        let map = obcm_testkit::build_bench_map();
        let clock = StdClock(Instant::now());
        let riding = run_scene(&map, "riding", 0.5, 0.0, &clock);
        let overview = run_scene(&map, "overview", 30.0, 0.0, &clock);
        assert_eq!(riding.stats.lod, 1, "riding must select the fine LOD");
        assert_eq!(overview.stats.lod, 0, "overview must select the coarse LOD");
        assert!(riding.stats.features_drawn > 0, "riding scene must draw features");
    }

    /// The route scene must actually land overlay pixels — the magenta stroke, and *more* pixels
    /// once chevrons are enabled — or its hash line would be pinning a route-free frame.
    #[test]
    fn route_scene_draws_stroke_and_chevrons() {
        let map = obcm_testkit::build_bench_map();
        let src = SliceSource(&map);
        let tables = MapTables::parse(&src).expect("bench map must parse");
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut scratch = RenderScratch::new();
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        let bg = color_fn(reader.backdrop_style().map(|s| s.color).unwrap_or(0xFFFF));
        let cx = (tables.bbox.min_lon + tables.bbox.max_lon) / 2;
        let cy = (tables.bbox.min_lat + tables.bbox.max_lat) / 2;
        let vp = Viewport::new_rotated(WIDTH as f32, HEIGHT as f32, cx, cy, zoom_for_mpp(4.0), 0.0);
        let route = StaticRoute::at(cx, cy);

        let mut frame = |arrows_at: Option<u32>| {
            let mut buf = vec![0u16; (WIDTH * HEIGHT) as usize];
            let mut fb = Framebuffer565::new(&mut buf, WIDTH, HEIGHT);
            scratch.render(&mut fb, &reader, &vp, bg, RenderConfig::default(), color_fn);
            let (chunks, _, drawn) =
                scratch.draw_route(&mut fb, &vp, &route, color_fn(0xF81F), 11, color_fn(0xFFFF), arrows_at);
            (buf, chunks, drawn)
        };

        let (plain, chunks, drawn) = frame(None);
        assert_eq!(chunks, 2, "both static chunks must intersect the mid-zoom view");
        assert!(drawn > 0, "the route stroke must survive the view clip");
        let magenta = plain.iter().filter(|&&p| p == 0xF81F).count();
        assert!(magenta > 100, "expected a visible magenta route stroke, got {magenta} px");

        let (arrowed, ..) = frame(Some(route.cum_m[1]));
        let white_gain =
            arrowed.iter().filter(|&&p| p == 0xFFFF).count() - plain.iter().filter(|&&p| p == 0xFFFF).count();
        assert!(white_gain > 20, "chevrons must add white pixels over the stroke, gained {white_gain} px");
    }

    /// Two renders of the same scene hash identically — the tripwire's own repeatability.
    #[test]
    fn frame_hash_is_repeatable() {
        let map = obcm_testkit::build_bench_map();
        let clock = StdClock(Instant::now());
        let a = run_scene(&map, "mid-rot", 4.0, 35.0, &clock);
        let b = run_scene(&map, "mid-rot", 4.0, 35.0, &clock);
        assert_eq!(a.hash, b.hash);
    }

    /// The scene and corridor counters are as reproducible as the pixels — the property the golden
    /// file's counter half stands on, asserted the same way `frame_hash_is_repeatable` asserts the
    /// pixel half.
    #[test]
    fn gated_values_are_repeatable_across_runs() {
        assert_eq!(records(&run_matrix(), &[]), records(&run_matrix(), &[]));
        let (a, _) = run_corridor_matrix();
        let (b, _) = run_corridor_matrix();
        assert_eq!(records(&[], &a), records(&[], &b));
    }

    fn scene_result(name: &str, hash: u64, chunks_visited: usize) -> SceneResult {
        SceneResult {
            name: name.into(),
            collect_us: 0,
            sort_us: 0,
            draw_us: 0,
            total_us: 0,
            stats: RenderStats { chunks_visited, ..RenderStats::default() },
            hash,
        }
    }

    fn corridor_result(name: &str, map_reads: u32) -> CorridorResult {
        CorridorResult {
            name: name.into(),
            results: 6,
            map_reads,
            map_bytes: 21216,
            route_reads: 6,
            route_bytes: 7902,
            us: 0,
        }
    }

    /// A run and the golden file it was written from, plus the same run's file with one substring
    /// edited — the shape every check test below uses.
    fn a_run() -> ([SceneResult; 1], [CorridorResult; 1], String) {
        let scenes = [scene_result("riding", 1, 4)];
        let corridor = [corridor_result("thin/all/cold", 42)];
        let golden = golden_lines(&scenes, &corridor);
        (scenes, corridor, golden)
    }

    #[test]
    fn golden_parser_rejects_malformed_duplicate_unknown_and_missing_keys() {
        let (.., golden) = a_run();
        assert!(parse_golden(&golden).is_ok());
        assert!(parse_golden(&golden.replace("hash=0x0000000000000001", "hash=nope"))
            .unwrap_err()
            .contains("invalid hash"));
        assert!(parse_golden(&golden.replace("chunks_visited=4", "chunks_visited=four"))
            .unwrap_err()
            .contains("invalid chunks_visited"));
        assert!(parse_golden(&format!("{golden}{golden}")).unwrap_err().contains("duplicates case"));
        assert!(parse_golden(&golden.replace("map_sd_reads=", "map_sd_readz="))
            .unwrap_err()
            .contains("unknown key `map_sd_readz`"));
        // A dropped counter must be an error, never a silent zero that would gate nothing.
        assert!(parse_golden(&golden.replace(" map_sd_reads=0", ""))
            .unwrap_err()
            .contains("missing key `map_sd_reads`"));
        assert!(parse_golden(&golden.replace(" map_bytes=21216", "")).unwrap_err().contains("missing key `map_bytes`"));
    }

    /// The gate's whole purpose: identical pixels, one moved counter, and CI goes red — in either
    /// matrix.
    #[test]
    fn check_fails_on_a_counter_delta_with_every_hash_intact() {
        let (scenes, corridor, golden) = a_run();
        assert!(check_golden(&scenes, &corridor, &golden));
        assert!(!check_golden(&scenes, &corridor, &golden.replace("chunks_visited=4", "chunks_visited=5")));
        assert!(!check_golden(&scenes, &corridor, &golden.replace("map_reads=42", "map_reads=43")));
    }

    #[test]
    fn check_still_fails_on_a_hash_delta_and_on_either_name_set_difference() {
        let (scenes, corridor, golden) = a_run();
        assert!(!check_golden(&scenes, &corridor, &golden.replace("0x0000000000000001", "0x0000000000000002")));
        // A golden entry with no current case…
        let stale = format!("{golden}{}", golden_lines(&[scene_result("overview", 3, 16)], &[]));
        assert!(!check_golden(&scenes, &corridor, &stale));
        // …and, in the same edit, a current case with no golden entry.
        assert!(!check_golden(&scenes, &corridor, &golden.replace("riding ", "mid ")));
    }
}
