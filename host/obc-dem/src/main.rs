//! `obc-dem` — the CLI over [`obc_dem`]: fetch source DEM tiles, bake OBCT terrain.
//!
//! `fetch` downloads source tiles; `bake` resamples them to native terrain; `surface` adds
//! geographic levels and bounds. Only `fetch` uses the network. See the crate docs for the
//! determinism contract that split exists to protect.

use std::path::PathBuf;
use std::process::ExitCode;

use obc_dem::bake::{bake_cells, bake_shard, BakeParams, BakeReport, CellDone, V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::crest::REPORT_M;
use obc_dem::fetch::{fetch_tiles, Fetched};
use obc_dem::geotiff::DemMosaic;
use obc_dem::reference::ReferenceArchive;
use obc_dem::BboxUdeg;
use obc_elevation::{COPERNICUS_ATTRIBUTION, SOURCE_DATASET};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("fetch") => fetch(&args[1..]),
        Some("bake") => bake(&args[1..]),
        Some("surface") => surface(&args[1..]),
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
  obc-dem surface <input.obcd> <output.obcd>
  obc-dem fetch --bbox <min_lat,min_lon,max_lat,max_lon> --out <dir>
  obc-dem bake  --sources <dir> --bbox <min_lat,min_lon,max_lat,max_lon>
                (--out <dir> | --shard <file.obcd>)
                [--reference <archive root>]
                [--posting-log2 <4..16>] [--cell-log2 <10..28>] [--quiet]

  --bbox is LATITUDE FIRST — min_lat,min_lon,max_lat,max_lon — unlike
  `obc-pack --bbox`, which is lon,lat,lon,lat. Both numbers in an Alpine box are
  plausible on either axis, so nothing can catch the mix-up for you.

  --out <dir>    one .obcd file per terrain cell (what a bakery publishes)
  --shard <file> one .obcd covering the whole box (a sidecar beside a map)

  Defaults are the v1 baked pairing: posting 2^9 µdeg (~57 x 39 m at 47N),
  cell 2^19 µdeg (1024^2 samples, a 2 MiB block). Both are OBCT header data, so
  a different pairing is a re-bake, not a format change.

  --reference <root> a reference archive, or a mirror of one: `index.json` plus
                     int16 GeoTIFF tiles under `16/<ti>/<tj>.tif`, written by
                     host/obc-dem/reference/ingest.py. Where it covers the box,
                     summits and ridge crests the 2^9 lattice loses are raised
                     to the reference ground, in the baked samples themselves,
                     so every consumer reads one surface. Cells it does not
                     cover come out byte-identical to a run without it, so
                     national LiDAR may stop at a border. The bake streams one
                     tile at a time and reads each pixel once. The reference
                     keeps its own attribution.

`fetch` downloads Copernicus GLO-30 tiles from the AWS Open Data mirror; `bake`
never touches the network.";

fn surface(args: &[String]) -> Result<(), String> {
    let (mut input, mut output) = (None::<String>, None::<String>);
    for arg in args {
        match arg.as_str() {
            other if other.starts_with("--") => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
            other if input.is_none() => input = Some(other.to_string()),
            other if output.is_none() => output = Some(other.to_string()),
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let (input, output) = (input.ok_or("surface: missing input.obcd")?, output.ok_or("surface: missing output.obcd")?);
    if input == output {
        return Err("surface: input and output must differ".into());
    }
    let bytes = std::fs::read(&input).map_err(|e| format!("{input}: {e}"))?;
    let file = std::fs::File::create(&output).map_err(|e| format!("{output}: {e}"))?;
    obc_dem::surface::convert(&bytes, std::io::BufWriter::new(file))?;
    let size = std::fs::metadata(&output).map_err(|e| e.to_string())?.len();
    println!("{output}: {size} bytes (source {} bytes)", bytes.len());
    Ok(())
}

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
    let mut reference = None::<PathBuf>;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--sources" => sources = Some(next_value(&mut it, "--sources")?.into()),
            "--reference" => reference = Some(next_value(&mut it, "--reference")?.into()),
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
    let archive = match &reference {
        Some(root) => {
            let archive = ReferenceArchive::open(root)?;
            if !quiet {
                println!("{} reference tile(s) indexed in {}", archive.len(), root.display());
            }
            Some(archive)
        }
        None => None,
    };
    let archive = archive.as_ref();
    let progress = |cell: CellDone| {
        if !quiet {
            let CellDone { index, total, ci, cj, written, lifted } = cell;
            let what = if written { "baked" } else { "empty" };
            let lifts = if lifted == 0 { String::new() } else { format!(", {lifted} lifted") };
            println!("  [{index}/{total}] cell {cell_log2}/{ci}/{cj} {what}{lifts}");
        }
    };

    let report = match (&out, &shard) {
        (Some(dir), _) => bake_cells(&mosaic, params, archive, dir, progress)?,
        (_, Some(path)) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let report = bake_shard(&mosaic, params, archive, std::io::BufWriter::new(file), progress)?;
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
    if reference.is_some() {
        println!("\nThe reference DEM keeps its own attribution, which must travel with this container.");
        println!("host/obc-dem/reference/README.md holds the wording for each source.");
    }
    Ok(())
}

fn summarise(report: &BakeReport) {
    let covered = report.samples_total - report.samples_nodata;
    let pct = if report.samples_total == 0 { 0.0 } else { covered as f64 * 100.0 / report.samples_total as f64 };
    println!(
        "{}/{} cells written, {covered}/{} samples covered ({pct:.1} %)",
        report.cells_written, report.cells_total, report.samples_total
    );
    if !report.sources.is_empty() {
        let keys: Vec<&str> = report.sources.iter().map(String::as_str).collect();
        println!("reference source(s) the lifts come from: {}", keys.join(", "));
    }
    // A mirror carries the whole index and the tiles of one box, so a tile the index names and the
    // mirror lacks is normal at the edges and a short copy in the middle. Either way it costs lifts
    // without failing, so the count is the operator's only sight of it.
    if !report.reference_tiles_absent.is_empty() {
        println!(
            "{} tile(s) the index names are not in this archive — lifts there were skipped",
            report.reference_tiles_absent.len()
        );
    }
    let lifts = report.lifts;
    if lifts.nodes == 0 {
        return;
    }
    // §9 puts no ceiling on a lift, so the size of the largest one is the operator's only signal
    // that a reference carries a spike rather than a cliff the source lost.
    let (lat, lon) = lifts.max_at;
    println!(
        "{} sample(s) lifted, largest {} m at {:.6},{:.6}; {} above {REPORT_M} m",
        lifts.nodes,
        lifts.max_m,
        f64::from(lat) / 1e6,
        f64::from(lon) / 1e6,
        lifts.over_report,
    );
    if lifts.over_report > 0 {
        println!("A lift above {REPORT_M} m is a wall the source lost, or a broken reference. Check that position.");
    }
}

fn parse_log2(text: &str) -> Result<u8, String> {
    text.parse::<u8>().map_err(|_| format!("`{text}` is not a log2 exponent"))
}
