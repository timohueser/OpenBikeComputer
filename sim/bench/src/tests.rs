use super::*;
use crate::{
    golden::{parse_golden, records},
    scenes::{StaticRoute, COLD_SCENE, HEIGHT, WIDTH},
};
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_display::Framebuffer565;
use obc_reader::{MapCache, MapTables, Reader, SliceSource};
use obc_render::{zoom_for_mpp, RenderConfig, RenderScratch, RenderStats, Viewport};

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
    let r = run_scene(&map, "overview", 30.0, 0.0, true, &clock);
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
    let riding = run_scene(&map, "riding", 0.5, 0.0, true, &clock);
    let overview = run_scene(&map, "overview", 30.0, 0.0, true, &clock);
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
    let white_gain = arrowed.iter().filter(|&&p| p == 0xFFFF).count() - plain.iter().filter(|&&p| p == 0xFFFF).count();
    assert!(white_gain > 20, "chevrons must add white pixels over the stroke, gained {white_gain} px");
}

/// Why [`COLD_SCENE`] is in the matrix, as an assertion. `map_sd_reads` counts every `read_at`,
/// `map_chunk_misses` only the geometry fills, so their difference *is* the frame's
/// quadtree-index fills. Every warmed scene has none — which is what left the index cache
/// invisible to the golden file — and the cold record is the one that has some. If a later
/// change warms it by accident this fails, instead of the row quietly becoming a second
/// `overview`.
#[test]
fn only_the_cold_scene_fills_index_blocks() {
    for r in run_matrix() {
        let s = &r.stats;
        if r.name == COLD_SCENE.0 {
            assert!(s.map_sd_reads > s.map_chunk_misses, "the cold scene must fill quadtree-index blocks");
        } else {
            assert_eq!(s.map_sd_reads, s.map_chunk_misses, "warmed scene `{}` must fill no index block", r.name);
        }
    }
}

/// Two renders of the same scene hash identically — the tripwire's own repeatability.
#[test]
fn frame_hash_is_repeatable() {
    let map = obcm_testkit::build_bench_map();
    let clock = StdClock(Instant::now());
    let a = run_scene(&map, "mid-rot", 4.0, 35.0, true, &clock);
    let b = run_scene(&map, "mid-rot", 4.0, 35.0, true, &clock);
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
    assert!(parse_golden(&golden.replace(" map_sd_reads=0", "")).unwrap_err().contains("missing key `map_sd_reads`"));
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
