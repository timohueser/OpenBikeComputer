//! The build of one cell from its request bytes, stage by stage, with the time of each stage.

use crate::faces::{records, EdgeKind, FacePlan, Portal, Rules, Storeys, FRONT_LEFT, OUTSIDE};
use crate::frame::CellKey;
use crate::graph::{self, WayGraph};
use crate::mesh;
use crate::openings;
use crate::tile::TileCache;
use crate::transport::{fnv, Request};
use crate::CellInput;

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
    /// With the storeys: the time of the storeys' faces and records (ms).
    pub storey_time: f64,
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
    Ok(CellBuild { request: Some(fnv(bytes)), ..stages(start, request, cache, rules, now)?.0 })
}

/// `build_cell_with` of a request that is not encoded, without its digest. A tile that the request
/// lacks and `cache` does not hold counts as absent.
pub fn build_request(
    request: Request,
    cache: &mut TileCache,
    rules: &Rules,
    now: &mut dyn FnMut() -> f64,
) -> Result<CellBuild, String> {
    Ok(stages(now(), request, cache, rules, now)?.0)
}

/// `build_request`, then the storeys over the ground: their faces, their records after storey 0's in the one block
/// (`records::storeys`) and what their shells read. The graph splits each way at the end lines that the tiles carry
/// (`graph::level_tags::split`). Storey 0's faces and far block are those of `build_request`.
pub fn build_storeys(
    request: Request,
    cache: &mut TileCache,
    rules: &Rules,
    now: &mut dyn FnMut() -> f64,
) -> Result<CellBuild, String> {
    let (b, input, under) = stages(now(), request, cache, rules, now)?;
    Ok(with_storeys(b, &input, under, rules, now))
}

/// The storeys of a cell's build of level 0 (`stages`) and what their shells read.
fn with_storeys(
    mut b: CellBuild,
    input: &CellInput,
    under: Option<records::Under>,
    rules: &Rules,
    now: &mut dyn FnMut() -> f64,
) -> CellBuild {
    let start = now();
    b.storeys = records::storeys(&b.graph, input, rules, b.key, &mut b.records, under);
    let plan = &b.faces;
    let sharp = |f: u32| f != OUTSIDE && mesh::params::sharp_face(&plan.faces[f as usize]);
    let origin_y = b.key.origin().y as f64;
    b.storeys.ground_ends = (plan.edges.iter())
        .filter(|e| {
            e.kind == EdgeKind::DeckEdge && e.left != OUTSIDE && e.right != OUTSIDE && (sharp(e.left) || sharp(e.right))
        })
        .flat_map(|e| mesh::points::edge_points(plan, e, origin_y))
        .collect();
    let portals = (plan.edges.iter()).filter(|e| e.kind == EdgeKind::Portal && e.left != OUTSIDE && e.right != OUTSIDE);
    let labels = if crate::graph::BORES { mesh::face_labels(&plan.faces, &b.records) } else { Vec::new() };
    for e in portals.filter(|_| crate::graph::BORES) {
        let (front, back) = if e.flags & FRONT_LEFT != 0 { (e.left, e.right) } else { (e.right, e.left) };
        let (Some(front), Some(back)) = (labels[front as usize], labels[back as usize]) else { continue };
        // Each piece from a to b with the entering way on its left.
        for w in mesh::points::edge_points(plan, e, origin_y).windows(2) {
            let (a, c) = if e.flags & FRONT_LEFT != 0 { (w[0], w[1]) } else { (w[1], w[0]) };
            b.storeys.portals.push(Portal { a, b: c, front, back });
        }
    }
    b.storey_time = now() - start;
    b
}

/// `build_storeys` of a cell's input, untimed: the crate's tests build made cases of bores with it.
#[cfg(all(test, feature = "bores"))]
pub(crate) fn build_input(input: &CellInput, rules: &Rules) -> CellBuild {
    let (b, under) = level_0(input, rules, 0.0, &mut || 0.0);
    with_storeys(b, input, under, rules, &mut || 0.0)
}

/// The far block of a request's cell alone (`records::far_block`): the stages input and graph of
/// `build_request`, then the records' lines without their faces. It is the `far` of a full build of
/// the request.
pub fn far_request(request: Request, cache: &mut TileCache, rules: &Rules) -> Result<Vec<u32>, String> {
    let key = request.key;
    let input = request.input(cache)?;
    records::far_block(&cell_graph(key, &input, rules), &input, rules, key)
}

/// The way graph of a cell: with the storeys, its ways follow the levels that the tiles carry. The ways of the built
/// tunnel ends lie on the ground up to their openings (`openings::built`).
pub fn cell_graph(key: CellKey, input: &CellInput, rules: &Rules) -> WayGraph {
    let levelled = cfg!(feature = "storeys").then_some(rules.structures.edge_band_m);
    graph::build_open(key, input, &rules.sections, levelled, &openings::built(input).0)
}

fn stages(
    start: f64,
    request: Request,
    cache: &mut TileCache,
    rules: &Rules,
    now: &mut dyn FnMut() -> f64,
) -> Result<(CellBuild, CellInput, Option<records::Under>), String> {
    let (tiles, pages) = (request.tiles.len(), request.pages.len());
    let input = request.input(cache)?;
    let read = now();
    let (mut b, under) = level_0(&input, rules, read, now);
    (b.tiles, b.pages, b.times[0]) = (tiles, pages, read - start);
    Ok((b, input, under))
}

/// The graph, the faces of level 0, their records and the far block of a cell's input, timed from `read`, and the
/// storey under the ground from the same slice.
fn level_0(
    input: &CellInput,
    rules: &Rules,
    read: f64,
    now: &mut dyn FnMut() -> f64,
) -> (CellBuild, Option<records::Under>) {
    let key = input.key;
    let graph = cell_graph(key, input, rules);
    let built = now();
    let (mut faces, records, far, under) = records::near_under(&graph, input, rules, key);
    faces.cuts = openings::built(input).1;
    let times = vec![0.0, built - read, now() - built];
    let records = records.words;
    let (storeys, storey_time) = (Storeys::default(), 0.0);
    let (tiles, pages) = (0, 0);
    (CellBuild { key, request: None, tiles, pages, graph, faces, records, far, times, storeys, storey_time }, under)
}
