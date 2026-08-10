//! WXR1 (#1240) — the throwaway measurement spike that gates the weather rewrite (epic #1248).
//!
//! It answers four questions with numbers instead of arithmetic-on-a-napkin:
//!
//! 1. what one **global 0.01 degree cycle** costs in wall time and peak RSS on a 4-core box;
//! 2. what a general **LZ codec** buys over the OBCG raw4/RLE4 pair on genuinely upsampled
//!    coarse data, on native 1 km radar texture and on dry fields;
//! 3. which **tile edge** and **shard size** to publish on, against the hard 30 M-cell
//!    `obcg::MAX_GRID_CELLS` ceiling and against what a 90 km corridor has to fetch;
//! 4. how big the **OBCW corridor bundle** actually is at 1 km / 90 km / nine frames.
//!
//! Everything runs off the checked-in upstream fixtures — no network. The four real products
//! (DWD RV 1 km, ICON-EU 6.5 km, MRMS+HRRR, GFS 27.75 km) are decoded through the production
//! adapters and then **mosaicked with cell replication** onto the canonical global lattice: a
//! synthetic global scene made of real texture. The fixtures were captured at two different wall
//! clocks (14:20Z for the European pair, 16:58Z for the American pair), so the composed scene is
//! not a real instant of weather — it is a realistic *shape* of one, which is all a size and CPU
//! measurement needs.
//!
//! This module is deliberately not wired into `cycle`, `emit` or `publish`: it is a measurement
//! rig with a shelf life, and WXR7 deletes it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use obc_formats::obcg::{self, FrameInput};
use obc_formats::precip4;
use rayon::prelude::*;

use crate::fetch::FixtureUpstream;
use crate::geometry::GridGeometry;
use crate::source::{dwd_rv, gfs, hrrr, icon_eu, mrms, us, Adapter, AdapterOutcome, BakedProduct};

// ---------------------------------------------------------------------------------------------
// The canonical lattice under test: global 0.01 degrees, 36,000 x 18,000 = 648 M cells.
//
// Since WXR3 (#1242) these are aliases of the production lattice in `crate::canonical`, so the
// spike can never quietly measure a different grid than the one the baker publishes. What the
// spike still keeps of its own is the *scene*: it composes fixtures captured at two different
// wall clocks by lead offset, which the production time axis (nearest validity within
// `canonical::MAX_FRAME_SKEW_S`) would correctly refuse to do. That fake global instant is the
// whole point of a size measurement, and it is why this rig is not built on the production
// mosaic. WXR7 deletes it.
// ---------------------------------------------------------------------------------------------

pub const CELL_UDEG: u32 = crate::canonical::CANONICAL.cell_udeg;
pub const LATTICE_WIDTH: u32 = crate::canonical::CANONICAL.width;
pub const LATTICE_HEIGHT: u32 = crate::canonical::CANONICAL.height;
pub const LATTICE_SOUTH_UDEG: i32 = crate::canonical::CANONICAL.south_lat_udeg;
pub const LATTICE_WEST_UDEG: i32 = crate::canonical::CANONICAL.west_lon_udeg;
/// Nine frames, +0 .. +120 min in 15-minute steps.
pub const CYCLE_FRAMES: u32 = crate::canonical::CYCLE_FRAMES;
pub const FRAME_STEP_MIN: i32 = crate::canonical::FRAME_STEP_MIN as i32;
/// The whole point of the spike: one cycle must fit here, with fetch and publish alongside it.
pub const CYCLE_BUDGET_SECONDS: f64 = 300.0;
/// The production box: 4 vCPU / 8 GB KVM.
pub const BOX_RAM_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// 96 cycles a day for 30 days: the R2 class-A write multiplier.
pub const CYCLES_PER_MONTH: u64 = 96 * 30;

// ---------------------------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------------------------

struct Options {
    fixtures: PathBuf,
    threads: usize,
    tile_edges: Vec<u16>,
    shards: Vec<(u32, u32)>,
    phases: Vec<Phase>,
    dump_tiles: Option<PathBuf>,
    sample_tiles: usize,
    entries_per_page: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Cycle,
    Shard,
    Codec,
    Corridor,
}

impl Phase {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "cycle" => Ok(Self::Cycle),
            "shard" => Ok(Self::Shard),
            "codec" => Ok(Self::Codec),
            "corridor" => Ok(Self::Corridor),
            other => Err(format!("unknown phase {other} (cycle|shard|codec|corridor)")),
        }
    }
}

pub fn usage() -> String {
    "obc-wx-bake spike [--fixtures <dir>] [--threads <n>] [--tile-edges 64,128,256] \
     [--shards 6144x4608,3072x3072] [--only cycle,shard,codec,corridor] \
     [--dump-tiles <dir>] [--sample-tiles <n>] [--entries-per-page <n>]"
        .to_string()
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        fixtures: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        threads: 4,
        tile_edges: vec![64, 128, 256],
        shards: vec![(6_144, 4_608), (6_144, 3_072), (3_072, 3_072), (1_536, 1_536), (1_024, 1_024)],
        phases: vec![Phase::Cycle, Phase::Shard, Phase::Codec, Phase::Corridor],
        dump_tiles: None,
        sample_tiles: 512,
        entries_per_page: 512,
    };
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{argument} needs a value"));
        match argument.as_str() {
            "--fixtures" => options.fixtures = PathBuf::from(value()?),
            "--threads" => options.threads = value()?.parse().map_err(|_| "--threads wants an integer")?,
            "--tile-edges" => {
                options.tile_edges = value()?
                    .split(',')
                    .map(|text| text.parse::<u16>().map_err(|_| format!("bad tile edge {text}")))
                    .collect::<Result<_, _>>()?;
            }
            "--shards" => options.shards = value()?.split(',').map(parse_shard).collect::<Result<_, _>>()?,
            "--only" => options.phases = value()?.split(',').map(Phase::parse).collect::<Result<_, _>>()?,
            "--dump-tiles" => options.dump_tiles = Some(PathBuf::from(value()?)),
            "--sample-tiles" => {
                options.sample_tiles = value()?.parse().map_err(|_| "--sample-tiles wants an integer")?
            }
            "--entries-per-page" => {
                options.entries_per_page = value()?.parse().map_err(|_| "--entries-per-page wants an integer")?;
            }
            other => return Err(format!("unknown argument {other}\n{}", usage())),
        }
    }
    if options.threads == 0 || options.tile_edges.is_empty() || options.shards.is_empty() {
        return Err("--threads, --tile-edges and --shards must all be non-empty".to_string());
    }
    if options.entries_per_page == 0 || options.entries_per_page > obcg::MAX_ENTRIES_PER_PAGE {
        return Err(format!("--entries-per-page must be 1..={}", obcg::MAX_ENTRIES_PER_PAGE));
    }
    Ok(options)
}

fn parse_shard(text: &str) -> Result<(u32, u32), String> {
    let (width, height) = text.split_once('x').ok_or_else(|| format!("bad shard {text} (want <w>x<h>)"))?;
    let width: u32 = width.parse().map_err(|_| format!("bad shard width in {text}"))?;
    let height: u32 = height.parse().map_err(|_| format!("bad shard height in {text}"))?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > obcg::MAX_GRID_CELLS {
        return Err(format!("shard {text} is empty or over the {} cell ceiling", obcg::MAX_GRID_CELLS));
    }
    Ok((width, height))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let options = parse_args(args)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.threads)
        .build()
        .map_err(|error| format!("rayon pool: {error}"))?;

    println!("# WXR1 spike (#1240)");
    println!(
        "lattice {LATTICE_WIDTH} x {LATTICE_HEIGHT} @ {CELL_UDEG} udeg = {} M cells, {CYCLE_FRAMES} frames/cycle",
        (u64::from(LATTICE_WIDTH) * u64::from(LATTICE_HEIGHT)) / 1_000_000
    );
    println!("threads {} (pinned; the production box is 4 vCPU / 8 GB)", options.threads);

    let decode_start = Instant::now();
    let mosaic = Mosaic::from_fixtures(&options.fixtures)?;
    println!("\n## Upstream decode (fixtures, single-threaded, production adapters)");
    println!("wall {:.2} s, peak RSS {}", decode_start.elapsed().as_secs_f64(), bytes(peak_rss_bytes()));
    for layer in &mosaic.layers {
        let anchor = &layer.frames[0].1;
        println!(
            "  {:8} {:5} frames  {:5} x {:5} @ {} udeg ({} m nominal)",
            layer.id,
            layer.frames.len(),
            anchor.geom.width,
            anchor.geom.height,
            anchor.geom.cell_lat_udeg,
            anchor.geom.cell_size_m
        );
    }

    pool.install(|| -> Result<(), String> {
        if options.phases.contains(&Phase::Cycle) {
            phase_cycle(&options, &mosaic)?;
        }
        if options.phases.contains(&Phase::Shard) {
            phase_shard(&options, &mosaic)?;
        }
        if options.phases.contains(&Phase::Codec) {
            phase_codec(&options, &mosaic)?;
        }
        if options.phases.contains(&Phase::Corridor) {
            phase_corridor(&options, &mosaic)?;
        }
        Ok(())
    })?;

    println!("\n## Process peak RSS over the whole run: {}", bytes(peak_rss_bytes()));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The mosaic: real fixtures, cell-replicated onto the canonical lattice
// ---------------------------------------------------------------------------------------------

struct SourceFrame {
    geom: GridGeometry,
    cells: Vec<u8>,
}

struct Layer {
    id: &'static str,
    /// `(lead minutes, frame)`, in publication order.
    frames: Vec<(i32, SourceFrame)>,
}

impl Layer {
    /// The frame a mosaic at `lead_min` should sample: nearest lead wins, earliest breaks ties.
    fn nearest(&self, lead_min: i32) -> &SourceFrame {
        &self
            .frames
            .iter()
            .min_by_key(|(offset, _)| ((offset - lead_min).abs(), *offset))
            .expect("a layer always has at least one frame")
            .1
    }
}

pub struct Mosaic {
    layers: Vec<Layer>,
}

impl Mosaic {
    /// Bake the four production adapters off the checked-in fixtures, coarsest layer first.
    ///
    /// Three honest caveats about the scene this produces, none of which move a byte count:
    ///
    /// - **Two wall clocks.** The European fixtures were captured at 14:20Z and the American ones
    ///   at 16:58Z, so each pair is baked against its own `now`. The composed scene is a realistic
    ///   global *shape*, not a real instant.
    /// - **The nine frames are not nine independent global scenes.** The GFS floor is hourly, so
    ///   `Layer::nearest` maps canonical leads 0..90 min onto one +60 min field and 105/120 onto
    ///   +120. Since the floor covers ~96 % of the lattice, seven of nine frames share one floor.
    ///   Tiles are encoded independently with no cross-frame coding, so sizes are unaffected — but
    ///   nothing here supports a claim about inter-frame variability.
    /// - **GFS does not quite close the globe.** Its window is 1439 x 719 cells spanning
    ///   +/-179.875 / +/-89.875 degrees, so ~1.4 M canonical cells (0.2 %) at the poles and the
    ///   antimeridian have no source at all and stay at intensity 15. Those tiles are therefore
    ///   permanently non-dry, which very slightly inflates the non-dry tile fraction.
    fn from_fixtures(dir: &Path) -> Result<Self, String> {
        let mut products = Vec::new();
        let mut european = european_upstream(dir)?;
        products.push(bake(&dwd_rv::DwdRv, &mut european, ts("2026-08-09T14:30:00Z"))?);
        products.push(bake(&icon_eu::IconEu, &mut european, ts("2026-08-09T14:30:00Z"))?);
        let mut american = american_upstream(dir)?;
        products.push(bake(&us::UsComposite, &mut american, ts("2026-08-09T17:00:00Z"))?);
        products.push(bake(&gfs::GfsFloor, &mut american, ts("2026-08-09T17:00:00Z"))?);

        let mut layers: Vec<Layer> = products.into_iter().map(layer_of).collect();
        // Painter's algorithm: coarse first, finer sources overwrite. Equivalent to a
        // priority-ordered "first non-no-data source wins" per cell, and much cheaper.
        layers.sort_by_key(|layer| std::cmp::Reverse(layer.frames[0].1.geom.cell_area()));
        Ok(Self { layers })
    }

    /// Paint the window `[col0, col0 + width) x [row0, row0 + height)` of the canonical lattice.
    ///
    /// Cells no source covers stay [`precip4::INTENSITY_NODATA`] — missing is never dry.
    fn fill(&self, lead_min: i32, window: Window, out: &mut [u8]) {
        debug_assert_eq!(out.len(), window.cells());
        out.fill(precip4::INTENSITY_NODATA);
        let mut columns: Vec<i32> = vec![-1; window.width as usize];
        for layer in &self.layers {
            let frame = layer.nearest(lead_min);
            let geom = &frame.geom;
            let mut covers = false;
            for (index, slot) in columns.iter_mut().enumerate() {
                let lon = lattice_lon_udeg(window.col0 + index as u32);
                let column = (lon - i64::from(geom.west_lon_udeg)).div_euclid(i64::from(geom.cell_lon_udeg));
                *slot = if (0..i64::from(geom.width)).contains(&column) {
                    covers = true;
                    column as i32
                } else {
                    -1
                };
            }
            if !covers {
                continue;
            }
            for row in 0..window.height as usize {
                let lat = lattice_lat_udeg(window.row0 + row as u32);
                let source_row = (lat - i64::from(geom.south_lat_udeg)).div_euclid(i64::from(geom.cell_lat_udeg));
                if !(0..i64::from(geom.height)).contains(&source_row) {
                    continue;
                }
                let base = source_row as usize * geom.width as usize;
                let source = &frame.cells[base..base + geom.width as usize];
                let destination = &mut out[row * window.width as usize..(row + 1) * window.width as usize];
                for (cell, column) in destination.iter_mut().zip(&columns) {
                    if *column >= 0 {
                        let value = source[*column as usize];
                        if value != precip4::INTENSITY_NODATA {
                            *cell = value;
                        }
                    }
                }
            }
        }
    }
}

/// Cell-centre longitude of a canonical lattice column, in microdegrees.
fn lattice_lon_udeg(col: u32) -> i64 {
    i64::from(LATTICE_WEST_UDEG) + i64::from(col) * i64::from(CELL_UDEG) + i64::from(CELL_UDEG / 2)
}

/// Cell-centre latitude of a canonical lattice row, in microdegrees.
fn lattice_lat_udeg(row: u32) -> i64 {
    i64::from(LATTICE_SOUTH_UDEG) + i64::from(row) * i64::from(CELL_UDEG) + i64::from(CELL_UDEG / 2)
}

fn layer_of(product: BakedProduct) -> Layer {
    let anchor = product.geometry;
    let id = product.id;
    let frames = product
        .frames
        .into_iter()
        .map(|frame| {
            let geom = frame.source.map_or(anchor, |source| source.geometry);
            (frame.offset_min as i32, SourceFrame { geom, cells: frame.cells })
        })
        .collect();
    Layer { id, frames }
}

fn bake(adapter: &dyn Adapter, upstream: &mut FixtureUpstream, now: i64) -> Result<BakedProduct, String> {
    let mut warnings = Vec::new();
    match adapter.bake(upstream, None, now, &mut warnings)? {
        AdapterOutcome::Baked(product) => Ok(*product),
        AdapterOutcome::Unchanged => Err(format!("{}: fixture bake reported Unchanged", adapter.id())),
    }
}

fn ts(text: &str) -> i64 {
    crate::manifest::parse_rfc3339(text).expect("spike timestamp")
}

// ---------------------------------------------------------------------------------------------
// Fixture wiring (mirrors tests/cycle.rs and tests/us_gfs_cycle.rs)
// ---------------------------------------------------------------------------------------------

const RV_ETAG: &str = "\"6a788c2a-273800\"";
const HRRR_OBJECTS: [(u32, u64); 3] = [(2, 210_757_046), (3, 214_632_128), (4, 220_555_508)];
const HRRR_RANGES: [(u32, u32, u64); 9] = [
    (2, 120, 183_664_477),
    (3, 135, 25_809_346),
    (3, 150, 79_031_140),
    (3, 165, 132_718_351),
    (3, 180, 186_502_886),
    (4, 195, 26_244_769),
    (4, 210, 80_983_359),
    (4, 225, 136_058_399),
    (4, 240, 191_463_451),
];
const GFS_SPANS: [(u32, u64, u64); 16] = [
    (1, 537_540_348, 427_603_385),
    (2, 538_822_727, 428_091_880),
    (3, 539_798_514, 428_475_805),
    (4, 540_724_755, 428_752_482),
    (5, 542_923_155, 430_080_077),
    (6, 544_451_780, 431_023_684),
    (7, 542_096_820, 432_070_312),
    (8, 543_890_390, 433_033_986),
    (9, 543_734_730, 432_288_308),
    (10, 544_255_893, 432_328_102),
    (11, 544_322_108, 431_989_179),
    (12, 545_133_960, 432_276_114),
    (13, 541_397_261, 431_060_039),
    (14, 541_818_663, 430_713_865),
    (15, 542_144_204, 430_643_461),
    (16, 546_445_777, 433_214_890),
];

fn fixture(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    std::fs::read(dir.join(name)).map_err(|error| format!("fixture {name}: {error} (try --fixtures <dir>)"))
}

fn european_upstream(dir: &Path) -> Result<FixtureUpstream, String> {
    let mut upstream = FixtureUpstream::default();
    upstream.insert(dwd_rv::LATEST_URL, fixture(dir, "composite_rv_20260809_1420.tar")?, Some(RV_ETAG));
    let run = ts("2026-08-09T06:00:00Z");
    for lead in 0..=12u32 {
        upstream.insert(
            icon_eu::lead_url(run, lead),
            fixture(dir, &format!("icon-eu-2026080906_{lead:03}.grib2.bz2"))?,
            None,
        );
    }
    Ok(upstream)
}

fn american_upstream(dir: &Path) -> Result<FixtureUpstream, String> {
    let mut upstream = FixtureUpstream::default();
    let observation = ts("2026-08-09T16:58:00Z");
    let hrrr_run = ts("2026-08-09T15:00:00Z");
    let gfs_run = ts("2026-08-09T12:00:00Z");
    upstream.insert(mrms::object_url(observation), fixture(dir, "mrms-conus-20260809-165800.grib2.gz")?, None);
    for file in hrrr::SUBHOURLY_FILES {
        upstream.insert(
            hrrr::index_url(hrrr_run, file),
            fixture(dir, &format!("hrrr-conus-20260809T15-f{file:02}.idx"))?,
            None,
        );
    }
    for (file, object_len) in HRRR_OBJECTS {
        upstream.declare(hrrr::object_url(hrrr_run, file), object_len);
    }
    for (file, lead, start) in HRRR_RANGES {
        let object_len = HRRR_OBJECTS.iter().find(|(candidate, _)| *candidate == file).expect("declared object").1;
        let message = fixture(dir, &format!("hrrr-conus-20260809T15-prate-t{lead}.grib2"))?;
        upstream.insert_range(hrrr::object_url(hrrr_run, file), object_len, start, message);
    }
    for (lead, object_len, start) in GFS_SPANS {
        upstream.insert(
            gfs::index_url(gfs_run, lead),
            fixture(dir, &format!("gfs-global-20260809T12-f{lead:03}.idx"))?,
            None,
        );
        let span = fixture(dir, &format!("gfs-global-20260809T12-apcp-f{lead:03}.grib2"))?;
        upstream.insert_range(gfs::object_url(gfs_run, lead), object_len, start, span);
    }
    Ok(upstream)
}

// ---------------------------------------------------------------------------------------------
// Sharding
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Window {
    col0: u32,
    row0: u32,
    width: u32,
    height: u32,
}

impl Window {
    fn cells(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// The shard grid the canonical lattice is cut into: ceil division, last column/row truncated.
fn shard_windows(shard: (u32, u32)) -> Vec<Window> {
    let (shard_width, shard_height) = shard;
    let cols = LATTICE_WIDTH.div_ceil(shard_width);
    let rows = LATTICE_HEIGHT.div_ceil(shard_height);
    let mut windows = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows {
        for col in 0..cols {
            let col0 = col * shard_width;
            let row0 = row * shard_height;
            windows.push(Window {
                col0,
                row0,
                width: shard_width.min(LATTICE_WIDTH - col0),
                height: shard_height.min(LATTICE_HEIGHT - row0),
            });
        }
    }
    windows
}

fn geometry_for(window: Window, tile_edge: u16, entries_per_page: u16) -> GridGeometry {
    GridGeometry {
        south_lat_udeg: LATTICE_SOUTH_UDEG + (window.row0 * CELL_UDEG) as i32,
        west_lon_udeg: LATTICE_WEST_UDEG + (window.col0 * CELL_UDEG) as i32,
        cell_lat_udeg: CELL_UDEG,
        cell_lon_udeg: CELL_UDEG,
        width: window.width,
        height: window.height,
        cell_size_m: 1_000,
        tile_edge,
        entries_per_page,
    }
}

fn frame_input<'a>(geometry: &GridGeometry, cells: &'a [u8], lead_min: i32) -> FrameInput<'a> {
    FrameInput {
        product_id: obcg::PRODUCT_EXPERIMENTAL,
        tier: obcg::TIER_RADAR,
        flags: if lead_min == 0 { obcg::FLAG_OBSERVED } else { obcg::FLAG_FORECAST },
        valid_at: 1_800_000_000 + i64::from(lead_min) * 60,
        reference_time: 1_800_000_000,
        south_lat_udeg: geometry.south_lat_udeg,
        west_lon_udeg: geometry.west_lon_udeg,
        cell_lat_udeg: geometry.cell_lat_udeg,
        cell_lon_udeg: geometry.cell_lon_udeg,
        width: geometry.width,
        height: geometry.height,
        cell_size_m: geometry.cell_size_m,
        tile_edge: geometry.tile_edge,
        entries_per_page: geometry.entries_per_page,
        cells,
    }
}

/// One shard encode: which window, at which tile edge and directory paging.
#[derive(Clone, Copy)]
struct ShardJob {
    window: Window,
    tile_edge: u16,
    entries_per_page: u16,
    /// Also price every non-dry tile under a general LZ codec (per-tile deflate over the
    /// raw4 nibble packing), which is what WXR2 #1241 proposes to add above `precip4`.
    lz: bool,
}

#[derive(Default, Clone)]
struct ObjectStats {
    objects: u64,
    wet_objects: u64,
    bytes: u64,
    directory_bytes: u64,
    largest: u64,
    tiles: u64,
    dry_tiles: u64,
    partial_tiles: u64,
    partial_tile_bytes: u64,
    dry_shard_objects: u64,
    dry_shard_bytes: u64,
    lz_bytes: u64,
    lz_time: Duration,
    fill: Duration,
    encode: Duration,
    validate: Duration,
}

impl ObjectStats {
    fn merge(&mut self, other: &ObjectStats) {
        self.objects += other.objects;
        self.wet_objects += other.wet_objects;
        self.bytes += other.bytes;
        self.directory_bytes += other.directory_bytes;
        self.largest = self.largest.max(other.largest);
        self.tiles += other.tiles;
        self.dry_tiles += other.dry_tiles;
        self.partial_tiles += other.partial_tiles;
        self.partial_tile_bytes += other.partial_tile_bytes;
        self.dry_shard_objects += other.dry_shard_objects;
        self.dry_shard_bytes += other.dry_shard_bytes;
        self.lz_bytes += other.lz_bytes;
        self.lz_time += other.lz_time;
        self.fill += other.fill;
        self.encode += other.encode;
        self.validate += other.validate;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    /// The real mosaic: every fixture source, cell-replicated.
    Wet,
    /// Every cell dry. The format floor: directories plus the no-data padding of edge tiles.
    Dry,
}

impl Scene {
    fn label(self) -> &'static str {
        match self {
            Self::Wet => "wet (real mosaic)",
            Self::Dry => "dry (all cells 0)",
        }
    }
}

/// Encode one shard for one lead and account for it. This is the production emit path
/// (`encoded_len` + `encode_format` + `validate`), not a shortcut, so the timings are honest.
fn encode_shard(mosaic: &Mosaic, job: ShardJob, lead_min: i32, scene: Scene) -> ObjectStats {
    let ShardJob { window, tile_edge, entries_per_page, lz } = job;
    let geometry = geometry_for(window, tile_edge, entries_per_page);
    let mut cells = vec![precip4::INTENSITY_DRY; window.cells()];
    let started = Instant::now();
    if scene == Scene::Wet {
        mosaic.fill(lead_min, window, &mut cells);
    }
    let wet = cells.iter().any(|&value| value != precip4::INTENSITY_DRY);
    let fill = started.elapsed();

    let input = frame_input(&geometry, &cells, lead_min);
    let mut scratch = vec![0u8; usize::from(tile_edge) * usize::from(tile_edge)];
    let started = Instant::now();
    let length = obcg::encoded_len(&input, &mut scratch).expect("spike shard length") as usize;
    let mut bytes = vec![0u8; length];
    obcg::encode_format(&input, &mut scratch, &mut bytes).expect("spike shard encode");
    let encode = started.elapsed();

    let started = Instant::now();
    let header = obcg::validate(&bytes, &mut scratch).expect("spike shard self-validation");
    let validate = started.elapsed();

    let mut stats = ObjectStats {
        objects: 1,
        wet_objects: u64::from(wet),
        bytes: length as u64,
        directory_bytes: u64::from(header.page_count()) * u64::from(header.page_bytes()),
        largest: length as u64,
        tiles: u64::from(header.tile_count()),
        fill,
        encode,
        validate,
        ..ObjectStats::default()
    };
    if !wet {
        stats.dry_shard_objects = 1;
        stats.dry_shard_bytes = length as u64;
    }
    let edge = u32::from(tile_edge);
    for index in 0..header.tile_count() {
        let page = header.page_of_entry(index);
        let offset = header.page_offset(page).expect("page offset") as usize;
        let page_slice = &bytes[offset..offset + header.page_bytes() as usize];
        let entry = obcg::decode_entry(page_slice, (index - page * u32::from(header.entries_per_page)) as usize)
            .expect("spike directory entry");
        let (tile_col, tile_row) = (index % header.tile_cols(), index / header.tile_cols());
        if entry.is_dry() {
            stats.dry_tiles += 1;
        } else if lz {
            gather_tile(&cells, &geometry, tile_col, tile_row, &mut scratch);
            let packed = raw4_pack(&scratch);
            let started = Instant::now();
            stats.lz_bytes += deflate(&packed, 6).len() as u64;
            stats.lz_time += started.elapsed();
        }
        if (tile_col + 1) * edge > geometry.width || (tile_row + 1) * edge > geometry.height {
            stats.partial_tiles += 1;
            stats.partial_tile_bytes += u64::from(entry.encoded_len);
        }
    }
    stats
}

fn run_cycle_matrix(
    mosaic: &Mosaic,
    shard: (u32, u32),
    tile_edge: u16,
    entries_per_page: u16,
    scene: Scene,
    lz: bool,
) -> (ObjectStats, Duration) {
    let windows = shard_windows(shard);
    let leads: Vec<i32> = (0..CYCLE_FRAMES as i32).map(|frame| frame * FRAME_STEP_MIN).collect();
    let jobs: Vec<(Window, i32)> =
        leads.iter().flat_map(|lead| windows.iter().map(move |window| (*window, *lead))).collect();
    let started = Instant::now();
    let stats = jobs
        .par_iter()
        .map(|(window, lead)| {
            encode_shard(mosaic, ShardJob { window: *window, tile_edge, entries_per_page, lz }, *lead, scene)
        })
        .reduce(ObjectStats::default, |mut accumulated, stats| {
            accumulated.merge(&stats);
            accumulated
        });
    (stats, started.elapsed())
}

fn report_cycle(
    label: &str,
    shard: (u32, u32),
    tile_edge: u16,
    stats: &ObjectStats,
    wall: Duration,
    memory: MemoryShape,
) {
    let windows = shard_windows(shard).len();
    println!(
        "  {label:18} tile {tile_edge:3}  shard {}x{} ({windows} shards)  wall {:7.2} s  objects {:5}  \
         total {:>10}  largest {:>9}",
        shard.0,
        shard.1,
        wall.as_secs_f64(),
        stats.objects,
        bytes(stats.bytes),
        bytes(stats.largest)
    );
    println!(
        "  {:18} tiles {} of which dry {:.1}% / partial-edge {} ({} of payload); directories {}; \
         dry shards {} ({})",
        "",
        stats.tiles,
        100.0 * stats.dry_tiles as f64 / stats.tiles.max(1) as f64,
        stats.partial_tiles,
        bytes(stats.partial_tile_bytes),
        bytes(stats.directory_bytes),
        stats.dry_shard_objects,
        bytes(stats.dry_shard_bytes)
    );
    println!(
        "  {:18} thread-occupancy s (sum of per-task elapsed, ~5% over real CPU): fill {:.1}, encode {:.1}, \
         self-validate {:.1}; bytes excluding all-dry shards {}",
        "",
        stats.fill.as_secs_f64(),
        stats.encode.as_secs_f64(),
        stats.validate.as_secs_f64(),
        bytes(stats.bytes - stats.dry_shard_bytes)
    );
    if stats.lz_time > Duration::ZERO {
        let lz_total = stats.directory_bytes + stats.lz_bytes + u64::from(obcg::HEADER_LEN as u32) * stats.objects;
        println!(
            "  {:18} per-tile deflate6 instead of raw4/RLE4: total {} ({:.2}x smaller), payload {} -> {}, \
         +{:.1} thread-occupancy s",
            "",
            bytes(lz_total),
            stats.bytes as f64 / lz_total.max(1) as f64,
            bytes(stats.bytes - stats.directory_bytes - u64::from(obcg::HEADER_LEN as u32) * stats.objects),
            bytes(stats.lz_bytes),
            stats.lz_time.as_secs_f64()
        );
    }
    println!(
        "  {:18} memory shape: {} resident upstream layers + {} threads x {} per-shard working set \
         = {} steady state (a materialised global mosaic would be {} of cells alone)",
        "",
        bytes(memory.resident_layers),
        memory.threads,
        bytes(memory.per_shard),
        bytes(memory.resident_layers + memory.threads as u64 * memory.per_shard),
        bytes(u64::from(LATTICE_WIDTH) * u64::from(LATTICE_HEIGHT))
    );
}

/// The working set the GO verdict is conditional on: the spike streams **one shard per thread**
/// and never materialises the global mosaic. Deliverable 1 of #1240 asked for peak RSS "per
/// shard"; `ru_maxrss` is a monotonic process-wide high-water mark and structurally cannot give
/// that, so this is the honest answer — the shape, computed from the sizes actually allocated.
#[derive(Clone, Copy)]
struct MemoryShape {
    resident_layers: u64,
    per_shard: u64,
    threads: usize,
}

impl MemoryShape {
    fn of(mosaic: &Mosaic, shard: (u32, u32), tile_edge: u16, threads: usize) -> Self {
        let resident_layers = mosaic
            .layers
            .iter()
            .map(|layer| layer.frames.iter().map(|(_, frame)| frame.cells.len() as u64).sum::<u64>())
            .sum();
        // Per shard: the cell buffer, the encode output (bounded by raw4 of every tile), one
        // tile scratch, and the deflate scratch the LZ pass uses.
        let cells = u64::from(shard.0) * u64::from(shard.1);
        let tile = u64::from(tile_edge) * u64::from(tile_edge);
        Self { resident_layers, per_shard: cells + cells / 2 + tile + tile, threads }
    }
}

fn phase_cycle(options: &Options, mosaic: &Mosaic) -> Result<(), String> {
    let shard = options.shards[0];
    println!("\n## Phase 1 — one full global cycle ({CYCLE_FRAMES} frames), by tile edge");
    println!(
        "(shard grid fixed at {}x{}; the production emit path: encoded_len + encode + validate)",
        shard.0, shard.1
    );
    for &tile_edge in &options.tile_edges {
        for scene in [Scene::Wet, Scene::Dry] {
            let (stats, wall) = run_cycle_matrix(mosaic, shard, tile_edge, options.entries_per_page, scene, true);
            let memory = MemoryShape::of(mosaic, shard, tile_edge, options.threads);
            report_cycle(scene.label(), shard, tile_edge, &stats, wall, memory);
            let verdict = if wall.as_secs_f64() <= CYCLE_BUDGET_SECONDS { "WITHIN" } else { "OVER" };
            println!(
                "  {:18} => {verdict} the {CYCLE_BUDGET_SECONDS:.0} s **wall** budget ({:.1}x on {} threads); \
         process peak RSS so far {} of {} box RAM (ru_maxrss is a running high-water mark, not a per-row figure)",
                "",
                CYCLE_BUDGET_SECONDS / wall.as_secs_f64(),
                options.threads,
                bytes(peak_rss_bytes()),
                bytes(BOX_RAM_BYTES)
            );
        }
    }
    Ok(())
}

fn phase_shard(options: &Options, mosaic: &Mosaic) -> Result<(), String> {
    // The largest edge under test, not the first: this table is read alongside the recommended
    // tile edge, and heading it with whatever happened to be first in --tile-edges is a trap.
    let tile_edge = *options.tile_edges.iter().max().expect("checked non-empty");
    println!("\n## Phase 2 — shard size sweep (wet mosaic, tile edge {tile_edge})");
    println!("  a 90 km disc is 2.516 deg x 1.617 deg at 50 N; E[objects] = (1 + 2.516/deg_w)(1 + 1.617/deg_h)");
    for &shard in &options.shards {
        let cells = u64::from(shard.0) * u64::from(shard.1);
        let (stats, wall) = run_cycle_matrix(mosaic, shard, tile_edge, options.entries_per_page, Scene::Wet, false);
        let degrees_w = f64::from(shard.0) / 100.0;
        let degrees_h = f64::from(shard.1) / 100.0;
        let expected = (1.0 + 2.516 / degrees_w) * (1.0 + 1.617 / degrees_h);
        let memory = MemoryShape::of(mosaic, shard, tile_edge, options.threads);
        report_cycle("wet (real mosaic)", shard, tile_edge, &stats, wall, memory);
        println!(
            "  {:18} {cells} cells/shard ({:.0}% of the 30 M ceiling), {degrees_w:.1} deg x {degrees_h:.1} deg, \
             E[objects per corridor frame] {expected:.2}, R2 class-A writes/month {} (every shard + the manifest PUT; \
             {} of those objects were all-dry)",
            "",
            100.0 * cells as f64 / obcg::MAX_GRID_CELLS as f64,
            thousands((stats.objects + 1) * CYCLES_PER_MONTH),
            stats.dry_shard_objects
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Phase 3 — codec comparison
// ---------------------------------------------------------------------------------------------

/// A named patch of the lattice with a known dominant source, for the codec comparison.
///
/// Dimensions are cells and every one is a multiple of 256, so the 64/128/256 columns of the
/// table describe **exactly the same ground** rather than three differently-cropped samples.
struct Region {
    name: &'static str,
    note: &'static str,
    south_deg: f64,
    west_deg: f64,
    width: u32,
    height: u32,
}

const REGIONS: [Region; 4] = [
    Region {
        name: "radar-1km-de",
        note: "DWD RV native 1 km texture (Germany)",
        south_deg: 47.5,
        west_deg: 6.5,
        width: 768,
        height: 512,
    },
    Region {
        name: "radar-1km-us",
        note: "MRMS native 1 km texture (CONUS)",
        south_deg: 33.0,
        west_deg: -100.0,
        width: 1_536,
        height: 1_024,
    },
    Region {
        name: "model-6.5km",
        note: "ICON-EU 6.5 km cell-replicated to 1 km (S France/N Spain, outside RV)",
        south_deg: 40.0,
        west_deg: -4.0,
        width: 1_536,
        height: 512,
    },
    Region {
        name: "floor-27.75km",
        note: "GFS 27.75 km floor cell-replicated to 1 km (tropical Atlantic)",
        south_deg: -5.0,
        west_deg: -25.0,
        width: 1_536,
        height: 1_024,
    },
];

fn window_of(region: &Region) -> Window {
    let col0 = ((region.west_deg * 1e6 - f64::from(LATTICE_WEST_UDEG)) / f64::from(CELL_UDEG)).round() as u32;
    let row0 = ((region.south_deg * 1e6 - f64::from(LATTICE_SOUTH_UDEG)) / f64::from(CELL_UDEG)).round() as u32;
    Window { col0, row0, width: region.width, height: region.height }
}

#[derive(Default)]
struct CodecTotals {
    tiles: u64,
    cells: u64,
    raw4: u64,
    rle4: u64,
    canonical: u64,
    deflate_nibbles: u64,
    deflate_bytes_input: u64,
    deflate_fast: u64,
    canonical_time: Duration,
    deflate_time: Duration,
}

impl CodecTotals {
    fn merge(&mut self, other: &CodecTotals) {
        self.tiles += other.tiles;
        self.cells += other.cells;
        self.raw4 += other.raw4;
        self.rle4 += other.rle4;
        self.canonical += other.canonical;
        self.deflate_nibbles += other.deflate_nibbles;
        self.deflate_bytes_input += other.deflate_bytes_input;
        self.deflate_fast += other.deflate_fast;
        self.canonical_time += other.canonical_time;
        self.deflate_time += other.deflate_time;
    }
}

/// The tile a `tile_edge`-tiled OBCG object would encode at `(tile_col, tile_row)`, with the
/// north/east padding cells at no-data exactly as `OBCG_Spec` §5 requires.
fn gather_tile(cells: &[u8], geometry: &GridGeometry, tile_col: u32, tile_row: u32, out: &mut [u8]) {
    let edge = usize::from(geometry.tile_edge);
    out.fill(precip4::INTENSITY_NODATA);
    for row in 0..edge {
        let source_row = tile_row as usize * edge + row;
        if source_row >= geometry.height as usize {
            break;
        }
        let first = tile_col as usize * edge;
        let span = edge.min(geometry.width as usize - first);
        let source = source_row * geometry.width as usize + first;
        out[row * edge..row * edge + span].copy_from_slice(&cells[source..source + span]);
    }
}

fn raw4_pack(cells: &[u8]) -> Vec<u8> {
    cells.chunks_exact(2).map(|pair| pair[0] | (pair[1] << 4)).collect()
}

fn deflate(data: &[u8], level: u32) -> Vec<u8> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(level));
    encoder.write_all(data).expect("deflate into a Vec cannot fail");
    encoder.finish().expect("deflate finish")
}

fn inflate(data: &[u8]) -> Vec<u8> {
    let mut decoder = flate2::write::DeflateDecoder::new(Vec::new());
    decoder.write_all(data).expect("inflate into a Vec cannot fail");
    decoder.finish().expect("inflate finish")
}

fn phase_codec(options: &Options, mosaic: &Mosaic) -> Result<(), String> {
    println!("\n## Phase 3 — codec: OBCG canonical (raw4/RLE4) vs a general LZ, per tile");
    println!("  deflate is applied per tile so every tile stays independently Range-readable.");
    for &tile_edge in &options.tile_edges {
        println!(
            "\n  tile edge {tile_edge} ({} cells, {} raw4 bytes)",
            u32::from(tile_edge).pow(2),
            u32::from(tile_edge).pow(2) / 2
        );
        println!(
            "  {:14} {:>7} {:>11} {:>11} {:>11} {:>11} {:>11}  {:>8} {:>13}",
            "region", "tiles", "raw4", "RLE4", "canonical", "deflate6", "deflate1", "vs canon", "deflate6(1B/cell)"
        );
        let mut all = CodecTotals::default();
        for region in &REGIONS {
            let totals = measure_region(mosaic, region, tile_edge, options)?;
            println!(
                "  {:14} {:>7} {:>11} {:>11} {:>11} {:>11} {:>11}  {:>7.2}x {:>13}",
                region.name,
                totals.tiles,
                bytes(totals.raw4),
                bytes(totals.rle4),
                bytes(totals.canonical),
                bytes(totals.deflate_nibbles),
                bytes(totals.deflate_fast),
                totals.canonical as f64 / totals.deflate_nibbles.max(1) as f64,
                bytes(totals.deflate_bytes_input)
            );
            println!("  {:14} {}", "", region.note);
            all.merge(&totals);
        }
        println!(
            "  {:14} aggregate over all four regions ({} tiles, {} of raw4): canonical encode {:.0} MB/s, \
             deflate6 {:.0} MB/s — one timing over the whole block, because per-region timings of a \
             handful of tiles are noise",
            "",
            all.tiles,
            bytes(all.raw4),
            (all.raw4 as f64 / 1e6) / all.canonical_time.as_secs_f64().max(1e-9),
            (all.raw4 as f64 / 1e6) / all.deflate_time.as_secs_f64().max(1e-9)
        );
        let dry = dry_tile_totals(tile_edge);
        println!(
            "  {:14} {:>7} {:>11} {:>11} {:>11} {:>11} {:>11}",
            "uniform-dry",
            dry.tiles,
            bytes(dry.raw4),
            bytes(dry.rle4),
            bytes(dry.canonical),
            bytes(dry.deflate_nibbles),
            bytes(dry.deflate_fast)
        );
    }
    Ok(())
}

/// A full tile of one intensity: the shape of upsampled coarse data at its limit, and the
/// shape of the padding an edge tile carries. RLE4 cannot beat one byte per 16 cells.
fn dry_tile_totals(tile_edge: u16) -> CodecTotals {
    let cells = vec![precip4::INTENSITY_DRY; usize::from(tile_edge) * usize::from(tile_edge)];
    let packed = raw4_pack(&cells);
    CodecTotals {
        tiles: 1,
        cells: cells.len() as u64,
        raw4: (cells.len() / 2) as u64,
        rle4: (cells.len() / 16) as u64,
        canonical: precip4::encoded_cells_len(&cells).expect("dry tile") as u64,
        deflate_nibbles: deflate(&packed, 6).len() as u64,
        deflate_fast: deflate(&packed, 1).len() as u64,
        ..CodecTotals::default()
    }
}

fn measure_region(mosaic: &Mosaic, region: &Region, tile_edge: u16, options: &Options) -> Result<CodecTotals, String> {
    let window = window_of(region);
    let mut cells = vec![precip4::INTENSITY_NODATA; window.cells()];
    mosaic.fill(0, window, &mut cells);
    let edge = u32::from(tile_edge) as usize;
    let tile_cols = (window.width as usize) / edge;
    let tile_rows = (window.height as usize) / edge;
    if tile_cols == 0 || tile_rows == 0 {
        return Err(format!("region {} is smaller than one {tile_edge}-cell tile", region.name));
    }
    let mut tiles: Vec<Vec<u8>> = Vec::new();
    for tile_row in 0..tile_rows {
        for tile_col in 0..tile_cols {
            let mut tile = Vec::with_capacity(edge * edge);
            for row in 0..edge {
                let start = (tile_row * edge + row) * window.width as usize + tile_col * edge;
                tile.extend_from_slice(&cells[start..start + edge]);
            }
            tiles.push(tile);
        }
    }
    // Take an evenly spread sample so the numbers do not depend on one corner of the region.
    let stride = tiles.len().div_ceil(options.sample_tiles.max(1)).max(1);
    let sample: Vec<&Vec<u8>> = tiles.iter().step_by(stride).collect();

    let mut totals = CodecTotals::default();
    let mut scratch = vec![0u8; edge * edge / 2];
    for tile in &sample {
        let packed = raw4_pack(tile);
        totals.tiles += 1;
        totals.cells += tile.len() as u64;
        totals.raw4 += (tile.len() / 2) as u64;
        totals.rle4 += rle4_len(tile) as u64;
        let started = Instant::now();
        let encoding = precip4::encode_cells(tile, &mut scratch).map_err(|error| format!("{:?}", error))?;
        totals.canonical_time += started.elapsed();
        totals.canonical += u64::from(encoding.encoded_len);
        let started = Instant::now();
        totals.deflate_nibbles += deflate(&packed, 6).len() as u64;
        totals.deflate_time += started.elapsed();
        totals.deflate_bytes_input += deflate(tile, 6).len() as u64;
        totals.deflate_fast += deflate(&packed, 1).len() as u64;
    }
    if let Some(root) = &options.dump_tiles {
        dump(root, region.name, tile_edge, &sample)?;
    }
    Ok(totals)
}

/// The maximal-run RLE4 length, uncapped by the raw4 tie-break — what `OBCG_Spec` §5 costs
/// before the encoder falls back to raw4.
fn rle4_len(cells: &[u8]) -> usize {
    let mut runs = 0usize;
    let mut index = 0usize;
    while index < cells.len() {
        let value = cells[index];
        let mut length = 1usize;
        while index + length < cells.len() && cells[index + length] == value && length < 16 {
            length += 1;
        }
        runs += 1;
        index += length;
    }
    runs
}

/// Write the sampled tiles as individual raw4 payloads so external codecs (zstd, lz4, brotli)
/// can be measured on exactly the same bytes without adding a dependency to this crate.
fn dump(root: &Path, region: &str, tile_edge: u16, tiles: &[&Vec<u8>]) -> Result<(), String> {
    let dir = root.join(format!("t{tile_edge}")).join(region);
    std::fs::create_dir_all(&dir).map_err(|error| format!("dump {}: {error}", dir.display()))?;
    for (index, tile) in tiles.iter().enumerate() {
        let path = dir.join(format!("tile-{index:05}.raw4"));
        std::fs::write(&path, raw4_pack(tile)).map_err(|error| format!("dump {}: {error}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Phase 4 — the 90 km corridor
// ---------------------------------------------------------------------------------------------

const CORRIDOR_RADIUS_M: f64 = 90_000.0;
/// The site phase 4b measures the OBCG fetch cost at: mid-latitude, inside 1 km radar coverage.
const CORRIDOR_FETCH_SITE: Site =
    Site { name: "Frankfurt 50.1N", lat_deg: 50.11, lon_deg: 8.68, source: "DWD RV 1 km, all 9" };
const METRES_PER_DEGREE_LAT: f64 = 111_320.0;
/// `OBCW_Spec` §2's current producer policy, and WXR5's proposed replacement.
const OBCW_PRODUCER_CAP: u64 = 65_536;
const OBCW_PROPOSED_CAP: u64 = 262_144;
/// `OBCW_Spec` §2's fixed sections for a nine-frame bundle: header + hourly + frame descriptors.
const OBCW_FIXED: u64 = 112 + 24 * 24 + 48 * CYCLE_FRAMES as u64;

struct Site {
    name: &'static str,
    lat_deg: f64,
    lon_deg: f64,
    /// Which fixture source actually paints this corridor. It is the difference between a
    /// measured bundle that means something and one that is 6.5 km model fill replicated 7x7 —
    /// the most RLE4-friendly shape that exists. Only Frankfurt is 1 km radar in all nine frames.
    source: &'static str,
}

/// Ordered by latitude: a 90 km disc costs *more* columns the further **north** a rider is,
/// because a degree of longitude shrinks. The worst case is the top of the list, not 50 N.
const SITES: [Site; 6] = [
    Site { name: "Tromso 69.6N", lat_deg: 69.65, lon_deg: 18.96, source: "ICON-EU 6.5 km fill" },
    Site { name: "Reykjavik 64.1N", lat_deg: 64.13, lon_deg: -21.90, source: "ICON-EU 6.5 km fill" },
    Site { name: "Oslo 59.9N", lat_deg: 59.91, lon_deg: 10.75, source: "ICON-EU 6.5 km fill" },
    Site { name: "Frankfurt 50.1N", lat_deg: 50.11, lon_deg: 8.68, source: "DWD RV 1 km, all 9" },
    Site { name: "Chicago 41.9N", lat_deg: 41.88, lon_deg: -87.63, source: "MRMS 1 km f0, HRRR 3 km" },
    Site { name: "Bogota 4.7N", lat_deg: 4.71, lon_deg: -74.07, source: "GFS 27.75 km floor" },
];

fn corridor_window(site: &Site) -> Window {
    let half_lat = CORRIDOR_RADIUS_M / METRES_PER_DEGREE_LAT;
    let half_lon = half_lat / site.lat_deg.to_radians().cos();
    let col0 = ((site.lon_deg - half_lon) * 1e6 - f64::from(LATTICE_WEST_UDEG)) / f64::from(CELL_UDEG);
    let row0 = ((site.lat_deg - half_lat) * 1e6 - f64::from(LATTICE_SOUTH_UDEG)) / f64::from(CELL_UDEG);
    let width = (2.0 * half_lon * 1e6 / f64::from(CELL_UDEG)).ceil() as u32;
    let height = (2.0 * half_lat * 1e6 / f64::from(CELL_UDEG)).ceil() as u32;
    Window { col0: col0.floor() as u32, row0: row0.floor() as u32, width, height }
}

/// Metres per 0.01 degree of latitude: the north-south cell height of the canonical lattice, and
/// the east-west column pitch the phone resamples a corridor onto.
const LATTICE_CELL_METRES: f64 = 1_113.2;

/// How many columns a corridor window becomes once the phone resamples it onto a **uniform
/// ~1,113 m east-west pitch** — the same physical size as the lattice's north-south cell.
///
/// The degree lattice oversamples east-west by `1/cos(lat)`: a 0.01 degree column is only
/// `1113 x cos(lat)` metres wide, 715 m at 50 N and 387 m at Tromso, finer than anything any
/// source produces. Resampling to a fixed metric pitch spends that surplus once and gives the
/// same cell count for the same ground radius at every latitude — 162 columns for a 90 km disc,
/// north to south and east to west alike.
///
/// This replaces an earlier `round(1/cos lat)` **column merge**, which was worse in three ways a
/// review caught: it produced 1,428 m cells at Frankfurt (coarser than the 1 km radar painting
/// them), it stepped 2x across 48.19 N where `round()` flips, and taking each group's maximum
/// biased the result wet. Nearest-neighbour is also what `OBCG_Spec` §6 and `OBCW_Spec` §5
/// mandate everywhere else — no interpolation, no fabricated sub-cell precision.
fn uniform_pitch_width(window: Window, lat_deg: f64) -> u32 {
    let span_metres = f64::from(window.width) * LATTICE_CELL_METRES * lat_deg.to_radians().cos();
    ((span_metres / LATTICE_CELL_METRES).round() as u32).max(1)
}

/// The east-west cell pitch, in metres, that `out_width` columns give this window at `lat_deg`.
fn column_pitch_metres(window: Window, lat_deg: f64, out_width: u32) -> f64 {
    f64::from(window.width) * LATTICE_CELL_METRES * lat_deg.to_radians().cos() / f64::from(out_width)
}

/// Exact OBCW v1 byte cost of a nine-frame bundle over `window`, per `OBCW_Spec` §§2-6.
/// OBCW has no dry sentinel: a dry 256-cell tile still costs 16 RLE4 bytes.
///
/// `out_width` is the bundle's column count. Pass `window.width` for the raw degree lattice, or
/// [`uniform_pitch_width`] for the phone-normalised grid; columns are taken **nearest-neighbour**
/// from the lattice, never averaged and never merged. `OBCW_Spec` §5 states the window as a bbox
/// plus per-frame width/height and derives cells affinely, so a bundle grid that is not aligned
/// to the source lattice is already expressible — the device reads no per-axis stride.
fn obcw_bundle_bytes(mosaic: &Mosaic, window: Window, out_width: u32, worst_case: bool) -> (u64, u64, u64) {
    let width = out_width;
    let tile_cols = width.div_ceil(16) as usize;
    let tile_rows = window.height.div_ceil(16) as usize;
    let tile_count = (tile_cols * tile_rows) as u64;
    let mut payload = 0u64;
    let mut cells = vec![precip4::INTENSITY_NODATA; window.cells()];
    let mut reduced = vec![precip4::INTENSITY_NODATA; width as usize * window.height as usize];
    for frame in 0..CYCLE_FRAMES as i32 {
        if worst_case {
            payload += tile_count * 128;
            continue;
        }
        mosaic.fill(frame * FRAME_STEP_MIN, window, &mut cells);
        // Nearest-neighbour: output column c samples the lattice column under its centre.
        let columns: Vec<usize> = (0..width as usize)
            .map(|index| {
                ((2 * index + 1) * window.width as usize / (2 * width as usize)).min(window.width as usize - 1)
            })
            .collect();
        for row in 0..window.height as usize {
            let source = &cells[row * window.width as usize..(row + 1) * window.width as usize];
            let destination = &mut reduced[row * width as usize..(row + 1) * width as usize];
            for (cell, column) in destination.iter_mut().zip(&columns) {
                *cell = source[*column];
            }
        }
        for tile_row in 0..tile_rows {
            for tile_col in 0..tile_cols {
                let mut tile = [precip4::INTENSITY_NODATA; 256];
                for row in 0..16 {
                    let global_row = tile_row * 16 + row;
                    if global_row >= window.height as usize {
                        continue;
                    }
                    for column in 0..16 {
                        let global_col = tile_col * 16 + column;
                        if global_col >= width as usize {
                            continue;
                        }
                        tile[row * 16 + column] = reduced[global_row * width as usize + global_col];
                    }
                }
                payload += precip4::encoded_tile_len(&tile).expect("corridor tile") as u64;
            }
        }
    }
    let directory = tile_count * 12 * u64::from(CYCLE_FRAMES);
    (OBCW_FIXED + directory + payload, tile_count, directory)
}

fn phase_corridor(options: &Options, mosaic: &Mosaic) -> Result<(), String> {
    println!("\n## Phase 4a — the OBCW corridor bundle: 90 km disc, 1 km lattice, {CYCLE_FRAMES} frames");
    println!("  OBCW has no dry sentinel, so a dry 16x16 tile still costs 16 RLE4 bytes.");
    println!(
        "  {:18} {:24} {:>11} {:>7} {:>10} {:>12} {:>12}  {:>13}  verdict",
        "site", "what paints it", "cells/frame", "tiles", "dir/frame", "measured", "raw4 worst", "radar-scaled"
    );
    let reference = corridor_window(&CORRIDOR_FETCH_SITE);
    let (reference_bytes, reference_tiles, reference_directory) =
        obcw_bundle_bytes(mosaic, reference, reference.width, false);
    let frankfurt_payload_per_tile =
        (reference_bytes - reference_directory - OBCW_FIXED) / (reference_tiles * u64::from(CYCLE_FRAMES));
    println!(
        "  radar-scaled = the same tile count priced at Frankfurt's measured {frankfurt_payload_per_tile} B/tile \
         of genuine 1 km radar."
    );
    for site in &SITES {
        let window = corridor_window(site);
        let (measured, tiles, directory) = obcw_bundle_bytes(mosaic, window, window.width, false);
        let (worst, _, _) = obcw_bundle_bytes(mosaic, window, window.width, true);
        // What the site would measure under genuine 1 km radar texture, priced from Frankfurt's
        // own per-tile payload — the only site in the table that is radar-covered in all nine
        // frames. Without this the northern rows read as reassurance they have not earned.
        let radar_scaled = OBCW_FIXED + directory + tiles * u64::from(CYCLE_FRAMES) * frankfurt_payload_per_tile;
        println!(
            "  {:18} {:24} {:>4} x {:<4} {:>7} {:>10} {:>12} {:>12}  {:>13}  {} / {}",
            site.name,
            site.source,
            window.width,
            window.height,
            tiles,
            bytes(directory / u64::from(CYCLE_FRAMES)),
            bytes(measured),
            bytes(worst),
            bytes(radar_scaled),
            if measured <= OBCW_PRODUCER_CAP { "fits 64 KiB" } else { "OVER 64 KiB" },
            if radar_scaled <= OBCW_PROPOSED_CAP { "radar-scaled fits 256 KiB" } else { "radar-scaled OVER 256 KiB" }
        );
    }

    println!(
        "\n## Phase 4a-norm — the same bundles after the phone resamples onto a uniform ~{:.0} m \
         east-west pitch",
        LATTICE_CELL_METRES
    );
    println!(
        "  Nearest-neighbour onto the same physical pitch as the lattice's north-south cell. A \
         fixed ground radius then costs the same cell count at every latitude, cells stay square, \
         and there is no step where a rounding rule flips."
    );
    println!(
        "  {:18} {:>11} {:>9} {:>7} {:>12} {:>12} {:>13} {:>10}",
        "site", "cells/frame", "E-W pitch", "tiles", "measured", "raw4 worst", "radar-scaled", "headroom"
    );
    for site in &SITES {
        let window = corridor_window(site);
        let out_width = uniform_pitch_width(window, site.lat_deg);
        let (measured, tiles, directory) = obcw_bundle_bytes(mosaic, window, out_width, false);
        let (worst, _, _) = obcw_bundle_bytes(mosaic, window, out_width, true);
        let radar_scaled = OBCW_FIXED + directory + tiles * u64::from(CYCLE_FRAMES) * frankfurt_payload_per_tile;
        println!(
            "  {:18} {:>4} x {:<4} {:>7.0} m {:>7} {:>12} {:>12} {:>13} {:>9.1}%",
            site.name,
            out_width,
            window.height,
            column_pitch_metres(window, site.lat_deg, out_width),
            tiles,
            bytes(measured),
            bytes(worst),
            bytes(radar_scaled),
            100.0 * (1.0 - worst as f64 / OBCW_PROPOSED_CAP as f64)
        );
    }
    // The swept worst case, not a six-city sample: round-2 review found the previous mechanism's
    // maximum sitting at 48.19 N, between two of the listed cities, where nobody had looked.
    let mut worst_latitude = 0.0;
    let mut worst_bytes = 0u64;
    let mut widest = 0u32;
    let mut narrowest = u32::MAX;
    let mut latitude = 0.0;
    while latitude <= 80.0 {
        let site = Site { name: "sweep", lat_deg: latitude, lon_deg: 0.0, source: "n/a" };
        let window = corridor_window(&site);
        let out_width = uniform_pitch_width(window, latitude);
        widest = widest.max(out_width);
        narrowest = narrowest.min(out_width);
        let (worst, _, _) = obcw_bundle_bytes(mosaic, window, out_width, true);
        if worst > worst_bytes {
            worst_bytes = worst;
            worst_latitude = latitude;
        }
        latitude += 0.05;
    }
    println!(
        "  swept 0-80 N at 0.05 deg: worst raw4 bundle {} at {worst_latitude:.2} N ({:.1}% headroom \
         under the 256 KiB cap); column count stays in {narrowest}..={widest} across the whole sweep",
        bytes(worst_bytes),
        100.0 * (1.0 - worst_bytes as f64 / OBCW_PROPOSED_CAP as f64)
    );

    println!("\n## Phase 4b — what a corridor must fetch out of OBCG, by tile edge and page size");
    println!(
        "  (site {}, shard {}x{}; bytes = header + covering directory pages + covering tile payloads, x9 frames)",
        CORRIDOR_FETCH_SITE.name, options.shards[0].0, options.shards[0].1
    );
    println!(
        "  {:>5} {:>6} {:>8} {:>7} {:>11} {:>7} {:>12} {:>14} {:>14} {:>13}",
        "tile",
        "epp",
        "objects",
        "tiles",
        "over-fetch",
        "pages",
        "page bytes",
        "fetched RLE4",
        "fetched LZ",
        "inflate+scan"
    );
    let window = corridor_window(&CORRIDOR_FETCH_SITE);
    for &tile_edge in &options.tile_edges {
        for entries_per_page in [128u16, 512, obcg::MAX_ENTRIES_PER_PAGE] {
            let cost = corridor_fetch_cost(mosaic, options.shards[0], window, tile_edge, entries_per_page);
            println!(
                "  {tile_edge:>5} {entries_per_page:>6} {:>8} {:>7} {:>10.1}x {:>7} {:>12} {:>14} {:>14} {:>10.2} ms",
                cost.objects_per_frame * u64::from(CYCLE_FRAMES),
                cost.tiles,
                (cost.tiles * u64::from(tile_edge).pow(2)) as f64
                    / (u64::from(window.width) * u64::from(window.height) * u64::from(CYCLE_FRAMES)) as f64,
                cost.pages,
                bytes(cost.page_bytes),
                bytes(cost.fetched),
                bytes(cost.fetched_lz),
                cost.inflate_scan.as_secs_f64() * 1000.0
            );
        }
    }
    println!(
        "  inflate+scan is the whole nine-frame bundle on one core of this box; a phone core is \
         slower, but the ratio between tile edges is what the tile-edge decision turns on."
    );
    Ok(())
}

#[derive(Default)]
struct FetchCost {
    /// OBCG objects the corridor touches per frame — the WXR4 #1243 tradeoff, measured rather
    /// than taken from the E[objects] formula.
    objects_per_frame: u64,
    tiles: u64,
    pages: u64,
    page_bytes: u64,
    fetched: u64,
    /// The same fetch if the tile payloads were per-tile deflate6 instead of raw4/RLE4.
    fetched_lz: u64,
    /// Phone-side cost of the over-fetch: inflating every fetched tile and unpacking its nibbles
    /// into cells, for the whole nine-frame bundle. This is the number the tile-edge decision
    /// was otherwise being made without.
    inflate_scan: Duration,
}

/// Every shard of the grid the corridor window overlaps. A corridor near a shard seam legitimately
/// straddles two or four objects; refusing to model that would hide the exact cost #1243 asks about.
fn shards_covering(shard: (u32, u32), window: Window) -> Vec<Window> {
    shard_windows(shard)
        .into_iter()
        .filter(|candidate| {
            window.col0 < candidate.col0 + candidate.width
                && candidate.col0 < window.col0 + window.width
                && window.row0 < candidate.row0 + candidate.height
                && candidate.row0 < window.row0 + window.height
        })
        .collect()
}

/// What nine Range-read frames of one corridor cost out of the OBCG shards that cover it.
fn corridor_fetch_cost(mosaic: &Mosaic, shard: (u32, u32), window: Window, tile_edge: u16, epp: u16) -> FetchCost {
    let covering = shards_covering(shard, window);
    let mut cost = FetchCost { objects_per_frame: covering.len() as u64, ..FetchCost::default() };
    let edge = u32::from(tile_edge);
    let mut scratch = vec![0u8; usize::from(tile_edge) * usize::from(tile_edge)];
    for shard_window in covering {
        let geometry = geometry_for(shard_window, tile_edge, epp);
        let mut cells = vec![precip4::INTENSITY_NODATA; shard_window.cells()];
        // The corridor clipped to this shard: a straddling corridor pays for its slice of each.
        let clip_col0 = window.col0.max(shard_window.col0);
        let clip_col1 = (window.col0 + window.width).min(shard_window.col0 + shard_window.width);
        let clip_row0 = window.row0.max(shard_window.row0);
        let clip_row1 = (window.row0 + window.height).min(shard_window.row0 + shard_window.height);
        let first_col = (clip_col0 - shard_window.col0) / edge;
        let last_col = (clip_col1 - 1 - shard_window.col0) / edge;
        let first_row = (clip_row0 - shard_window.row0) / edge;
        let last_row = (clip_row1 - 1 - shard_window.row0) / edge;
        for lead in 0..CYCLE_FRAMES as i32 {
            mosaic.fill(lead * FRAME_STEP_MIN, shard_window, &mut cells);
            let input = frame_input(&geometry, &cells, lead * FRAME_STEP_MIN);
            let length = obcg::encoded_len(&input, &mut scratch).expect("corridor shard length") as usize;
            let mut object = vec![0u8; length];
            obcg::encode_format(&input, &mut scratch, &mut object).expect("corridor shard encode");
            let header = obcg::validate(&object, &mut scratch).expect("corridor shard validation");
            let mut page_set = std::collections::BTreeSet::new();
            cost.fetched += obcg::HEADER_LEN as u64;
            for tile_row in first_row..=last_row {
                for tile_col in first_col..=last_col {
                    let index = tile_row * header.tile_cols() + tile_col;
                    let page = header.page_of_entry(index);
                    page_set.insert(page);
                    let offset = header.page_offset(page).expect("page offset") as usize;
                    let page_slice = &object[offset..offset + header.page_bytes() as usize];
                    let entry =
                        obcg::decode_entry(page_slice, (index - page * u32::from(header.entries_per_page)) as usize)
                            .expect("corridor entry");
                    cost.tiles += 1;
                    cost.fetched += u64::from(entry.encoded_len);
                    if entry.is_dry() {
                        continue;
                    }
                    gather_tile(&cells, &geometry, tile_col, tile_row, &mut scratch);
                    let compressed = deflate(&raw4_pack(&scratch), 6);
                    cost.fetched_lz += compressed.len() as u64;
                    // What the phone pays for the over-fetch: inflate, then unpack the nibbles
                    // into one byte per cell, which is what a corridor crop reads.
                    let started = Instant::now();
                    let packed = inflate(&compressed);
                    let mut unpacked = vec![0u8; packed.len() * 2];
                    for (pair, byte) in unpacked.chunks_exact_mut(2).zip(&packed) {
                        pair[0] = byte & 0x0f;
                        pair[1] = byte >> 4;
                    }
                    cost.inflate_scan += started.elapsed();
                    std::hint::black_box(&unpacked);
                }
            }
            cost.pages += page_set.len() as u64;
            cost.page_bytes += page_set.len() as u64 * u64::from(header.page_bytes());
            cost.fetched += page_set.len() as u64 * u64::from(header.page_bytes());
            cost.fetched_lz += page_set.len() as u64 * u64::from(header.page_bytes()) + obcg::HEADER_LEN as u64;
        }
    }
    cost
}

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

fn bytes(value: u64) -> String {
    const UNITS: [(&str, f64); 4] = [("GB", 1e9), ("MB", 1e6), ("kB", 1e3), ("B", 1.0)];
    for (unit, scale) in UNITS {
        if value as f64 >= scale {
            return format!("{:.2} {unit}", value as f64 / scale);
        }
    }
    "0 B".to_string()
}

fn thousands(value: u64) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (index, character) in text.chars().enumerate() {
        if index > 0 && (text.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(character);
    }
    out
}

/// Process peak resident set size. `ru_maxrss` is bytes on macOS and kilobytes on Linux; both
/// are the true high-water mark, not a sample.
fn peak_rss_bytes() -> u64 {
    // SAFETY: `getrusage` writes into a fully owned, zeroed `rusage` and reads nothing else.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return 0;
        }
        usage
    };
    let maximum = usage.ru_maxrss.max(0) as u64;
    if cfg!(target_os = "macos") {
        maximum
    } else {
        maximum * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical lattice must land exactly on the poles and the antimeridian, or OBCG's
    /// derived north/east edges overflow the spec's bounds.
    #[test]
    fn the_canonical_lattice_closes_exactly() {
        assert_eq!(i64::from(LATTICE_SOUTH_UDEG) + i64::from(LATTICE_HEIGHT) * i64::from(CELL_UDEG), 90_000_000);
        assert_eq!(i64::from(LATTICE_WEST_UDEG) + i64::from(LATTICE_WIDTH) * i64::from(CELL_UDEG), 180_000_000);
    }

    /// Every shard grid the spike offers must tile the lattice exactly and stay under the
    /// 30 M-cell object ceiling — the constraint that makes sharding mandatory at all.
    #[test]
    fn shard_grids_tile_the_lattice_and_respect_the_object_ceiling() {
        for shard in [(6_144u32, 4_608u32), (6_144, 3_072), (3_072, 3_072), (1_536, 1_536), (1_024, 1_024)] {
            let windows = shard_windows(shard);
            let covered: u64 = windows.iter().map(|window| window.cells() as u64).sum();
            assert_eq!(covered, u64::from(LATTICE_WIDTH) * u64::from(LATTICE_HEIGHT), "{shard:?} leaves a gap");
            for window in &windows {
                assert!(window.cells() as u64 <= obcg::MAX_GRID_CELLS, "{window:?} is over the ceiling");
            }
        }
    }

    /// The mosaic's nearest-lead rule must be deterministic and prefer the earlier frame on a
    /// tie, or two runs of the same cycle disagree about which source painted a cell.
    #[test]
    fn nearest_lead_is_deterministic_and_prefers_the_earlier_frame() {
        let frame = |offset: i32| {
            (
                offset,
                SourceFrame {
                    geom: GridGeometry {
                        south_lat_udeg: 0,
                        west_lon_udeg: 0,
                        cell_lat_udeg: 10_000,
                        cell_lon_udeg: 10_000,
                        width: 2,
                        height: 2,
                        cell_size_m: 1_000,
                        tile_edge: 16,
                        entries_per_page: 16,
                    },
                    cells: vec![offset as u8; 4],
                },
            )
        };
        let layer = Layer { id: "test", frames: vec![frame(0), frame(60), frame(120)] };
        assert_eq!(layer.nearest(15).cells[0], 0);
        assert_eq!(layer.nearest(30).cells[0], 0, "a tie takes the earlier lead");
        assert_eq!(layer.nearest(45).cells[0], 60);
    }

    /// The corridor window is the number WXR5 #1244 re-derived: 252 x 162 cells at 50 N.
    #[test]
    fn the_ninety_kilometre_disc_matches_the_re_derived_corridor() {
        let window = corridor_window(&Site { name: "50 N", lat_deg: 50.0, lon_deg: 0.0, source: "n/a" });
        assert_eq!((window.width, window.height), (252, 162));
    }

    /// Timo's decision, as arithmetic. Two properties, both swept rather than sampled at cities:
    /// the resampled bundle's raw4 worst case stays well under the producer cap at every latitude,
    /// and the east-west cell pitch stays within a couple of percent of the north-south one.
    ///
    /// The second assertion is the guard. The `round(1/cos lat)` merge this replaced satisfied the
    /// cap (222,880 B at 48.19 N, 15 % headroom) but produced a 1,428 m east-west cell against a
    /// 1,113 m north-south one — so a cap-only test would not have caught it. The round-2 review
    /// found that maximum by sweeping; this test is what makes the sweep permanent.
    #[test]
    fn uniform_pitch_bounds_the_corridor_in_bytes_at_every_latitude() {
        // A byte model only: `worst_case` skips the mosaic, so no fixtures are needed.
        let mosaic = Mosaic { layers: Vec::new() };
        // Comfortably under the 256 KiB cap, and comfortably under the 217.7 KiB the merge left.
        let budget = 200 * 1024;
        let mut latitude = 0.0f64;
        while latitude <= 80.0 {
            let site = Site { name: "sweep", lat_deg: latitude, lon_deg: 0.0, source: "n/a" };
            let window = corridor_window(&site);
            let out_width = uniform_pitch_width(window, latitude);
            let (worst, _, _) = obcw_bundle_bytes(&mosaic, window, out_width, true);
            assert!(worst <= budget, "{latitude:.2} N: raw4 worst case {worst} B is over the {budget} B budget");
            let pitch = column_pitch_metres(window, latitude, out_width);
            let ratio = pitch / LATTICE_CELL_METRES;
            assert!(
                (0.98..=1.02).contains(&ratio),
                "{latitude:.2} N: east-west pitch {pitch:.0} m is {ratio:.3}x the north-south cell"
            );
            latitude += 0.05;
        }
    }

    /// A corridor that straddles a shard seam must be priced across every shard it overlaps, not
    /// refused. This replaced an `.expect()` that aborted the whole rig on a plausible flag
    /// combination.
    #[test]
    fn a_straddling_corridor_is_priced_across_every_shard_it_overlaps() {
        let shard = (6_144u32, 4_608u32);
        let inside = Window { col0: 100, row0: 100, width: 252, height: 162 };
        assert_eq!(shards_covering(shard, inside).len(), 1);
        // Straddling the first vertical seam, and then the first corner.
        let seam = Window { col0: 6_144 - 100, row0: 100, width: 252, height: 162 };
        assert_eq!(shards_covering(shard, seam).len(), 2);
        let corner = Window { col0: 6_144 - 100, row0: 4_608 - 100, width: 252, height: 162 };
        assert_eq!(shards_covering(shard, corner).len(), 4);
    }

    /// A dry OBCW tile costs 16 bytes and a dry OBCG tile costs nothing: the sentinel asymmetry
    /// the corridor arithmetic turns on.
    #[test]
    fn the_dry_tile_costs_differ_between_the_two_containers() {
        let dry = [precip4::INTENSITY_DRY; 256];
        assert_eq!(precip4::encoded_tile_len(&dry).expect("dry tile"), 16);
        assert_eq!(rle4_len(&dry), 16);
    }

    #[test]
    fn raw4_packing_is_the_spec_nibble_order() {
        assert_eq!(raw4_pack(&[1, 2, 3, 4]), vec![0x21, 0x43]);
    }
}
