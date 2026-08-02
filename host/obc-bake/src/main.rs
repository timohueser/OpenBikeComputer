//! `obc-bake` CLI — flags in, [`obc_bake`] out.
//!
//! ```text
//! obc-bake regions [--regions FILE]
//! obc-bake bake --out TREE --base-url URL [REGION…] [--skin ID]… [flags]
//! obc-bake publish TREE --base-url URL [--target dir:PATH|r2] [--generated-at TS] [--dry-run]
//! obc-bake verify TREE [--sample N]
//! obc-bake check-obcm-version [--catalog-url URL]
//! ```
//!
//! `bake` and `publish` are separate commands on purpose: a bake is hours and may
//! be resumed, re-run, or done on a different machine from the one holding the
//! credentials. The tree in between is the interface, and it is exactly the tree
//! `obc-pack catalog` walks.
//!
//! A bake resolves curated regions to grid cells, writes the catalog root and
//! digest-pinned satellites, and leaves publishing as a separate resumable step.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use obc_bake::publish::{DirStore, ObjectStore, PublishOptions, RcloneStore};
use obc_pack::catalog::CatalogOptions;

const USAGE: &str = "\
usage:
  obc-bake regions [--regions FILE]
      List the curated regions this binary would bake.

  obc-bake bake [REGION…] [flags]
      Bake selected regions into the shared cell tree and generate its catalog.
        --out TREE           output tree (default: ./obc-bake)
        --schema-id ID       published schema id (default: bikepacking)
        --schema-revision N  store revision (default: 1)
        --bands FILE         band table (default: OBCA recommendation)
        --skin ID            skin to publish (repeatable; default: all skins/)
        --generated-at TS    pin the catalog's generated_at
        --base-url URL       catalog object base (default: /obc-bake while staging)
        --regions FILE       curated region list
        --presets-dir DIR    schema.json + skins/ (default: builder/presets)
        --source SOURCE      Geofabrik base/directory, or planet PBF URL/file with --all
        --cache DIR          extract download cache
        --force              re-bake even when unchanged
        --no-land            skip land generation
        --chunk-size N       override schema chunk_size
        --fail-fast          stop at the first failure
        --summary-json FILE  write the machine-readable run summary
        --all                update/bake the whole planet through resumable source shards

  obc-bake terrain [REGION…] --sources DIR [flags]
      Bake the curated coverage's OBCT terrain cells into the tree's terrain band.
      Terrain has its OWN revision track: this never re-bakes an OBCM cell, and a
      schema bump never re-bakes a terrain cell (OBCC_Spec.md §13).
        --out TREE              output tree (default: ./obc-bake)
        --sources DIR           source DEM GeoTIFFs (`obc-dem fetch --out DIR`)
        --dataset-id ID         source dataset (default: copernicus-glo-30)
        --dataset-version V     its release identity (default: 2021-1)
        --terrain-revision N    terrain store revision (default: 1)
        --posting-log2 P        sample lattice, µdeg log2 (default: 9)
        --cell-log2 S           terrain cell size, µdeg log2 (default: 19)
        --regions FILE          curated region list
        --base-url URL          catalog object base
        --generated-at TS       pin the catalog's generated_at
        --cache DIR             extract/poly download cache
        --source SOURCE         Geofabrik base or directory (for the .poly files)
        --force                 re-bake even when unchanged

  obc-bake publish TREE --base-url URL [flags]
      Regenerate and publish content first, then replace catalog.json last.
        --target TARGET      `dir:PATH` (default: dry run) or `r2`
        --generated-at TS    pin generated_at (RFC 3339 UTC)
        --dry-run            generate + plan, upload nothing
        --verbose            report per-object upload and verification progress

  obc-bake verify TREE [--sample N]
      Verify catalog pins, cell header bboxes, reader round-trips and lockstep.

  obc-bake check-obcm-version [--catalog-url URL]
      Compare the published catalog with this build's OBCM version; skip when no
      URL is configured (--catalog-url or OBC_CATALOG_URL).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("");
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    let result = match command {
        "regions" => run_regions(rest),
        "bake" => run_bake(rest),
        "terrain" => run_terrain(rest),
        "publish" => run_publish(rest),
        "verify" => run_verify(rest),
        "check-obcm-version" => run_guard(rest),
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "" => Err(USAGE.to_string()),
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("obc-bake: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The flags, parsed once: repeated `--x v` pairs plus bare switches.
struct Flags {
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

impl Flags {
    fn parse(args: &[String], known_switches: &[&str], known_values: &[&str]) -> Result<(Self, Vec<String>), String> {
        let mut values = Vec::new();
        let mut switches = Vec::new();
        let mut positional = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if known_switches.contains(&name) {
                    switches.push(name.to_string());
                } else {
                    if !known_values.contains(&name) {
                        return Err(format!("unknown flag `--{name}`\n\n{USAGE}"));
                    }
                    let value = it.next().ok_or_else(|| format!("--{name} needs a value\n\n{USAGE}"))?;
                    values.push((name.to_string(), value.clone()));
                }
            } else {
                positional.push(a.clone());
            }
        }
        Ok((Self { values, switches }, positional))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.values.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.values.iter().filter(|(k, _)| k == name).map(|(_, v)| v.clone()).collect()
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}

fn run_regions(args: &[String]) -> Result<(), String> {
    let (flags, _) = Flags::parse(args, &[], &["regions"])?;
    let regions = obc_bake::regions::load(flags.get("regions").map(Path::new))?;
    for region in &regions {
        println!("{:<44} {}", region.id, region.name);
    }
    println!("\n{} regions", regions.len());
    Ok(())
}

fn run_bake(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(
        args,
        &["force", "no-land", "fail-fast", "all"],
        &[
            "out",
            "schema-id",
            "schema-revision",
            "bands",
            "skin",
            "generated-at",
            "regions",
            "presets-dir",
            "source",
            "cache",
            "chunk-size",
            "summary-json",
            "base-url",
        ],
    )?;
    let out = PathBuf::from(flags.get("out").unwrap_or("obc-bake"));

    let all_regions = obc_bake::regions::load(flags.get("regions").map(Path::new))?;
    if flags.has("all") {
        if !positional.is_empty() {
            return Err("`--all` cannot be combined with region selectors; omit `--all` for a curated bake".into());
        }
        let presets_dir = PathBuf::from(flags.get("presets-dir").unwrap_or("builder/presets"));
        return run_planet_bake(&flags, out, all_regions, &presets_dir);
    }
    // Region ids are positional (`obc-bake bake … europe/germany europe/switzerland`),
    // which also reads as "these regions
    // are baked *together*" — and for cells that is not cosmetic, because co-baked
    // neighbours are what complete each other's border cells.
    let wanted = positional;
    let regions: Vec<_> = if wanted.is_empty() {
        all_regions
    } else {
        for want in &wanted {
            if !all_regions.iter().any(|r| &r.id == want) {
                return Err(format!("`{want}` is not in the curated region list — add it there first"));
            }
        }
        all_regions.into_iter().filter(|r| wanted.contains(&r.id)).collect()
    };

    let presets_dir = PathBuf::from(flags.get("presets-dir").unwrap_or("builder/presets"));
    run_cell_bake(&flags, out, regions, &presets_dir)
}

/// Bake selected curated regions into the shared cell catalog.
///
/// The schema is one packer config plus a revision and a band table; skins restyle it
/// without changing a style id. The run ends by generating the catalog because a cell
/// tree without its root and satellites is not something a consumer can read.
fn run_cell_bake(
    flags: &Flags,
    out: PathBuf,
    regions: Vec<obc_bake::regions::Region>,
    presets_dir: &Path,
) -> Result<(), String> {
    let schema = obc_bake::presets::load_schema(presets_dir)?;
    // Keep the small canonical renderer input locked to the schema before a
    // potentially hours-long bake starts.
    obc_bake::previews::check_source(&schema.config)?;
    let skin_ids = flags.all("skin");
    // Default: every skin in the directory. A hosted catalog's whole point is that the
    // skins are free — publishing a subset by accident is the mistake worth avoiding,
    // not publishing one too many.
    let loaded = obc_bake::presets::load_skins(presets_dir, (!skin_ids.is_empty()).then_some(&skin_ids))?;
    let skins: Vec<&obc_bake::presets::StyleDoc> = loaded.iter().collect();

    let bands = match flags.get("bands") {
        Some(path) => obc_pack::grid::BandTable::load(path)?,
        None => obc_pack::grid::BandTable::recommended(),
    };
    let revision: u32 = match flags.get("schema-revision") {
        Some(v) => v.parse().map_err(|_| "--schema-revision needs a number".to_string())?,
        None => 1,
    };

    let cache = flags.get("cache").map(PathBuf::from).unwrap_or_else(default_cache_dir);
    let source_spec = flags.get("source").unwrap_or(obc_bake::source::GeofabrikExtracts::DEFAULT_BASE_URL);
    let source = obc_bake::source::from_spec(source_spec, &cache);
    let cutter = obc_bake::cells::ObcCutter {
        no_land: flags.has("no-land"),
        chunk_size: match flags.get("chunk-size") {
            Some(v) => Some(v.parse().map_err(|_| "--chunk-size needs a number".to_string())?),
            None => None,
        },
    };

    let bakery = obc_bake::cells::CellBakery {
        regions: &regions,
        schema: &schema,
        skins: &skins,
        source: source.as_ref(),
        cutter: &cutter,
        opts: obc_bake::cells::CellBakeOptions {
            out: out.clone(),
            force: flags.has("force"),
            fail_fast: flags.has("fail-fast"),
            bands,
            schema_id: flags.get("schema-id").unwrap_or("bikepacking").to_string(),
            schema_revision: revision,
            // Whatever `obc-bake terrain` has already published into this tree. Discovered rather
            // than flagged: the terrain a cell samples must be the terrain the same catalog
            // publishes, and a flag would be a second place for the two to disagree.
            terrain: obc_bake::terrain::in_tree(&out)?,
        },
    };
    let summary = bakery.run(&obc_pack::progress::Progress::stdout())?;
    print!("{}", summary.render());
    if let Some(path) = flags.get("summary-json") {
        let json = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("{json}\n")).map_err(|e| format!("{path}: {e}"))?;
    }

    finish_tree(flags, &out)?;

    if summary.ok() {
        Ok(())
    } else {
        Err(format!(
            "{} plan(s) failed, {} region(s) have an incomplete cell set — see the summary above",
            summary.failures().len(),
            summary.uncovered_regions.len()
        ))
    }
}

fn run_planet_bake(
    flags: &Flags,
    out: PathBuf,
    regions: Vec<obc_bake::regions::Region>,
    presets_dir: &Path,
) -> Result<(), String> {
    use obc_bake::planet::{ReplicationUpdater as _, ShardRunner as _};

    let schema = obc_bake::presets::load_schema(presets_dir)?;
    obc_bake::previews::check_source(&schema.config)?;
    let skin_ids = flags.all("skin");
    let loaded = obc_bake::presets::load_skins(presets_dir, (!skin_ids.is_empty()).then_some(&skin_ids))?;
    let skins: Vec<&obc_bake::presets::StyleDoc> = loaded.iter().collect();
    let bands = match flags.get("bands") {
        Some(path) => obc_pack::grid::BandTable::load(path)?,
        None => obc_pack::grid::BandTable::recommended(),
    };
    let revision: u32 = match flags.get("schema-revision") {
        Some(value) => value.parse().map_err(|_| "--schema-revision needs a number".to_string())?,
        None => 1,
    };
    let cache = flags.get("cache").map(PathBuf::from).unwrap_or_else(default_cache_dir);
    let progress = obc_pack::progress::Progress::stdout();
    // Fail before an 80+ GB transfer when the required source-sharding tool is
    // unavailable. Tests inject the runner at the library boundary; the CLI uses
    // the real executable (or OBC_OSMIUM for a deliberate alternate path).
    let runner = obc_bake::planet::OsmiumRunner::default();
    runner.check()?;
    let updater = obc_bake::planet::PyOsmiumUpdater::default();
    let source = flags.get("source");
    let remote_source = source.is_none_or(|value| value.starts_with("http://") || value.starts_with("https://"));
    if remote_source {
        updater.check()?;
    }
    let polygons =
        obc_bake::source::GeofabrikExtracts::new(obc_bake::source::GeofabrikExtracts::DEFAULT_BASE_URL, &cache);
    let region_presets = obc_bake::planet::resolve_region_presets(&regions, &polygons, &bands, &progress)?;
    let input = obc_bake::planet::resolve_planet_with(source, &cache, &progress, &updater)?;
    let shards = obc_bake::planet::PlanetSharder { input: &input, cache: &cache, runner: &runner }.run(&progress)?;
    let cutter = obc_bake::cells::ObcCutter {
        no_land: flags.has("no-land"),
        chunk_size: match flags.get("chunk-size") {
            Some(value) => Some(value.parse().map_err(|_| "--chunk-size needs a number".to_string())?),
            None => None,
        },
    };
    let summary = obc_bake::planet::PlanetBake {
        input: &input,
        leaves: &shards.leaves,
        regions: &region_presets,
        schema: &schema,
        skins: &skins,
        cutter: &cutter,
        source_leaves_reused: shards.reused,
        source_leaves_refreshed: shards.refreshed,
        source_leaves_changed: shards.changed,
        opts: obc_bake::cells::CellBakeOptions {
            out: out.clone(),
            force: flags.has("force"),
            fail_fast: flags.has("fail-fast"),
            bands,
            schema_id: flags.get("schema-id").unwrap_or("bikepacking").to_string(),
            schema_revision: revision,
            // Whatever `obc-bake terrain` has already published into this tree. Discovered rather
            // than flagged: the terrain a cell samples must be the terrain the same catalog
            // publishes, and a flag would be a second place for the two to disagree.
            terrain: obc_bake::terrain::in_tree(&out)?,
        },
    }
    .run(&progress)?;
    print!("{}", summary.render());
    if let Some(path) = flags.get("summary-json") {
        let json = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("{json}\n")).map_err(|e| format!("{path}: {e}"))?;
    }
    if summary.ok() {
        finish_tree(flags, &out)?;
        Ok(())
    } else {
        Err(format!("{} planet leaf/leaves failed — see the summary above", summary.failures.len()))
    }
}

/// `obc-bake terrain` — the terrain artifact class, baked on its own track.
///
/// A separate command from `bake` rather than a phase of it, which is the CLI saying what
/// `OBCC_Spec.md` §13.2 says: these are two stores with two revisions, and one is not a step of
/// the other. It needs no OSM extract — only the `.poly` outlines, to know which squares the
/// curated coverage touches.
fn run_terrain(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(
        args,
        &["force"],
        &[
            "out",
            "sources",
            "dataset-id",
            "dataset-version",
            "terrain-revision",
            "posting-log2",
            "cell-log2",
            "regions",
            "presets-dir",
            "source",
            "cache",
            "base-url",
            "generated-at",
        ],
    )?;
    let out = PathBuf::from(flags.get("out").unwrap_or("obc-bake"));
    let sources = PathBuf::from(
        flags
            .get("sources")
            .ok_or("terrain needs --sources DIR — a directory of source DEM GeoTIFFs (`obc-dem fetch --out DIR`)")?,
    );

    let all_regions = obc_bake::regions::load(flags.get("regions").map(Path::new))?;
    let regions: Vec<_> = if positional.is_empty() {
        all_regions
    } else {
        for want in &positional {
            if !all_regions.iter().any(|r| &r.id == want) {
                return Err(format!("`{want}` is not in the curated region list — add it there first"));
            }
        }
        all_regions.into_iter().filter(|r| positional.contains(&r.id)).collect()
    };

    let number = |name: &str, default: u32| -> Result<u32, String> {
        match flags.get(name) {
            Some(value) => value.parse().map_err(|_| format!("--{name} needs a number")),
            None => Ok(default),
        }
    };
    let log2 = |name: &str, default: u8| -> Result<u8, String> {
        u8::try_from(number(name, u32::from(default))?).map_err(|_| format!("--{name} is out of range"))
    };
    let doc = obc_bake::terrain::TerrainDoc {
        dataset_id: flags.get("dataset-id").unwrap_or("copernicus-glo-30").to_string(),
        dataset_version: flags.get("dataset-version").unwrap_or("2021-1").to_string(),
        posting_log2: log2("posting-log2", obc_dem::bake::V1_POSTING_LOG2)?,
        cell_log2: log2("cell-log2", obc_dem::bake::V1_CELL_LOG2)?,
        revision: number("terrain-revision", 1)?,
        // The credit is a licence obligation and is never retyped here: it comes from the one
        // `const` in `obc-dem`, travels into the catalog, and a consumer reads it from there.
        attribution: obc_dem::COPERNICUS_ATTRIBUTION.to_string(),
    };

    let cache = flags.get("cache").map(PathBuf::from).unwrap_or_else(default_cache_dir);
    let source_spec = flags.get("source").unwrap_or(obc_bake::source::GeofabrikExtracts::DEFAULT_BASE_URL);
    let source = obc_bake::source::from_spec(source_spec, &cache);
    let cutter = obc_bake::terrain::DemCutter::open(&sources)?;
    println!("{} source DEM tile(s) from {}", cutter.tiles(), sources.display());

    let summary = obc_bake::terrain::TerrainBakery {
        regions: &regions,
        source: source.as_ref(),
        cutter: &cutter,
        opts: obc_bake::terrain::TerrainBakeOptions { out: out.clone(), doc, force: flags.has("force") },
    }
    .run(&obc_pack::progress::Progress::stdout())?;
    print!("{}", summary.render());

    finish_tree(&flags, &out)?;
    println!("\n{}", obc_dem::COPERNICUS_ATTRIBUTION);
    Ok(())
}

/// The catalog is generated even after a partial run: it is what `verify` reads,
/// and a store that cannot be inspected is worse than one that visibly has holes.
fn finish_tree(flags: &Flags, out: &Path) -> Result<(), String> {
    let base_url = flags
        .get("base-url")
        .map(str::to_owned)
        .or_else(|| std::env::var("OBC_MAPS_BASE_URL").ok().filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| "/obc-bake".into());
    let opts = obc_pack::catalog::CatalogOptions::new(
        &base_url,
        flags.get("generated-at").map_or_else(obc_pack::catalog::now_timestamp, str::to_string),
    );
    let seed = obc_pack::catalog::generate(out, &opts)?;
    let previews = obc_bake::previews::generate(out, &seed.root)?;
    let generated = obc_pack::catalog::generate(out, &opts)?;
    for w in &generated.warnings {
        eprintln!("warning: {w}");
    }
    obc_pack::catalog::write_all_atomic(out, &generated)?;
    let cells: u32 = generated.root.cell_index.iter().map(|c| c.cell_count).sum();
    let known_empty: u32 = generated.root.cell_index.iter().map(|c| c.known_empty_count).sum();
    println!(
        "\n{}: {cells} artifact cell(s) + {known_empty} known-empty across {} bands, {} region(s), {} skin(s), {} preview(s), {} satellite document(s)",
        out.join(obc_pack::catalog::DEFAULT_MANIFEST_NAME).display(),
        generated.root.cell_index.len(),
        generated.root.regions.len(),
        generated.root.skins.len(),
        previews.skins,
        generated.satellites.len()
    );

    Ok(())
}

/// `obc-bake verify TREE [--sample N]` — the cell tree's own acceptance gate.
fn run_verify(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &[], &["sample"])?;
    let tree = PathBuf::from(positional.first().ok_or_else(|| format!("verify needs a cell tree\n\n{USAGE}"))?);
    let sample = match flags.get("sample") {
        Some(v) => v.parse().map_err(|_| "--sample needs a number".to_string())?,
        None => obc_bake::verify::CellTreeVerifyOptions::default().sample,
    };
    let guard = obc_bake::guard::check_cell_store(&tree)?;
    print!("{}", guard.render());
    let report = obc_bake::verify::verify_cell_tree(&tree, obc_bake::verify::CellTreeVerifyOptions { sample })?;
    print!("{}", report.render());
    if guard.ok() && report.ok() {
        Ok(())
    } else {
        Err(format!("{} guard problem(s), {} verify problem(s)", guard.problems.len(), report.problems.len()))
    }
}

fn run_publish(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &["dry-run", "verbose"], &["base-url", "target", "generated-at"])?;
    let tree = positional.first().ok_or_else(|| format!("publish needs a bake tree\n\n{USAGE}"))?;
    let base_url = flags
        .get("base-url")
        .map(str::to_owned)
        .or_else(|| std::env::var("OBC_MAPS_BASE_URL").ok().filter(|value| !value.trim().is_empty()))
        .ok_or_else(|| {
            format!(
                "publish needs --base-url URL or OBC_MAPS_BASE_URL — it is where this tree becomes visible\n\n{USAGE}"
            )
        })?;

    let target = flags.get("target").unwrap_or("");
    let dry_run = flags.has("dry-run") || target.is_empty();
    let store: Box<dyn ObjectStore> = match target {
        "r2" => Box::new(RcloneStore::from_env()?),
        t if t.starts_with("dir:") => Box::new(DirStore::new(&t["dir:".len()..])),
        "" => Box::new(DirStore::new(".")), // unused: dry_run is on
        other => return Err(format!("unknown --target `{other}` (expected `r2` or `dir:PATH`)")),
    };

    let generated_at = flags.get("generated-at").map_or_else(obc_pack::catalog::now_timestamp, str::to_string);
    println!("publishing {tree} → {}{}", store.describe(), if dry_run { " (dry run)" } else { "" });
    // R2 publishes are long enough that silence looks like a hang. Keep local
    // directory publishes quiet unless explicitly requested, but always report
    // progress for the real operator path.
    let publish_opts = PublishOptions { dry_run, verbose: flags.has("verbose") || target == "r2" };
    let opts = CatalogOptions::new(&base_url, generated_at);
    let report = obc_bake::publish::publish(Path::new(tree), store.as_ref(), &opts, publish_opts)?;
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "{} cells, {} region(s), {} skin(s), {} objects, {} bytes{}",
        report.cells,
        report.regions.len(),
        report.skins,
        report.objects,
        report.bytes,
        if dry_run { " — nothing uploaded" } else { "" }
    );
    Ok(())
}

fn run_guard(args: &[String]) -> Result<(), String> {
    let (flags, _) = Flags::parse(args, &[], &["catalog-url"])?;
    let outcome = obc_bake::guard::check(flags.get("catalog-url"))?;
    let text = outcome.render();
    if outcome.ok() {
        println!("{text}");
        Ok(())
    } else {
        Err(text)
    }
}

/// Same cache root the packer and the builder use (`OBCM_CACHE_DIR`), so a
/// developer's already-downloaded extracts are reused.
fn default_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OBCM_CACHE_DIR") {
        return PathBuf::from(dir).join("geofabrik");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache/obcm/geofabrik")
}
