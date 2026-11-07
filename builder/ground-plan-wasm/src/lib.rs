//! The TrailView ground plan in the browser: the request codec and the cell build of `obc-ground-plan`
//! behind `wasm-bindgen`. The same functions build natively, so the tests run them without a browser.
//!
//! wasm-bindgen objects hold WASM memory until `free()`: a caller frees each `CellRequest`,
//! `CellPlan` and `MeshInput`.

use obc_ground_plan::cell::{build_cell, CellBuild};
use obc_ground_plan::faces::{FacePlan, Rules};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::mesh::{mesh_cell, Reads, Rim};
use obc_ground_plan::terrain::StoredPage;
use obc_ground_plan::transport::{self, FeatureTile, Flat, Request};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub type Error = JsError;
#[cfg(not(target_arch = "wasm32"))]
pub type Error = String;

#[cfg(target_arch = "wasm32")]
fn error(text: String) -> Error {
    JsError::new(&text)
}
#[cfg(not(target_arch = "wasm32"))]
fn error(text: String) -> Error {
    text
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    /// Workers and Node both have `performance`.
    #[wasm_bindgen(js_namespace = performance)]
    fn now() -> f64;
}
#[cfg(not(target_arch = "wasm32"))]
fn now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// The names and colours of the codes of a `CellPlan`, and its stages, as JSON.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn vocabulary() -> String {
    transport::vocabulary()
}

/// The pages that cell (x, y) of zoom 16 reads, `words.wanted` of the vocabulary per page: level,
/// page x, page y, then the stored tile that holds it (z, x, y) and its quadrant.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn wanted(x: u32, y: u32) -> Vec<u32> {
    transport::wanted_words(CellKey { x, y })
}

/// The zoom-13 feature tiles that cell (x, y) of zoom 16 reads, as column, row pairs: one to four.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn feature_tiles(x: u32, y: u32) -> Vec<u32> {
    CellKey { x, y }.feature_tiles().into_iter().flat_map(|(column, row)| [column, row]).collect()
}

/// The inputs of one cell as a caller collects them, in any order.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct CellRequest {
    request: Request,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl CellRequest {
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(constructor))]
    pub fn new(x: u32, y: u32) -> CellRequest {
        CellRequest { request: Request { key: CellKey { x, y }, tiles: Vec::new(), pages: Vec::new() } }
    }

    /// The plain bytes (after gzip) of zoom-13 feature tile (column, row).
    pub fn add_tile(&mut self, column: u32, row: u32, bytes: &[u8]) {
        self.request.tiles.push(FeatureTile { column, row, bytes: bytes.to_vec() });
    }

    /// A stored page: 65,536 codes, row by row from north; a height is `offset + code × scale`.
    pub fn add_page(&mut self, level: u8, x: u32, y: u32, offset: f32, scale: f32, codes: &[u16]) {
        self.request.pages.push(StoredPage { level, x, y, offset, scale, codes: codes.to_vec() });
    }

    /// The canonical request bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.request.encode()
    }
}

/// A built cell: the flat plan, its stats and digest, and the time of each stage.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct CellPlan {
    flat: Flat,
    built: CellBuild,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl CellPlan {
    pub fn samples(&self) -> Vec<i32> {
        self.flat.samples.clone()
    }

    pub fn ways(&self) -> Vec<u32> {
        self.flat.ways.clone()
    }

    pub fn points(&self) -> Vec<u32> {
        self.flat.points.clone()
    }

    pub fn crossings(&self) -> Vec<u32> {
        self.flat.crossings.clone()
    }

    /// The faces of level 0: x, y per vertex.
    pub fn vertices(&self) -> Vec<i32> {
        self.flat.faces.vertices.clone()
    }

    /// `words.face` of the vocabulary per face.
    pub fn faces(&self) -> Vec<u32> {
        self.flat.faces.faces.clone()
    }

    /// `words.ring` per ring: first index into `ring_vertices`, count; a face's first ring is its outer ring.
    pub fn rings(&self) -> Vec<u32> {
        self.flat.faces.rings.clone()
    }

    pub fn ring_vertices(&self) -> Vec<u32> {
        self.flat.faces.ring_vertices.clone()
    }

    /// `words.edge` per edge.
    pub fn edges(&self) -> Vec<u32> {
        self.flat.faces.edges.clone()
    }

    pub fn stats(&self) -> String {
        self.flat.stats.clone()
    }

    /// Milliseconds per stage, in the order of the vocabulary's `stages`.
    pub fn times(&self) -> Vec<f64> {
        self.built.times.clone()
    }

    /// The digest of the plan, or an error where it holds a NaN: it costs about as much as the build of the graph.
    pub fn digest(&self) -> Result<String, Error> {
        self.flat.digest(&self.built).map_err(error)
    }

    /// What the neighbours' meshes read of this cell: `Rim::to_words`.
    pub fn rim(&self) -> Vec<u32> {
        Rim::of(&self.built.faces).to_words()
    }

    /// What the cell's mesh reads, kept apart so the plan can go while the mesh waits for its
    /// neighbours' rims.
    pub fn mesh_input(&self) -> MeshInput {
        MeshInput { plan: self.built.faces.clone(), records: self.built.records.clone(), last: None }
    }
}

/// A cell's faces and near records, for its mesh.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct MeshInput {
    plan: FacePlan,
    records: Vec<u32>,
    /// What the last mesh read of the rims.
    last: Option<Reads>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl MeshInput {
    /// The near records: the block whose labels the mesh's corners name, for the GPU.
    pub fn records(&self) -> Vec<u32> {
        self.records.clone()
    }

    /// The bytes of the cell's near mesh (`CellMesh::to_bytes`) from the rims of the cell and its eight
    /// neighbours, one after another in any order; a missing neighbour counts as one with no sharp
    /// face. None when the mesh reads the same of them as the last call did (that call's mesh or error
    /// stands), so a caller may call it at every change of a rim.
    pub fn mesh(&mut self, rims: &[u32]) -> Result<Option<Vec<u8>>, Error> {
        let rims = Rim::from_words(rims).map_err(error)?;
        let reads = Reads::of(&self.plan, &rims);
        if self.last.as_ref() == Some(&reads) {
            return Ok(None);
        }
        let mesh = mesh_cell(&self.plan, &self.records, &reads, &mut now);
        self.last = Some(reads);
        Ok(Some(mesh.map_err(|e| error(format!("{e:?}")))?.to_bytes()))
    }
}

/// Builds a cell from a request; the request meets the checks of `build_bytes`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn build(request: &CellRequest) -> Result<CellPlan, Error> {
    build_bytes(&request.to_bytes())
}

/// Builds a cell from canonical request bytes, with the embedded rule tables.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn build_bytes(bytes: &[u8]) -> Result<CellPlan, Error> {
    let built = build_cell(bytes, Rules::embedded(), &mut now).map_err(error)?;
    Ok(CellPlan { flat: Flat::of(&built), built })
}

/// The near records of a request's cell (`faces::records`), the words that the stamp pass reads.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn records(request: &CellRequest) -> Result<Vec<u32>, Error> {
    use obc_ground_plan::faces::records;
    let request = Request::decode(&request.to_bytes()).map_err(error)?;
    let key = request.key;
    let input = request.input().map_err(error)?;
    let rules = Rules::embedded();
    let graph = obc_ground_plan::graph::build(key, &input, &rules.sections);
    Ok(records::near(&graph, &input, rules, key, 0, true).1.words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features() -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../specs/vectors/trailview-features.bin")).unwrap()
    }

    /// The north-west cell of zoom-13 tile 4286/2888, the tile of the vector: its roads with heights lie there.
    const CELL: (u32, u32) = (4286 * 8, 2888 * 8);

    #[test]
    fn wanted_names_pages_of_levels_13_and_14_with_their_stored_tiles() {
        let words = wanted(CELL.0, CELL.1);
        assert!(!words.is_empty() && words.len().is_multiple_of(transport::WANTED_WORDS));
        for page in words.chunks(transport::WANTED_WORDS) {
            assert!(page[0] == 13 || page[0] == 14, "{page:?}");
            assert_eq!(page[3..6], [page[0] - 1, page[1] >> 1, page[2] >> 1]);
        }
    }

    #[test]
    fn a_request_in_any_order_gives_one_plan() {
        // Flat ground at 500 m on every wanted page, added in two orders.
        let pages: Vec<[u32; transport::WANTED_WORDS]> =
            wanted(CELL.0, CELL.1).chunks(transport::WANTED_WORDS).map(|p| p.try_into().unwrap()).collect();
        let request = |pages: &mut dyn Iterator<Item = &[u32; transport::WANTED_WORDS]>| {
            let mut request = CellRequest::new(CELL.0, CELL.1);
            for p in pages {
                request.add_page(p[0] as u8, p[1], p[2], 500.0, 0.01, &vec![0; 256 * 256]);
            }
            request.add_tile(4286, 2888, &features());
            request
        };
        let (a, b) = (request(&mut pages.iter()), request(&mut pages.iter().rev()));
        assert_eq!(a.to_bytes(), b.to_bytes());
        let plan = build(&a).unwrap();
        let stats: serde_json::Value = serde_json::from_str(&plan.stats()).unwrap();
        assert_eq!(stats["inputs"]["pages"].as_u64(), Some(pages.len() as u64));
        assert_eq!(stats["inputs"]["tiles"].as_u64(), Some(1));
        assert!(!plan.ways().is_empty(), "{stats}");
        let samples = plan.samples().len() as u32 / 2;
        for way in plan.ways().chunks(transport::WAY_WORDS) {
            assert!(way[4] + way[5] <= samples, "a way's samples lie inside the samples");
        }
        let vertices = plan.vertices().len() as u32 / 2;
        let (rings, ring_vertices) = (plan.rings(), plan.ring_vertices());
        assert!(!plan.faces().is_empty() && ring_vertices.iter().all(|&v| v < vertices));
        for ring in rings.chunks(transport::RING_WORDS) {
            assert!(ring[0] + ring[1] <= ring_vertices.len() as u32);
        }
        assert!(plan.edges().chunks(transport::EDGE_WORDS).all(|e| e[0] < vertices && e[1] < vertices));
        assert_eq!(plan.times().len(), obc_ground_plan::cell::STAGES.len());
        assert_eq!(build_bytes(&b.to_bytes()).unwrap().digest(), plan.digest());
        assert!(build_bytes(&a.to_bytes()[..40]).is_err());
        let mut input = plan.mesh_input();
        let mesh = input.mesh(&plan.rim()).unwrap().unwrap();
        assert_eq!(u32::from_le_bytes(mesh[..4].try_into().unwrap()), obc_ground_plan::mesh::MAGIC);
        assert_eq!(input.mesh(&plan.rim()).unwrap(), None);
    }
}
