//! `obc-pack` CLI — drop-in replacement for `packer/pack.py` (same positional
//! args + `--chunk-size`). The full ingest→quadtree→serialize pipeline is being
//! ported in stages; until ingest lands this entry point reports status and
//! exits non-zero, so the web builder's `OBC_PACK_BACKEND` flag keeps defaulting
//! to Python. The serializer (Stage 1) is exercised by the `serialize_from_dump`
//! dev binary and the crate tests.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("obc-pack {} (serializer stage)", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "obc-pack: the end-to-end pipeline (ingest → quadtree → serialize) is not \
         wired up yet — only the serializer stage is implemented.\n\
         Use the Python oracle (packer/pack.py) for now; validate the serializer \
         with `cargo test -p obc-pack` and the `serialize_from_dump` binary."
    );
    ExitCode::FAILURE
}
