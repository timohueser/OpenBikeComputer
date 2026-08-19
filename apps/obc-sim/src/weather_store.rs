//! Host fixture adapter for the production OBCW reader.

#![allow(dead_code)]

use std::path::Path;

/// The `--weather demo:<scenario>` cell patterns (WX10 look-tuning material): each is a pure
/// deterministic `(row, col, drift) → intensity` function on the 48 × 48 demo grid, chosen to
/// exercise a visually distinct slice of the firmware-owned `RAIN_STYLE` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoScenario {
    /// Four showers from drizzle to a torrential core — the default; mostly-dry map with hard
    /// cell edges, the "does the dither read as transparency?" scene.
    Scattered,
    /// Wide, patchy intensity 1–3 — the low-coverage end of the table, worst case for legibility
    /// of the basemap through rain.
    Drizzle,
    /// A sharp southwest–northeast front: dry ahead, drizzle edge, heavy core band behind — the
    /// "no smoothing at a boundary" scene.
    Frontal,
    /// One large violent cell with a violet ≥50 mm/h core over a heavy field — the high-coverage
    /// end, worst case for map legibility under rain.
    Storm,
    /// Everywhere dry, all nine frames — the dashboard's DRY FOR 2 HOURS state (complete
    /// two-hour coverage, every sample dry).
    Dry,
    /// An approaching light-rain front that reaches the map centre a few frames in — the
    /// dashboard's RAIN IN NN MIN state.
    Incoming,
    /// **WX12's route-projection scenario**: dry within 8 cells of the grid centre, a stationary
    /// ≥ 10 mm/h ring 8–14 cells out, moderate rain beyond. A parked rider at the centre reads
    /// DRY FOR 2 HOURS; a rider projected along a route crosses the ring — on the Grimsel
    /// fixture, ~44 min out with the sweep's `--at 1500` replay (inside the 45-min heavy-rain
    /// horizon, so the engine fires RAIN AHEAD). Deterministic in every direction.
    StormAhead,
    /// **WX12's gust scenario**: every frame dry, but the hourly rows forecast 22 m/s gusts —
    /// the dashboard stays calm while the dangerous-gust alert fires.
    Gusty,
    /// [`StormAhead`](DemoScenario::StormAhead)'s moderate sibling: the same stationary ring at
    /// band 6 (~2–5 mm/h — below every alert threshold), so the projected dashboard reads
    /// RAIN IN NN with **no** alert card — the decision-card state photographed clean.
    RainAhead,
}

impl DemoScenario {
    fn cell(self, row: usize, col: usize, drift: i64) -> u8 {
        let (r, c) = (row as i64, col as i64 - drift);
        let clamp = |v: i64| v.clamp(0, 12) as u8;
        match self {
            // Four blobs (row, col, peak intensity): quadratic falloff into the 1..=12 bands.
            DemoScenario::Scattered => {
                const BLOBS: [(i64, i64, i64); 4] = [(14, 30, 13), (30, 16, 9), (38, 38, 7), (20, 8, 5)];
                let mut best = 0i64;
                for (br, bc, peak) in BLOBS {
                    let (dr, dc) = (r - br, c - bc);
                    best = best.max(peak - (dr * dr + dc * dc) / 6);
                }
                clamp(best)
            }
            // Patchy 1–3: a coarse deterministic hash keeps ~half the cells dry.
            DemoScenario::Drizzle => {
                let h = (r / 3).wrapping_mul(7).wrapping_add((c / 3).wrapping_mul(13)) % 8;
                clamp(h - 4)
            }
            // Distance to the diagonal front line `row + col = 48 + drift`: dry ahead (positive),
            // ramping through the bands behind it.
            DemoScenario::Frontal => {
                let behind = (r + c) - 48;
                clamp(behind / 2)
            }
            // A violent core at the grid center over a broad heavy field.
            DemoScenario::Storm => {
                let (dr, dc) = (r - 24, c - 24);
                let d2 = dr * dr + dc * dc;
                clamp(14 - d2 / 24)
            }
            DemoScenario::Dry => 0,
            // A light SW front reaching the centre around the fourth 15-minute frame: dry ahead,
            // drizzle-to-moderate bands behind — never storm-grade, so the card reads RAIN IN.
            DemoScenario::Incoming => {
                let behind = (row as i64 + col as i64) - 84 + drift * 6;
                clamp(behind / 2).min(6)
            }
            // Stationary radial ring (deliberately no drift — the *projection* supplies the
            // motion): dry core, storm ring, moderate field. Uses the raw column (not the
            // drifted `c`) so every frame is identical.
            DemoScenario::StormAhead | DemoScenario::RainAhead => {
                let ring = if self == DemoScenario::StormAhead { 10 } else { 6 };
                let (dr, dc) = (r - 24, col as i64 - 24);
                let d2 = dr * dr + dc * dc;
                if d2 < 8 * 8 {
                    0
                } else if d2 < 14 * 14 {
                    ring
                } else {
                    3
                }
            }
            DemoScenario::Gusty => 0,
        }
    }
}

/// The sim's loaded weather store for the frame loop (WX10): one validated bundle held
/// resident (a host convenience — the device streams from the flat card; the *shared* path is the adapter +
/// renderer this hands each frame), plus the WX7 fixed cache, which is keyed by
/// generation + bundle CRC and therefore survives across frames and reloads safely.
pub struct SimWeather {
    bytes: Vec<u8>,
    /// Full-validation proof for `bytes`: sampling and rain leases fast-reopen from this rather
    /// than CRC-walking/decoding every tile on each GUI frame.
    mount: obc_weather::ValidatedBundle,
    cache: obc_weather::WeatherCache,
    /// `--weather-now` override; `None` treats the bundle's own first frame as current — the
    /// deterministic-fixture default that makes `--weather <file.obcw> --png` render rain out of the box.
    now_override: Option<i64>,
    /// The `--weather demo` recipe, retained so [`sync_clock`](Self::sync_clock) can re-anchor
    /// the bundle onto the app's real clock: the screens compare frame timestamps against that
    /// clock, and a bundle pinned to the deterministic fixture instant is months stale (or
    /// future) in a live GUI session — every honest state then reads WEATHER UPDATE NEEDED.
    /// `None` for a store loaded from disk (real bundles must age truthfully).
    demo_recipe: Option<DemoRecipe>,
    /// The instant the demo bundle is currently stamped at (`GENERATED_AT` until re-anchored).
    anchor: i64,
}

/// The deterministic fixture instant demo bundles are born at (2027-01-15T08:00Z): headless
/// commands pin the app clock here (the `weather_clock` rule in `main`), so previews and
/// snapshot sweeps are byte-stable. A live session re-anchors away from it via `sync_clock`.
const DEMO_GENERATED_AT: i64 = 1_800_000_000;
/// Live-clock drift beyond which `sync_clock` re-stamps the demo bundle (5 min — the tightest
/// real refresh cadence the phone offers, safely inside the ~900 s window after which the dry
/// scenario honestly expires).
const DEMO_REANCHOR_LIVE_S: i64 = 300;
/// Scripted-clock threshold: a headless script can elapse synthetic minutes (`I` alone is
/// 301 s), and a re-anchor inside a scripted run would silently change preview bytes — so a
/// non-live clock only re-stamps for a jump no script can produce (review #1230 F1).
const DEMO_REANCHOR_SCRIPTED_S: i64 = 86_400;

/// The `--weather demo` shape retained for re-anchoring: `(scenario, map bbox)`.
type DemoRecipe = (Option<DemoScenario>, (i32, i32, i32, i32));

impl SimWeather {
    /// Resolve `--weather`'s argument: `demo` / `demo:<scenario>` synthesizes a deterministic
    /// bundle over the loaded map's bbox ([`demo`](Self::demo) — scenarios in [`DemoScenario`];
    /// `demo:hourly` builds an hourly-only bundle with **no** rain frames, the WX11 explicit
    /// hourly-only state); anything else is one OBCW file ([`load`](Self::load)).
    pub fn from_arg(arg: &str, now_override: Option<i64>, map_bbox: (i32, i32, i32, i32)) -> Option<Self> {
        if let Some(rest) = arg.strip_prefix("demo") {
            let scenario = match rest.strip_prefix(':').unwrap_or("") {
                "" | "scattered" => Some(DemoScenario::Scattered),
                "drizzle" => Some(DemoScenario::Drizzle),
                "frontal" => Some(DemoScenario::Frontal),
                "storm" => Some(DemoScenario::Storm),
                "dry" => Some(DemoScenario::Dry),
                "incoming" => Some(DemoScenario::Incoming),
                "stormahead" => Some(DemoScenario::StormAhead),
                "rainahead" => Some(DemoScenario::RainAhead),
                "gusty" => Some(DemoScenario::Gusty),
                "hourly" => None,
                other => {
                    eprintln!(
                        "--weather demo:{other}: unknown scenario (scattered|drizzle|frontal|storm|dry|incoming|stormahead|rainahead|gusty|hourly)"
                    );
                    return None;
                }
            };
            Some(Self::demo(scenario, map_bbox, now_override))
        } else {
            Self::load(Path::new(arg), now_override)
        }
    }

    /// The instant this store treats as "now" when the app clock isn't pinned: the explicit
    /// `--weather-now`, else the bundle's first rain frame (so demo/fixture rain renders out of
    /// the box), else `valid_from` for a frameless (hourly-only) bundle.
    pub fn effective_now(&self) -> Option<i64> {
        if let Some(now) = self.now_override {
            return Some(now);
        }
        let source = obc_formats::io::SliceSource(&self.bytes);
        let reader = self.mount.reader(&source).ok()?;
        Some(reader.frame(0).map(|f| f.valid_at).unwrap_or(reader.header().valid_from))
    }

    /// Sample the loaded bundle into the production resident [`WeatherSnapshot`]
    /// (`obc-app`'s — the exact struct the screens consume) at `pos` (`(lat, lon)` µdeg), with
    /// frame samples advanced along the active route when the app supplied a WX12
    /// [`RideProjection`](obc_app::RideProjection) — the production `sample_along` path.
    pub fn snapshot(
        &mut self,
        pos: Option<(i32, i32)>,
        projection: Option<(&obc_route::RouteReader<'_>, obc_app::RideProjection)>,
    ) -> Option<obc_app::WeatherSnapshot> {
        let source = obc_formats::io::SliceSource(&self.bytes);
        let reader = self.mount.reader(&source).ok()?;
        obc_app::WeatherSnapshot::sample_along(&reader, &mut self.cache, pos, projection).ok()
    }

    /// The held bundle's bytes — the companion reads its generation/timestamp for the §11.4
    /// context and producer-generation comparison.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Header identity from the store's one full-validation pass. Companion polling uses this
    /// instead of reopening and CRC-walking the whole resident bundle every GUI frame.
    pub fn validated_identity(&self) -> (u32, i64, u32) {
        let header = self.mount.header();
        (header.generation, header.generated_at, header.crc32)
    }

    /// Adopt an in-memory bundle (the `--weather live` path, and the companion's commit): the
    /// bytes must still be a valid OBCW object, so a service that answered with nonsense produces
    /// `None` rather than a store the screens would have to defend against.
    ///
    /// No `demo_recipe`, deliberately — a live bundle is a real observation and must **age**.
    /// Re-stamping it onto the clock the way a demo bundle is re-anchored would turn a stalled
    /// baker into a permanently fresh-looking nowcast, which is the exact lie the epic forbids.
    pub fn from_bytes(bytes: Vec<u8>, now_override: Option<i64>) -> Option<Self> {
        let source = obc_formats::io::SliceSource(&bytes);
        let mount = obc_weather::WeatherReader::open(&source).ok()?.validated();
        Some(Self { bytes, mount, cache: obc_weather::WeatherCache::new(), now_override, demo_recipe: None, anchor: 0 })
    }

    /// Load and validate one OBCW file. `None` when the file is missing or malformed.
    pub fn load(path: &Path, now_override: Option<i64>) -> Option<Self> {
        Self::from_bytes(std::fs::read(path).ok()?, now_override)
    }

    /// A deterministic in-memory demo bundle over `(west, south, east, north)` microdegrees: a
    /// 48 × 48-cell grid, nine 15-minute frames (the radar-de policy shape, so the WX11 two-hour
    /// derivations have full coverage) whose cells come from the chosen [`DemoScenario`],
    /// drifting two cells east per frame — or **no** frames at all (`None`: the hourly-only
    /// bundle). Exercises the exact adapter → renderer path against any loaded map — cell edges
    /// stay hard (nearest-neighbour, no smoothing), so the scenarios double as look-tuning
    /// material for the WX10/WX11 review rounds.
    pub fn demo(scenario: Option<DemoScenario>, bbox: (i32, i32, i32, i32), now_override: Option<i64>) -> Self {
        let bytes = Self::demo_bundle(scenario, bbox, DEMO_GENERATED_AT);
        let mount = obc_weather::WeatherReader::open(&obc_formats::io::SliceSource(&bytes))
            .expect("generated demo weather is valid")
            .validated();
        Self {
            bytes,
            mount,
            cache: obc_weather::WeatherCache::new(),
            now_override,
            demo_recipe: Some((scenario, bbox)),
            anchor: DEMO_GENERATED_AT,
        }
    }

    /// Encode the demo bundle with every timestamp anchored at `generated_at` — the pure half of
    /// [`demo`](Self::demo), reused by [`sync_clock`](Self::sync_clock)'s re-anchor.
    fn demo_bundle(scenario: Option<DemoScenario>, bbox: (i32, i32, i32, i32), generated_at: i64) -> Vec<u8> {
        use obc_formats::obcw::{
            encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, CONDITION_MOSTLY_CLEAR,
            CONDITION_PARTLY_CLOUDY, CONDITION_RAIN, CONDITION_SHOWERS, CONDITION_THUNDERSTORM, HOURLY_COUNT,
            HOURLY_INTERVAL_SECONDS, QUALITY_FORECAST, TILE_CELLS,
        };
        let generated: i64 = generated_at;
        const GRID: usize = 48; // 3 × 3 tiles
        let (west, south, east, north) = bbox;
        // Hourly rows consistent with the chosen rain scenario (WX11 reads them on the
        // dashboard card + hourly screen), with mild deterministic variation so the hourly list
        // reads like a real day, not 24 clones.
        let (base_condition, wet_tenth_mm, wet_pct) = match scenario {
            Some(DemoScenario::Dry) | Some(DemoScenario::Gusty) => (CONDITION_MOSTLY_CLEAR, 0u16, 0u8),
            Some(DemoScenario::Incoming) => (CONDITION_SHOWERS, 8, 55),
            Some(DemoScenario::Storm) => (CONDITION_THUNDERSTORM, 62, 90),
            Some(DemoScenario::StormAhead) => (CONDITION_SHOWERS, 15, 65),
            Some(_) => (CONDITION_RAIN, 12, 60),
            None => (CONDITION_PARTLY_CLOUDY, 2, 30),
        };
        // The gusty scenario forecasts 22 m/s gusts (over the WX12 dangerous-gust threshold);
        // everything else stays a calm 7 m/s.
        let gust_deci_ms = if scenario == Some(DemoScenario::Gusty) { 220 } else { 70 };
        let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|i| HourlyRecord {
            valid_time_offset_s: i as u32 * HOURLY_INTERVAL_SECONDS,
            temperature_deci_c: 95 + ((i as i16 + 3) % 12) * 14,
            precipitation_tenth_mm: if i == 0 && scenario == Some(DemoScenario::Incoming) { 0 } else { wet_tenth_mm },
            precipitation_probability_pct: wet_pct,
            condition: if i == 0 && scenario == Some(DemoScenario::Incoming) {
                CONDITION_PARTLY_CLOUDY
            } else {
                base_condition
            },
            wind_from_deg: (200 + i as u16 * 15) % 360,
            wind_speed_deci_ms: 28 + (i as u16 % 5) * 13,
            wind_gust_deci_ms: gust_deci_ms,
            flags: 0,
        });
        let mut frames_tiles = Vec::new();
        if let Some(scenario) = scenario {
            let cell = |row: usize, col: usize, drift: i64| -> u8 { scenario.cell(row, col, drift) };
            for frame in 0..9i64 {
                let mut tiles = vec![[0u8; TILE_CELLS]; (GRID / 16) * (GRID / 16)];
                for row in 0..GRID {
                    for col in 0..GRID {
                        let tile = (row / 16) * (GRID / 16) + col / 16;
                        tiles[tile][(row % 16) * 16 + col % 16] = cell(row, col, frame * 2);
                    }
                }
                frames_tiles.push(tiles);
            }
        }
        let frames: Vec<RainFrameInput<'_>> = frames_tiles
            .iter()
            .enumerate()
            .map(|(i, tiles)| RainFrameInput {
                valid_at: generated + i as i64 * 900,
                width: GRID as u16,
                height: GRID as u16,
                cell_size_m: 1_000,
                quality_flags: QUALITY_FORECAST,
                tiles,
            })
            .collect();
        let input = BundleInput {
            generation: 1,
            request_id: 0xDEED_0001,
            generated_at: generated,
            valid_from: generated,
            valid_until: generated + 24 * 3_600,
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
        let mut bytes = vec![0u8; encoded_len(&input).expect("demo bundle length") as usize];
        let len = encode_format(&input, &mut bytes).expect("demo bundle encode");
        bytes.truncate(len);
        bytes
    }

    /// Re-anchor a demo bundle onto the app's clock once it drifts past the threshold — the
    /// sim's stand-in for the phone's periodic refresh. `live` says whether `now` comes from a
    /// real clock (the GUI's `SimClock`): live drifts re-stamp at 5 min so the honest states
    /// stay honest; scripted/headless clocks only for a jump no script can produce, keeping
    /// previews byte-stable. `--weather-now` always wins (the deterministic stale-scenario
    /// tool) and disk-loaded bundles age truthfully.
    ///
    /// Deliberate trade (review #1230 F5): under a live clock the demo can never age into the
    /// stale states — those stay reachable headlessly via `--weather-now`, by design; do not
    /// "fix" the re-stamp away.
    pub fn sync_clock(&mut self, now: i64, live: bool) {
        let Some((scenario, bbox)) = self.demo_recipe else { return };
        let threshold = if live { DEMO_REANCHOR_LIVE_S } else { DEMO_REANCHOR_SCRIPTED_S };
        if self.now_override.is_some() || (now - self.anchor).abs() <= threshold {
            return;
        }
        self.bytes = Self::demo_bundle(scenario, bbox, now);
        self.mount = obc_weather::WeatherReader::open(&obc_formats::io::SliceSource(&self.bytes))
            .expect("re-anchored bundle valid")
            .validated();
        self.anchor = now;
        // The tile cache keys on generation + bundle CRC, so the re-anchored bytes miss cleanly.
    }

    /// Run `frame` with this frame's rain lease: the production
    /// [`RainOverlayAdapter`](obc_app::RainOverlayAdapter) over the loaded bundle, or `None` when
    /// no frame is current at the effective instant (then the map renders rain-free, exactly as
    /// the device would). Closure-shaped because the adapter borrows a reader that borrows the
    /// bytes; nothing outlives the call.
    /// `now` is the app's live wall clock (`App::wall_unix_now`), so the lease and the screens'
    /// own freshness derivations are one instant — before review F5 the lease anchored on the
    /// bundle's first frame forever, so after ~15 min of GUI runtime the screens' time-step
    /// labels and the rendered raster silently diverged. An explicit `--weather-now` override
    /// still wins (the deterministic stale-scenario tool; the clock-anchor rule in `main`/`gui`
    /// makes the two coincide for every fixture command that doesn't pass `--clock`).
    pub fn lease<R>(
        &mut self,
        now: i64,
        step: u8,
        frame: impl FnOnce(Option<&mut dyn obc_render::RainOverlaySource>) -> R,
    ) -> R {
        // No self-sync: the caller synced this frame and knows whether its clock is live.
        let source = obc_formats::io::SliceSource(&self.bytes);
        let Ok(reader) = self.mount.reader(&source) else {
            return frame(None);
        };
        let now = self.now_override.unwrap_or(now);
        let adapter = obc_app::RainOverlayAdapter::at_step(&reader, &mut self.cache, now, step);
        match adapter {
            Some(mut adapter) => frame(Some(&mut adapter)),
            None => frame(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEATHER: &[u8] = include_bytes!("../../../specs/vectors/weather-minimal-dry.obcw");

    #[test]
    fn single_file_loader_validates_before_adopting_bytes() {
        let mut path = std::env::temp_dir();
        path.push(format!("obc-weather-{}-{}.obcw", std::process::id(), line!()));
        std::fs::write(&path, WEATHER).unwrap();
        assert_eq!(SimWeather::load(&path, None).unwrap().bytes(), WEATHER);
        std::fs::write(&path, &WEATHER[..511]).unwrap();
        assert!(SimWeather::load(&path, None).is_none());
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod sync_clock_tests {
    use super::*;

    const BBOX: (i32, i32, i32, i32) = (7_000_000, 46_000_000, 9_000_000, 48_000_000);

    /// A live clock months away from the fixture instant re-anchors the demo bundle, so the
    /// production reader finds a current frame — the GUI-replay regression this exists for.
    /// Forward and backward drifts both re-anchor, and a repeat at the same instant is a byte
    /// no-op (the churn invariant: the anchor must actually move — review #1230 F2).
    #[test]
    fn a_drifted_clock_reanchors_the_demo_bundle() {
        let mut w = SimWeather::demo(Some(DemoScenario::Storm), BBOX, None);
        let forward = DEMO_GENERATED_AT + 12_000_000;
        let before = w.bytes.clone();
        w.sync_clock(forward, true);
        assert_ne!(w.bytes, before, "forward drift re-stamps");
        let after_first = w.bytes.clone();
        w.sync_clock(forward, true);
        assert_eq!(w.bytes, after_first, "same-instant repeat is a byte no-op (no churn)");
        w.sync_clock(forward + 1, true);
        assert_eq!(w.bytes, after_first, "inside the live threshold from the NEW anchor");
        let real_now = DEMO_GENERATED_AT - 12_000_000; // months before the fixture instant
        w.sync_clock(real_now, true);
        assert_ne!(w.bytes, before, "bundle must re-stamp onto the drifted clock");
        let source = obc_formats::io::SliceSource(&w.bytes);
        let reader = obc_weather::WeatherReader::open(&source).expect("re-anchored bundle valid");
        let mut cache = obc_weather::WeatherCache::new();
        let current = reader.current_frame(real_now, &mut cache).expect("io");
        assert_eq!(current.map(|(index, _)| index), Some(0), "first frame current at the real clock");
    }

    /// Within the re-anchor threshold nothing changes — headless preview commands (whose clock
    /// is pinned to the fixture instant) stay byte-identical.
    #[test]
    fn a_pinned_clock_keeps_the_bundle_bytes() {
        let mut w = SimWeather::demo(Some(DemoScenario::Storm), BBOX, None);
        let before = w.bytes.clone();
        w.sync_clock(DEMO_GENERATED_AT + DEMO_REANCHOR_LIVE_S, true);
        assert_eq!(w.bytes, before, "no re-stamp inside the live threshold");
        w.sync_clock(DEMO_GENERATED_AT + DEMO_REANCHOR_SCRIPTED_S, false);
        assert_eq!(w.bytes, before, "a scripted clock ignores script-reachable elapse (I = 301 s)");
        w.sync_clock(DEMO_GENERATED_AT + DEMO_REANCHOR_SCRIPTED_S + 1, false);
        assert_ne!(w.bytes, before, "a real-world --clock jump still lands the anchor");
    }

    /// A store loaded from disk is a real bundle — it ages truthfully, never re-stamps.
    #[test]
    fn a_disk_loaded_bundle_never_reanchors() {
        let mut w = SimWeather::demo(Some(DemoScenario::Storm), BBOX, None);
        w.demo_recipe = None; // the load() shape, without needing a fixture on disk
        let before = w.bytes.clone();
        w.sync_clock(DEMO_GENERATED_AT + 12_000_000, true);
        assert_eq!(w.bytes, before, "no recipe, no re-stamp");
    }

    /// `--weather-now` is the deterministic stale-scenario tool — it must always win.
    #[test]
    fn a_now_override_disables_reanchoring() {
        let mut w = SimWeather::demo(Some(DemoScenario::Storm), BBOX, Some(DEMO_GENERATED_AT + 1_500));
        let before = w.bytes.clone();
        w.sync_clock(DEMO_GENERATED_AT + 12_000_000, true);
        assert_eq!(w.bytes, before, "override pins the bundle");
    }
}
