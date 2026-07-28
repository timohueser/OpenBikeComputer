//! Host render benchmark harness + pixel-hash tripwire (issue #327, epic #326).
//!
//! Renders a fixed 7-scene matrix through the **real pipeline** — `obcm-testkit`'s deterministic
//! fixture → `SliceSource` → `MapTables`/`MapCache`/`Reader` → `MapRenderer::render_timed` → the
//! device-resolution [`Framebuffer565`] — and prints per-stage timings (min of 10 after a warm-up),
//! the [`RenderStats`] counters, and an FNV-1a 64 hash of the frame's pixels.
//!
//! Two jobs, one binary:
//! - **Benchmark** (the epic's measuring instrument): the timings are the before/after numbers every
//!   #329 optimization lands with. Printed, never gated — shared CI runners are noisy.
//! - **Tripwire** (`--check`): the frame hashes are deterministic (seeded fixture, integer/`libm`
//!   math), so CI compares them against the committed `hashes.txt` and fails on any drift. A
//!   pure-motion refactor must not touch them; an intentional rendering change updates the golden
//!   file in the same PR — that is the review signal.
//!
//! Modes: default (print the table), `--repeat <N>` (repeat the whole matrix and report
//! min/median/max), `--write-hashes <file>`, `--check <file>` (exit 1 on mismatch), and `--map
//! <path> --mpp <f> --heading <deg>` — a manual escape hatch to run one scene against a real local
//! `.obcm` (never in CI: real maps aren't byte-stable fixtures).

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Instant;

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use obc_display::Framebuffer565;
use obc_reader::{ground_dist_m, BBox, MapCache, MapTables, Reader, SliceSource};
use obc_render::{zoom_for_mpp, Clock, MapRenderer, OverlayChunk, RenderStats, RouteOverlaySource, Viewport};

/// Device resolution — the LS021B7DD02 panel the shipping firmware renders at. The single
/// [`obc_display`] frame authority, not a re-declared literal.
const WIDTH: u32 = obc_display::ls021::FRAME_W as u32;
const HEIGHT: u32 = obc_display::ls021::FRAME_H as u32;

/// Timed iterations per scene (after one warm-up render that fills the chunk cache). The report is
/// the **min** of each stage — the noise-floor estimator for a deterministic workload.
const ITERS: usize = 10;

/// The fixed scene matrix: `(name, meters-per-pixel, heading°)`. Rides the fixture's two LODs
/// (riding = fine, mid/overview = coarse) both north-up and rotated; the overview pair must
/// saturate the span buffer (`features_dropped > 0`) or the fixture has gone stale.
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
    let mut renderer = MapRenderer::new();
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
    renderer.render_timed(&mut fb, &reader, &vp, bg, color_fn, clock);

    let (mut collect_us, mut sort_us, mut draw_us, mut total_us) = (u32::MAX, u32::MAX, u32::MAX, u64::MAX);
    let mut stats = RenderStats::default();
    for _ in 0..ITERS {
        let mut fb = Framebuffer565::new(&mut buf, WIDTH, HEIGHT);
        let t0 = clock.now_us();
        stats = renderer.render_timed(&mut fb, &reader, &vp, bg, color_fn, clock);
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
    let mut renderer = MapRenderer::new();
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

    let draw = |buf: &mut [u16], renderer: &mut MapRenderer| {
        let mut fb = Framebuffer565::new(buf, WIDTH, HEIGHT);
        let stats = renderer.render_timed(&mut fb, &reader, &vp, bg, color_fn, clock);
        renderer.draw_route(&mut fb, &vp, &route, route_c, obc_app::screen::ROUTE_WEIGHT, arrow_c, arrows_at);
        stats
    };

    draw(&mut buf, &mut renderer); // warm-up: fills the chunk cache

    let (mut collect_us, mut sort_us, mut draw_us, mut total_us) = (u32::MAX, u32::MAX, u32::MAX, u64::MAX);
    let mut stats = RenderStats::default();
    for _ in 0..ITERS {
        let t0 = clock.now_us();
        stats = draw(&mut buf, &mut renderer);
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
                    "scene `{name}` must saturate the span buffer (features_dropped > 0); \
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

fn print_table(results: &[SceneResult]) {
    println!(
        "{:<13} {:>3} {:>10} {:>8} {:>9} {:>9}  {:>6} {:>6} {:>7}  {:>6} {:>9}  hash",
        "scene", "lod", "collect", "sort", "draw", "total", "tried", "drawn", "dropped", "chunks", "hit/miss"
    );
    for r in results {
        let s = &r.stats;
        println!(
            "{:<13} {:>3} {:>8}us {:>6}us {:>7}us {:>7}us  {:>6} {:>6} {:>7}  {:>6} {:>4}/{:<4}  0x{:016x}",
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
            r.hash
        );
    }
}

/// Repeat the complete matrix and summarize each scene's end-to-end time. Each matrix result is
/// already the min of [`ITERS`] warmed renders; the outer median rejects process/scheduler noise,
/// while min/max expose the observed envelope used to set a review tolerance. Hashes must agree on
/// every repeat, keeping this timing mode covered by the same deterministic-pixel contract.
fn print_repeat_table(repeats: usize) {
    let runs: Vec<Vec<SceneResult>> = (0..repeats).map(|_| run_matrix()).collect();
    println!("{:13} {:>8} {:>8} {:>8} {:>8}  hash", "scene", "min", "median", "max", "spread");
    for scene in 0..runs[0].len() {
        let name = &runs[0][scene].name;
        let hash = runs[0][scene].hash;
        let mut totals: Vec<u64> = runs.iter().map(|run| run[scene].total_us).collect();
        assert!(
            runs.iter().all(|run| run[scene].name == *name && run[scene].hash == hash),
            "scene order or pixel hash changed between benchmark repeats"
        );
        totals.sort_unstable();
        let min = totals[0];
        let median = totals[totals.len() / 2];
        let max = totals[totals.len() - 1];
        println!("{name:13} {min:>6}us {median:>6}us {max:>6}us {spread:>6}us  0x{hash:016x}", spread = max - min);
    }
}

fn parse_golden_hashes(golden: &str) -> Result<BTreeMap<&str, u64>, String> {
    let mut expected = BTreeMap::new();
    for (index, raw) in golden.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (name, value) =
            line.split_once('=').ok_or_else(|| format!("line {} is not `name=0x<16 hex digits>`", index + 1))?;
        let digits = value
            .strip_prefix("0x")
            .filter(|digits| digits.len() == 16 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| format!("line {} has invalid hash `{value}`", index + 1))?;
        if name.is_empty() || name.trim() != name {
            return Err(format!("line {} has invalid scene name `{name}`", index + 1));
        }
        let hash = u64::from_str_radix(digits, 16)
            .map_err(|error| format!("line {} has invalid hash `{value}`: {error}", index + 1))?;
        if expected.insert(name, hash).is_some() {
            return Err(format!("line {} duplicates scene `{name}`", index + 1));
        }
    }
    Ok(expected)
}

/// Compare the run's hashes to the golden file (`name=0x<16 hex digits>` lines). Malformed or
/// duplicate lines, changed hashes, and any difference between the golden/current scene-name sets
/// print a focused diagnostic and fail the check.
fn check_hashes(results: &[SceneResult], golden: &str) -> bool {
    let expected = match parse_golden_hashes(golden) {
        Ok(expected) => expected,
        Err(error) => {
            eprintln!("GOLDEN INVALID: {error}");
            return false;
        }
    };
    let mut current = BTreeMap::new();
    let mut ok = true;
    for result in results {
        if current.insert(result.name.as_str(), result.hash).is_some() {
            eprintln!("CURRENT INVALID: duplicate scene `{}`", result.name);
            ok = false;
        }
    }
    for (&name, &want) in &expected {
        match current.get(name) {
            Some(&got) if want == got => {}
            Some(&got) => {
                eprintln!("HASH MISMATCH {name}: golden 0x{want:016x} != run 0x{got:016x}");
                ok = false;
            }
            None => {
                eprintln!("HASH STALE {name}: golden entry has no current scene");
                ok = false;
            }
        }
    }
    for (&name, &got) in &current {
        if !expected.contains_key(name) {
            eprintln!("HASH MISSING {name}: no golden entry (run 0x{got:016x})");
            ok = false;
        }
    }
    ok
}

fn hash_lines(results: &[SceneResult]) -> String {
    results.iter().map(|r| format!("{}=0x{:016x}\n", r.name, r.hash)).collect()
}

/// What the hand-parsed CLI asked for. No CLI framework — five flags, parsed by hand.
enum Mode {
    Table,
    Repeat(usize),
    WriteHashes(String),
    Check(String),
    Custom { map: String, mpp: f32, heading: f32 },
}

fn parse_args() -> Result<Mode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut write, mut check, mut map, mut repeat) = (None, None, None, None);
    let (mut mpp, mut heading) = (4.0f32, 0.0f32);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match a.as_str() {
            "--write-hashes" => write = Some(val("--write-hashes")?),
            "--check" => check = Some(val("--check")?),
            "--repeat" => {
                let n: usize = val("--repeat")?.parse().map_err(|e| format!("--repeat: {e}"))?;
                if n == 0 || n.is_multiple_of(2) {
                    return Err("--repeat must be a positive odd number (so the median is unambiguous)".into());
                }
                repeat = Some(n);
            }
            "--map" => map = Some(val("--map")?),
            "--mpp" => mpp = val("--mpp")?.parse().map_err(|e| format!("--mpp: {e}"))?,
            "--heading" => heading = val("--heading")?.parse().map_err(|e| format!("--heading: {e}"))?,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(match (write, check, map, repeat) {
        (Some(f), None, None, None) => Mode::WriteHashes(f),
        (None, Some(f), None, None) => Mode::Check(f),
        (None, None, Some(map), None) => Mode::Custom { map, mpp, heading },
        (None, None, None, Some(n)) => Mode::Repeat(n),
        (None, None, None, None) => Mode::Table,
        _ => return Err("pick one of --repeat / --write-hashes / --check / --map".into()),
    })
}

fn main() -> ExitCode {
    let mode = match parse_args() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("obc-bench: {e}");
            eprintln!(
                "usage: obc-bench [--repeat <odd-N> | --write-hashes <file> | --check <file> | --map <path> [--mpp <f>] [--heading <deg>]]"
            );
            return ExitCode::FAILURE;
        }
    };

    match mode {
        Mode::Table => print_table(&run_matrix()),
        Mode::Repeat(n) => print_repeat_table(n),
        Mode::WriteHashes(path) => {
            let results = run_matrix();
            print_table(&results);
            if let Err(e) = std::fs::write(&path, hash_lines(&results)) {
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
            let results = run_matrix();
            print_table(&results);
            if !check_hashes(&results, &golden) {
                eprintln!("frame hashes drifted from {path} — intentional rendering change? regenerate with --write-hashes and commit it in the same PR");
                return ExitCode::FAILURE;
            }
            println!("all {} frame hashes match {path}", results.len());
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

    /// A full-map overview view must push past `MAX_SPANS` so the priority-drop path is exercised —
    /// through the real render, exactly as the runtime assert in `run_matrix` demands.
    #[test]
    fn overview_scene_saturates_span_buffer() {
        let map = obcm_testkit::build_bench_map();
        let clock = StdClock(Instant::now());
        let r = run_scene(&map, "overview", 30.0, 0.0, &clock);
        assert!(r.stats.features_tried > obc_render::MAX_SPANS, "fixture density under MAX_SPANS");
        assert!(r.stats.features_dropped > 0, "overview must overflow the span buffer");
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
        let mut renderer = MapRenderer::new();
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        let bg = color_fn(reader.backdrop_style().map(|s| s.color).unwrap_or(0xFFFF));
        let cx = (tables.bbox.min_lon + tables.bbox.max_lon) / 2;
        let cy = (tables.bbox.min_lat + tables.bbox.max_lat) / 2;
        let vp = Viewport::new_rotated(WIDTH as f32, HEIGHT as f32, cx, cy, zoom_for_mpp(4.0), 0.0);
        let route = StaticRoute::at(cx, cy);

        let mut frame = |arrows_at: Option<u32>| {
            let mut buf = vec![0u16; (WIDTH * HEIGHT) as usize];
            let mut fb = Framebuffer565::new(&mut buf, WIDTH, HEIGHT);
            renderer.render(&mut fb, &reader, &vp, bg, color_fn);
            let (chunks, _, drawn) =
                renderer.draw_route(&mut fb, &vp, &route, color_fn(0xF81F), 11, color_fn(0xFFFF), arrows_at);
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

    fn hash_result(name: &str, hash: u64) -> SceneResult {
        SceneResult {
            name: name.into(),
            collect_us: 0,
            sort_us: 0,
            draw_us: 0,
            total_us: 0,
            stats: RenderStats::default(),
            hash,
        }
    }

    #[test]
    fn golden_hash_parser_rejects_malformed_and_duplicate_lines() {
        assert!(parse_golden_hashes("riding=not-a-hash\n").unwrap_err().contains("invalid hash"));
        assert!(parse_golden_hashes("riding=0x0000000000000001\nriding=0x0000000000000001\n")
            .unwrap_err()
            .contains("duplicates scene"));
    }

    #[test]
    fn hash_check_rejects_both_missing_and_stale_scene_names() {
        let results = [hash_result("riding", 1), hash_result("route", 2)];
        assert!(!check_hashes(&results, "riding=0x0000000000000001\noverview=0x0000000000000003\n"));
        assert!(check_hashes(&results, "riding=0x0000000000000001\nroute=0x0000000000000002\n"));
    }
}
