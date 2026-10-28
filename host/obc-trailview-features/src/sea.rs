//! The sea, which OSM maps only as coastlines: the water polygons of osmdata.openstreetmap.de, the
//! shapefile of `water-polygons-split-3857.zip` in Web Mercator metres.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::geometry::contains;
use crate::tiles::{Tiles, MIN_ZOOM};
use crate::{Part, EQUATOR_M};

pub(crate) const SHAPEFILE: &str = "water-polygons-split-3857/water_polygons.shp";
const POLYGON: i32 = 5;

/// The polygons of the archive at `zip` that the tiles of the region at MIN_ZOOM meet, in world
/// coordinates, each outer ring before its holes, and each vertex at sea level, 0 m.
pub fn polygons(zip: &Path, tiles: &Tiles) -> Result<Vec<Vec<Part>>, String> {
    let error = |e: &dyn std::fmt::Display| format!("{}: {e}", zip.display());
    let file = File::open(zip).map_err(|e| error(&e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| error(&e))?;
    let shapefile = archive.by_name(SHAPEFILE).map_err(|e| error(&e))?;
    read(BufReader::with_capacity(1 << 20, shapefile), tiles).map_err(|e| error(&e))
}

/// The polygons of a shapefile. Its file has no spatial index and the archive deflates it, so the
/// reader passes over each record once and decodes those whose box the tiles meet.
fn read(mut shapefile: impl Read, tiles: &Tiles) -> Result<Vec<Vec<Part>>, String> {
    let be = |bytes: &[u8], at: usize| i32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let le = |bytes: &[u8], at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let double = |bytes: &[u8], at: usize| f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let mut head = [0u8; 100];
    shapefile.read_exact(&mut head).map_err(|e| e.to_string())?;
    if be(&head, 0) != 9994 {
        return Err("not a shapefile".into());
    }
    // Lengths count 16-bit words.
    let (mut at, end) = (head.len(), 2 * be(&head, 24) as usize);
    let (mut polygons, mut record) = (Vec::new(), Vec::new());
    while at < end {
        let mut header = [0u8; 8];
        shapefile.read_exact(&mut header).map_err(|e| e.to_string())?;
        record.resize(2 * be(&header, 4) as usize, 0);
        shapefile.read_exact(&mut record).map_err(|e| e.to_string())?;
        at += header.len() + record.len();
        if record.len() < 44 || le(&record, 0) != POLYGON {
            continue;
        }
        let corner = |x: usize, y: usize| world(double(&record, x), double(&record, y));
        let frame = Part { outer: true, points: vec![corner(4, 28), corner(20, 12)], z: None };
        if !tiles.meets(MIN_ZOOM, &[frame]) {
            continue;
        }
        let (count, points) = (le(&record, 36) as usize, le(&record, 40) as usize);
        let first = 44 + 4 * count;
        if record.len() < first + 16 * points {
            return Err("a polygon record is shorter than its points".into());
        }
        let starts: Vec<usize> = (0..count).map(|k| le(&record, 44 + 4 * k) as usize).chain([points]).collect();
        let rings = starts.windows(2).map(|w| {
            let ring: Vec<[f64; 2]> = (w[0]..w[1])
                .map(|i| world(double(&record, first + 16 * i), double(&record, first + 16 * i + 8)))
                .collect();
            // A shapefile ring repeats its first point at its end.
            ring[..ring.len().saturating_sub(1)].to_vec()
        });
        polygons.push(grouped(rings.filter(|ring| ring.len() >= 3).collect()));
    }
    Ok(polygons)
}

/// World coordinates of Web Mercator metres.
fn world(x: f64, y: f64) -> [f64; 2] {
    [0.5 + x / EQUATOR_M, 0.5 - y / EQUATOR_M]
}

/// The rings of a record as parts, each outer ring before the holes that lie in it. A shapefile turns
/// an outer ring clockwise and a hole counterclockwise, with y to the north: with y down, an outer
/// ring has a positive area.
fn grouped(rings: Vec<Vec<[f64; 2]>>) -> Vec<Part> {
    let twice_area = |r: &[[f64; 2]]| {
        (0..r.len()).map(|i| r[i][0] * r[(i + 1) % r.len()][1] - r[(i + 1) % r.len()][0] * r[i][1]).sum::<f64>()
    };
    let (outers, holes): (Vec<_>, Vec<_>) = rings.into_iter().partition(|ring| twice_area(ring) > 0.0);
    let mut parts = Vec::new();
    for (k, outer) in outers.iter().enumerate() {
        let inside = |hole: &&Vec<[f64; 2]>| outers.iter().position(|o| contains(&[o], hole[0])) == Some(k);
        for (ring, is_outer) in [(outer, true)].into_iter().chain(holes.iter().filter(inside).map(|h| (h, false))) {
            parts.push(Part { outer: is_outer, points: ring.clone(), z: Some(vec![0.0; ring.len()]) });
        }
    }
    parts
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A shapefile of polygon records, each a list of rings in metres, each ring closed.
    pub(crate) fn shapefile(records: &[Vec<Vec<[f64; 2]>>]) -> Vec<u8> {
        let mut body = Vec::new();
        for (n, rings) in records.iter().enumerate() {
            let points: Vec<[f64; 2]> = rings.iter().flatten().copied().collect();
            let mut content = Vec::new();
            content.extend(POLYGON.to_le_bytes());
            let xs = points.iter().map(|p| p[0]);
            let ys = points.iter().map(|p| p[1]);
            for v in [xs.clone().fold(f64::MAX, f64::min), ys.clone().fold(f64::MAX, f64::min)] {
                content.extend(v.to_le_bytes());
            }
            for v in [xs.fold(f64::MIN, f64::max), ys.fold(f64::MIN, f64::max)] {
                content.extend(v.to_le_bytes());
            }
            content.extend((rings.len() as i32).to_le_bytes());
            content.extend((points.len() as i32).to_le_bytes());
            let mut start = 0;
            for ring in rings {
                content.extend((start as i32).to_le_bytes());
                start += ring.len();
            }
            for [x, y] in points {
                content.extend(x.to_le_bytes());
                content.extend(y.to_le_bytes());
            }
            body.extend((n as i32 + 1).to_be_bytes());
            body.extend((content.len() as i32 / 2).to_be_bytes());
            body.extend(content);
        }
        let mut head = vec![0u8; 100];
        head[0..4].copy_from_slice(&9994i32.to_be_bytes());
        head[24..28].copy_from_slice(&((100 + body.len()) as i32 / 2).to_be_bytes());
        head[28..32].copy_from_slice(&1000i32.to_le_bytes());
        head[32..36].copy_from_slice(&POLYGON.to_le_bytes());
        head.extend(body);
        head
    }

    /// A square of `side` metres at (x, y), clockwise with y to the north when `clockwise`.
    pub(crate) fn square(x: f64, y: f64, side: f64, clockwise: bool) -> Vec<[f64; 2]> {
        let mut ring = vec![[x, y], [x, y + side], [x + side, y + side], [x + side, y], [x, y]];
        if !clockwise {
            ring.reverse();
        }
        ring
    }

    #[test]
    fn the_sea_near_the_region_is_its_outer_rings_with_their_islands_at_sea_level() {
        // A region of 0.1° off the equator; a sea square over it with an island and a sea square far away.
        let tiles = Tiles::new([0.0, 0.0, 0.1, 0.1]);
        let near = vec![square(-5000.0, -5000.0, 30000.0, true), square(1000.0, 1000.0, 500.0, false)];
        let far = vec![square(5e6, 5e6, 1000.0, true)];
        let polygons = read(&shapefile(&[far, near])[..], &tiles).unwrap();
        assert_eq!(polygons.len(), 1);
        let parts = &polygons[0];
        assert_eq!(parts.iter().map(|p| (p.outer, p.points.len())).collect::<Vec<_>>(), [(true, 4), (false, 4)]);
        assert!(parts.iter().all(|p| p.z.as_ref().unwrap().iter().all(|&z| z == 0.0)));
        // Metres east and north are world units east and south of the middle of the world.
        let [x, y] = parts[1].points[0];
        assert!((x - (0.5 + 1000.0 / EQUATOR_M)).abs() < 1e-15 && (y - (0.5 - 1000.0 / EQUATOR_M)).abs() < 1e-15);
    }
}
