//! Integer crop bounds and source header metadata.

use osmpbf::BlobReader;

/// `decimicro / 1e7`, never `* 1e-7`, so coords match osmium exactly.
#[inline]
pub fn to_deg(decimicro: i32) -> f64 {
    decimicro as f64 / 1e7
}

/// A `--bbox` crop region, held in the PBF's own decimicro-degree integer grid — the fixed point
/// `osmium::Location` stores. [`Bbox::contains`] is then an integer comparison, so the in-process
/// crop cannot disagree with `osmium extract` about a node sitting a float ULP from the boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bbox {
    min_lon: i32,
    min_lat: i32,
    max_lon: i32,
    max_lat: i32,
}

/// Degrees → osmium's fixed point: `std::round` half-away-from-zero, same as
/// libosmium's `double_to_fix`. Rust's `f64::round` rounds the same way.
#[inline]
fn to_fix(deg: f64) -> i32 {
    (deg * 1e7).round() as i32
}

impl Bbox {
    /// Parse a `W,S,E,N` degrees spec, as strictly as `osmium extract` parses its own `--bbox`:
    /// four finite in-range numbers, west strictly west of east and south strictly south of north.
    ///
    /// A box wrapping the antimeridian is rejected. Every stage downstream — the header bbox, the
    /// quadtree root box, the land clip — assumes `min < max` in plain degrees.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(format!("--bbox wants four comma-separated numbers W,S,E,N (got {spec:?})"));
        }
        let mut v = [0.0f64; 4];
        for (slot, text) in v.iter_mut().zip(&parts) {
            *slot = text
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .ok_or_else(|| format!("--bbox: {text:?} is not a finite number (expected degrees, W,S,E,N)"))?;
        }
        let [w, s, e, n] = v;
        for (name, deg, limit) in [("west", w, 180.0), ("east", e, 180.0), ("south", s, 90.0), ("north", n, 90.0)] {
            if deg < -limit || deg > limit {
                return Err(format!("--bbox: {name} {deg} is outside ±{limit}°"));
            }
        }
        if w >= e {
            return Err(format!(
                "--bbox: west ({w}) must be strictly west of east ({e}); a box crossing the antimeridian is not \
                 supported — pack the two halves separately"
            ));
        }
        if s >= n {
            return Err(format!("--bbox: south ({s}) must be strictly south of north ({n})"));
        }
        Ok(Bbox { min_lon: to_fix(w), min_lat: to_fix(s), max_lon: to_fix(e), max_lat: to_fix(n) })
    }

    /// The box back in degrees, snapped to the decimicro grid it was parsed onto. Handed to
    /// `osmium extract` on the multi-input merge path, so both croppers see the identical box.
    pub fn to_degrees(self) -> (f64, f64, f64, f64) {
        (to_deg(self.min_lon), to_deg(self.min_lat), to_deg(self.max_lon), to_deg(self.max_lat))
    }

    /// Inclusive integer microdegree coordinates contained in this box.
    pub fn microdegree_bounds(self) -> (i64, i64, i64, i64) {
        (
            (i64::from(self.min_lon) + 9).div_euclid(10),
            (i64::from(self.min_lat) + 9).div_euclid(10),
            i64::from(self.max_lon).div_euclid(10),
            i64::from(self.max_lat).div_euclid(10),
        )
    }

    /// Closed on all four edges, exactly like `osmium::Box::contains`.
    #[inline]
    pub fn contains(&self, lon: i32, lat: i32) -> bool {
        lon >= self.min_lon && lon <= self.max_lon && lat >= self.min_lat && lat <= self.max_lat
    }
}

/// The area of a `W,S,E,N` degree box on the sphere, in km². Pack time follows the region size far
/// more closely than the source file size does: the box decides how much survives ingest.
pub fn box_area_km2((w, s, e, n): (f64, f64, f64, f64)) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0088;
    let lon_span = (e - w).to_radians();
    let lat_band = n.to_radians().sin() - s.to_radians().sin();
    EARTH_RADIUS_KM * EARTH_RADIUS_KM * lon_span * lat_band
}

/// The `W,S,E,N` box a source declares in its PBF header, if it declares one. It is the first blob
/// of the file, so the answer costs one read. A source without it is not an error.
pub fn declared_bbox(path: &str) -> Result<Option<(f64, f64, f64, f64)>, String> {
    let mut reader = BlobReader::from_path(path).map_err(|e| format!("open {path}: {e}"))?;
    let Some(blob) = reader.next() else { return Ok(None) };
    let blob = blob.map_err(|e| format!("read {path}: {e}"))?;
    match blob.decode().map_err(|e| format!("read {path}: {e}"))? {
        osmpbf::BlobDecode::OsmHeader(header) => {
            Ok(header.bbox().map(|b| (b.left, b.bottom, b.right, b.top)).filter(|(w, s, e, n)| w < e && s < n))
        }
        _ => Ok(None),
    }
}
