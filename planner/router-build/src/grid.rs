//! The offline grid of the planner: the Web Mercator tiles of one zoom that the coverage overlaps,
//! each clipped to the coverage. A rider downloads the routing blocks and the routes of a cell.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use planner_router::shape::{self, P};
use serde_json::{json, Value};

use crate::catalog::{decode, encode};

/// The zoom of the cells; the route catalog names the cells of its routes at this zoom.
pub const ZOOM: u32 = 9;
/// The zoom of the far files of the long trails: at most 12 of their cells cover a view of about
/// 640 by 427 km in the Alps.
pub const TRAIL_ZOOM: u32 = 7;

/// The fractional tile coordinates of a point at `zoom`.
pub fn mercator(lon: f64, lat: f64, zoom: u32) -> (f64, f64) {
    let count = f64::from(1u32 << zoom);
    ((lon + 180.0) / 360.0 * count, (1.0 - lat.to_radians().tan().asinh() / PI) / 2.0 * count)
}

/// West, south, east and north of a tile, in degrees.
pub fn tile_bounds(zoom: u32, x: u32, y: u32) -> [f64; 4] {
    let n = f64::from(1u32 << zoom);
    let latitude = |row: f64| (PI * (1.0 - 2.0 * row / n)).sinh().atan().to_degrees();
    let (x, y) = (f64::from(x), f64::from(y));
    [x / n * 360.0 - 180.0, latitude(y + 1.0), (x + 1.0) / n * 360.0 - 180.0, latitude(y)]
}

/// The grid cells that `bounds` overlaps, each clipped to `bounds`, by `x`, then `y`.
pub fn cells(bounds: [f64; 4]) -> Vec<(String, [f64; 4])> {
    cells_at(bounds, ZOOM)
}

/// The cells `<zoom>-<x>-<y>` that `bounds` overlaps with a positive area, each clipped to
/// `bounds`, by `x`, then `y`.
pub fn cells_at(bounds: [f64; 4], zoom: u32) -> Vec<(String, [f64; 4])> {
    let last = (1i64 << zoom) - 1;
    let tile = |lon, lat| {
        let (x, y) = mercator(lon, lat, zoom);
        ((x as i64).clamp(0, last) as u32, (y as i64).clamp(0, last) as u32)
    };
    let [west, south, east, north] = bounds;
    let ((left, top), (right, bottom)) = (tile(west, north), tile(east, south));
    let mut cells = Vec::new();
    for x in left..=right {
        for y in top..=bottom {
            let b = tile_bounds(zoom, x, y);
            let clipped = [b[0].max(west), b[1].max(south), b[2].min(east), b[3].min(north)];
            if clipped[0] < clipped[2] && clipped[1] < clipped[3] {
                cells.push((format!("{zoom}-{x}-{y}"), clipped));
            }
        }
    }
    cells
}

/// The tiles at `zoom` whose box intersects the bounding box of the segment from `a` to `b`,
/// edges included.
pub fn touched(a: P, b: P, zoom: u32) -> impl Iterator<Item = (u32, u32)> {
    let last = (1i64 << zoom) - 1;
    let tile = |p: P| mercator(p[0] as f64 * 1e-6, p[1] as f64 * 1e-6, zoom);
    let range = move |a: f64, b: f64| (a.min(b).ceil() as i64 - 1).max(0)..=(a.max(b).floor() as i64).min(last);
    let ((ax, ay), (bx, by)) = (tile(a), tile(b));
    range(ax, bx).flat_map(move |x| range(ay, by).map(move |y| (x as u32, y as u32)))
}

/// The pieces of a line in each tile at `zoom`: the runs of its segments that touch the tile, so
/// the segment that crosses a tile edge ends the piece on one side and starts the one on the other.
fn pieces(line: &[P], zoom: u32) -> BTreeMap<String, Vec<Vec<P>>> {
    let mut pieces = BTreeMap::<String, Vec<(usize, usize)>>::new();
    for (k, w) in line.windows(2).enumerate() {
        for (x, y) in touched(w[0], w[1], zoom) {
            let runs = pieces.entry(format!("{zoom}-{x}-{y}")).or_default();
            match runs.last_mut() {
                Some(run) if run.1 == k => run.1 = k + 1,
                _ => runs.push((k, k + 1)),
            }
        }
    }
    pieces.into_iter().map(|(tile, runs)| (tile, runs.iter().map(|&(a, b)| line[a..=b].to_vec()).collect())).collect()
}

/// The far file of each cell of `cells`, tiles at `TRAIL_ZOOM`: each trail whose far line touches
/// the cell, with the pieces of its line there, at the finest tolerance from `FAR_TOLERANCE_M` up,
/// doubled each time, with which the file has at most `max_bytes`. A piece keeps its first and its
/// last segment, which it shares with the pieces of the neighbouring cells. A cell whose file is
/// larger with each piece reduced to these segments is an error.
pub fn trail_tiles(trails: &[Value], cells: &[&str], max_bytes: usize) -> Result<BTreeMap<String, Value>, String> {
    let mut tiles: BTreeMap<&str, Vec<(&Value, Vec<P>)>> = cells.iter().map(|&cell| (cell, Vec::new())).collect();
    let mut trails: Vec<&Value> = trails.iter().collect();
    trails.sort_by_key(|entry| entry["id"].as_i64());
    for entry in trails {
        for line in entry["far_udeg"].as_array().into_iter().flatten().map(decode) {
            for (cell, pieces) in pieces(&line, TRAIL_ZOOM) {
                if let Some(tile) = tiles.get_mut(cell.as_str()) {
                    tile.extend(pieces.into_iter().map(|piece| (entry, piece)));
                }
            }
        }
    }
    tiles.into_iter().map(|(cell, pieces)| Ok((cell.to_string(), far_file(cell, &pieces, max_bytes)?))).collect()
}

fn far_file(cell: &str, pieces: &[(&Value, Vec<P>)], max_bytes: usize) -> Result<Value, String> {
    let mut tolerance = crate::catalog::trails::FAR_TOLERANCE_M;
    loop {
        let mut reduced = true;
        let mut entries: Vec<Value> = Vec::new();
        for (entry, piece) in pieces {
            let n = piece.len();
            let mut keep = vec![0, 1, n - 2, n - 1];
            keep.sort_unstable();
            keep.dedup();
            let simple = shape::simplify(piece, &keep, tolerance).0;
            reduced &= simple.len() == keep.len();
            if entries.last().is_none_or(|last| last["id"] != entry["id"]) {
                let mut far = (*entry).clone();
                far["far_udeg"] = json!([]);
                entries.push(far);
            }
            let last = entries.last_mut().expect("an entry");
            last["far_udeg"].as_array_mut().expect("a list").push(json!(encode(&simple)));
        }
        let document = json!({"format": 1, "tolerance_m": tolerance as u64, "trails": entries});
        if file_bytes(&document).len() <= max_bytes {
            return Ok(document);
        }
        if reduced {
            return Err(format!("The far file {cell} has more than {max_bytes} bytes at every tolerance"));
        }
        tolerance *= 2.0;
    }
}

/// The bytes of a published file: its JSON with sorted keys, and a line end.
pub fn file_bytes(document: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&crate::sort_keys(document.clone())).expect("JSON values serialize");
    bytes.push(b'\n');
    bytes
}

/// The route catalog of each cell of `cells`: the records of `catalog` that name the cell, by
/// `id`. A record can name a cell outside the grid, which has no file. A long record repeats in
/// each cell of its stages, so it goes there without its list of parts and its profile.
pub fn route_tiles(catalog: &Value, cells: &[&str]) -> Result<BTreeMap<String, Value>, String> {
    if catalog["format"] != 1 {
        return Err("Unsupported route catalog".into());
    }
    let mut routes: Vec<Value> = catalog["routes"].as_array().ok_or("The route catalog has no routes")?.clone();
    routes.sort_by_key(|record| record["id"].as_i64());
    for record in routes.iter_mut().filter_map(Value::as_object_mut) {
        record.remove("parts");
        record.remove("trail_profile_m");
    }
    let mut tiles: BTreeMap<String, Vec<&Value>> = cells.iter().map(|cell| (cell.to_string(), Vec::new())).collect();
    for record in &routes {
        for cell in record["cells"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if let Some(tile) = tiles.get_mut(cell) {
                tile.push(record);
            }
        }
    }
    Ok(tiles.into_iter().map(|(cell, routes)| (cell, json!({"format": 1, "routes": routes}))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{decode, encode};

    #[test]
    fn the_cells_of_freiburg_are_the_zoom_9_tiles_it_overlaps_clipped_to_it() {
        let bounds = [7.77, 47.97, 7.93, 48.14];
        let cells = cells(bounds);
        let ids: Vec<&str> = cells.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["9-267-177", "9-267-178"]);
        let edge = tile_bounds(ZOOM, 267, 177)[1];
        assert_eq!(edge, tile_bounds(ZOOM, 267, 178)[3]);
        assert_eq!((cells[0].1, cells[1].1), ([7.77, edge, 7.93, 48.14], [7.77, 47.97, 7.93, edge]));

        let catalog = json!({"format": 1, "routes": [
            {"id": 2, "cells": ["9-267-178", "9-300-1"]},
            {"id": 1, "cells": ["9-267-177", "9-267-178"], "parts": [{"id": 2}], "stage_count": 1, "trail_profile_m": [1]},
        ]});
        let tiles = route_tiles(&catalog, &ids).unwrap();
        let routes =
            |cell: &str| tiles[cell]["routes"].as_array().unwrap().iter().map(|r| r["id"].clone()).collect::<Vec<_>>();
        assert_eq!((routes("9-267-177"), routes("9-267-178")), (vec![json!(1)], vec![json!(1), json!(2)]));
        assert_eq!(
            tiles["9-267-177"]["routes"][0],
            json!({"id": 1, "cells": ["9-267-177", "9-267-178"], "stage_count": 1})
        );
    }

    #[test]
    fn a_far_file_takes_the_finest_tolerance_that_fits_and_its_pieces_share_their_border_segments() {
        let b = tile_bounds(TRAIL_ZOOM, 66, 45);
        let lat = (b[1] + b[3]) / 2.0;
        // Across the edge of 66 and 67, a vertex each 760 m, 75 m north and south in turn.
        let line: Vec<P> = (0..40)
            .map(|k| {
                let north = if k % 2 == 0 { 75.0 } else { -75.0 } / 111_320.0;
                [((b[2] - 0.195 + 0.01 * k as f64) * 1e6).round() as i32, ((lat + north) * 1e6).round() as i32]
            })
            .collect();
        let trails = [json!({"id": 1, "kind": "hiking", "far_udeg": [encode(&line)]})];
        let cells = ["7-66-45", "7-66-46", "7-67-45"];
        let tiles = trail_tiles(&trails, &cells, usize::MAX).unwrap();
        let piece = |tiles: &BTreeMap<String, Value>, cell: &str| decode(&tiles[cell]["trails"][0]["far_udeg"][0]);
        let (west, east) = (piece(&tiles, "7-66-45"), piece(&tiles, "7-67-45"));
        assert_eq!((&west[west.len() - 2..], &east[..2]), (&line[19..21], &line[19..21]));
        assert!(west.len() > 4 && tiles["7-66-45"]["tolerance_m"] == 100);
        assert_eq!(tiles["7-66-46"]["trails"], json!([]));

        let fine = file_bytes(&tiles["7-66-45"]).len();
        let coarse = trail_tiles(&trails, &cells, fine - 1).unwrap();
        assert_eq!((piece(&coarse, "7-66-45").len(), &coarse["7-66-45"]["tolerance_m"]), (4, &json!(200)));
        let least = file_bytes(&coarse["7-66-45"]).len();
        let error = trail_tiles(&trails, &cells, least - 1).unwrap_err();
        assert!(error.contains("7-66-45"), "{error}");
    }
}
