//! Throwaway Stage-3 gating probe (handover §3.1). Dumps, for every node, its id,
//! its decimicrodegree (1e-7) integer lon/lat, and the **f64 bit patterns** of
//! lon/lat computed *osmium's* way: `decimicro as f64 / 1e7` — division by the
//! exact integer `1e7`, NOT `* 1e-7` (which would round twice and flip the last
//! bit). osmpbf's own `.lon()` uses `1e-9 * nano_lon()` and diverges, so we never
//! call it. Compared bit-for-bit against the Python osmium dump (`node_probe.py`)
//! to prove the coordinate read matches before any geometry is built.
//!
//! Usage: `node_probe <pbf>` → sorted `id x y lon_bits lat_bits` lines on stdout.

use std::fmt::Write as _;
use std::process::ExitCode;

use osmpbf::{Element, ElementReader};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: node_probe <pbf>");
        return ExitCode::FAILURE;
    };
    let reader = match ElementReader::from_path(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("open {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    // (id, decimicro_lon, decimicro_lat, lon_bits, lat_bits)
    let mut rows: Vec<(i64, i32, i32, u64, u64)> = Vec::new();
    let res = reader.for_each(|el| {
        let (id, dx, dy) = match el {
            Element::Node(n) => (n.id(), n.decimicro_lon(), n.decimicro_lat()),
            Element::DenseNode(n) => (n.id(), n.decimicro_lon(), n.decimicro_lat()),
            _ => return,
        };
        let lon = dx as f64 / 1e7;
        let lat = dy as f64 / 1e7;
        rows.push((id, dx, dy, lon.to_bits(), lat.to_bits()));
    });
    if let Err(e) = res {
        eprintln!("read {path}: {e}");
        return ExitCode::FAILURE;
    }

    rows.sort_by_key(|r| r.0);
    let mut out = String::with_capacity(rows.len() * 48);
    for (id, dx, dy, lb, ab) in rows {
        let _ = writeln!(out, "{id} {dx} {dy} {lb} {ab}");
    }
    print!("{out}");
    ExitCode::SUCCESS
}
