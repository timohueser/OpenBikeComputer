//! The TrailView ground plan in the browser: the request codec and the cell build of `obc-ground-plan`
//! behind `wasm-bindgen`. The same functions build natively, so the tests run them without a browser.
//!
//! wasm-bindgen objects hold WASM memory until `free()`: a caller frees each `CellRequest`,
//! `CellPlan` and `MeshInput`.

use std::cell::{OnceCell, RefCell};

use obc_ground_plan::cell::{build_cell, build_request, build_storeys, far_request, CellBuild};
use obc_ground_plan::faces::{FacePlan, Rules};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::mesh::{mesh_cell, Reads, Rim};
use obc_ground_plan::terrain::{StoredPage, PAGE};
use obc_ground_plan::tile::TileCache;
use obc_ground_plan::transport::{self, fnv, FeatureTile, Flat, Request};

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
    /// The feature tiles that exist and that the instance holds parsed, without their bytes.
    named: Vec<(u32, u32)>,
}

/// The named tiles that `tiles` does not hold; each held one counts as used now.
fn unheld(named: &[(u32, u32)], tiles: &mut TileCache) -> Vec<(u32, u32)> {
    named.iter().copied().filter(|&(column, row)| !tiles.touch(column, row)).collect()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl CellRequest {
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(constructor))]
    pub fn new(x: u32, y: u32) -> CellRequest {
        CellRequest {
            request: Request { key: CellKey { x, y }, tiles: Vec::new(), pages: Vec::new() },
            named: Vec::new(),
        }
    }

    /// The plain bytes (after gzip) of zoom-13 feature tile (column, row). The build parses them once
    /// per instance; later requests can name the tile.
    pub fn add_tile(&mut self, column: u32, row: u32, bytes: Vec<u8>) {
        self.request.tiles.push(FeatureTile { column, row, bytes });
    }

    /// Zoom-13 feature tile (column, row), which exists and whose bytes an earlier request of this
    /// instance held.
    pub fn name_tile(&mut self, column: u32, row: u32) {
        self.named.push((column, row));
    }

    /// A stored page: 65,536 codes, row by row from north; a height is `offset + code × scale`.
    pub fn add_page(
        &mut self,
        level: u8,
        x: u32,
        y: u32,
        offset: f32,
        scale: f32,
        codes: Vec<u16>,
    ) -> Result<(), Error> {
        if codes.len() != PAGE * PAGE {
            return Err(error(format!("page {level}/{x}/{y} has {} codes, not {}", codes.len(), PAGE * PAGE)));
        }
        self.request.pages.push(StoredPage { level, x, y, offset, scale, codes });
        Ok(())
    }

    /// The named tiles that the instance no longer holds, as column, row pairs: the caller gives
    /// their bytes. A build right after an empty answer finds every named tile.
    pub fn missing(&self) -> Vec<u32> {
        PARSED
            .with_borrow_mut(|tiles| unheld(&self.named, tiles))
            .into_iter()
            .flat_map(|(column, row)| [column, row])
            .collect()
    }

    /// The canonical request bytes; an error where a tile is only named.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        match self.named.first() {
            Some((column, row)) => Err(error(format!("tile 13/{column}/{row} is named without its bytes"))),
            None => Ok(self.request.encode()),
        }
    }
}

/// A built cell: the flat plan, its stats and digest, and the time of each stage. The flat plan is made at the
/// first call that reads it: the records' builds read only the rim and the mesh input.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct CellPlan {
    flat: OnceCell<Flat>,
    built: CellBuild,
}

impl CellPlan {
    fn flat(&self) -> &Flat {
        self.flat.get_or_init(|| Flat::of(&self.built))
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl CellPlan {
    pub fn samples(&self) -> Vec<i32> {
        self.flat().samples.clone()
    }

    pub fn ways(&self) -> Vec<u32> {
        self.flat().ways.clone()
    }

    pub fn points(&self) -> Vec<u32> {
        self.flat().points.clone()
    }

    pub fn crossings(&self) -> Vec<u32> {
        self.flat().crossings.clone()
    }

    /// The faces of level 0: x, y per vertex.
    pub fn vertices(&self) -> Vec<i32> {
        self.flat().faces.vertices.clone()
    }

    /// `words.face` of the vocabulary per face.
    pub fn faces(&self) -> Vec<u32> {
        self.flat().faces.faces.clone()
    }

    /// `words.ring` per ring: first index into `ring_vertices`, count; a face's first ring is its outer ring.
    pub fn rings(&self) -> Vec<u32> {
        self.flat().faces.rings.clone()
    }

    pub fn ring_vertices(&self) -> Vec<u32> {
        self.flat().faces.ring_vertices.clone()
    }

    /// `words.edge` per edge.
    pub fn edges(&self) -> Vec<u32> {
        self.flat().faces.edges.clone()
    }

    pub fn stats(&self) -> String {
        self.flat().stats.clone()
    }

    /// Milliseconds per stage, in the order of the vocabulary's `stages`.
    pub fn times(&self) -> Vec<f64> {
        self.built.times.clone()
    }

    /// The digest of the plan, or an error where it holds a NaN: it costs about as much as the build of the graph.
    pub fn digest(&self) -> Result<String, Error> {
        self.flat().digest(&self.built).map_err(error)
    }

    /// What the neighbours' meshes read of this cell: `Rim::to_words`.
    pub fn rim(&self) -> Vec<u32> {
        Rim::of(&self.built.faces).to_words()
    }

    /// What the cell's mesh reads, kept apart so the plan can go while the mesh waits for its
    /// neighbours' rims.
    pub fn mesh_input(&self) -> MeshInput {
        MeshInput {
            plan: self.built.faces.clone(),
            records: self.built.records.clone(),
            last: None,
            #[cfg(feature = "storeys")]
            storeys: self.built.storeys.clone(),
        }
    }

    /// The milliseconds of the storeys' faces and records.
    #[cfg(feature = "storeys")]
    pub fn storey_time(&self) -> f64 {
        self.built.storey_times
    }

    /// The far block of the records' lines (`records::far` of obc-ground-plan), or why the crate
    /// refused it: the records stand either way.
    pub fn far(&self) -> Result<Vec<u32>, Error> {
        self.built.far.clone().map_err(error)
    }
}

/// A cell's faces and near records, for its mesh.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct MeshInput {
    plan: FacePlan,
    records: Vec<u32>,
    /// What the last mesh read of the rims.
    last: Option<Reads>,
    #[cfg(feature = "storeys")]
    storeys: obc_ground_plan::faces::Storeys,
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

    /// The bytes of the meshes of the cell's storeys over the ground (`StoreyMesh::to_bytes`).
    #[cfg(feature = "storeys")]
    pub fn storeys(&self) -> Result<Vec<u8>, Error> {
        let rules = Rules::embedded();
        obc_ground_plan::mesh::shell::mesh_storeys(self.plan.cell, &self.storeys, &self.records, &rules.structures)
            .map(|m| m.to_bytes())
            .map_err(|e| error(format!("{e:?}")))
    }
}

/// The zoom-13 tiles that an instance keeps parsed: a worker builds the cells about the eye, which
/// share their tiles. A town tile takes about 2 MB of the heap.
const TILES: usize = 6;
// A cell reads up to 4 tiles, and `build` marks its named tiles used first: the build's own parses
// then never let one of them go.
const _: () = assert!(TILES >= 4);

thread_local! {
    static PARSED: RefCell<TileCache> = RefCell::new(TileCache::new(TILES));
}

/// `f` of a request's inputs with the instance's parsed tiles, once it holds every named tile: a named
/// tile that is not held would build as an absent one.
fn held<T>(request: CellRequest, f: impl FnOnce(Request, &mut TileCache) -> Result<T, String>) -> Result<T, Error> {
    let CellRequest { request, named } = request;
    PARSED
        .with_borrow_mut(|tiles| match unheld(&named, tiles).first() {
            Some((column, row)) => Err(format!("tile 13/{column}/{row} is named, and the instance no longer holds it")),
            None => f(request, tiles),
        })
        .map_err(error)
}

/// Builds a cell from a request and takes its inputs; the request meets the checks of `build_bytes`.
/// Each tile parses once per instance, so an instance serves one feature archive. With `digest` the
/// stats hold the digest of the canonical bytes, as `build_bytes` gives it, and no tile is only named.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn build(request: CellRequest, digest: bool) -> Result<CellPlan, Error> {
    let digest = if digest { Some(fnv(&request.to_bytes()?)) } else { None };
    let named = request.named.len();
    let mut built = held(request, build_of)?;
    built.request = digest;
    built.tiles += named;
    Ok(CellPlan { flat: OnceCell::new(), built })
}

/// The far block of a request's cell alone (`cell::far_request`), the `far` of its `build`; it takes
/// the request as `build` does.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn far(request: CellRequest) -> Result<Vec<u32>, Error> {
    held(request, |inputs, tiles| far_request(inputs, tiles, Rules::embedded()))
}

/// Builds a cell from canonical request bytes, with the embedded rule tables.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn build_bytes(bytes: &[u8]) -> Result<CellPlan, Error> {
    let built = if cfg!(feature = "storeys") {
        let built = Request::decode(bytes).and_then(|r| build_of(r, &mut TileCache::new(4)));
        built.map(|b| CellBuild { request: Some(fnv(bytes)), ..b })
    } else {
        build_cell(bytes, Rules::embedded(), &mut now)
    };
    Ok(CellPlan { flat: OnceCell::new(), built: built.map_err(error)? })
}

/// The cell build of a request: with the feature `storeys`, its storeys over the ground too.
fn build_of(request: Request, tiles: &mut TileCache) -> Result<CellBuild, String> {
    let build = if cfg!(feature = "storeys") { build_storeys } else { build_request };
    build(request, tiles, Rules::embedded(), &mut now)
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
    fn a_request_in_any_order_gives_one_plan_and_one_far_block() {
        // Flat ground at 500 m on every wanted page, added in two orders.
        let pages: Vec<[u32; transport::WANTED_WORDS]> =
            wanted(CELL.0, CELL.1).chunks(transport::WANTED_WORDS).map(|p| p.try_into().unwrap()).collect();
        let request = |pages: &mut dyn Iterator<Item = &[u32; transport::WANTED_WORDS]>| {
            let mut request = CellRequest::new(CELL.0, CELL.1);
            for p in pages {
                request.add_page(p[0] as u8, p[1], p[2], 500.0, 0.01, vec![0; 256 * 256]).unwrap();
            }
            request
        };
        let (mut a, mut b) = (request(&mut pages.iter()), request(&mut pages.iter().rev()));
        a.add_tile(4286, 2888, features());
        b.add_tile(4286, 2888, features());
        let bytes = b.to_bytes().unwrap();
        assert_eq!(a.to_bytes().unwrap(), bytes);
        let plan = build(a, true).unwrap();
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
        assert_eq!(build_bytes(&bytes).unwrap().digest(), plan.digest());
        assert!(build_bytes(&bytes[..40]).is_err());
        let mut input = plan.mesh_input();
        let mesh = input.mesh(&plan.rim()).unwrap().unwrap();
        assert_eq!(u32::from_le_bytes(mesh[..4].try_into().unwrap()), obc_ground_plan::mesh::MAGIC);
        assert_eq!(input.mesh(&plan.rim()).unwrap(), None);
        // The instance holds the tile now: a request that names it builds the same records, rim and mesh.
        let mut named = request(&mut pages.iter());
        named.name_tile(4286, 2888);
        assert!(named.missing().is_empty() && named.to_bytes().is_err());
        let again = build(named, false).unwrap();
        assert_eq!((again.mesh_input().records(), again.rim()), (input.records(), plan.rim()));
        assert_eq!(again.mesh_input().mesh(&plan.rim()).unwrap().unwrap(), mesh);
        // A full build, one from the bytes, a far-only build and one by the named tile give one far block.
        let block = plan.far().unwrap();
        assert!(block[3] > 0, "the cell's roads reach it");
        assert_eq!((again.far().unwrap(), build_bytes(&bytes).unwrap().far().unwrap()), (block.clone(), block.clone()));
        let mut alone = request(&mut pages.iter());
        alone.add_tile(4286, 2888, features());
        let mut named = request(&mut pages.iter());
        named.name_tile(4286, 2888);
        assert_eq!((far(alone).unwrap(), far(named).unwrap()), (block.clone(), block));
        // A tile never given is missing, and the builds refuse it instead of building without it.
        let unknown = || {
            let mut r = request(&mut pages.iter());
            r.name_tile(4286, 2887);
            r
        };
        assert_eq!(unknown().missing(), [4286, 2887]);
        assert!(build(unknown(), false).is_err() && far(unknown()).is_err());
        assert!(CellRequest::new(CELL.0, CELL.1).add_page(14, 0, 0, 0.0, 1.0, vec![0; 100]).is_err());
    }
}
