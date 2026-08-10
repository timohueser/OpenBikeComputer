//! WX12 (#1197) route-projection pins: `WeatherSnapshot::sample_along` samples every rain frame
//! at the rider's **expected route position** for that frame's timestamp (progress + pace × Δt,
//! clamped at the route end), inside a conservative one-cell corridor — so the two-hour decision
//! answers for the *ride*, not the parking spot. Deterministic across firmware and simulator:
//! everything here is the production reader/route/snapshot path over committed fixture bytes.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_app::weather::{rain_outlook, RainOutlook, RideProjection, WeatherSnapshot, RAIN_MIN_INTENSITY};
use obc_app::RainOverlayAdapter;
use obc_formats::io::SliceSource;
use obc_formats::obcw::{
    encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, HOURLY_COUNT, QUALITY_FORECAST, TILE_CELLS,
};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource as MapSliceSource};
use obc_render::{RainOverlaySource, RainSampling, RenderConfig, Viewport};
use obc_route::{RouteIndex, RouteReader};
use obc_weather::{WeatherCache, WeatherReader};

mod common;
use common::{build_min_obcm, Buf};

const T0: i64 = 1_800_000_000;
const GRID: usize = 48;

/// The committed Grimsel fixture (~18.7 km) — the same bytes the App-level tests ride.
const GRIMSEL: &[u8] = include_bytes!("../../../apps/obc-sim/assets/grimsel-climb.obcr");

fn grimsel_index() -> RouteIndex {
    let src = SliceSource(GRIMSEL);
    RouteIndex::read(&src).unwrap()
}

/// A generous bbox around the whole route (sampled along its length, padded well past a cell) so
/// every projected position lies strictly inside the grid.
fn route_bbox(route: &RouteReader) -> (i32, i32, i32, i32) {
    padded_route_bbox(route, 20_000)
}

/// [`route_bbox`] with the padding named — the pace-spread tests need the widened claim corridor
/// (up to ten cells at the +2 h horizon) to stay comfortably inside the product.
fn padded_route_bbox(route: &RouteReader, pad_udeg: i32) -> (i32, i32, i32, i32) {
    let (mut south, mut west, mut north, mut east) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    let mut m = 0;
    while m <= route.total_distance_m {
        let p = route.position_at(m).unwrap();
        south = south.min(p.lat);
        north = north.max(p.lat);
        west = west.min(p.lon);
        east = east.max(p.lon);
        m += 100;
    }
    (south - pad_udeg, west - pad_udeg, north + pad_udeg, east + pad_udeg)
}

/// The frame-grid cell containing `(lat, lon)` — the exact `cell_index` arithmetic of the
/// production reader (half-open bbox, integer scaling).
fn cell_of(bbox: (i32, i32, i32, i32), lat: i32, lon: i32) -> (usize, usize) {
    let (south, west, north, east) = bbox;
    let row = (lat as i64 - south as i64) * GRID as i64 / (north as i64 - south as i64);
    let col = (lon as i64 - west as i64) * GRID as i64 / (east as i64 - west as i64);
    (row as usize, col as usize)
}

/// Encode a nine-frame bundle over `bbox` whose per-frame wet cells come from `wet`:
/// `wet(frame) -> Vec<(row, col, band)>`; everything else dry. Hourly rows are calm.
fn bundle(bbox: (i32, i32, i32, i32), wet: impl Fn(usize) -> Vec<(usize, usize, u8)>) -> Vec<u8> {
    let (south, west, north, east) = bbox;
    let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|i| HourlyRecord {
        valid_time_offset_s: i as u32 * 3_600,
        temperature_deci_c: 120,
        precipitation_tenth_mm: 0,
        precipitation_probability_pct: 0,
        condition: 0,
        wind_from_deg: 0,
        wind_speed_deci_ms: 0,
        wind_gust_deci_ms: 0,
        flags: 0,
    });
    let tile_cols = GRID / 16;
    let mut frames_tiles = Vec::new();
    for frame in 0..9usize {
        let mut tiles = vec![[0u8; TILE_CELLS]; tile_cols * tile_cols];
        for (row, col, band) in wet(frame) {
            let tile = (row / 16) * tile_cols + col / 16;
            tiles[tile][(row % 16) * 16 + col % 16] = band;
        }
        frames_tiles.push(tiles);
    }
    let frames: Vec<RainFrameInput<'_>> = frames_tiles
        .iter()
        .enumerate()
        .map(|(i, tiles)| RainFrameInput {
            valid_at: T0 + i as i64 * 900,
            width: GRID as u16,
            height: GRID as u16,
            cell_size_m: 1_000,
            quality_flags: QUALITY_FORECAST,
            tiles,
        })
        .collect();
    let input = BundleInput {
        generation: 1,
        request_id: 0x0BC0_1197,
        generated_at: T0,
        valid_from: T0 - 3_600,
        valid_until: T0 + 24 * 3_600,
        south_lat_udeg: south,
        west_lon_udeg: west,
        north_lat_udeg: north,
        east_lon_udeg: east,
        grid_origin_lat_udeg: south,
        grid_origin_lon_udeg: west,
        flags: 0,
        hourly: &hourly,
        frames: &frames,
    };
    let mut bytes = vec![0u8; encoded_len(&input).unwrap() as usize];
    let n = encode_format(&input, &mut bytes).unwrap();
    bytes.truncate(n);
    bytes
}

/// Every frame's recorded sample position is the projection formula's route point — progress
/// advanced by pace × Δt from the anchor, clamped to the route end for the far frames.
#[test]
fn frame_samples_ride_the_projection_and_clamp_at_the_route_end() {
    let idx = grimsel_index();
    let src = SliceSource(GRIMSEL);
    let route = RouteReader::new(&idx, &src);
    let bbox = route_bbox(&route);
    let bytes = bundle(bbox, |_| Vec::new());
    let source = SliceSource(&bytes);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();

    let start = route.position_at(0).unwrap();
    let proj = RideProjection { progress_m: 0, speed_cms: 500, now: T0 };
    let snap =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();
    assert!(snap.projected);
    assert_eq!(snap.frames.len(), 9);
    for (k, frame) in snap.frames.iter().enumerate() {
        // 5 m/s × k × 900 s, clamped by `position_at` at the 18.7 km route end.
        let expect = route.position_at(k as u32 * 4_500).unwrap();
        assert_eq!((frame.lat, frame.lon), (expect.lat, expect.lon), "frame {k}");
    }
    // The far frames (k ≥ 5 ⇒ 22.5 km > total) all sit at the destination.
    let end = route.position_at(route.total_distance_m).unwrap();
    assert_eq!((snap.frames[8].lat, snap.frames[8].lon), (end.lat, end.lon), "past the end: the destination");

    // Frames at/before the anchor sample at the *current* progress — the past isn't reconstructed.
    let anchored = RideProjection { progress_m: 2_000, speed_cms: 500, now: T0 + 1_800 };
    let snap =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, anchored)))
            .unwrap();
    let here = route.position_at(2_000).unwrap();
    assert_eq!((snap.frames[0].lat, snap.frames[0].lon), (here.lat, here.lon), "a past frame samples here");
    assert_eq!((snap.frames[2].lat, snap.frames[2].lon), (here.lat, here.lon), "the anchor frame samples here");
    let later = route.position_at(2_000 + 4_500).unwrap();
    assert_eq!((snap.frames[3].lat, snap.frames[3].lon), (later.lat, later.lon), "+15 min rides ahead");
}

/// The decision itself: a storm cell parked on the route ~45 min ahead. The parked read is an
/// honest DRY FOR 2 HOURS (every current-position sample dry, full coverage); the *ride* read
/// crosses the cell and says STORM IN 45 — and never claims dry.
#[test]
fn projected_decision_sees_the_storm_the_parked_one_misses() {
    let idx = grimsel_index();
    let src = SliceSource(GRIMSEL);
    let route = RouteReader::new(&idx, &src);
    let bbox = route_bbox(&route);

    // The cells the rider will be under at +45 and +60 min (frames 3 and 4) at 5 m/s.
    let start = route.position_at(0).unwrap();
    let at3 = route.position_at(3 * 4_500).unwrap();
    let at4 = route.position_at(4 * 4_500).unwrap();
    let start_cell = cell_of(bbox, start.lat, start.lon);
    let wet3 = cell_of(bbox, at3.lat, at3.lon);
    let wet4 = cell_of(bbox, at4.lat, at4.lon);
    assert_ne!(start_cell, wet3, "fixture sanity: the storm is not over the start");
    let bytes = bundle(bbox, move |frame| match frame {
        3 => vec![(wet3.0, wet3.1, 10)],
        4 => vec![(wet4.0, wet4.1, 10)],
        _ => Vec::new(),
    });
    let source = SliceSource(&bytes);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();

    // Parked at the start: every sample at the current position, honestly dry for two hours.
    let parked = WeatherSnapshot::sample(&reader, &mut cache, Some((start.lat, start.lon))).unwrap();
    assert!(!parked.projected);
    assert_eq!(rain_outlook(&parked, T0), RainOutlook::Dry, "the storm never crosses the parking spot");

    // Riding: frame 3's sample lands under the storm cell — STORM IN 45.
    let proj = RideProjection { progress_m: 0, speed_cms: 500, now: T0 };
    let riding =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();
    assert_eq!(rain_outlook(&riding, T0), RainOutlook::StormIn { minutes: 45 }, "the ride crosses the storm");
}

/// Review F1's sibling: once the projection runs out of route it clamps at the finish and stands
/// there. A finished rider keeps riding, so the finish point's sky says nothing about where they
/// will be — those frames count as **no coverage** and the two-hour dry claim is refused, while
/// rain actually sitting on the destination is still reported.
#[test]
fn frames_projected_past_the_route_end_carry_no_dry_claim() {
    let idx = grimsel_index();
    let src = SliceSource(GRIMSEL);
    let route = RouteReader::new(&idx, &src);
    let bbox = padded_route_bbox(&route, 200_000);
    let start = route.position_at(0).unwrap();
    let end = route.position_at(route.total_distance_m).unwrap();

    // 5 m/s over the 18.7 km route: the projection reaches the finish inside the second hour.
    let proj = RideProjection { progress_m: 0, speed_cms: 500, now: T0 };
    let dry = bundle(bbox, |_| Vec::new());
    let source = SliceSource(&dry);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();

    // Parked on the same all-dry sky: an honest DRY FOR 2 HOURS.
    let parked = WeatherSnapshot::sample(&reader, &mut cache, Some((start.lat, start.lon))).unwrap();
    assert_eq!(rain_outlook(&parked, T0), RainOutlook::Dry);

    let riding =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();
    assert!(!riding.frames[3].past_route_end, "18.7 km isn't reached at +45 min");
    assert!(riding.frames[5].past_route_end, "…but it is by +75 min");
    assert_eq!(
        rain_outlook(&riding, T0),
        RainOutlook::UpdateNeeded,
        "a projection standing on the finish line can't promise the next two hours"
    );

    // Rain parked on the destination is still worth saying — warnings may use clamped frames.
    let end_cell = cell_of(bbox, end.lat, end.lon);
    let wet = bundle(bbox, move |frame| if frame >= 5 { vec![(end_cell.0, end_cell.1, 6)] } else { Vec::new() });
    let source = SliceSource(&wet);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    let riding =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();
    assert_eq!(rain_outlook(&riding, T0), RainOutlook::RainIn { minutes: 75 }, "the destination's rain still reports");
}

/// Review F3: the projection's own positional uncertainty at the far horizon is 2–4 cells wide, so
/// a one-cell corridor is too narrow to *promise* dryness. The claim corridor grows with the
/// horizon — a cell three steps off the +45 min position (inside that frame's four-cell claim
/// corridor, outside every frame's one-cell warning corridor) refuses DRY, while the warning path
/// stays exactly where it was: no RAIN IN is manufactured from it.
#[test]
fn the_claim_corridor_widens_with_the_horizon_while_warnings_do_not() {
    let idx = grimsel_index();
    let src = SliceSource(GRIMSEL);
    let route = RouteReader::new(&idx, &src);
    let bbox = padded_route_bbox(&route, 200_000);

    // 2 m/s: 14.4 km in two hours, so the 18.7 km route never runs out and the route-end clamp
    // stays out of this test's way.
    let proj = RideProjection { progress_m: 0, speed_cms: 200, now: T0 };
    let start = route.position_at(0).unwrap();
    let at = |k: usize| route.position_at(k as u32 * 1_800).unwrap();
    let frame_cells: Vec<(usize, usize)> = (0..9).map(|k| cell_of(bbox, at(k).lat, at(k).lon)).collect();

    // Control: nothing wet anywhere is an honest DRY FOR 2 HOURS along the projection — which is
    // what makes the widened refusal below a real difference and not a blanket "never dry".
    let dry = bundle(bbox, |_| Vec::new());
    let source = SliceSource(&dry);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    let riding =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();
    assert_eq!(rain_outlook(&riding, T0), RainOutlook::Dry, "the widened corridor still lets a clean sky be dry");

    // A cell on frame 3's axis, 3 steps out: inside its claim corridor (half-width 4 at +45 min on
    // the declared 1 km grid), and more than one step from *every* frame's projected cell, so no
    // one-cell warning corridor can see it.
    let target = frame_cells[3];
    let far_enough = |c: &(usize, usize)| frame_cells.iter().all(|f| c.0.abs_diff(f.0) + c.1.abs_diff(f.1) >= 2);
    let candidates =
        [(target.0 + 3, target.1), (target.0 - 3, target.1), (target.0, target.1 + 3), (target.0, target.1 - 3)];
    let wet_cell =
        candidates.into_iter().find(far_enough).expect("fixture: a 3-step cell clear of every warning corridor");

    let wet = bundle(bbox, move |_| vec![(wet_cell.0, wet_cell.1, 8)]);
    let source = SliceSource(&wet);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    let riding =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();

    assert_eq!(riding.frames[3].intensity, 0, "the one-cell warning corridor never sees it — no false RAIN IN");
    assert!(riding.frames[3].spread_uncertain, "…but the pace-spread corridor does");
    assert_eq!(
        rain_outlook(&riding, T0),
        RainOutlook::UpdateNeeded,
        "one cell would have said DRY; the rider's plausible position spread refuses it"
    );
}

/// The corridor is conservative in exactly one direction: a wet cell one step beside the
/// projected line raises the sample (a warning), while the centre cell alone decides validity —
/// and the unprojected path stays exact-cell (byte-compatible with WX11's screens).
#[test]
fn corridor_widens_warnings_but_only_under_projection() {
    let idx = grimsel_index();
    let src = SliceSource(GRIMSEL);
    let route = RouteReader::new(&idx, &src);
    let bbox = route_bbox(&route);

    let start = route.position_at(0).unwrap();
    let (row, col) = cell_of(bbox, start.lat, start.lon);
    // The rider's own cell stays dry; its northern neighbour rains band 5, every frame.
    let bytes = bundle(bbox, move |_| vec![(row + 1, col, 5)]);
    let source = SliceSource(&bytes);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();

    // Unprojected: exact-cell semantics — dry (the WX11 contract, unchanged).
    let parked = WeatherSnapshot::sample(&reader, &mut cache, Some((start.lat, start.lon))).unwrap();
    assert_eq!(parked.frames[0].intensity, 0, "no corridor without a projection");

    // Projected (speed 0 keeps every frame at the same spot): the neighbour counts — wet.
    let proj = RideProjection { progress_m: 0, speed_cms: 0, now: T0 };
    let riding =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();
    assert_eq!(riding.frames[0].intensity, 5, "the one-cell corridor reports the wet neighbour");
    assert_eq!(rain_outlook(&riding, T0), RainOutlook::RainIn { minutes: 0 });
}

// ---------------------------------------------------------------------------------------------
// The display/decision boundary (#1250)
// ---------------------------------------------------------------------------------------------

/// **The guard that lets the renderer interpolate.** `RAIN_SAMPLING` ships
/// [`RainSampling::Bilinear`], which paints bands no provider cell reported. OBCW §5 and OBCG §6
/// permit that for *display only* and keep **data queries** normatively nearest-neighbour — no
/// claim, alert, alert-clear or DRY decision may derive from an interpolated value.
///
/// Today that holds structurally: the decision path calls `obc_weather`'s `intensity_at` and never
/// enters `obc-render`. Structure is not a test, so this pins the *behaviour* two ways.
///
/// **1. The decision reads the selected cell exactly.** The rider is parked a hair inside a DRY
/// cell, hard against the corner it shares with three band-12 neighbours — the position where
/// nearest and bilinear disagree most. Nearest says dry; a bilinear read of the same point lands
/// near the ¾-weighted average of 12, far above `RAIN_MIN_INTENSITY`. The parked snapshot takes no
/// corridor (that is the projected path's widening, exercised above), so `Dry` here is a direct
/// assertion that the sample was the rider's own cell and nothing else. If the decision path is
/// ever refactored onto a smoothing sampler, this flips to `RainIn` and fails.
///
/// **2. Running the renderer cannot move the decision.** The overlay and the decision path share
/// one [`WeatherCache`], which is the realistic leak: a mode that populated or evicted differently
/// could change what a later query reads. So the whole decision — snapshot frames, outlook, and
/// the alert candidates — is recomputed after drawing a frame in **each** of the four modes and
/// must come back identical every time, including to the pre-render baseline.
#[test]
fn the_decision_path_is_identical_in_every_sampling_mode() {
    let idx = grimsel_index();
    let src = SliceSource(GRIMSEL);
    let route = RouteReader::new(&idx, &src);
    let bbox = route_bbox(&route);
    let (south, west, north, east) = bbox;

    // The rider's own cell, and the µdeg span of one cell on each axis.
    let start = route.position_at(0).unwrap();
    let home = cell_of(bbox, start.lat, start.lon);
    let cell_lat = (north as i64 - south as i64) / GRID as i64;
    let cell_lon = (east as i64 - west as i64) / GRID as i64;
    assert!(cell_lat > 8 && cell_lon > 8, "fixture sanity: cells are wide enough to sit inside");

    // Park just inside `home`'s north-east corner: ~1 µdeg short of the boundary on both axes, so
    // the point is still unambiguously in `home` for a floor, but a 2 x 2 bilinear stencil anchored
    // there weights the three wet neighbours ~3:1 against it.
    let corner_lat = south as i64 + (home.0 as i64 + 1) * cell_lat - 1;
    let corner_lon = west as i64 + (home.1 as i64 + 1) * cell_lon - 1;
    let (lat, lon) = (corner_lat as i32, corner_lon as i32);
    assert_eq!(cell_of(bbox, lat, lon), home, "the parked point must still floor into the dry cell");

    // `home` dry, its three corner-sharing neighbours torrential, on every frame.
    let (r, c) = home;
    let wet = vec![(r, c + 1, 12u8), (r + 1, c, 12), (r + 1, c + 1, 12)];
    assert!(r + 1 < GRID && c + 1 < GRID, "fixture sanity: the wet neighbours are inside the grid");
    let bytes = bundle(bbox, move |_| wet.clone());
    let source = SliceSource(&bytes);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();

    // Fixture teeth: the three neighbours really do read band 12 through this same reader, so the
    // dry answer below is nearest-neighbour doing its job and not a storm accidentally encoded
    // into the wrong tile. A step of one cell in each direction leaves `home`.
    for (dlat, dlon) in [(0i64, cell_lon), (cell_lat, 0), (cell_lat, cell_lon)] {
        let probe = WeatherSnapshot::sample(
            &reader,
            &mut cache,
            Some(((lat as i64 + dlat) as i32, (lon as i64 + dlon) as i32)),
        )
        .unwrap();
        assert_eq!(probe.frames[0].intensity, 12, "fixture sanity: the neighbour at {dlat},{dlon} must be torrential");
    }

    // 1. Nearest-neighbour is what the claim is built on, and it says dry.
    let baseline = WeatherSnapshot::sample(&reader, &mut cache, Some((lat, lon))).unwrap();
    assert_eq!(
        baseline.frames[0].intensity, 0,
        "the decision must read the rider's own cell exactly — an interpolated read here is ~9, not 0"
    );
    assert_eq!(
        rain_outlook(&baseline, T0),
        RainOutlook::Dry,
        "surrounded by band 12 on three sides, the selected cell is dry and the claim must say so"
    );
    let baseline_alerts = obc_app::weather_alerts::evaluate(&baseline, T0);

    // 2. Draw the overlay in all four modes over the shared cache; the decision must not budge.
    let map = build_min_obcm(0xF800);
    for mode in [RainSampling::Nearest, RainSampling::Bilinear, RainSampling::Jitter, RainSampling::EdgeSoften] {
        {
            let mut adapter = RainOverlayAdapter::current(&reader, &mut cache, T0).expect("frame 0 is current at T0");
            let stats = draw_rain_frame(&map, &mut adapter, (lat, lon), mode);
            // Without this the loop is a dead guard: aim the camera anywhere off the product and
            // every pixel resolves off-grid before a single fetch, so the overlay touches the
            // shared cache not at all and the assertions below hold vacuously. Demand evidence
            // that the overlay really ran and really painted.
            assert!(stats.rain_tiles > 0, "{mode:?}: the overlay decoded no tile — camera is off the rain grid");
            assert!(stats.rain_px > 0, "{mode:?}: the overlay painted no pixel — nothing was exercised");
        }
        let after = WeatherSnapshot::sample(&reader, &mut cache, Some((lat, lon))).unwrap();
        assert_eq!(after.frames, baseline.frames, "{mode:?}: rendering changed the sampled frames");
        assert_eq!(rain_outlook(&after, T0), RainOutlook::Dry, "{mode:?}: rendering changed the outlook");
        assert_eq!(
            obc_app::weather_alerts::evaluate(&after, T0),
            baseline_alerts,
            "{mode:?}: rendering changed the alert candidates"
        );
    }
}

/// Render one map frame with the rain overlay forced into `mode`, camera on `cam`, for its side
/// effects on the shared [`WeatherCache`]. The pixels are the subject of `obc-render`'s own mode
/// tests; here only the cache traffic matters — but the returned stats are what let the caller
/// prove the overlay was actually entered.
fn draw_rain_frame(
    map: &[u8],
    rain: &mut dyn RainOverlaySource,
    cam: (i32, i32),
    mode: RainSampling,
) -> obc_render::RenderStats {
    struct ZeroClock;
    impl obc_render::Clock for ZeroClock {
        fn now_us(&self) -> u64 {
            0
        }
    }

    let map_cache = MapCache::new();
    let src = MapSliceSource(map);
    let tables = MapTables::parse(&src).expect("valid map");
    let reader = Reader::new(&src, &tables, &map_cache);
    let mut buf = Buf::new(120, 120);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    // 40 m/px centred on the rider's **own** coordinate — the camera must sit on the rain product,
    // which is the padded Grimsel bbox, not at (0, 0). 40 m/px is comfortably inside the overlay's
    // zoom regime, so the rain path runs rather than returning early.
    let vp = Viewport::new(120.0, 120.0, cam.1, cam.0, obc_render::zoom_for_mpp(40.0));
    scratch.render_rain_sampled_timed(
        &mut buf,
        &reader,
        &vp,
        Rgb888::BLACK,
        RenderConfig::default(),
        Some(rain),
        mode,
        |c| {
            let (r, g, b) = rgb565_to_rgb888(c);
            Rgb888::new(r, g, b)
        },
        &ZeroClock,
    )
}

/// **The corridor arm of the same guarantee (#1250 review F3).** OBCW §5 names "corridor and
/// dry-claim walks" as normatively nearest-neighbour, and this is the path where an interpolated
/// value produces the epic's worst failure: not a missed warning but a **fabricated DRY claim**.
///
/// `the_decision_path_is_identical_in_every_sampling_mode` cannot reach it by construction — it
/// parks the rider, and the parked snapshot takes no corridor at all. So this one projects.
///
/// The fixture is built so the *only* thing standing between the rider and DRY FOR 2 HOURS is one
/// corridor probe reading a real cell exactly:
///
/// - The ride is stationary under projection (`speed_cms: 0`), so every frame samples the start
///   point and only the corridor's half-width varies — one cell at the anchor, widening by one
///   per 15-minute frame as pace uncertainty accumulates.
/// - A single cell at **band 1** — the weakest thing that counts as rain — sits exactly **two**
///   cells north. That is outside the one-cell warning corridor, so no frame reports rain and the
///   headline is not a warning; and inside the pace-spread corridor from frame 1 onward, so those
///   frames' dry claims must be refused.
///
/// The bbox is padded far past the widest corridor (ten cells at the +2 h frame) on purpose: with
/// the ordinary padding the outermost probes run off the grid and refuse the claim for their own
/// reason, which would mask exactly what this test is trying to isolate.
///
/// Nearest-neighbour reads that cell as 1, `corridor_is_dry` fails closed, and the outlook is
/// `UpdateNeeded` — "I cannot promise you two dry hours". Average it with either dry neighbour and
/// it floors to 0: the corridor reports clean and the device promises DRY FOR 2 HOURS over ground
/// it was told is wet.
///
/// **Verified by mutation.** Replacing `corridor_is_dry`'s probe with a faithful two-cell
/// interpolation — mean when both cells are real, falling back to the selected cell at a no-data
/// or off-grid neighbour, i.e. the renderer's own rule applied to a query — flips every frame from
/// `spread_uncertain` to clean and turns the headline into `RainOutlook::Dry`. It is also the
/// *only* thing in the `obc-app` suite that mutation trips: everything else passes under it.
#[test]
fn an_interpolated_corridor_probe_would_fabricate_a_dry_claim() {
    let idx = grimsel_index();
    let src = SliceSource(GRIMSEL);
    let route = RouteReader::new(&idx, &src);
    // Padded well past the widest pace-spread corridor (10 cells at the +2 h frame) so the only
    // thing that can refuse a dry claim is the band-1 cell, never the grid edge.
    let bbox = padded_route_bbox(&route, 200_000);
    let (south, _, north, _) = bbox;

    let start = route.position_at(0).unwrap();
    let home = cell_of(bbox, start.lat, start.lon);
    let cell_lat = (north as i64 - south as i64) / GRID as i64;

    // One band-1 cell two cells north of the rider, on every frame. Nothing else is wet.
    let wet_row = home.0 + 2;
    assert!(wet_row < GRID, "fixture sanity: the wet cell is inside the grid");
    let wet = vec![(wet_row, home.1, RAIN_MIN_INTENSITY)];
    let bytes = bundle(bbox, move |_| wet.clone());
    let source = SliceSource(&bytes);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();

    // Fixture teeth, both directions: the cell two north really is band 1, and the cell one north
    // — the warning corridor's reach — really is dry. Without the second, the refusal below could
    // be an ordinary rain warning rather than the corridor doing its job.
    let two_north =
        WeatherSnapshot::sample(&reader, &mut cache, Some((start.lat + 2 * cell_lat as i32, start.lon))).unwrap();
    assert_eq!(two_north.frames[0].intensity, RAIN_MIN_INTENSITY, "fixture sanity: two cells north is band 1");
    let one_north =
        WeatherSnapshot::sample(&reader, &mut cache, Some((start.lat + cell_lat as i32, start.lon))).unwrap();
    assert_eq!(one_north.frames[0].intensity, 0, "fixture sanity: one cell north must be dry");

    // Stationary under projection: every frame samples the start, so the corridor half-width is
    // the only thing that changes between them.
    let proj = RideProjection { progress_m: 0, speed_cms: 0, now: T0 };
    let riding =
        WeatherSnapshot::sample_along(&reader, &mut cache, Some((start.lat, start.lon)), Some((&route, proj))).unwrap();
    assert!(riding.projected);

    // No frame reports rain — the wet cell is outside every frame's one-cell warning corridor.
    assert!(
        riding.frames.iter().all(|f| f.intensity == 0),
        "the band-1 cell must sit outside the warning corridor, or this tests a warning and not a claim"
    );
    // The anchor frame's corridor is one cell wide and genuinely clean; every later frame's
    // corridor reaches the band-1 cell and must refuse its dry claim.
    assert!(!riding.frames[0].spread_uncertain, "the anchor's one-cell corridor is clean");
    for (k, frame) in riding.frames.iter().enumerate().skip(1) {
        assert!(frame.spread_uncertain, "frame {k}: a corridor reaching band 1 must refuse the dry claim");
    }

    // The headline: not dry, and not a warning either — an honest "I can't promise two dry hours".
    assert_eq!(
        rain_outlook(&riding, T0),
        RainOutlook::UpdateNeeded,
        "an interpolated corridor probe would floor band 1 to dry and fabricate DRY FOR 2 HOURS here"
    );
}
