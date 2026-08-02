//! `obc-dem` — the CLI over [`obc_dem`]: fetch source DEM tiles, bake OBCT terrain.
//!
//! Two subcommands, and the split between them is the design: `fetch` is the only thing that
//! touches the network, `bake` is a pure function of a tile directory and a box. See the crate
//! docs for the determinism contract that split exists to protect.

use std::path::PathBuf;
use std::process::ExitCode;

use obc_dem::bake::{bake_cells, bake_shard, BakeParams, BakeReport, V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::fetch::{fetch_tiles, Fetched};
use obc_dem::geotiff::DemMosaic;
use obc_dem::{BboxUdeg, COPERNICUS_ATTRIBUTION, SOURCE_DATASET};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fetch") => fetch(&args[1..]),
        Some("bake") => bake(&args[1..]),
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("obc-dem: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
usage:
  obc-dem fetch --bbox <min_lat,min_lon,max_lat,max_lon> --out <dir>
  obc-dem bake  --sources <dir> --bbox <min_lat,min_lon,max_lat,max_lon>
                (--out <dir> | --shard <file.obcd>)
                [--posting-log2 <4..16>] [--cell-log2 <10..28>] [--quiet]

  --bbox is LATITUDE FIRST — min_lat,min_lon,max_lat,max_lon — unlike
  `obc-pack --bbox`, which is lon,lat,lon,lat. Both numbers in an Alpine box are
  plausible on either axis, so nothing can catch the mix-up for you.

  --out <dir>    one .obcd file per terrain cell (what a bakery publishes)
  --shard <file> one .obcd covering the whole box (a sidecar beside a map)

  Defaults are the v1 baked pairing: posting 2^9 µdeg (~57 x 39 m at 47N),
  cell 2^19 µdeg (1024^2 samples, a 2 MiB block). Both are OBCT header data, so
  a different pairing is a re-bake, not a format change.

`fetch` downloads Copernicus GLO-30 tiles from the AWS Open Data mirror; `bake`
never touches the network.";

/// `--flag value` parsing, in the shape `obc-mkimage` established: no argument crate, and an
/// unknown flag is an error rather than something silently ignored.
fn next_value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    it.next().cloned().ok_or_else(|| format!("{flag} needs a value"))
}

fn fetch(args: &[String]) -> Result<(), String> {
    let (mut bbox, mut out) = (None, None::<PathBuf>);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--bbox" => bbox = Some(BboxUdeg::parse(&next_value(&mut it, "--bbox")?)?),
            "--out" => out = Some(next_value(&mut it, "--out")?.into()),
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let bbox = bbox.ok_or("fetch: missing --bbox")?;
    let out = out.ok_or("fetch: missing --out")?;

    let paths = fetch_tiles(bbox, &out, |tile, outcome| match outcome {
        Fetched::Cached => println!("  {} (cached)", tile.file_name()),
        Fetched::Downloaded(len) => println!("  {} ({:.1} MB)", tile.file_name(), *len as f64 / 1e6),
        Fetched::Absent => println!("  {} — no object on the mirror (ocean or outside coverage)", tile.file_name()),
    })?;
    println!("{} tile(s) in {}", paths.len(), out.display());
    println!("\n{SOURCE_DATASET}: {COPERNICUS_ATTRIBUTION}");
    Ok(())
}

fn bake(args: &[String]) -> Result<(), String> {
    let (mut sources, mut bbox, mut out, mut shard) = (None::<PathBuf>, None, None::<PathBuf>, None::<PathBuf>);
    let (mut posting_log2, mut cell_log2, mut quiet) = (V1_POSTING_LOG2, V1_CELL_LOG2, false);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--sources" => sources = Some(next_value(&mut it, "--sources")?.into()),
            "--bbox" => bbox = Some(BboxUdeg::parse(&next_value(&mut it, "--bbox")?)?),
            "--out" => out = Some(next_value(&mut it, "--out")?.into()),
            "--shard" => shard = Some(next_value(&mut it, "--shard")?.into()),
            "--posting-log2" => posting_log2 = parse_log2(&next_value(&mut it, "--posting-log2")?)?,
            "--cell-log2" => cell_log2 = parse_log2(&next_value(&mut it, "--cell-log2")?)?,
            "--quiet" => quiet = true,
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let sources = sources.ok_or("bake: missing --sources")?;
    let bbox = bbox.ok_or("bake: missing --bbox")?;
    if out.is_some() == shard.is_some() {
        return Err("bake: give exactly one of --out <dir> (a file per cell) or --shard <file>".to_string());
    }
    let params = BakeParams { posting_log2, cell_log2, bbox };

    let mosaic = DemMosaic::open_dir(&sources)?;
    if !quiet {
        println!("{} source tile(s) from {}", mosaic.len(), sources.display());
    }
    let progress = |done: u64, total: u64, ci: u32, cj: u32, written: bool| {
        if !quiet {
            let what = if written { "baked" } else { "empty" };
            println!("  [{done}/{total}] cell {cell_log2}/{ci}/{cj} {what}");
        }
    };

    let report = match (&out, &shard) {
        (Some(dir), _) => bake_cells(&mosaic, params, dir, progress)?,
        (_, Some(path)) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let report = bake_shard(&mosaic, params, std::io::BufWriter::new(file), progress)?;
            if !quiet {
                let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                println!("{} — {len} bytes", path.display());
            }
            report
        }
        _ => unreachable!("checked above"),
    };
    summarise(&report);
    println!("\n{SOURCE_DATASET}: {COPERNICUS_ATTRIBUTION}");
    Ok(())
}

fn summarise(report: &BakeReport) {
    let BakeReport { cells_total, cells_written, samples_total, samples_nodata } = *report;
    let covered = samples_total - samples_nodata;
    let pct = if samples_total == 0 { 0.0 } else { covered as f64 * 100.0 / samples_total as f64 };
    println!("{cells_written}/{cells_total} cells written, {covered}/{samples_total} samples covered ({pct:.1} %)");
}

fn parse_log2(text: &str) -> Result<u8, String> {
    text.parse::<u8>().map_err(|_| format!("`{text}` is not a log2 exponent"))
}
