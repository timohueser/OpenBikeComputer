//! WX12 (#1197) route-projection pins: `WeatherSnapshot::sample_along` samples every rain frame
//! at the rider's **expected route position** for that frame's timestamp (progress + pace × Δt,
//! clamped at the route end), inside a conservative one-cell corridor — so the two-hour decision
//! answers for the *ride*, not the parking spot. Deterministic across firmware and simulator:
//! everything here is the production reader/route/snapshot path over committed fixture bytes.

use obc_app::weather::{rain_outlook, RainOutlook, RideProjection, WeatherSnapshot};
use obc_formats::io::SliceSource;
use obc_formats::obcw::{
    encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, HOURLY_COUNT, QUALITY_FORECAST, TILE_CELLS,
};
use obc_route::{RouteIndex, RouteReader};
use obc_weather::{WeatherCache, WeatherReader};

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
    (south - 20_000, west - 20_000, north + 20_000, east + 20_000)
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
