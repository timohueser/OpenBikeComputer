//! `obc-pack` CLI — flags in, [`obc_pack::pipeline::pack`] out.
//!
//! The pipeline itself (`.osm.pbf` → `.obcm`: ingest → bbox → land → per-LOD
//! simplify + quadtree → serialize) lives in the library, because the desktop app
//! (#906) links the packer rather than spawning it and the two must not be able to
//! diverge. This file owns the command line and nothing else.
//!
//! Positional CLI: `<pbf...> <config.json> <out.obcm>`, plus `--bbox W,S,E,N`
//! (crop the sources to a box during ingest — see [`obc_pack::ingest`]),
//! `--chunk-size`, `--no-land`, `--dump-pois` (print the classified POI list for
//! eyeballing), and `--dump-hours` (print each POI's parsed weekly schedule). It
//! prints one stage string per phase ("Merging", "Pass 0/1/2", "Calculating BBox",
//! "Generating land", "Building Quadtree", "Serializing", "Writing") so the web
//! builder UI can show progress — it matches these prefixes, and their order here
//! is the order it expects. `obc-pack schema` prints the config's JSON Schema
//! envelope — the web builder serves it so the editor's capability always matches
//! the binary that packs (`schema --catalog` prints the catalog manifest's schema
//! instead). `obc-pack catalog <bake-tree>` walks a bakery's output tree and writes
//! the map-catalog manifest (`OBCC_Spec.md`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use obc_pack::config::Config;
use obc_pack::ingest::Bbox;
use obc_pack::pipeline::{pack, PackOptions};
use obc_pack::progress::{PackError, Progress};

struct Args {
    pbfs: Vec<String>,
    config: String,
    output: String,
    opts: PackOptions,
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut opts = PackOptions::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            // Validated here, before any file is opened: a malformed or inside-out
            // box must fail with a sentence, not with an empty map an hour later.
            "--bbox" => opts.bbox = Some(Bbox::parse(&it.next().ok_or("--bbox needs W,S,E,N in degrees")?)?),
            "--chunk-size" => {
                opts.chunk_size = Some(it.next().and_then(|s| s.parse().ok()).ok_or("--chunk-size needs a number")?);
            }
            "--no-land" => opts.no_land = true,
            "--dump-pois" => opts.dump_pois = true,
            "--dump-hours" => opts.dump_hours = true,
            _ => positional.push(a),
        }
    }
    // `<pbf...> <config.json> <out.obcm>`: last two positionals are config + output.
    if positional.len() < 3 {
        return Err("usage: obc-pack <pbf...> <config.json> <out.obcm> [--bbox W,S,E,N] [--chunk-size N] [--no-land] \
                    [--dump-pois] [--dump-hours]\n       \
                    obc-pack schema                                 (print the config JSON Schema envelope)\n       \
                    obc-pack catalog <bake-tree> --base-url <url>   (write a bake tree's catalog manifest)"
            .into());
    }
    let output = positional.pop().unwrap();
    let config = positional.pop().unwrap();
    Ok(Args { pbfs: positional, config, output, opts })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let config = Config::load(&args.config)?;
    // `Progress::stdout()` carries no cancel token, so the CLI's only way out is
    // the one it always had: Ctrl-C, which takes the process with it.
    match pack(&args.pbfs, &config, Path::new(&args.output), &args.opts, &Progress::stdout()) {
        Ok(_) => Ok(()),
        Err(PackError::Failed(e)) => Err(e),
        // Unreachable without a token, but the CLI must not claim success either.
        Err(PackError::Cancelled) => Err("cancelled".into()),
    }
}

/// `obc-pack catalog <bake-tree> --base-url <url> [--out <path>|-] [--generated-at <ts>]`
///
/// Walks a bake output tree and writes the map-catalog manifest (`OBCC_Spec.md`).
/// Kept in the packer rather than a separate tool because the fact it exists to state
/// — the OBCM version an artifact carries — is this binary's own format version, and a
/// bakery already has `obc-pack` on the box.
fn run_catalog(args: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: obc-pack catalog <bake-tree> --base-url <url> [--out <path>|-] \
                         [--generated-at <YYYY-MM-DDTHH:MM:SSZ>]";
    let mut tree: Option<PathBuf> = None;
    let mut base_url: Option<String> = None;
    let mut out: Option<String> = None;
    let mut generated_at: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--base-url" => base_url = Some(it.next().ok_or("--base-url needs a URL")?.clone()),
            "--out" => out = Some(it.next().ok_or("--out needs a path (or `-` for stdout)")?.clone()),
            "--generated-at" => {
                generated_at = Some(it.next().ok_or("--generated-at needs an RFC 3339 UTC instant")?.clone());
            }
            other if other.starts_with("--") => return Err(format!("unknown flag `{other}`\n{USAGE}")),
            other if tree.replace(PathBuf::from(other)).is_some() => {
                return Err(format!("only one bake tree can be walked\n{USAGE}"));
            }
            _ => {}
        }
    }
    let tree = tree.ok_or_else(|| USAGE.to_string())?;
    let base_url =
        base_url.ok_or_else(|| format!("--base-url is required — it is where this tree gets published\n{USAGE}"))?;

    // No `--generated-at` ⇒ the system clock, the single wall-clock read on this path.
    // CI should pass it explicitly so a re-run of the same bake is byte-reproducible.
    let opts = obc_pack::catalog::CatalogOptions {
        base_url,
        generated_at: generated_at.unwrap_or_else(obc_pack::catalog::now_timestamp),
    };
    let generated = obc_pack::catalog::generate(&tree, &opts)?;
    // Coverage holes are not fatal, but they must never be silent: a region that
    // quietly failed one preset looks exactly like a deliberate curation choice.
    for w in &generated.warnings {
        eprintln!("obc-pack catalog: warning: {w}");
    }
    match out.as_deref() {
        // Inspection only — stdout cannot be swapped in atomically, so it is never a
        // publish target.
        Some("-") => print!("{}", obc_pack::catalog::manifest_json(&generated.manifest)),
        _ => {
            let path = out.map_or_else(|| tree.join(obc_pack::catalog::DEFAULT_MANIFEST_NAME), PathBuf::from);
            obc_pack::catalog::write_atomic(&path, &generated.manifest)?;
            println!(
                "{}: {} artifacts across {} presets",
                path.display(),
                generated.manifest.artifacts.len(),
                generated.manifest.presets.len()
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("obc-pack {} (merge + ingest + relations + land + quadtree + serialize)", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) == Some("schema") {
        match args.get(1).map(String::as_str) {
            Some("--config") => print!("{}", obc_pack::config::config_schema_json()),
            Some("--catalog") => print!("{}", obc_pack::catalog::catalog_schema_json()),
            _ => println!("{}", obc_pack::config::schema_envelope()),
        }
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) == Some("catalog") {
        return match run_catalog(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("obc-pack catalog: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("obc-pack: {e}");
            ExitCode::FAILURE
        }
    }
}
