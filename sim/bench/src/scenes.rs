use std::time::Instant;

use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_display::Framebuffer565;
use obc_map_scene::{ground_dist_m, BBox};
use obc_reader::{MapCache, MapTables, Reader, SliceSource};
use obc_render::{
    zoom_for_mpp, Clock, OverlayChunk, RenderConfig, RenderScratch, RenderStats, RouteOverlaySource, Viewport,
};

use crate::golden::scene_values;

/// Device resolution — the LS021B7DD02 panel the shipping firmware renders at. The single
/// [`obc_display`] frame authority, not a re-declared literal.
pub(super) const WIDTH: u32 = obc_display::ls021::FRAME_W as u32;
pub(super) const HEIGHT: u32 = obc_display::ls021::FRAME_H as u32;

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

/// The one **cold** record: `overview` rendered once into a fresh cache, counters taken from that
/// first frame. Every scene above is warmed first, and a warmed frame performs zero quadtree-index
/// fills — so without this row the golden file cannot see the index cache at all, only the geometry
/// one. Overview is the pick: it walks the most nodes, so its first frame carries the most index
/// traffic of any scene in the matrix.
pub(super) const COLD_SCENE: (&str, f32, f32) = ("overview-cold", 30.0, 0.0);

/// [`Clock`] over [`std::time::Instant`]: µs since construction, threaded through `render_timed`
/// so `collect_us`/`sort_us`/`draw_us` are real host wall time.
pub(super) struct StdClock(pub(super) Instant);

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
pub(super) struct SceneResult {
    pub(super) name: String,
    pub(super) collect_us: u32,
    pub(super) sort_us: u32,
    pub(super) draw_us: u32,
    pub(super) total_us: u64,
    pub(super) stats: RenderStats,
    pub(super) hash: u64,
}

/// Render one scene through the steady-state device flow: parse-once tables, one `MapCache` reused
/// across iterations, camera at the map's bbox center. Warm-up once (fills the chunk cache), then
/// time [`ITERS`] renders and keep the min of each stage; counters come from the last iteration and
/// the hash from the final frame.
///
/// `warm == false` reports that first frame instead — one render into an empty cache, which is the
/// only shape in which the counters include index-block fills (see [`COLD_SCENE`]).
pub(super) fn run_scene(
    map: &[u8],
    name: &str,
    mpp: f32,
    heading_deg: f32,
    warm: bool,
    clock: &StdClock,
) -> SceneResult {
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
    // sees (a slow pan re-hits last frame's chunks), not the cold SD-fill. A cold case reports this
    // very frame and stops.
    let mut fb = Framebuffer565::new(&mut buf, WIDTH, HEIGHT);
    let t0 = clock.now_us();
    let first = scratch.render_timed(&mut fb, &reader, &vp, bg, RenderConfig::default(), color_fn, clock);
    let first_us = clock.now_us() - t0;
    if !warm {
        return SceneResult {
            name: name.into(),
            collect_us: first.collect_us,
            sort_us: first.sort_us,
            draw_us: first.draw_us,
            total_us: first_us,
            stats: first,
            hash: frame_hash(&buf),
        };
    }

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
pub(super) struct StaticRoute {
    /// Absolute `(lon, lat)` microdegree points per chunk.
    chunks: Vec<Vec<(i32, i32)>>,
    /// Cumulative route distance (m) at each chunk's first point.
    pub(super) cum_m: Vec<u32>,
    total_m: u32,
}

impl StaticRoute {
    /// Anchor [`ROUTE_DELTAS`] at `(cx, cy)` and accumulate the same ground metric the real
    /// route format stores (seam vertices contribute zero between chunks).
    pub(super) fn at(cx: i32, cy: i32) -> Self {
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
pub(super) fn run_matrix() -> Vec<SceneResult> {
    let map = obcm_testkit::build_bench_map();
    let clock = StdClock(Instant::now());
    let mut results: Vec<SceneResult> = SCENES
        .iter()
        .map(|&(name, mpp, heading)| {
            let r = run_scene(&map, name, mpp, heading, true, &clock);
            assert_overview_saturates(&r);
            r
        })
        .collect();
    let (cold, cold_mpp, cold_heading) = COLD_SCENE;
    let cold = run_scene(&map, cold, cold_mpp, cold_heading, false, &clock);
    assert_overview_saturates(&cold);
    results.push(cold);
    // The route-overlay scene: the map scenes carry no route, so this eighth frame is what puts
    // `draw_route`'s stroke + chevrons under the hash tripwire.
    results.push(run_route_scene(&map, &clock));
    results
}

/// Every overview scene — the cold one included — must push past the frame's feature ceiling, or
/// the fixture isn't dense enough and the priority-drop path went unexercised.
fn assert_overview_saturates(r: &SceneResult) {
    if r.name.starts_with("overview") {
        assert!(
            r.stats.features_dropped > 0,
            "scene `{}` must saturate the frame budget (features_dropped > 0); \
             the fixture isn't dense enough — grow obcm_testkit::build_bench_map",
            r.name
        );
    }
}

/// The scene table. `chunks`/`hit/miss`/`sd`/`bytes` are the gated read counters — `sd` is
/// `ByteSource::read_at` calls (one card read each on the device, index blocks included) and
/// `bytes` what they moved.
///
/// The timing columns are the min of [`ITERS`] warmed renders **except** on a `-cold` row, whose
/// times are its one un-warmed render and are not comparable with its neighbours'.
pub(super) fn print_table(results: &[SceneResult]) {
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

/// Repeat the complete matrix and summarize each scene's end-to-end time. Every warmed matrix
/// result is already the min of [`ITERS`] renders; the outer median rejects process/scheduler noise,
/// while min/max expose the observed envelope used to set a review tolerance.
///
/// The [`COLD_SCENE`] row is the exception and must be read differently: it is a **single**
/// un-warmed render, so its median is one sample and its envelope is ordinary run-to-run noise plus
/// the cold fills — wider and slower than its warm twin for reasons that are not a regression. Set
/// tolerances from the warmed rows. Every gated value — hash and read counters alike — must agree
/// on every repeat regardless, keeping this timing mode covered by the same determinism contract
/// the golden file gates.
pub(super) fn print_repeat_table(repeats: usize) {
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
