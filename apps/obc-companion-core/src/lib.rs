//! The companion app's Rust core: verified network and terrain cells in, a small map out, and
//! routes planned over that map by the device's own router.
//!
//! The phone fetches and verifies the cells (OBCC §9); this crate trusts the files it is handed.
//! Assembly is the engine the website runs. Routing is `obc-route` at the device's
//! [`NAV_MAX_NODES`], so a route the phone plans is the route the device would plan.
//! `src/ffi.rs` is the C ABI over [`assemble`] and [`CellMap::route`].

mod ffi;

use obc_elevation::{ElevationSource, NullElevation, TerrainElevation, DEFAULT_TILE_SLOTS};
use obc_formats::io::{ByteSink, Error as IoError, SliceSource, WindowSource};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::{NavError, NavPhase, NavPlanner, NavScratch, RouteIndex, RouteReader, Step, NAV_MAX_NODES};
use obcm_assemble::grid::CellId;
use obcm_assemble::{
    assemble_full, CellInput, KnownEmptyInput, MemoryScratch, MemorySource, MemoryStore, NoClock, Options, Schema,
    Skin, TerrainCellInput, TerrainJob, TerrainParams,
};
use serde::Deserialize;

/// The part of the catalog root (OBCC §3) that assembly reads. Unknown fields are ignored.
#[derive(Deserialize)]
struct Catalog {
    schema: Schema,
    /// Nothing is drawn from the map, but the assembler stamps a skin; the first one does.
    skins: Vec<Skin>,
    terrain: Option<CatalogTerrain>,
}

/// The terrain block's lattice (OBCC §13.1).
#[derive(Deserialize)]
struct CatalogTerrain {
    posting_log2: u8,
    cell_log2: u8,
}

/// One assembly request. Every path names a file whose length and digest the caller has already
/// checked against the catalog.
#[derive(Deserialize)]
pub struct Job {
    pub cells: Vec<JobCell>,
    #[serde(default)]
    pub known_empty: Vec<JobKnownEmpty>,
    #[serde(default)]
    pub terrain: Vec<JobTerrainCell>,
}

#[derive(Deserialize)]
pub struct JobCell {
    pub id: String,
    pub band: String,
    #[serde(default)]
    pub partial: bool,
    pub path: String,
}

#[derive(Deserialize)]
pub struct JobKnownEmpty {
    pub id: String,
    pub band: String,
}

#[derive(Deserialize)]
pub struct JobTerrainCell {
    pub id: String,
    /// Lowercase hex, from the terrain index.
    pub sha256: String,
    pub path: String,
}

/// An assembled map, held in memory. It is immutable, so routes share nothing but its bytes.
pub struct CellMap {
    bytes: Vec<u8>,
    tables: MapTables,
}

/// Assemble the job's cells into one map under the catalog root `catalog`.
///
/// Holes are accepted: the selection is a few discs around route endpoints, with only the network
/// band, so every geometry band is absent by design and a missing network cell is ground the
/// router cannot reach.
pub fn assemble(catalog: &str, job: Job) -> Result<CellMap, String> {
    let catalog: Catalog = serde_json::from_str(catalog).map_err(|e| format!("the catalog: {e}"))?;
    let skin = catalog.skins.first().ok_or("the catalog has no skin")?;
    let read = |path: &str| std::fs::read(path).map(MemorySource).map_err(|e| format!("{path}: {e}"));
    let cell_bytes = job.cells.iter().map(|c| read(&c.path)).collect::<Result<Vec<_>, _>>()?;
    let mut cells = Vec::with_capacity(job.cells.len());
    for (c, src) in job.cells.iter().zip(&cell_bytes) {
        cells.push(CellInput { id: CellId::parse(&c.id)?, band: c.band.clone(), src, partial: c.partial });
    }
    let known_empty = job
        .known_empty
        .iter()
        .map(|e| Ok(KnownEmptyInput { id: CellId::parse(&e.id)?, band: e.band.clone() }))
        .collect::<Result<Vec<_>, String>>()?;
    let terrain_bytes = job.terrain.iter().map(|c| read(&c.path)).collect::<Result<Vec<_>, _>>()?;
    let terrain = match (catalog.terrain, job.terrain.is_empty()) {
        (_, true) => None,
        (None, false) => return Err("terrain cells, but the catalog has no terrain block".into()),
        (Some(t), false) => {
            let mut cells = Vec::with_capacity(job.terrain.len());
            for (c, src) in job.terrain.iter().zip(&terrain_bytes) {
                cells.push(TerrainCellInput { id: CellId::parse(&c.id)?, src, sha256: Some(sha256_hex(&c.sha256)?) });
            }
            Some(TerrainJob { params: TerrainParams { posting_log2: t.posting_log2, cell_log2: t.cell_log2 }, cells })
        }
    };
    let opts = Options { accept_holes: true, accept_partial: true, ..Options::default() };
    let mut store = MemoryStore::default();
    assemble_full(
        cells,
        known_empty,
        terrain,
        &catalog.schema,
        skin,
        &opts,
        &mut store,
        &NoClock,
        &MemoryScratch::new(),
    )
    .map_err(|e| e.to_string())?;
    let bytes = store.map.0;
    let tables = MapTables::parse(&SliceSource(&bytes)).map_err(|e| format!("the assembled map: {e:?}"))?;
    Ok(CellMap { bytes, tables })
}

fn sha256_hex(hex: &str) -> Result<[u8; 32], String> {
    let bad = || format!("{hex:?} is not a lowercase SHA-256");
    if hex.len() != 64 {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// One routed point, as the device's route file stores it. `ele` is `None` where the map has no
/// terrain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub lon: i32,
    pub lat: i32,
    pub ele: Option<i16>,
    /// Surface class of the incoming segment; 0 is unknown.
    pub surface: u8,
    pub elevation_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leg {
    pub points: Vec<Point>,
    pub distance_m: u32,
    pub ascent_m: u32,
}

/// Why a route failed. The planner reports every snap failure as a no-path; the phase it failed
/// in tells the two apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    /// No road within `SNAP_RADIUS_M` of an endpoint.
    NoRoad,
    NoPath,
    /// The device's node table filled before the goal: too far to route here.
    Exhausted,
}

impl CellMap {
    /// Plan from `from` to `to`, both `(lon, lat)` microdegrees, under the map's nav profile
    /// `profile`, exactly as the device plans a route.
    pub fn route(&self, from: (i32, i32), to: (i32, i32), profile: u8) -> Result<Leg, RouteError> {
        let src = SliceSource(&self.bytes);
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &self.tables, &cache);
        let window = self.tables.terrain().and_then(|t| WindowSource::new(&src, t.offset, t.len));
        let mut terrain = window.as_ref().and_then(|w| TerrainElevation::<DEFAULT_TILE_SLOTS>::parse(w).ok());
        let elev: &mut dyn ElevationSource = match terrain.as_mut() {
            Some(t) => t,
            None => &mut NullElevation,
        };
        let mut planner = Box::new(NavPlanner::new(from, to, "", profile));
        let mut scratch = NavScratch::<NAV_MAX_NODES>::new_boxed();
        let mut tiles = NavTileCache::new();
        let mut sink = VecSink::default();
        let stats = loop {
            let snapping = planner.phase() == NavPhase::Snap;
            match planner.step(&reader, &mut scratch, &mut tiles, elev, &mut sink) {
                Step::Running => {}
                Step::Done(stats) => break stats,
                Step::Failed(_) if snapping => return Err(RouteError::NoRoad),
                Step::Failed(NavError::NoPath) => return Err(RouteError::NoPath),
                Step::Failed(NavError::Exhausted) => return Err(RouteError::Exhausted),
            }
        };
        let points = decode(&sink.0, stats.has_elevation).map_err(|_| RouteError::NoPath)?;
        Ok(Leg { points, distance_m: stats.total_distance_m, ascent_m: stats.total_ascent_m })
    }
}

fn decode(obcr: &[u8], has_elevation: bool) -> Result<Vec<Point>, IoError> {
    let src = SliceSource(obcr);
    let index = RouteIndex::read(&src)?;
    let route = RouteReader::new(&index, &src);
    let mut out = Vec::new();
    let mut chunk = heapless::Vec::new();
    for k in 0..route.chunks().len() {
        route.decode_chunk(k, &mut chunk)?;
        // Consecutive chunks repeat their seam point.
        for p in chunk.iter().skip(usize::from(k > 0)) {
            out.push(Point {
                lon: p.lon,
                lat: p.lat,
                ele: p.elevation().filter(|_| has_elevation),
                surface: p.surface,
                elevation_incomplete: p.elevation_incomplete,
            });
        }
    }
    Ok(out)
}

#[derive(Default)]
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), IoError> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, at: u32, b: &[u8]) -> Result<(), IoError> {
        let at = at as usize;
        self.0.get_mut(at..at + b.len()).ok_or(IoError::BadOffset)?.copy_from_slice(b);
        Ok(())
    }
}
