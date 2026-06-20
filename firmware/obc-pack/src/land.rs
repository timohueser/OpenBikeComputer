//! `land.rs` — native port of `packer/obcm/land_ingest.py` (**Stage 5**). Generates
//! the land-fill polygons for an extract by clipping the global
//! [land-polygons-split-3857] dataset to the map's bounding box.
//!
//! [land-polygons-split-3857]: https://osmdata.openstreetmap.de/data/land-polygons.html
//!
//! The oracle path is `fiona` (read the shapefile) + `pyproj` (reproject) +
//! `shapely` (clip). This replaces all three with no Python:
//!
//!   - **Shapefile read** — the `.shp` is parsed directly (the polygon record
//!     format is trivial), with a per-record **bounding-box skip** so only records
//!     that touch the query box are decoded. This matches what GDAL's bbox filter
//!     does (the dataset has no spatial index, so GDAL also scans record MBRs).
//!   - **Reproject** — EPSG:3857 here is the *Web Mercator Auxiliary Sphere*
//!     (`SPHEROID` radius = `6378137`, the `.prj` confirms), i.e. the closed-form
//!     spherical mercator. The forward/inverse below match `pyproj`'s EPSG:3857 to
//!     ~1e-14° (verified) — far below the microdegree quantization — so the
//!     generated land is µdeg-identical to the oracle. No PROJ datum grids.
//!   - **Clip** — the same GEOS `intersection` shapely uses, against the box built
//!     from the forward-projected bbox corners (shapely `box(...)` ring order),
//!     done in 3857 *before* reprojecting the result (exactly the oracle order).
//!
//! Output is one [`Geom::Polygon`] per land face (a clip can split one shapefile
//! polygon into several, or yield a multipolygon — we flatten to simple polygons,
//! the Stage-4 relation convention, so each flows through the existing
//! simplify+quadtree path; `main.rs` styles them with the `natural.land` style).

use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};

use geos::{Geom as _, Geometry};

use crate::geom::{collect_polygons, geom_from_geos, ring_to_coordseq, Geom};

/// EPSG:3857 auxiliary-sphere radius = WGS84 semi-major axis (see the `.prj`).
const R: f64 = 6_378_137.0;

/// Where `land_ingest.py` caches the dataset (`~/.cache/obcm/land`).
const LAND_URL: &str = "https://osmdata.openstreetmap.de/download/land-polygons-split-3857.zip";

// --- Reprojection (closed-form spherical Web Mercator) ---------------------

/// EPSG:3857 → EPSG:4326: (meters east, meters north) → (lon°, lat°).
#[inline]
fn merc_inverse(x: f64, y: f64) -> (f64, f64) {
    let lon = (x / R).to_degrees();
    let lat = (2.0 * (y / R).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    (lon, lat)
}

/// EPSG:4326 → EPSG:3857: (lon°, lat°) → (meters east, meters north). Used only
/// for the clip-box corners (mirrors the oracle's `Transformer.transform`).
#[inline]
fn merc_forward(lon: f64, lat: f64) -> (f64, f64) {
    let x = R * lon.to_radians();
    let y = R * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0).tan().ln();
    (x, y)
}

fn reproject_ring(ring: &mut [(f64, f64)]) {
    for p in ring.iter_mut() {
        *p = merc_inverse(p.0, p.1);
    }
}

/// Reproject every vertex of a (possibly multi/nested) geometry from 3857 → deg,
/// in place — applied to a clip result before it is split into polygons.
fn reproject_geom(g: &mut Geom) {
    match g {
        Geom::Line(c) => reproject_ring(c),
        Geom::Polygon { exterior, interiors } => {
            reproject_ring(exterior);
            for r in interiors {
                reproject_ring(r);
            }
        }
        Geom::Multi(parts) => {
            for p in parts {
                reproject_geom(p);
            }
        }
        Geom::Empty => {}
    }
}

// --- Public entry ----------------------------------------------------------

/// Land polygons for `bbox_deg = (min_lon, min_lat, max_lon, max_lat)`, clipped to
/// it and reprojected to degrees. One [`Geom::Polygon`] per face. Mirrors
/// `land_ingest.get_land_polygons`.
pub fn get_land_polygons(bbox_deg: (f64, f64, f64, f64)) -> Result<Vec<Geom>, String> {
    let shp = ensure_dataset()?;
    let (min_lon, min_lat, max_lon, max_lat) = bbox_deg;
    // Project the bbox corners to 3857 for the filter + clip box. Mercator is
    // monotone in both axes, so corners stay corners (min→min, max→max).
    let (qminx, qminy) = merc_forward(min_lon, min_lat);
    let (qmaxx, qmaxy) = merc_forward(max_lon, max_lat);
    let qbox = (qminx, qminy, qmaxx, qmaxy);
    let box_geom = box_polygon(qbox)?;

    let mut out = Vec::new();
    read_shapefile(&shp, qbox, &box_geom, &mut out)?;
    Ok(out)
}

// --- Shapefile reader (.shp polygon records, with bbox skip) ---------------

#[inline]
fn be_i32(b: &[u8]) -> i32 {
    i32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
#[inline]
fn le_i32(b: &[u8]) -> i32 {
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
#[inline]
fn le_f64(b: &[u8]) -> f64 {
    f64::from_le_bytes(b[0..8].try_into().unwrap())
}

/// Shapefile polygon shape-type code.
const SHP_POLYGON: i32 = 5;

/// Scan `shp`, decode every polygon record whose MBR meets `qbox` (EPSG:3857),
/// clip it, reproject, and push the resulting polygons to `out`. Records outside
/// the box are skipped after reading only their 36-byte header+MBR.
fn read_shapefile(
    shp: &Path,
    qbox: (f64, f64, f64, f64),
    box_geom: &Geometry,
    out: &mut Vec<Geom>,
) -> Result<(), String> {
    let file = File::open(shp).map_err(|e| format!("open {}: {e}", shp.display()))?;
    // A large buffer keeps the per-record header reads + body skips mostly in
    // memory, so the scan is ~one sequential pass over the (~1.3 GB) file.
    let mut r = BufReader::with_capacity(1 << 20, file);

    let mut header = [0u8; 100];
    r.read_exact(&mut header).map_err(|e| format!("read shp header: {e}"))?;
    if be_i32(&header[0..4]) != 9994 {
        return Err(format!("{}: not a shapefile (bad file code)", shp.display()));
    }
    let (qminx, qminy, qmaxx, qmaxy) = qbox;

    loop {
        // Record header: 8 bytes big-endian (record number, content length in
        // 16-bit words). A clean EOF here ends the file.
        let mut rh = [0u8; 8];
        if let Err(e) = r.read_exact(&mut rh) {
            if e.kind() == ErrorKind::UnexpectedEof {
                break;
            }
            return Err(format!("read record header: {e}"));
        }
        let content_len = (be_i32(&rh[4..8]) as i64) * 2; // bytes of record content

        // Shape type (little-endian) leads the content.
        let mut t = [0u8; 4];
        r.read_exact(&mut t).map_err(|e| format!("read shape type: {e}"))?;
        let mut consumed = 4i64;
        if le_i32(&t) != SHP_POLYGON {
            // Null (0) or other (the land dataset is all polygons) → skip body.
            r.seek_relative(content_len - consumed).map_err(|e| format!("skip record: {e}"))?;
            continue;
        }

        // Record MBR (4 doubles) — the skip filter, matching GDAL/`fiona`.
        let mut bbuf = [0u8; 32];
        r.read_exact(&mut bbuf).map_err(|e| format!("read record bbox: {e}"))?;
        consumed += 32;
        let (bxmin, bymin) = (le_f64(&bbuf[0..8]), le_f64(&bbuf[8..16]));
        let (bxmax, bymax) = (le_f64(&bbuf[16..24]), le_f64(&bbuf[24..32]));
        if bxmax < qminx || bxmin > qmaxx || bymax < qminy || bymin > qmaxy {
            r.seek_relative(content_len - consumed).map_err(|e| format!("skip record: {e}"))?;
            continue;
        }

        // In range: read the rest (parts + points) and decode the rings.
        let mut body = vec![0u8; (content_len - consumed) as usize];
        r.read_exact(&mut body).map_err(|e| format!("read record body: {e}"))?;
        let rings = parse_polygon_rings(&body)?;
        let fully_inside = bxmin >= qminx && bxmax <= qmaxx && bymin >= qminy && bymax <= qmaxy;
        process_record(rings, fully_inside, box_geom, out);
    }
    Ok(())
}

/// Decode a polygon record body (starting at `NumParts`) into its rings. Parts are
/// start indices into the point array; the last ring runs to `NumPoints`.
fn parse_polygon_rings(body: &[u8]) -> Result<Vec<Vec<(f64, f64)>>, String> {
    if body.len() < 8 {
        return Err("polygon record too short".into());
    }
    let num_parts = le_i32(&body[0..4]) as usize;
    let num_points = le_i32(&body[4..8]) as usize;
    let parts_off = 8;
    let points_off = parts_off + num_parts * 4;
    if body.len() < points_off + num_points * 16 {
        return Err("polygon record truncated".into());
    }
    let mut starts: Vec<usize> = (0..num_parts)
        .map(|i| le_i32(&body[parts_off + i * 4..parts_off + i * 4 + 4]) as usize)
        .collect();
    starts.push(num_points);

    let mut rings = Vec::with_capacity(num_parts);
    for p in 0..num_parts {
        let (s, e) = (starts[p], starts[p + 1]);
        let mut ring = Vec::with_capacity(e.saturating_sub(s));
        for i in s..e {
            let o = points_off + i * 16;
            ring.push((le_f64(&body[o..o + 8]), le_f64(&body[o + 8..o + 16])));
        }
        rings.push(ring);
    }
    Ok(rings)
}

/// Clip one shapefile record's rings (in 3857) to the box, reproject, and append
/// the resulting polygons. A record fully inside the box skips the GEOS clip — the
/// oracle's `intersection` of a contained polygon returns it unchanged.
fn process_record(
    rings: Vec<Vec<(f64, f64)>>,
    fully_inside: bool,
    box_geom: &Geometry,
    out: &mut Vec<Geom>,
) {
    // Fast path: a single-ring polygon already inside the box needs no GEOS at
    // all (the common case for inland extracts) — just reproject and emit.
    if fully_inside && rings.len() == 1 {
        let mut ext = rings.into_iter().next().unwrap();
        if ext.len() < 4 {
            return;
        }
        reproject_ring(&mut ext);
        out.push(Geom::Polygon { exterior: ext, interiors: Vec::new() });
        return;
    }

    let Some(geom_3857) = geos_polygon_from_rings(&rings) else { return };
    let result = if fully_inside {
        geom_3857
    } else {
        match geom_3857.intersection(box_geom) {
            Ok(g) => g,
            Err(_) => return,
        }
    };
    let mut g = geom_from_geos(&result);
    if g.is_empty() {
        return;
    }
    reproject_geom(&mut g);
    collect_polygons(g, out);
}

/// Build a GEOS geometry from a record's rings (still in 3857). A single ring is a
/// plain polygon; multiple rings (holes and/or disjoint outers) go through
/// `build_area`, which applies the even-odd nesting rule to attach holes — robust
/// to ring winding, matching how the oracle's OGR reader assembles the feature.
fn geos_polygon_from_rings(rings: &[Vec<(f64, f64)>]) -> Option<Geometry> {
    if rings.len() == 1 {
        if rings[0].len() < 4 {
            return None;
        }
        let ext = Geometry::create_linear_ring(ring_to_coordseq(&rings[0])).ok()?;
        return Geometry::create_polygon(ext, vec![]).ok();
    }
    let lines: Vec<Geometry> = rings
        .iter()
        .filter(|r| r.len() >= 2)
        .filter_map(|r| Geometry::create_line_string(ring_to_coordseq(r)).ok())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let mls = Geometry::create_multiline_string(lines).ok()?;
    mls.build_area().ok()
}

/// The clip box as a GEOS polygon, in shapely `box(minx,miny,maxx,maxy)` ring
/// order (same as [`crate::geom::clip_to_box`]).
fn box_polygon((minx, miny, maxx, maxy): (f64, f64, f64, f64)) -> Result<Geometry, String> {
    let ring = [(maxx, miny), (maxx, maxy), (minx, maxy), (minx, miny), (maxx, miny)];
    let lr = Geometry::create_linear_ring(ring_to_coordseq(&ring))
        .map_err(|e| format!("clip box ring: {e}"))?;
    Geometry::create_polygon(lr, vec![]).map_err(|e| format!("clip box polygon: {e}"))
}

// --- Dataset cache ---------------------------------------------------------

fn cache_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME not set")?;
    Ok(PathBuf::from(home).join(".cache/obcm/land"))
}

/// Return the cached `land_polygons.shp`, downloading + extracting the dataset on
/// first use (~950 MB). Mirrors `land_ingest.py`'s cache layout. The oracle's
/// `Last-Modified` freshness check is intentionally dropped — it needs an HTTP
/// HEAD and the oracle itself proceeds with the local copy on any network error;
/// delete the cache dir to force a refresh.
fn ensure_dataset() -> Result<PathBuf, String> {
    let dir = cache_dir()?;
    let shp = dir.join("land-polygons-split-3857/land_polygons.shp");
    if shp.exists() {
        return Ok(shp);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let zip = dir.join("land-polygons.zip");
    let zip_s = zip.to_string_lossy();
    let dir_s = dir.to_string_lossy();
    eprintln!("Downloading land polygons (~950 MB, one-time) from {LAND_URL} ...");
    run_tool("curl", &["-fL", "--retry", "3", "-o", &zip_s, LAND_URL])?;
    eprintln!("Extracting land polygons ...");
    run_tool("unzip", &["-o", "-q", &zip_s, "-d", &dir_s])?;
    if !shp.exists() {
        return Err(format!("land dataset missing after download: {}", shp.display()));
    }
    Ok(shp)
}

fn run_tool(cmd: &str, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(cmd).args(args).status().map_err(|e| {
        format!("failed to run `{cmd}` ({e}); install it or pre-populate the land cache")
    })?;
    if !status.success() {
        return Err(format!("`{cmd}` exited with {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forward then inverse round-trips to the input (well within µdeg).
    #[test]
    fn reproject_roundtrip() {
        for &(lon, lat) in &[(7.42, 43.73), (14.5, 35.9), (0.0, 0.0), (-122.3, 47.6)] {
            let (x, y) = merc_forward(lon, lat);
            let (lon2, lat2) = merc_inverse(x, y);
            assert!((lon - lon2).abs() < 1e-9, "lon {lon} -> {lon2}");
            assert!((lat - lat2).abs() < 1e-9, "lat {lat} -> {lat2}");
        }
    }

    /// Spot-check against known EPSG:3857 anchors (origin + the world half-width).
    #[test]
    fn reproject_known_points() {
        let (x, y) = merc_forward(0.0, 0.0);
        assert!(x.abs() < 1e-6 && y.abs() < 1e-6, "origin maps to (0,0)");
        // 180° E is exactly half the Mercator world width (πR).
        let (x180, _) = merc_forward(180.0, 0.0);
        assert!((x180 - std::f64::consts::PI * R).abs() < 1e-3);
    }

    /// `parse_polygon_rings` decodes a hand-built single-ring (square) record body.
    #[test]
    fn parse_one_square_ring() {
        let pts: [(f64, f64); 5] = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)];
        let mut body = Vec::new();
        body.extend_from_slice(&1i32.to_le_bytes()); // NumParts
        body.extend_from_slice(&(pts.len() as i32).to_le_bytes()); // NumPoints
        body.extend_from_slice(&0i32.to_le_bytes()); // part 0 starts at index 0
        for (x, y) in pts {
            body.extend_from_slice(&x.to_le_bytes());
            body.extend_from_slice(&y.to_le_bytes());
        }
        let rings = parse_polygon_rings(&body).expect("parse");
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].len(), 5);
        assert_eq!(rings[0][2], (10.0, 10.0));
    }
}
