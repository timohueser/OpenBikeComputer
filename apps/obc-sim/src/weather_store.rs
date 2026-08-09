//! Host filesystem adapter for the production OBCW reader/slot selector.
//!
//! WX14 will attach transfers and controls. WX7 deliberately adds only the truthful file seam so
//! simulator tests and firmware boot already make byte-for-byte identical A/B decisions.

#![allow(dead_code)]

use std::{
    cell::RefCell,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use obc_formats::io::{ByteSource, Error as SourceError};
use obc_weather::{select_slots, validate_slot, Candidate, Slot, SlotSelection, SlotValidation};

pub struct FileSource {
    file: RefCell<File>,
    len: u32,
}

impl FileSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let raw_len = file.metadata()?.len();
        let len = u32::try_from(raw_len)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "OBCW file exceeds uint32 length"))?;
        Ok(Self { file: RefCell::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u32, out: &mut [u8]) -> Result<(), SourceError> {
        let end = offset
            .checked_add(u32::try_from(out.len()).map_err(|_| SourceError::BadOffset)?)
            .ok_or(SourceError::BadOffset)?;
        if end > self.len {
            return Err(SourceError::BadOffset);
        }
        let mut file = self.file.try_borrow_mut().map_err(|_| SourceError::Io)?;
        file.seek(SeekFrom::Start(offset as u64)).map_err(|_| SourceError::Io)?;
        file.read_exact(out).map_err(|_| SourceError::Io)
    }

    fn len(&self) -> u32 {
        self.len
    }
}

pub fn inspect_root(root: &Path) -> SlotSelection {
    select_slots(inspect(root, Slot::A), inspect(root, Slot::B))
}

pub fn open_active(root: &Path, selection: SlotSelection) -> std::io::Result<Option<(Candidate, FileSource)>> {
    let Some(expected) = selection.active else { return Ok(None) };
    let source = FileSource::open(&root.join(expected.slot.root_file_name()))?;
    match validate_slot(expected.slot, &source) {
        SlotValidation::Valid(actual) if actual == expected => Ok(Some((actual, source))),
        _ => Ok(None),
    }
}

fn inspect(root: &Path, slot: Slot) -> SlotValidation {
    match FileSource::open(&root.join(slot.root_file_name())) {
        Ok(source) => validate_slot(slot, &source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SlotValidation::Missing,
        Err(_) => SlotValidation::Unreadable,
    }
}

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
        }
    }
}

/// The sim's loaded weather store for the frame loop (WX10): the active slot's bundle held
/// resident (a host convenience — the device streams from SD; the *shared* path is the adapter +
/// renderer this hands each frame), plus the WX7 fixed cache, which is keyed by
/// generation + bundle CRC and therefore survives across frames and reloads safely.
pub struct SimWeather {
    bytes: Vec<u8>,
    cache: obc_weather::WeatherCache,
    /// `--weather-now` override; `None` treats the bundle's own first frame as current — the
    /// deterministic-fixture default that makes `--weather <dir> --png` render rain out of the box.
    now_override: Option<i64>,
}

impl SimWeather {
    /// Resolve `--weather`'s argument: `demo` / `demo:<scenario>` synthesizes a deterministic
    /// bundle over the loaded map's bbox ([`demo`](Self::demo) — scenarios in [`DemoScenario`]);
    /// anything else is a WEATHER.A/WEATHER.B store root ([`load`](Self::load)).
    pub fn from_arg(arg: &str, now_override: Option<i64>, map_bbox: (i32, i32, i32, i32)) -> Option<Self> {
        if let Some(rest) = arg.strip_prefix("demo") {
            let scenario = match rest.strip_prefix(':').unwrap_or("") {
                "" | "scattered" => DemoScenario::Scattered,
                "drizzle" => DemoScenario::Drizzle,
                "frontal" => DemoScenario::Frontal,
                "storm" => DemoScenario::Storm,
                other => {
                    eprintln!("--weather demo:{other}: unknown scenario (scattered|drizzle|frontal|storm)");
                    return None;
                }
            };
            Some(Self::demo(scenario, map_bbox, now_override))
        } else {
            Self::load(Path::new(arg), now_override)
        }
    }

    /// Load the newest valid generation from a WEATHER.A/WEATHER.B root, exactly as boot selection
    /// does. `None` when neither slot holds a valid bundle.
    pub fn load(root: &Path, now_override: Option<i64>) -> Option<Self> {
        let selection = inspect_root(root);
        let (candidate, _) = open_active(root, selection).ok().flatten()?;
        let bytes = std::fs::read(root.join(candidate.slot.root_file_name())).ok()?;
        Some(Self { bytes, cache: obc_weather::WeatherCache::new(), now_override })
    }

    /// A deterministic in-memory demo bundle over `(west, south, east, north)` microdegrees: a
    /// 48 × 48-cell grid, three 15-minute frames whose cells come from the chosen
    /// [`DemoScenario`], drifting two cells east per frame. Exercises the exact adapter →
    /// renderer path against any loaded map — cell edges stay hard (nearest-neighbour, no
    /// smoothing), so the scenarios double as look-tuning material for the WX10 review rounds.
    pub fn demo(scenario: DemoScenario, bbox: (i32, i32, i32, i32), now_override: Option<i64>) -> Self {
        use obc_formats::obcw::{
            encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, CONDITION_RAIN, HOURLY_COUNT,
            HOURLY_INTERVAL_SECONDS, QUALITY_FORECAST, TILE_CELLS,
        };
        const GENERATED_AT: i64 = 1_800_000_000;
        const GRID: usize = 48; // 3 × 3 tiles
        let (west, south, east, north) = bbox;
        let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|i| HourlyRecord {
            valid_time_offset_s: i as u32 * HOURLY_INTERVAL_SECONDS,
            temperature_deci_c: 140,
            precipitation_tenth_mm: 12,
            precipitation_probability_pct: 60,
            condition: CONDITION_RAIN,
            wind_from_deg: 240,
            wind_speed_deci_ms: 40,
            wind_gust_deci_ms: 70,
            flags: 0,
        });
        let cell = |row: usize, col: usize, drift: i64| -> u8 { scenario.cell(row, col, drift) };
        let mut frames_tiles = Vec::new();
        for frame in 0..3i64 {
            let mut tiles = vec![[0u8; TILE_CELLS]; (GRID / 16) * (GRID / 16)];
            for row in 0..GRID {
                for col in 0..GRID {
                    let tile = (row / 16) * (GRID / 16) + col / 16;
                    tiles[tile][(row % 16) * 16 + col % 16] = cell(row, col, frame * 2);
                }
            }
            frames_tiles.push(tiles);
        }
        let frames: Vec<RainFrameInput<'_>> = frames_tiles
            .iter()
            .enumerate()
            .map(|(i, tiles)| RainFrameInput {
                valid_at: GENERATED_AT + i as i64 * 900,
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
            generated_at: GENERATED_AT,
            valid_from: GENERATED_AT,
            valid_until: GENERATED_AT + 24 * 3_600,
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
        Self { bytes, cache: obc_weather::WeatherCache::new(), now_override }
    }

    /// Run `frame` with this frame's rain lease: the production
    /// [`RainOverlayAdapter`](obc_app::RainOverlayAdapter) over the loaded bundle, or `None` when
    /// no frame is current at the effective instant (then the map renders rain-free, exactly as
    /// the device would). Closure-shaped because the adapter borrows a reader that borrows the
    /// bytes; nothing outlives the call.
    pub fn lease<R>(&mut self, frame: impl FnOnce(Option<&mut dyn obc_render::RainOverlaySource>) -> R) -> R {
        let source = obc_formats::io::SliceSource(&self.bytes);
        let Ok(reader) = obc_weather::WeatherReader::open(&source) else {
            return frame(None);
        };
        let now = self.now_override.or_else(|| reader.frame(0).ok().map(|f| f.valid_at));
        let adapter = now.and_then(|now| obc_app::RainOverlayAdapter::current(&reader, &mut self.cache, now));
        match adapter {
            Some(mut adapter) => frame(Some(&mut adapter)),
            None => frame(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const A: &[u8] = include_bytes!("../../../specs/vectors/weather-minimal-dry.obcw");
    const B: &[u8] = include_bytes!("../../../specs/vectors/weather-dwd-96x96-9f.obcw");

    struct TempRoot(std::path::PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("obc-wx7-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn file_adapter_matches_the_shared_slice_selector() {
        let root = TempRoot::new("parity");
        fs::write(root.0.join(Slot::A.root_file_name()), A).unwrap();
        fs::write(root.0.join(Slot::B.root_file_name()), B).unwrap();
        let file_selection = inspect_root(&root.0);
        let slice_selection = select_slots(
            validate_slot(Slot::A, &obc_formats::io::SliceSource(A)),
            validate_slot(Slot::B, &obc_formats::io::SliceSource(B)),
        );
        assert_eq!(file_selection, slice_selection);
        let (candidate, source) = open_active(&root.0, file_selection).unwrap().unwrap();
        assert_eq!(validate_slot(candidate.slot, &source), SlotValidation::Valid(candidate));
    }

    #[test]
    fn corrupt_or_missing_files_fail_closed_without_whole_file_allocation() {
        let root = TempRoot::new("corrupt");
        fs::write(root.0.join(Slot::A.root_file_name()), A).unwrap();
        fs::write(root.0.join(Slot::B.root_file_name()), &B[..511]).unwrap();
        let selection = inspect_root(&root.0);
        assert_eq!(selection.active.unwrap().slot, Slot::A);
        fs::remove_file(root.0.join(Slot::A.root_file_name())).unwrap();
        assert_eq!(inspect_root(&root.0).active, None);
    }
}
