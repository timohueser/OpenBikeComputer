//! Stage-3 ingest dump (handover §6.1): run the real Rust [`ingest_osm`] and emit
//! its features + coastlines as **microdegree-rounded** vertices (JSON), for the
//! ingest-only multiset comparison against the Python oracle's Stage-3-expected
//! set (`packer/tests/harness/dump_ingest.py` + `compare_ingest.py`). Rounding is
//! `round(v*1e6)` ties-even — the same the serializer applies — so the comparison
//! is at the granularity that actually reaches the file. Isolates the ingest port
//! from quadtree/serialize.
//!
//! Usage: `ingest_dump <pbf> <config.json> <out.json>`

use std::process::ExitCode;

use obc_pack::config::Config;
use obc_pack::geom::Geom;
use obc_pack::ingest::ingest_osm;
use serde::Serialize;

#[derive(Serialize)]
struct OutFeature {
    style_id: u8,
    kind: &'static str,
    /// `rings[0]` exterior, `rings[1..]` holes; each vertex `[microdeg_lon, microdeg_lat]`.
    rings: Vec<Vec<[i64; 2]>>,
}

#[derive(Serialize)]
struct Out {
    features: Vec<OutFeature>,
    coastlines: Vec<Vec<[i64; 2]>>,
}

/// `int(round(v*1e6))` with banker's rounding — matches `serialize.rs::to_udeg`
/// and the Python `int(round(...))` in the oracle dump.
#[inline]
fn ud(v: f64) -> i64 {
    (v * 1e6).round_ties_even() as i64
}

fn ring(coords: &[(f64, f64)]) -> Vec<[i64; 2]> {
    coords.iter().map(|&(x, y)| [ud(x), ud(y)]).collect()
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (pbf, cfg_path, out_path) = match (args.next(), args.next(), args.next()) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => {
            eprintln!("usage: ingest_dump <pbf> <config.json> <out.json>");
            return ExitCode::FAILURE;
        }
    };

    let config = match Config::load(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config: {e}");
            return ExitCode::FAILURE;
        }
    };
    let ing = match ingest_osm(&pbf, &config) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("ingest: {e}");
            return ExitCode::FAILURE;
        }
    };

    let features = ing
        .features
        .iter()
        .map(|f| match &f.geom {
            Geom::Polygon { exterior, interiors } => {
                let mut rings = vec![ring(exterior)];
                rings.extend(interiors.iter().map(|r| ring(r)));
                OutFeature { style_id: f.style_id, kind: "polygon", rings }
            }
            Geom::Line(c) => OutFeature { style_id: f.style_id, kind: "line", rings: vec![ring(c)] },
            _ => OutFeature { style_id: f.style_id, kind: "line", rings: Vec::new() },
        })
        .collect();
    let coastlines = ing.coastlines.iter().map(|c| ring(c)).collect();

    let out = Out { features, coastlines };
    let json = serde_json::to_string(&out).expect("serialize");
    match std::fs::write(&out_path, json) {
        Ok(()) => {
            eprintln!(
                "wrote {out_path}: {} features, {} coastlines",
                ing.features.len(),
                ing.coastlines.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("write {out_path}: {e}");
            ExitCode::FAILURE
        }
    }
}
