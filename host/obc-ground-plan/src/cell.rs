//! The build of one cell from its request bytes, stage by stage, with the time of each stage.

use crate::faces::{records, FacePlan, Rules, Storeys};
use crate::frame::CellKey;
use crate::graph::{self, WayGraph};
use crate::tile::TileCache;
use crate::transport::{fnv, Request};

/// The stages in their order: `times` holds one time per stage.
pub const STAGES: [&str; 3] = ["input", "graph", "faces"];

/// A built cell: what its request held, its plan and the time of each stage in milliseconds.
#[derive(Clone, Debug)]
pub struct CellBuild {
    pub key: CellKey,
    /// The digest of the request bytes, for a build from them.
    pub request: Option<String>,
    pub tiles: usize,
    pub pages: usize,
    pub graph: WayGraph,
    /// The faces of level 0.
    pub faces: FacePlan,
    /// The near records of the faces (`records::Built::near`): the block that the GPU reads and whose
    /// labels the mesh's corners name.
    pub records: Vec<u32>,
    /// The far block of the records' lines (`records::far`), or why it is refused: the records stand
    /// either way.
    pub far: Result<Vec<u32>, String>,
    pub times: Vec<f64>,
    /// The storeys over the ground, whose records `records` holds after storey 0's; none unless asked
    /// (`build_storeys`).
    pub storeys: Storeys,
    /// With the storeys: the times of the levels of the way graph and of the storeys' faces and records (ms).
    pub storey_times: [f64; 2],
}

/// Builds the cell of a request: `input` decodes the request, the feature tiles and the terrain;
/// `graph` builds the way graph; `faces` builds the faces of level 0, their records and the far block
/// in one pass.
/// `now` gives milliseconds; no output byte depends on it, so a caller that does not time passes
/// `&mut || 0.0`.
pub fn build_cell(bytes: &[u8], rules: &Rules, now: &mut dyn FnMut() -> f64) -> Result<CellBuild, String> {
    build_cell_with(bytes, &mut TileCache::new(4), rules, now)
}

/// `build_cell` with the tiles that `cache` holds parsed: a caller that keeps one cache from cell to
/// cell parses each tile once. A cache keys a tile by its column and row, so it serves one feature
/// archive.
pub fn build_cell_with(
    bytes: &[u8],
    cache: &mut TileCache,
    rules: &Rules,
    now: &mut dyn FnMut() -> f64,
) -> Result<CellBuild, String> {
    let start = now();
    let request = Request::decode(bytes)?;
    Ok(CellBuild { request: Some(fnv(bytes)), ..stages(start, request, cache, rules, false, now)? })
}

/// `build_cell_with` of a request that is not encoded, without its digest. A tile that the request
/// lacks and `cache` does not hold counts as absent.
pub fn build_request(
    request: Request,
    cache: &mut TileCache,
    rules: &Rules,
    now: &mut dyn FnMut() -> f64,
) -> Result<CellBuild, String> {
    stages(now(), request, cache, rules, false, now)
}

/// `build_request` with the storeys over the ground: the levels of the way graph (`graph::levels`), then the
/// storeys' faces, their records after storey 0's in the one block (`records::storeys`), and what their shells read.
/// Storey 0's faces and far block are those of `build_request`.
pub fn build_storeys(
    request: Request,
    cache: &mut TileCache,
    rules: &Rules,
    now: &mut dyn FnMut() -> f64,
) -> Result<CellBuild, String> {
    stages(now(), request, cache, rules, true, now)
}

/// The far block of a request's cell alone (`records::far_block`): the stages input and graph of
/// `build_request`, then the records' lines without their faces. It is the `far` of a full build of
/// the request.
pub fn far_request(request: Request, cache: &mut TileCache, rules: &Rules) -> Result<Vec<u32>, String> {
    let key = request.key;
    let input = request.input(cache)?;
    records::far_block(&graph::build(key, &input, &rules.sections), &input, rules, key)
}

fn stages(
    start: f64,
    request: Request,
    cache: &mut TileCache,
    rules: &Rules,
    storeys: bool,
    now: &mut dyn FnMut() -> f64,
) -> Result<CellBuild, String> {
    let (key, tiles, pages) = (request.key, request.tiles.len(), request.pages.len());
    let input = request.input(cache)?;
    let read = now();
    let mut graph = graph::build(key, &input, &rules.sections);
    let built = now();
    if storeys {
        graph::levels(&mut graph, &input, &rules.sections, &rules.structures);
    }
    let levelled = now();
    let (faces, mut records, far) = records::near(&graph, &input, rules, key, 0, true);
    let faced = now();
    let storeys = if storeys { records::storeys(&graph, &input, rules, key, &mut records) } else { Storeys::default() };
    let storey_times = [levelled - built, now() - faced];
    let times = vec![read - start, built - read, faced - levelled];
    let records = records.words;
    Ok(CellBuild { key, request: None, tiles, pages, graph, faces, records, far, times, storeys, storey_times })
}
