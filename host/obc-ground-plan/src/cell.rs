//! The build of one cell from its request bytes, stage by stage, with the time of each stage.

use crate::frame::CellKey;
use crate::graph::{self, WayGraph};
use crate::sections::Sections;
use crate::transport::{fnv, Request};

/// The stages in their order: `times` holds one time per stage.
pub const STAGES: [&str; 2] = ["input", "graph"];

/// A built cell: what its request held, its plan and the time of each stage in milliseconds.
#[derive(Clone, Debug)]
pub struct CellBuild {
    pub key: CellKey,
    /// The digest of the request bytes.
    pub request: String,
    pub tiles: usize,
    pub pages: usize,
    pub graph: WayGraph,
    pub times: Vec<f64>,
}

/// Builds the cell of a request: `input` decodes the request, the feature tiles and the terrain;
/// `graph` builds the way graph. `now` gives milliseconds; no output byte depends on it, so a caller
/// that does not time passes `&mut || 0.0`.
pub fn build_cell(bytes: &[u8], rules: &Sections, now: &mut dyn FnMut() -> f64) -> Result<CellBuild, String> {
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
    let graph = graph::build(key, &input, rules);
    lap(&mut times);
    Ok(CellBuild { key, request: digest, tiles, pages, graph, times })
}
