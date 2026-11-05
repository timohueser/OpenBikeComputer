//! The TrailView ground plan in the browser: the request codec and the cell build of `obc-ground-plan`
//! behind `wasm-bindgen`. The same functions build natively, so the tests run them without a browser.
//!
//! wasm-bindgen objects hold WASM memory until `free()`: a caller frees each `CellRequest` and
//! `CellPlan`.

use obc_ground_plan::cell::{build_cell, CellBuild};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::sections::Sections;
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
}

/// Builds a cell from a request; the request meets the checks of `build_bytes`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn build(request: &CellRequest) -> Result<CellPlan, Error> {
    build_bytes(&request.to_bytes())
}

/// Builds a cell from canonical request bytes, with the embedded rule tables.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn build_bytes(bytes: &[u8]) -> Result<CellPlan, Error> {
    let built = build_cell(bytes, Sections::embedded(), &mut now).map_err(error)?;
    Ok(CellPlan { flat: Flat::of(&built), built })
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
        assert_eq!(plan.times().len(), obc_ground_plan::cell::STAGES.len());
        assert_eq!(build_bytes(&b.to_bytes()).unwrap().digest(), plan.digest());
        assert!(build_bytes(&a.to_bytes()[..40]).is_err());
    }
}
