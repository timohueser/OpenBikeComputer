//! The offline grid of the planner: the Web Mercator tiles of one zoom that the coverage overlaps,
//! each clipped to the coverage. A rider downloads the routing blocks and the routes of a cell.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use serde_json::{json, Value};

/// The zoom of the cells; the route catalog names the cells of its routes at this zoom.
pub const ZOOM: u32 = 9;

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

/// The cells `<zoom>-<x>-<y>` that `bounds` overlaps, each clipped to `bounds`, by `x`, then `y`.
pub fn cells(bounds: [f64; 4]) -> Vec<(String, [f64; 4])> {
    let last = (1i64 << ZOOM) - 1;
    let tile = |lon, lat| {
        let (x, y) = mercator(lon, lat, ZOOM);
        ((x as i64).clamp(0, last) as u32, (y as i64).clamp(0, last) as u32)
    };
    let [west, south, east, north] = bounds;
    let ((left, top), (right, bottom)) = (tile(west, north), tile(east, south));
    let mut cells = Vec::new();
    for x in left..=right {
        for y in top..=bottom {
            let b = tile_bounds(ZOOM, x, y);
            let clipped = [b[0].max(west), b[1].max(south), b[2].min(east), b[3].min(north)];
            if clipped[0] < clipped[2] && clipped[1] < clipped[3] {
                cells.push((format!("{ZOOM}-{x}-{y}"), clipped));
            }
        }
    }
    cells
}

/// The route catalog of each cell of `cells`: the records of `catalog` that name the cell, by
/// `id`. A record can name a cell outside the grid, which has no file.
pub fn route_tiles(catalog: &Value, cells: &[&str]) -> Result<BTreeMap<String, Value>, String> {
    if catalog["format"] != 1 {
        return Err("Unsupported route catalog".into());
    }
    let mut routes: Vec<&Value> =
        catalog["routes"].as_array().ok_or("The route catalog has no routes")?.iter().collect();
    routes.sort_by_key(|record| record["id"].as_i64());
    let mut tiles: BTreeMap<String, Vec<&Value>> = cells.iter().map(|cell| (cell.to_string(), Vec::new())).collect();
    for record in routes {
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
            {"id": 1, "cells": ["9-267-177", "9-267-178"]},
        ]});
        let tiles = route_tiles(&catalog, &ids).unwrap();
        let routes =
            |cell: &str| tiles[cell]["routes"].as_array().unwrap().iter().map(|r| r["id"].clone()).collect::<Vec<_>>();
        assert_eq!((routes("9-267-177"), routes("9-267-178")), (vec![json!(1)], vec![json!(1), json!(2)]));
    }
}
