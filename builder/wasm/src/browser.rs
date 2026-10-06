use obc_formats::grid;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

fn error(name: &str, message: &str) -> JsValue {
    let error = js_sys::Error::new(message);
    error.set_name(name);
    error.into()
}

fn check_log2(log2: f64) -> Result<u32, JsValue> {
    if log2.fract() != 0.0 || !(grid::MIN_CELL_LOG2 as f64..=grid::MAX_CELL_LOG2 as f64).contains(&log2) {
        return Err(error(
            "GridError",
            &format!("cell size 2^{log2} µdeg is outside the grid's {}..={}", grid::MIN_CELL_LOG2, grid::MAX_CELL_LOG2),
        ));
    }
    Ok(log2 as u32)
}

#[wasm_bindgen]
pub fn obc_builder_constants() -> Vec<f64> {
    vec![
        grid::GRID_ORIGIN as f64,
        grid::WORLD_SIDE as f64,
        grid::MIN_CELL_LOG2 as f64,
        grid::MAX_CELL_LOG2 as f64,
        65_536.0,
        crate::gpx::MAX_ROUTE_POINTS as f64,
    ]
}

#[wasm_bindgen]
pub fn obc_grid_cell_size(log2: u32) -> f64 {
    grid::cell_size(log2) as f64
}

#[wasm_bindgen]
pub fn obc_grid_axis_cells(log2: u32) -> f64 {
    grid::axis_cells(log2) as f64
}

#[wasm_bindgen]
pub fn obc_grid_id_width(log2: u32) -> usize {
    grid::id_width(log2)
}

#[wasm_bindgen]
pub fn obc_grid_cell_id(log2: f64, i: f64, j: f64) -> Result<Vec<f64>, JsValue> {
    let size = check_log2(log2)?;
    let n = grid::axis_cells(size) as f64;
    if i.fract() != 0.0 || j.fract() != 0.0 || !(0.0..n).contains(&i) || !(0.0..n).contains(&j) {
        return Err(error(
            "GridError",
            &format!("cell 2^{log2}/{i}/{j} is outside the world box (indices must be 0..{})", n - 1.0),
        ));
    }
    Ok(vec![log2, i, j])
}

#[wasm_bindgen]
pub fn obc_grid_parse_id(value: &str) -> Result<Vec<f64>, JsValue> {
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        let quoted = js_sys::JSON::stringify(&JsValue::from_str(value))?.as_string().unwrap_or_default();
        return Err(error("GridError", &format!("cell id {quoted} is not <log2>/<i>/<j>")));
    }
    let number = |p: &str| p.parse::<f64>().unwrap_or(f64::INFINITY);
    obc_grid_cell_id(number(parts[0]), number(parts[1]), number(parts[2]))
}

#[wasm_bindgen]
pub fn obc_grid_format_id(log2: u32, i: f64, j: f64) -> String {
    let width = grid::id_width(log2);
    let (i, j) = ((i as i64).to_string(), (j as i64).to_string());
    format!("{log2}/{i:0>width$}/{j:0>width$}")
}

#[wasm_bindgen]
pub fn obc_grid_square(log2: u32, i: f64, j: f64) -> Vec<f64> {
    let (min_lon, min_lat, max_lon, max_lat) = grid::cell_square(log2, i as i64, j as i64);
    vec![min_lat as f64, min_lon as f64, max_lat as f64, max_lon as f64]
}

#[wasm_bindgen]
pub fn obc_grid_containing(log2: u32, lat: f64, lon: f64) -> Vec<f64> {
    let size = grid::cell_size(log2) as f64;
    vec![
        log2 as f64,
        ((lat - grid::GRID_ORIGIN as f64) / size).floor(),
        ((lon - grid::GRID_ORIGIN as f64) / size).floor(),
    ]
}

#[wasm_bindgen]
pub fn obc_grid_contains(log2: u32, i: f64, j: f64, lat: f64, lon: f64) -> bool {
    let (min_lon, min_lat, max_lon, max_lat) = grid::cell_square(log2, i as i64, j as i64);
    lat >= min_lat as f64 && lat < max_lat as f64 && lon >= min_lon as f64 && lon < max_lon as f64
}

#[wasm_bindgen]
pub fn obc_grid_intersecting(
    log2: f64,
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
    max_cells: f64,
) -> Result<Vec<f64>, JsValue> {
    let log2 = check_log2(log2)?;
    let origin = grid::GRID_ORIGIN as f64;
    let world_max = origin + grid::WORLD_SIDE as f64;
    if [min_lat, min_lon, max_lat, max_lon].iter().any(|v| v.is_nan())
        || min_lat > max_lat
        || min_lon > max_lon
        || max_lat < origin
        || min_lat >= world_max
        || max_lon < origin
        || min_lon >= world_max
    {
        return Ok(Vec::new());
    }
    let n = grid::axis_cells(log2) as f64;
    let size = grid::cell_size(log2) as f64;
    let index = |v: f64| ((v - origin) / size).floor().clamp(0.0, n - 1.0) as i64;
    let (i0, i1, j0, j1) = (index(min_lat), index(max_lat), index(min_lon), index(max_lon));
    let count = (i1 - i0 + 1) * (j1 - j0 + 1);
    if count as f64 > max_cells {
        return Err(error("GridError", &format!("this box covers {count} cells of size 2^{log2} µdeg, more than the {max_cells} this client will enumerate at once — select a smaller area")));
    }
    let mut out = Vec::with_capacity(count as usize * 3);
    for i in i0..=i1 {
        for j in j0..=j1 {
            out.extend([log2 as f64, i as f64, j as f64]);
        }
    }
    Ok(out)
}

#[wasm_bindgen]
pub fn obc_grid_on_line(value: f64, log2: u32) -> bool {
    value.fract() == 0.0 && grid::on_grid_line(value as i64, log2)
}

#[wasm_bindgen]
pub fn obc_grid_coverage(cells: &[f64]) -> Vec<f64> {
    let mut box_ = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    if cells.is_empty() {
        return Vec::new();
    }
    for c in cells.chunks_exact(3) {
        let (min_lon, min_lat, max_lon, max_lat) = grid::cell_square(c[0] as u32, c[1] as i64, c[2] as i64);
        box_[0] = box_[0].min(min_lat as f64);
        box_[1] = box_[1].min(min_lon as f64);
        box_[2] = box_[2].max(max_lat as f64);
        box_[3] = box_[3].max(max_lon as f64);
    }
    box_.to_vec()
}

#[wasm_bindgen]
pub fn obc_gpx_read(text: &str, fallback: &str) -> Result<String, JsValue> {
    let imported = crate::gpx::read(text, fallback).map_err(|e| error("GpxError", &e))?;
    serde_json::to_string(&imported).map_err(|e| error("GpxError", &e.to_string()))
}

#[wasm_bindgen]
pub fn obc_gpx_parse(text: &str, fallback: &str) -> Result<String, JsValue> {
    let route = crate::gpx::parse(text, fallback).map_err(|e| error("GpxError", &e))?;
    serde_json::to_string(&route).map_err(|e| error("GpxError", &e.to_string()))
}

#[wasm_bindgen(js_name = IncrementalSha256)]
pub struct IncrementalSha256(Option<Sha256>);

impl Default for IncrementalSha256 {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl IncrementalSha256 {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self(Some(Sha256::new()))
    }

    pub fn update(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        let hash = self.0.as_mut().ok_or_else(|| error("Error", "Sha256.update after digest()"))?;
        hash.update(bytes);
        Ok(())
    }

    pub fn digest(&mut self) -> Result<Vec<u8>, JsValue> {
        let hash = self.0.take().ok_or_else(|| error("Error", "Sha256.digest called twice"))?;
        Ok(hash.finalize().to_vec())
    }
}
