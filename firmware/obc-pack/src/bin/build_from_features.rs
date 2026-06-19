//! Harness tool (Stage 2): read a per-LOD feature dump (JSON from
//! `packer/tests/harness/dump_features.py`), build the quadtree in Rust, and
//! write the resulting `.obcm`. Compared against the Python reference with
//! `obcm_diff` to validate the quadtree + clip port.
//!
//! Usage: `build_from_features <features.json> <out.obcm>`

use std::process::ExitCode;

use obc_pack::feature_dump::FeatureDump;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (in_path, out_path) = match (args.next(), args.next()) {
        (Some(i), Some(o)) => (i, o),
        _ => {
            eprintln!("usage: build_from_features <features.json> <out.obcm>");
            return ExitCode::FAILURE;
        }
    };

    let json = match std::fs::read_to_string(&in_path) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("read {in_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let dump: FeatureDump = match serde_json::from_str(&json) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {in_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let bytes = dump.to_obcm();
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        eprintln!("write {out_path}: {e}");
        return ExitCode::FAILURE;
    }
    eprintln!("wrote {} ({} bytes)", out_path, bytes.len());
    ExitCode::SUCCESS
}
