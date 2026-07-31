//! `obc-bake` CLI — flags in, [`obc_bake`] out.
//!
//! ```text
//! obc-bake regions [--regions FILE]
//! obc-bake bake --out TREE [--regions FILE] [--presets-dir DIR] [--region ID]…
//!               [--source BASE] [--cache DIR] [--force] [--no-land] [--fail-fast]
//!               [--summary-json FILE]
//! obc-bake bake --cells --out TREE --base-url URL [REGION…] [--skin ID]…
//!               [--schema-revision N] [--bands FILE] [flags as above]
//! obc-bake publish TREE --base-url URL [--v2] [--target dir:PATH|r2] [--generated-at TS] [--dry-run]
//! obc-bake verify TREE [--sample N]
//! obc-bake check-obcm-version [--catalog-url URL]
//! ```
//!
//! `bake` and `publish` are separate commands on purpose: a bake is hours and may
//! be resumed, re-run, or done on a different machine from the one holding the
//! credentials. The tree in between is the interface, and it is exactly the tree
//! `obc-pack catalog` walks.
//!
//! `--cells` selects the cell path (#1016 P2): the same curated regions, resolved to
//! grid cells and published as an `OBCC_Spec.md` §11 catalog rather than as one
//! artifact per (region × preset). It is a flag rather than a separate command
//! because the scoping — which regions, from which source, with which cache — is
//! identical, and because the two paths will overlap for exactly as long as the
//! migration does.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use obc_bake::bake::{BakeOptions, Bakery, ObcPacker};
use obc_bake::publish::{DirStore, ObjectStore, PublishOptions, RcloneStore};
use obc_pack::catalog::CatalogOptions;
use obc_pack::progress::Progress;

const USAGE: &str = "\
usage:
  obc-bake regions [--regions FILE]
      List the curated regions this binary would bake.

  obc-bake bake --out TREE [flags]
      Bake one whole-region artifact per region, against the one schema, into a
      tree `obc-pack catalog` accepts (the v1 path, kept for the migration).
        --cells              bake GRID CELLS instead (OBCA/OBCC v2, epic #1016):
                             regions become selections, cells become the artifacts.
                             Regions may be named positionally, e.g.
                               obc-bake bake --cells --out t --base-url U \\
                                 europe/germany europe/switzerland
                             --base-url URL      required: where the tree is published
                             --schema-id ID      the published schema id (bikepacking)
                             --schema-revision N the store's revision (default: 1)
                             --bands FILE        the band table (default: OBCA §1.5 v1)
                             --skin ID           a skin to publish (repeatable;
                                                 default: every skin in skins/)
                             --generated-at TS   pin the catalog's generated_at
        --regions FILE       region list (default: the built-in curated list)
        --presets-dir DIR    the style documents: `schema.json` + `skins/<id>.json`
                             (default: builder/presets)
        --region ID          bake only this region (repeatable)
        --source BASE        extract source: an https:// base, or a local directory
                             (default: https://download.geofabrik.de)
        --cache DIR          extract download cache (default: $OBCM_CACHE_DIR or
                             ~/.cache/obcm/geofabrik)
        --force              re-bake even when nothing changed
        --no-land            skip land generation (skips the ~950 MB dataset)
        --chunk-size N       override the presets' chunk_size
        --fail-fast          stop at the first failure
        --summary-json FILE  write the machine-readable run summary

  obc-bake publish TREE --base-url URL [flags]
      Generate the manifest and publish the tree: artifacts first, manifest last.
        --base-url URL       where the tree is published (the manifest's url prefix)
        --v2                 publish a CELL tree (root + digest-pinned satellites)
        --target TARGET      `dir:PATH` (default: dry run) or `r2` (env credentials)
        --generated-at TS    pin the manifest's generated_at (RFC 3339 UTC)
        --dry-run            generate + plan, upload nothing
        --allow-shrink       permit dropping coverage the live catalog serves
                             (refused by default — publishing a partial bake over a
                             full one silently un-offers regions)

  obc-bake verify TREE [--sample N]
      Verify a cell tree against its own catalog: satellite digests, every cell's
      header bbox against its id, and a full reader round-trip on one cell in N
      (default 50; 1 = every cell). Also runs the cell-store lockstep guard.

  obc-bake check-obcm-version [--catalog-url URL]
      Fail if the published catalog is not this build's OBCM version. Skips when no
      URL is configured (--catalog-url or OBC_CATALOG_URL).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("");
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    let result = match command {
        "regions" => run_regions(rest),
        "bake" => run_bake(rest),
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
    fn parse(args: &[String], known_switches: &[&str]) -> Result<(Self, Vec<String>), String> {
        let mut values = Vec::new();
        let mut switches = Vec::new();
        let mut positional = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if known_switches.contains(&name) {
                    switches.push(name.to_string());
                } else {
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
    let (flags, _) = Flags::parse(args, &[])?;
    let regions = obc_bake::regions::load(flags.get("regions").map(Path::new))?;
    for region in &regions {
        println!("{:<44} {}", region.id, region.name);
    }
    println!("\n{} regions", regions.len());
    Ok(())
}

fn run_bake(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &["force", "no-land", "fail-fast", "cells"])?;
    let out = PathBuf::from(flags.get("out").ok_or_else(|| format!("bake needs --out TREE\n\n{USAGE}"))?);

    let all_regions = obc_bake::regions::load(flags.get("regions").map(Path::new))?;
    // Positional region ids and `--region` mean the same thing. The cell path's
    // canonical spelling is positional (`obc-bake bake --cells … europe/germany
    // europe/switzerland`), which is also the spelling that reads as "these regions
    // are baked *together*" — and for cells that is not cosmetic, because co-baked
    // neighbours are what complete each other's border cells.
    let mut wanted = flags.all("region");
    wanted.extend(positional);
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
    if flags.has("cells") {
        return run_cell_bake(&flags, out, regions, &presets_dir);
    }
    // Loud rather than ignored: `--preset <id>` used to pick one of several shipped
    // presets, and a caller still passing it (a script, a workflow input) would
    // otherwise get a bake of *something else* and no hint that its flag stopped
    // meaning anything.
    if !flags.all("preset").is_empty() {
        return Err(
            "`--preset` retired with the preset shelf (#1036): there is one schema, and a look is a skin stamped at \
             assembly time. Drop the flag to bake against `<presets-dir>/schema.json`."
                .to_string(),
        );
    }
    // One document, one artifact per region: the schema is the only style document
    // that can pack anything (a skin has no ladder and no routing table).
    let presets = vec![obc_bake::presets::load_schema(&presets_dir)?];

    let cache = flags.get("cache").map(PathBuf::from).unwrap_or_else(default_cache_dir);
    let source_spec = flags.get("source").unwrap_or(obc_bake::source::GeofabrikExtracts::DEFAULT_BASE_URL);
    let source = obc_bake::source::from_spec(source_spec, &cache);

    let packer = ObcPacker {
        no_land: flags.has("no-land"),
        chunk_size: match flags.get("chunk-size") {
            Some(v) => Some(v.parse().map_err(|_| "--chunk-size needs a number".to_string())?),
            None => None,
        },
    };
    let bakery = Bakery {
        regions: &regions,
        presets: &presets,
        source: source.as_ref(),
        packer: &packer,
        opts: BakeOptions { out, force: flags.has("force"), fail_fast: flags.has("fail-fast") },
    };

    let summary = bakery.run(&Progress::stdout())?;
    print!("{}", summary.render());
    if let Some(path) = flags.get("summary-json") {
        let json = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("{json}\n")).map_err(|e| format!("{path}: {e}"))?;
    }
    if summary.ok() {
        Ok(())
    } else {
        // Non-zero even when most of the matrix succeeded: a coverage hole must not
        // be something a green pipeline can hide.
        Err(format!(
            "{} job(s) failed, {} region(s) have no artifact — see the summary above",
            summary.failures().len(),
            summary.uncovered_regions.len()
        ))
    }
}

/// `obc-bake bake --cells …` — the cell path (#1016 P2).
///
/// Everything up to here is shared with the v1 matrix bake (which regions, from where,
/// cached where); from here the unit changes. The schema is one packer config plus a
/// revision and a band table; the skins are configs that restyle it without changing
/// a style id; the artifacts are cells. The run ends by generating the catalog into
/// the tree, because a cell tree without its root and satellites is not something a
/// consumer can read at all — unlike a v1 tree, whose artifacts are each a whole map.
fn run_cell_bake(
    flags: &Flags,
    out: PathBuf,
    regions: Vec<obc_bake::regions::Region>,
    presets_dir: &Path,
) -> Result<(), String> {
    let base_url = flags.get("base-url").ok_or_else(|| {
        format!("bake --cells needs --base-url URL — every cell's `url` is it plus the cell's path\n\n{USAGE}")
    })?;
    let schema = obc_bake::presets::load_schema(presets_dir)?;
    let skin_ids = flags.all("skin");
    // Default: every skin in the directory. A hosted catalog's whole point is that the
    // skins are free — publishing a subset by accident is the mistake worth avoiding,
    // not publishing one too many.
    let loaded = obc_bake::presets::load_skins(presets_dir, (!skin_ids.is_empty()).then_some(&skin_ids))?;
    let skins: Vec<&obc_bake::presets::StyleDoc> = loaded.iter().collect();

    let bands = match flags.get("bands") {
        Some(path) => obc_pack::grid::BandTable::load(path)?,
        None => obc_pack::grid::BandTable::v1(),
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
        },
    };
    let summary = bakery.run(&Progress::stdout())?;
    print!("{}", summary.render());
    if let Some(path) = flags.get("summary-json") {
        let json = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("{json}\n")).map_err(|e| format!("{path}: {e}"))?;
    }

    // The catalog is generated even after a partial run: it is what `obc-bake verify`
    // reads, and a store you cannot inspect is worse than one you can see the holes in.
    let opts = obc_pack::catalog::v2::CatalogV2Options::new(
        base_url,
        flags.get("generated-at").map_or_else(obc_pack::catalog::now_timestamp, str::to_string),
    );
    let generated = obc_pack::catalog::v2::generate(&out, &opts)?;
    for w in &generated.warnings {
        eprintln!("warning: {w}");
    }
    obc_pack::catalog::v2::write_all_atomic(&out, &generated)?;
    let cells: u32 = generated.root.cell_index.iter().map(|c| c.cell_count).sum();
    println!(
        "\n{}: {cells} cells across {} bands, {} region(s), {} skin(s), {} satellite document(s)",
        out.join(obc_pack::catalog::DEFAULT_MANIFEST_NAME).display(),
        generated.root.cell_index.len(),
        generated.root.regions.len(),
        generated.root.skins.len(),
        generated.satellites.len()
    );

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

/// `obc-bake verify TREE [--sample N]` — the cell tree's own acceptance gate.
fn run_verify(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &[])?;
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
    let (flags, positional) = Flags::parse(args, &["dry-run", "allow-shrink", "v2"])?;
    let tree = positional.first().ok_or_else(|| format!("publish needs a bake tree\n\n{USAGE}"))?;
    let base_url = flags
        .get("base-url")
        .ok_or_else(|| format!("--base-url is required — it is where this tree gets published\n\n{USAGE}"))?;

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
    let publish_opts = PublishOptions { dry_run, allow_shrink: flags.has("allow-shrink") };
    if flags.has("v2") {
        let opts = obc_pack::catalog::v2::CatalogV2Options::new(base_url, generated_at);
        let report = obc_bake::publish::publish_v2(Path::new(tree), store.as_ref(), &opts, publish_opts)?;
        for warning in &report.warnings {
            eprintln!("warning: {warning}");
        }
        if !report.coverage_lost.is_empty() {
            eprintln!("\n!!! COVERAGE REMOVED ({} regions) !!!", report.coverage_lost.len());
            for id in &report.coverage_lost {
                eprintln!("  {id}");
            }
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
        return Ok(());
    }

    let opts = CatalogOptions { base_url: base_url.to_string(), generated_at };
    let report = obc_bake::publish::publish(Path::new(tree), store.as_ref(), &opts, publish_opts)?;
    // A coverage hole is a warning in the generator and must stay visible here.
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if !report.coverage_lost.is_empty() {
        eprintln!("\n!!! COVERAGE REMOVED ({} artifacts) !!!", report.coverage_lost.len());
        for pair in &report.coverage_lost {
            eprintln!("  {pair}");
        }
    }
    println!(
        "{} artifacts across {} presets, {} objects, {} bytes{}",
        report.manifest.artifacts.len(),
        report.manifest.presets.len(),
        report.objects,
        report.bytes,
        if dry_run { " — nothing uploaded" } else { "" }
    );
    Ok(())
}

fn run_guard(args: &[String]) -> Result<(), String> {
    let (flags, _) = Flags::parse(args, &[])?;
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
