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
//! `bake` and `publish` are separate commands on purpose: a bake is hours and may be resumed,
//! re-run, or done on a different machine from the one holding the credentials. The tree in between
//! is the interface, and it is exactly the tree `obc-pack catalog` walks.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use obc_bake::publish::{DirStore, ObjectStore, PublishOptions, RcloneStore};
use obc_pack::catalog::CatalogOptions;

const USAGE: &str = "\
usage:
  obc-bake landmarks --snapshot FILE --boundary GEOJSON --out DIR
  obc-bake peak-candidates --osm FILE --boundary GEOJSON --out FILE
  obc-bake peaks --snapshot FILE --boundary GEOJSON --out DIR
      Compile pinned article and image captures offline for the map content stage.

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
        --no-terrain         skip the automatic terrain stage below
        --landmarks FILE     embed compiled landmark content.json and its photos
        --peaks FILE         embed compiled peak peaks.json and its photos
        --dem-sources DIR    source DEM GeoTIFFs for it (default: fetched into <cache>/dem)
        --reference DIR      reference archive mirror for the terrain stage's crest lifts
        --allow-short-reference  publish cells the reference mirror is short of tiles for

      A bake runs the terrain stage FIRST, automatically: contours are traced and the
      nav graph's ascents integrated from the terrain in the tree, so a bake without
      it quietly produces a flatter map. Incremental like the cells — a tree whose
      terrain is current pays one skip-pass.

  obc-bake terrain [REGION…] --sources DIR [flags]
      Bake the curated coverage's OBCT terrain cells into the tree's terrain band.
      Terrain has its OWN revision track: this never re-bakes an OBCM cell, and a
      schema bump never re-bakes a terrain cell (OBCC_Spec.md §13).
        --out TREE              output tree (default: ./obc-bake)
        --sources DIR           source DEM GeoTIFFs (default: fetched into <cache>/dem)
        --dataset-id ID         source dataset (default: copernicus-glo-30)
        --dataset-version V     its release identity (default: 2021-1)
        --terrain-revision N    terrain store revision (default: 1)
        --posting-log2 P        sample lattice, µdeg log2 (default: 9)
        --cell-log2 S           terrain cell size, µdeg log2 (default: 19)
        --reference DIR         reference archive mirror: `index.json` plus the tiles of the
                                box, from host/obc-dem/reference/ingest.py. Where it covers a
                                crest, the baked samples carry the finer model's height
                                (OBCT_Spec.md §9), each cell records the sources it used, and
                                their credits reach the catalog. A reference change is a
                                terrain revision bump (OBCC_Spec.md §13.2), which
                                re-stamps every cell and re-bakes only the cells the
                                changed archive tiles reach. A cell the mirror is short
                                of tiles for is REFUSED, with the --bbox to mirror: it
                                would be lifted on one side of a coverage edge only.
        --allow-short-reference publish such cells anyway, and warn
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
        "landmarks" => run_landmarks(rest),
        "peaks" => run_peaks(rest),
        "peak-candidates" => run_peak_candidates(rest),
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
        &["force", "no-land", "fail-fast", "all", "no-terrain", "allow-short-reference"],
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
            "dem-sources",
            "reference",
            "landmarks",
            "peaks",
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
    // Region ids are positional, which also reads as "these regions are baked together" — and for
    // cells that is not cosmetic, because co-baked neighbours are what complete each other's border
    // cells.
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
/// The schema is one packer config plus a revision and a band table; skins restyle it without
/// changing a style id. The run ends by generating the catalog because a cell tree without its root
/// and satellites is not something a consumer can read.
fn run_cell_bake(
    flags: &Flags,
    out: PathBuf,
    regions: Vec<obc_bake::regions::Region>,
    presets_dir: &Path,
) -> Result<(), String> {
    let schema = obc_bake::presets::load_schema(presets_dir)?;
    // Keep the small canonical renderer input locked to the schema before a potentially hours-long
    // bake starts.
    obc_bake::previews::check_source(&schema.config)?;
    let skin_ids = flags.all("skin");
    // Default: every skin in the directory. A hosted catalog's whole point is that the skins are
    // free, so publishing a subset by accident is the mistake worth avoiding.
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

    // The terrain stage runs first, and automatically: contours are traced and the nav graph's
    // per-edge ascents integrated from whatever terrain is in the tree, so a bake without it
    // quietly produces a flatter map — the failure mode is silence, which is why opting out is the
    // explicit flag. Incremental like everything else: a tree whose terrain band is current pays
    // one skip-pass over the cells.
    if !flags.has("no-terrain") {
        let doc_path = out.join(obc_bake::terrain::TERRAIN_DOC);
        let doc = if doc_path.is_file() {
            obc_bake::terrain::read_terrain_doc(&doc_path)?
        } else {
            obc_bake::terrain::TerrainDoc {
                dataset_id: "copernicus-glo-30".to_string(),
                dataset_version: "2021-1".to_string(),
                posting_log2: obc_dem::bake::V1_POSTING_LOG2,
                cell_log2: obc_dem::bake::V1_CELL_LOG2,
                revision: 1,
                attribution: obc_elevation::COPERNICUS_ATTRIBUTION.to_string(),
                references: Vec::new(),
            }
        };
        let sources = match flags.get("dem-sources") {
            Some(dir) => PathBuf::from(dir),
            None => ensure_dem_sources(&regions, source.as_ref(), &cache, doc.cell_log2)?,
        };
        let reference = flags.get("reference").map(PathBuf::from);
        let dem = obc_bake::terrain::DemCutter::open(&sources, reference.as_deref())?;
        println!("{} source DEM tile(s) from {}", dem.tiles(), sources.display());
        report_reference(&dem, reference.as_deref());
        let summary = obc_bake::terrain::TerrainBakery {
            regions: &regions,
            source: source.as_ref(),
            cutter: &dem,
            opts: obc_bake::terrain::TerrainBakeOptions {
                out: out.clone(),
                doc,
                force: false,
                allow_short_reference: flags.has("allow-short-reference"),
            },
        }
        .run(&obc_pack::progress::Progress::stdout())?;
        print!("{}", summary.render());
        // The credit is a licence obligation, printed wherever the dataset was used.
        println!("{}\n", obc_elevation::COPERNICUS_ATTRIBUTION);
    }

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
            landmarks: flags.get("landmarks").map(PathBuf::from),
            peaks: flags.get("peaks").map(PathBuf::from),
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
    // Fail before an 80+ GB transfer when the required source-sharding tool is unavailable. Tests
    // inject the runner at the library boundary; the CLI uses the real executable.
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
            landmarks: flags.get("landmarks").map(PathBuf::from),
            peaks: flags.get("peaks").map(PathBuf::from),
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

/// Fetch source posts for the complete terrain cells selected by the region polygons.
fn ensure_dem_sources(
    regions: &[obc_bake::regions::Region],
    source: &dyn obc_bake::source::ExtractSource,
    cache: &Path,
    cell_log2: u8,
) -> Result<PathBuf, String> {
    let progress = obc_pack::progress::Progress::stdout();
    let mut coverages = Vec::new();
    for region in regions {
        let poly = source.fetch_poly(region, &progress)?;
        let coverage =
            obc_bake::coverage::Coverage::parse_poly(&poly).map_err(|e| format!("{}.poly: {e}", region.id))?;
        coverages.push(coverage);
    }
    let bbox = terrain_source_bbox(&coverages, cell_log2)?;
    let dir = cache.join("dem");
    println!("Fetching GLO-30 tiles for the curated coverage into {}...", dir.display());
    let mut downloaded = 0u64;
    let mut cached = 0usize;
    let paths = obc_dem::fetch::fetch_tiles(bbox, &dir, |tile, outcome| match outcome {
        obc_dem::fetch::Fetched::Cached => cached += 1,
        obc_dem::fetch::Fetched::Downloaded(len) => {
            downloaded += len;
            println!("  {} ({:.1} MB)", tile.file_name(), *len as f64 / 1e6);
        }
        obc_dem::fetch::Fetched::Absent => {}
    })?;
    println!("{} tile(s) present ({cached} cached, {:.1} MB fetched)", paths.len(), downloaded as f64 / 1e6);
    Ok(dir)
}

/// Surface bounds also sample the cell's north/east edge. Keep those edges inclusive;
/// `fetch_tiles` adds the source-post interpolation padding beyond this box.
fn terrain_source_bbox(coverages: &[obc_bake::coverage::Coverage], cell_log2: u8) -> Result<obc_dem::BboxUdeg, String> {
    let log2 = u32::from(cell_log2);
    obc_pack::grid::CellId::new(log2, 0, 0)?;
    let (min_lon, min_lat, max_lon, max_lat) = coverages
        .iter()
        .flat_map(|coverage| coverage.cells(log2))
        .map(|cell| cell.square())
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
        .ok_or("no region resolved to a terrain cell")?;
    Ok(obc_dem::BboxUdeg {
        min_lat: min_lat.clamp(-90_000_000, 90_000_000) as i32,
        min_lon: min_lon.clamp(-180_000_000, 180_000_000) as i32,
        max_lat: max_lat.clamp(-90_000_000, 90_000_000) as i32,
        max_lon: max_lon.clamp(-180_000_000, 180_000_000) as i32,
    })
}

/// What the terrain stage's reference archive gives this run, in one line per source.
fn report_reference(cutter: &obc_bake::terrain::DemCutter, root: Option<&Path>) {
    use obc_bake::terrain::TerrainCutter as _;
    let (Some(root), Some(tiles)) = (root, cutter.reference_tiles()) else { return };
    println!("{tiles} reference tile(s) indexed in {}", root.display());
    for source in cutter.reference_credits() {
        println!("  {}: {} — {} ({})", source.key, source.product, source.attribution, source.licence);
    }
}

fn run_terrain(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(
        args,
        &["force", "allow-short-reference"],
        &[
            "out",
            "sources",
            "reference",
            "dataset-id",
            "dataset-version",
            "terrain-revision",
            "posting-log2",
            "cell-log2",
            "regions",
            "source",
            "cache",
            "base-url",
            "generated-at",
        ],
    )?;
    let out = PathBuf::from(flags.get("out").unwrap_or("obc-bake"));

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
        // `const` in `obc-elevation`, travels into the catalog, and a consumer reads it from there.
        attribution: obc_elevation::COPERNICUS_ATTRIBUTION.to_string(),
        // Filled from the archive by the run itself: the wording lives in its `index.json`, and a
        // credit an operator could retype here is one that can go stale.
        references: Vec::new(),
    };

    let cache = flags.get("cache").map(PathBuf::from).unwrap_or_else(default_cache_dir);
    let source_spec = flags.get("source").unwrap_or(obc_bake::source::GeofabrikExtracts::DEFAULT_BASE_URL);
    let source = obc_bake::source::from_spec(source_spec, &cache);
    // No --sources: fetch the curated coverage's GLO-30 tiles ourselves, exactly as `bake` does.
    let sources = match flags.get("sources") {
        Some(dir) => PathBuf::from(dir),
        None => ensure_dem_sources(&regions, source.as_ref(), &cache, doc.cell_log2)?,
    };
    let reference = flags.get("reference").map(PathBuf::from);
    let cutter = obc_bake::terrain::DemCutter::open(&sources, reference.as_deref())?;
    println!("{} source DEM tile(s) from {}", cutter.tiles(), sources.display());
    report_reference(&cutter, reference.as_deref());

    let summary = obc_bake::terrain::TerrainBakery {
        regions: &regions,
        source: source.as_ref(),
        cutter: &cutter,
        opts: obc_bake::terrain::TerrainBakeOptions {
            out: out.clone(),
            doc,
            force: flags.has("force"),
            allow_short_reference: flags.has("allow-short-reference"),
        },
    }
    .run(&obc_pack::progress::Progress::stdout())?;
    print!("{}", summary.render());

    // The catalog generator reads the tree's `schema.json`, the cell store's document, which a tree
    // that has only ever been terrained does not have. Regenerating is right when the cells are
    // already there and a hard error at the very end of the whole bake when they are not, so it is
    // gated on the file rather than attempted blind.
    let finished = if out.join("schema.json").exists() {
        finish_tree(&flags, &out)
    } else {
        println!("\nno `schema.json` in {} yet — bake the cells to generate the catalog", out.display());
        Ok(())
    };
    // Unconditional, and before the `?`: the credit is a licence obligation of the data that was
    // just written, so it cannot be something only a fully successful catalog pass gets to print.
    println!("\n{}", obc_elevation::COPERNICUS_ATTRIBUTION);
    finished
}

/// The catalog is generated even after a partial run: it is what `verify` reads, and a store that
/// cannot be inspected is worse than one that visibly has holes.
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
    // R2 publishes are long enough that silence looks like a hang. Local directory publishes stay
    // quiet unless explicitly requested.
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

/// Same cache root the packer and the builder use, so a developer's already-downloaded extracts are
/// reused.
fn default_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OBCM_CACHE_DIR") {
        return PathBuf::from(dir).join("geofabrik");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache/obcm/geofabrik")
}

fn run_landmarks(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &[], &["snapshot", "boundary", "out"])?;
    if !positional.is_empty() {
        return Err("landmarks accepts named flags only".into());
    }
    let snapshot = flags.get("snapshot").ok_or("landmarks requires --snapshot FILE")?;
    let boundary = flags.get("boundary").ok_or("landmarks requires --boundary GEOJSON")?;
    let output = flags.get("out").ok_or("landmarks requires --out DIR")?;
    let content = obc_pack::landmarks::compile(Path::new(snapshot), Path::new(boundary), Path::new(output))?;
    println!(
        "{} candidates, {} texts, {} photos ({} RGB222 bytes); {} omissions",
        content.counts.candidates,
        content.counts.texts,
        content.counts.images,
        content.counts.photo_bytes,
        content.omissions.len()
    );
    Ok(())
}

fn run_peak_candidates(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &[], &["osm", "boundary", "out"])?;
    if !positional.is_empty() {
        return Err("peak-candidates accepts named flags only".into());
    }
    obc_pack::landmarks::peaks::discover(
        Path::new(flags.get("osm").ok_or("peak-candidates requires --osm FILE")?),
        Path::new(flags.get("boundary").ok_or("peak-candidates requires --boundary GEOJSON")?),
        Path::new(flags.get("out").ok_or("peak-candidates requires --out FILE")?),
    )
}
fn run_peaks(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &[], &["snapshot", "boundary", "out"])?;
    if !positional.is_empty() {
        return Err("peaks accepts named flags only".into());
    }
    let content = obc_pack::landmarks::peaks::compile(
        Path::new(flags.get("snapshot").ok_or("peaks requires --snapshot FILE")?),
        Path::new(flags.get("boundary").ok_or("peaks requires --boundary GEOJSON")?),
        Path::new(flags.get("out").ok_or("peaks requires --out DIR")?),
    )?;
    println!(
        "{} peak candidates, {} articles, {} photos, {} associations; {} omissions",
        content.counts.candidates,
        content.counts.texts,
        content.counts.images,
        content.associations.len(),
        content.omissions.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::terrain_source_bbox;
    use obc_bake::coverage::Coverage;
    use obc_dem::fetch::{tiles_for, TileId};

    fn rectangle(w: f64, s: f64, e: f64, n: f64) -> Coverage {
        Coverage::parse_poly(&format!("test\n1\n {w} {s}\n {e} {s}\n {e} {n}\n {w} {n}\n {w} {s}\nEND\nEND\n")).unwrap()
    }

    #[test]
    fn dem_fetch_covers_cell_overhang_at_the_configured_grid_size() {
        let coverages = [rectangle(10.50, 48.40, 10.51, 48.41)];
        let bbox = terrain_source_bbox(&coverages, 19).unwrap();
        assert_eq!(
            (bbox.min_lon, bbox.min_lat, bbox.max_lon, bbox.max_lat),
            (10_485_760, 48_234_496, 11_010_048, 48_758_784)
        );
        assert!(tiles_for(bbox).contains(&TileId { lat: 48, lon: 11 }));
        let finer = terrain_source_bbox(&coverages, 18).unwrap();
        assert_eq!(finer.max_lon, 10_747_904);
        assert!(!tiles_for(finer).contains(&TileId { lat: 48, lon: 11 }));
    }

    #[test]
    fn dem_fetch_keeps_closed_cell_edges_and_combines_regions() {
        let west = rectangle(-0.01, -0.01, -0.009, -0.009);
        let bbox = terrain_source_bbox(std::slice::from_ref(&west), 14).unwrap();
        assert_eq!((bbox.min_lon, bbox.min_lat, bbox.max_lon, bbox.max_lat), (-16_384, -16_384, 0, 0));
        let tiles = tiles_for(bbox);
        for lat in [-1, 0] {
            for lon in [-1, 0] {
                assert!(tiles.contains(&TileId { lat, lon }), "source stencil across the closed cell edge");
            }
        }
        let combined = terrain_source_bbox(&[west, rectangle(10.50, 48.40, 10.51, 48.41)], 19).unwrap();
        assert_eq!(
            (combined.min_lon, combined.min_lat, combined.max_lon, combined.max_lat),
            (-524_288, -524_288, 11_010_048, 48_758_784)
        );
        assert!(terrain_source_bbox(&[], 19).is_err());
        assert!(terrain_source_bbox(&[], 255).is_err());
    }
}
