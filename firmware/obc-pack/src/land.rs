//! `land.rs` — land-fill polygons for an extract, by clipping the global
//! [land-polygons-split-3857] dataset to the map bbox. No GIS stack: a direct
//! shapefile read + closed-form reprojection + a GEOS clip.
//!
//! [land-polygons-split-3857]: https://osmdata.openstreetmap.de/data/land-polygons.html
//!
//!   - **Shapefile read** — parse the `.shp` directly, with a per-record
//!     bounding-box skip so only records touching the query box are decoded (the
//!     dataset has no spatial index).
//!   - **Reproject** — EPSG:3857 here is the Web Mercator Auxiliary Sphere
//!     (`SPHEROID` radius `6378137`, per the `.prj`): closed-form spherical
//!     mercator, no PROJ datum grids.
//!   - **Clip** — a GEOS `intersection` against the forward-projected bbox box,
//!     done in 3857 *before* reprojecting the result.
//!
//! Output is one [`Geom::Polygon`] per land face (flattened to simple polygons,
//! like the relation path, then styled `natural.land` in `main.rs`).

use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};

use geos::{Geom as _, Geometry};

use crate::geom::{box_polygon, collect_polygons, geom_from_geos, ring_to_coordseq, Geom};

/// EPSG:3857 auxiliary-sphere radius = WGS84 semi-major axis (see the `.prj`).
const R: f64 = 6_378_137.0;

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
/// for the clip-box corners.
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
/// in place.
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

/// Land polygons for `bbox_deg = (min_lon, min_lat, max_lon, max_lat)`, clipped and
/// reprojected to degrees. One [`Geom::Polygon`] per face.
pub fn get_land_polygons(bbox_deg: (f64, f64, f64, f64)) -> Result<Vec<Geom>, String> {
    let shp = ensure_dataset()?;
    let (min_lon, min_lat, max_lon, max_lat) = bbox_deg;
    // Project the bbox corners to 3857 for the filter + clip box. Mercator is
    // monotone in both axes, so corners stay corners (min→min, max→max).
    let (qminx, qminy) = merc_forward(min_lon, min_lat);
    let (qmaxx, qmaxy) = merc_forward(max_lon, max_lat);
    let qbox = (qminx, qminy, qmaxx, qmaxy);
    let box_geom = box_polygon(qbox).map_err(|e| format!("clip box: {e}"))?;

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
    // Large buffer so the scan is ~one sequential pass over the (~1.3 GB) file.
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

        // Record MBR (4 doubles) — the bounding-box skip filter.
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
    let mut starts: Vec<usize> =
        (0..num_parts).map(|i| le_i32(&body[parts_off + i * 4..parts_off + i * 4 + 4]) as usize).collect();
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

/// Clip one record's rings (in 3857) to the box, reproject, and append the polygons.
/// A record fully inside the box skips the GEOS clip (intersection would return it
/// unchanged).
fn process_record(rings: Vec<Vec<(f64, f64)>>, fully_inside: bool, box_geom: &Geometry, out: &mut Vec<Geom>) {
    // A single-ring polygon already inside the box needs no GEOS — reproject + emit.
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
/// to ring winding.
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

// --- Dataset cache ---------------------------------------------------------

fn cache_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME not set")?;
    Ok(PathBuf::from(home).join(".cache/obcm/land"))
}

/// Return the cached `land_polygons.shp`, downloading + extracting the dataset on
/// first use (~950 MB). There's no `Last-Modified` freshness check — delete the
/// cache dir to force a refresh.
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
    let status = std::process::Command::new(cmd)
        .args(args)
        .status()
        .map_err(|e| format!("failed to run `{cmd}` ({e}); install it or pre-populate the land cache"))?;
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

    // --- geos_polygon_from_rings + process_record ---------------------------

    /// A square in 3857 metres, CCW, closed. `cx,cy` = lower-left corner, `s` = side.
    fn box3857(cx: f64, cy: f64, s: f64) -> Vec<(f64, f64)> {
        vec![(cx, cy), (cx + s, cy), (cx + s, cy + s), (cx, cy + s), (cx, cy)]
    }

    /// Single ring ⇒ a plain GEOS polygon, no holes.
    #[test]
    fn geos_polygon_from_one_ring_is_hole_free() {
        let g = geos_polygon_from_rings(&[box3857(1_000_000.0, 2_000_000.0, 1000.0)]).expect("polygon");
        assert_eq!(g.get_num_interior_rings().unwrap(), 0, "a lone outer ring has no holes");
    }

    /// Outer + concentric inner ring ⇒ `build_area`'s even-odd rule attaches the
    /// inner as a hole (the real dataset's lakes-with-islands).
    #[test]
    fn geos_polygon_from_outer_and_inner_attaches_hole() {
        let outer = box3857(0.0, 0.0, 10_000.0);
        let inner = box3857(3_000.0, 3_000.0, 4_000.0); // fully inside the outer
        let g = geos_polygon_from_rings(&[outer, inner]).expect("polygon");
        assert_eq!(g.get_num_interior_rings().unwrap(), 1, "even-odd nesting makes the inner ring a hole");
    }

    /// `process_record` fully-inside single-ring fast path: no GEOS clip, just
    /// reproject + emit, matching `merc_inverse` exactly. `box_geom` is unused here.
    #[test]
    fn process_record_fully_inside_single_ring_skips_clip() {
        let s = 1000.0;
        let (cx, cy) = (1_000_000.0, 2_000_000.0);
        let ring3857 = box3857(cx, cy, s);
        let dummy = box_polygon((0.0, 0.0, 1.0, 1.0)).unwrap();

        let mut out = Vec::new();
        process_record(vec![ring3857.clone()], /* fully_inside */ true, &dummy, &mut out);
        assert_eq!(out.len(), 1, "one polygon emitted");
        match &out[0] {
            Geom::Polygon { exterior, interiors } => {
                assert!(interiors.is_empty(), "single-ring fast path has no holes");
                for (got, src) in exterior.iter().zip(ring3857.iter()) {
                    let (elon, elat) = merc_inverse(src.0, src.1);
                    assert!((got.0 - elon).abs() < 1e-12 && (got.1 - elat).abs() < 1e-12, "vertex reprojected exactly");
                }
            }
            other => panic!("expected a Polygon, got {other:?}"),
        }
    }

    /// `process_record` GEOS-clip path: a record straddling the query box is clipped,
    /// reprojected, and stays within the box (in degrees).
    #[test]
    fn process_record_straddling_is_clipped_to_box() {
        let qbox = (0.0, 0.0, 10_000.0, 10_000.0);
        let box_geom = box_polygon(qbox).unwrap();
        let record = box3857(-5_000.0, 2_000.0, 20_000.0); // overhangs left + right

        let mut out = Vec::new();
        process_record(vec![record], /* fully_inside */ false, &box_geom, &mut out);
        assert_eq!(out.len(), 1, "the clipped record yields one polygon");

        let (lon_min, lat_min) = merc_inverse(qbox.0, qbox.1);
        let (lon_max, lat_max) = merc_inverse(qbox.2, qbox.3);
        if let Geom::Polygon { exterior, .. } = &out[0] {
            for &(lon, lat) in exterior {
                assert!(lon >= lon_min - 1e-9 && lon <= lon_max + 1e-9, "clipped lon {lon} within box");
                assert!(lat >= lat_min - 1e-9 && lat <= lat_max + 1e-9, "clipped lat {lat} within box");
            }
        } else {
            panic!("expected a Polygon");
        }
    }
}
