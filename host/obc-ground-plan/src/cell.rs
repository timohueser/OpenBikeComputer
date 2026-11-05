//! The build of one cell from its request bytes, stage by stage, with the time of each stage.

use crate::faces::{records, FacePlan, Rules};
use crate::frame::CellKey;
use crate::graph::{self, WayGraph};
use crate::transport::{fnv, Request};

/// The stages in their order: `times` holds one time per stage.
pub const STAGES: [&str; 3] = ["input", "graph", "faces"];

/// A built cell: what its request held, its plan and the time of each stage in milliseconds.
#[derive(Clone, Debug)]
pub struct CellBuild {
    pub key: CellKey,
    /// The digest of the request bytes.
    pub request: String,
    pub tiles: usize,
    pub pages: usize,
    pub graph: WayGraph,
    /// The faces of level 0.
    pub faces: FacePlan,
    /// The near records of the faces (`records::Built::near`): the block that the GPU reads and whose
    /// labels the mesh's corners name.
    pub records: Vec<u32>,
    pub times: Vec<f64>,
}

/// Builds the cell of a request: `input` decodes the request, the feature tiles and the terrain;
/// `graph` builds the way graph; `faces` builds the faces of level 0 and their records in one pass.
/// `now` gives milliseconds; no output byte depends on it, so a caller that does not time passes
/// `&mut || 0.0`.
pub fn build_cell(bytes: &[u8], rules: &Rules, now: &mut dyn FnMut() -> f64) -> Result<CellBuild, String> {
    let mut times = Vec::with_capacity(STAGES.len());
    let mut last = now();
    let mut lap = |times: &mut Vec<f64>| {
        let t = now();
        times.push(t - last);
        last = t;
    };
    let request = Request::decode(bytes)?;
    let (key, digest, tiles, pages) = (request.key, fnv(bytes), request.tiles.len(), request.pages.len());
    let input = request.input()?;
    lap(&mut times);
    let graph = graph::build(key, &input, &rules.sections);
    lap(&mut times);
    let built = records::build(&graph, &input, rules, key, 0, true);
    lap(&mut times);
    Ok(CellBuild { key, request: digest, tiles, pages, graph, faces: built.faces, records: built.near.words, times })
}
