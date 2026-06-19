//! `land_probe` — dump `land::get_land_polygons(bbox)` as JSON for the Stage-5
//! land-parity gate (the land counterpart of `node_probe`). Emits each clipped +
//! reprojected land face's rings as raw lon/lat floats, so `compare_land.py` can
//! check the Rust land set against the Python oracle by area + count + vertices,
//! independent of the quadtree.
//!
//! Usage:  land_probe <min_lon> <min_lat> <max_lon> <max_lat> <out.json>

use std::fmt::Write as _;

use obc_pack::geom::Geom;
use obc_pack::land::get_land_polygons;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 5 {
        eprintln!("usage: land_probe <min_lon> <min_lat> <max_lon> <max_lat> <out.json>");
        std::process::exit(2);
    }
    let bbox = (parse(&a[0]), parse(&a[1]), parse(&a[2]), parse(&a[3]));
    let polys = get_land_polygons(bbox).unwrap_or_else(|e| {
        eprintln!("land_probe: {e}");
        std::process::exit(1);
    });

    let mut s = String::from("{\"polygons\":[");
    for (i, g) in polys.iter().enumerate() {
        if let Geom::Polygon { exterior, interiors } = g {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"ext\":");
            write_ring(&mut s, exterior);
            s.push_str(",\"holes\":[");
            for (j, h) in interiors.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                write_ring(&mut s, h);
            }
            s.push_str("]}");
        }
    }
    s.push_str("]}");
    std::fs::write(&a[4], s).expect("write json");
}

fn parse(s: &str) -> f64 {
    s.parse().unwrap_or_else(|_| {
        eprintln!("land_probe: bad number {s:?}");
        std::process::exit(2);
    })
}

fn write_ring(s: &mut String, ring: &[(f64, f64)]) {
    s.push('[');
    for (k, &(x, y)) in ring.iter().enumerate() {
        if k > 0 {
            s.push(',');
        }
        // Full f64 precision so the area comparison is exact up to the reproject.
        let _ = write!(s, "[{x:.10},{y:.10}]");
    }
    s.push(']');
}
