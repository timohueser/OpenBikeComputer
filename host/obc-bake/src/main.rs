//! `obc-bake` CLI — flags in, [`obc_bake`] out.
//!
//! ```text
//! obc-bake regions [--regions FILE]
//! obc-bake bake --out TREE [--regions FILE] [--presets-dir DIR] [--preset ID]… [--region ID]…
//!               [--source BASE] [--cache DIR] [--force] [--no-land] [--fail-fast]
//!               [--summary-json FILE]
//! obc-bake publish TREE --base-url URL [--target dir:PATH|r2] [--generated-at TS] [--dry-run]
//! obc-bake check-obcm-version [--catalog-url URL]
//! ```
//!
//! `bake` and `publish` are separate commands on purpose: a bake is hours and may
//! be resumed, re-run, or done on a different machine from the one holding the
//! credentials. The tree in between is the interface, and it is exactly the tree
//! `obc-pack catalog` walks.

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
      Bake (region × preset) into a tree `obc-pack catalog` accepts.
        --regions FILE       region list (default: the built-in curated list)
        --presets-dir DIR    style presets (default: builder/presets)
        --preset ID          bake only this preset (repeatable)
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
        --target TARGET      `dir:PATH` (default: dry run) or `r2` (env credentials)
        --generated-at TS    pin the manifest's generated_at (RFC 3339 UTC)
        --dry-run            generate + plan, upload nothing
        --allow-shrink       permit dropping coverage the live catalog serves
                             (refused by default — publishing a partial bake over a
                             full one silently un-offers regions)

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
        let presets = region.presets.as_ref().map_or_else(|| "all presets".to_string(), |p| p.join(", "));
        println!("{:<44} {:<28} {presets}", region.id, region.name);
    }
    println!("\n{} regions", regions.len());
    Ok(())
}

fn run_bake(args: &[String]) -> Result<(), String> {
    let (flags, _) = Flags::parse(args, &["force", "no-land", "fail-fast"])?;
    let out = PathBuf::from(flags.get("out").ok_or_else(|| format!("bake needs --out TREE\n\n{USAGE}"))?);

    let all_regions = obc_bake::regions::load(flags.get("regions").map(Path::new))?;
    let wanted = flags.all("region");
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
    let only = flags.all("preset");
    let presets = obc_bake::presets::load(&presets_dir, (!only.is_empty()).then_some(&only))?;

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

fn run_publish(args: &[String]) -> Result<(), String> {
    let (flags, positional) = Flags::parse(args, &["dry-run", "allow-shrink"])?;
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

    let opts = CatalogOptions {
        base_url: base_url.to_string(),
        generated_at: flags.get("generated-at").map_or_else(obc_pack::catalog::now_timestamp, str::to_string),
    };
    println!("publishing {tree} → {}{}", store.describe(), if dry_run { " (dry run)" } else { "" });
    let publish_opts = PublishOptions { dry_run, allow_shrink: flags.has("allow-shrink") };
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
