//! Harness tool: read a quadtree dump (JSON, emitted by
//! `packer/tests/harness/dump_tree.py`) and write the `.obcm` bytes the Rust
//! serializer produces. Used to byte-compare against `pack.py`'s output on the
//! same captured trees — the Stage-1 byte-parity gate.
//!
//! Usage: `serialize_from_dump <dump.json> <out.obcm>`

use std::process::ExitCode;

use obc_pack::dump::Dump;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (dump_path, out_path) = match (args.next(), args.next()) {
        (Some(d), Some(o)) => (d, o),
        _ => {
            eprintln!("usage: serialize_from_dump <dump.json> <out.obcm>");
            return ExitCode::FAILURE;
        }
    };

    let json = match std::fs::read_to_string(&dump_path) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("read {dump_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let dump: Dump = match serde_json::from_str(&json) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("parse {dump_path}: {e}");
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
