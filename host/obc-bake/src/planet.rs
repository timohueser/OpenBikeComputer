//! Whole-planet source preparation for `obc bake --all`.
//!
//! The packer intentionally retains the styled content it is about to cut. A
//! planet PBF therefore cannot be handed to it directly. This module uses Osmium's
//! reference-complete `smart` extraction in a binary hierarchy and yields
//! grid-aligned leaves; the bakery ingests one leaf at a time. The planet is the
//! `osm-planet` pin from the store with the daily diffs of `osm-replication`
//! applied, and only leaves whose canonical extracted bytes changed are
//! re-ingested.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_pack::catalog::CellSource;
use obc_pack::cut::{CellArtifact, CutOptions, SourceExtent};
use obc_pack::grid::{BandTable, CellId, UBox, GRID_ORIGIN};
use obc_pack::progress::Progress;
use serde::{Deserialize, Serialize};

use crate::cell_store::{paths as cell_paths, read_current as read_cell_state, CellSidecar, CellState};
use crate::cells::{CellBakeOptions, CellCutter};
use crate::coverage::Coverage;
use crate::known_empty::{EmptyChange, EmptyFact, KnownEmptyIndex};
use crate::presets::StyleDoc;
use crate::regions::Region;
use crate::source::ExtractSource;
use crate::util::{human_bytes, write_json};

/// An 8.39° leaf contains at most 32×32 fine cells. Dense leaves remain practical
/// for the retained Rust ingest while the source shard count stays below one
/// thousand. Every shipped band divides this size exactly.
pub const SOURCE_LEAF_LOG2: u32 = 23;
const SHARD_STATE_VERSION: u32 = 1;
const SHARD_HALO_UDEG: i64 = 1;
const PLANET_STATUS_FILE: &str = ".planet-bake/status.json";
/// One week of daily diffs: `osmium apply-changes` holds the diffs of one pass in memory.
const DIFFS_PER_PASS: usize = 7;

#[derive(Debug, Clone)]
pub struct PlanetInput {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub snapshot: String,
    pub replication: Option<Replication>,
}

/// The daily diffs applied to the planet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Replication {
    /// The `osm-planet` version, the day the diffs start at.
    pub from: String,
    /// The `osm-replication` version, the day of the last diff.
    pub to: String,
    pub diffs: usize,
}

/// Fail before any download when the planet of `--all` comes from the store and `osm-planet` has
/// no pin.
pub fn check_pinned(spec: Option<&str>) -> Result<(), String> {
    if spec.is_some() || obc_data::sources::Registry::live()?.pins.contains_key("osm-planet") {
        return Ok(());
    }
    Err("`osm-planet` has no pin in data/env/live.toml: pin it with `obc data refresh osm-planet --env live`".into())
}

/// The lock of `<cache>/planet`. A run holds it while it updates and reads the planet there, so a
/// second run waits instead of deleting a file in use.
pub fn lock_cache(cache: &Path, progress: &Progress) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let path = cache.join("planet.lock");
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path);
    let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => return Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => progress.log(format!("waiting for {}", path.display())),
        Err(std::fs::TryLockError::Error(e)) => return Err(format!("lock {}: {e}", path.display())),
    }
    file.lock().map_err(|e| format!("lock {}: {e}", path.display()))?;
    Ok(file)
}

/// The planet of `--all`: the PBF file that `spec` names, or else the `osm-planet` pin from the
/// store with the `osm-replication` diffs applied. The caller holds [`lock_cache`].
pub fn resolve_planet(
    spec: Option<&str>,
    cache: &Path,
    osmium: &OsmiumRunner,
    progress: &Progress,
) -> Result<PlanetInput, String> {
    let (path, snapshot, replication) = match spec {
        None => {
            let planet = obc_data::fetch::live("osm-planet", None, Vec::new())?;
            let from = planet.snapshot.version.clone();
            let diffs = obc_data::fetch::live("osm-replication", None, vec![("from".into(), from.clone())])?;
            update(&planet.paths[0], &from, &diffs, &cache.join("planet"), osmium, progress)?
        }
        Some(spec) if spec.starts_with("http://") || spec.starts_with("https://") => {
            return Err(format!(
                "--source {spec}: the planet comes from the store, as `osm-planet` and `osm-replication` in \
                 data/sources.toml; --source takes a planet PBF file"
            ))
        }
        Some(spec) => {
            let path = PathBuf::from(spec.strip_prefix("file://").unwrap_or(spec));
            if !path.is_file() {
                return Err(format!("planet source {} is not a file", path.display()));
            }
            let snapshot = local_snapshot(&path)?;
            (path, snapshot, None)
        }
    };

    progress.log(format!("Hashing planet source {}...", path.display()));
    let (bytes, sha256) = crate::hash::file(&path)?;
    progress.log(format!("  planet source: {} ({snapshot}, sha256 {}…)", human_bytes(bytes), &sha256[..12]));
    Ok(PlanetInput { path, bytes, sha256, snapshot, replication })
}

/// `planet`, the planet of the day `from`, with the diffs of `replication` applied, as
/// `<dir>/planet-<from>+<to>.osm.pbf`. An earlier result from the same planet is the start, so
/// only the newer diffs apply. Every other file in `dir` goes, because a planet is about 80 GB.
fn update(
    planet: &Path,
    from: &str,
    replication: &obc_data::fetch::Fetched,
    dir: &Path,
    osmium: &OsmiumRunner,
    progress: &Progress,
) -> Result<(PathBuf, String, Option<Replication>), String> {
    // The record is the state that the diffs start from, then the diff and the state of each day.
    let (mut start, mut diffs, mut diff) = (None, Vec::new(), None);
    for (file, path) in replication.snapshot.files.iter().zip(&replication.paths) {
        if file.name.ends_with(".osc.gz") {
            diff = Some(path.as_path());
            continue;
        }
        let timestamp = state_timestamp(path)?;
        match diff.take() {
            Some(diff) => diffs.push((diff, timestamp[..10].to_string())),
            None => start = Some(timestamp),
        }
    }
    if diffs.is_empty() {
        return Ok((planet.to_path_buf(), from.to_string(), None));
    }
    let start = start.ok_or("the `osm-replication` record has no state that its diffs start from")?;
    check_gap(from, &osmium.replication_timestamp(planet)?, &start)?;
    let to = replication.snapshot.version.clone();
    let path = dir.join(format!("planet-{from}+{to}.osm.pbf"));
    let applied = Replication { from: from.to_string(), to: to.clone(), diffs: diffs.len() };
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        files.push(entry.map_err(|e| format!("{}: {e}", dir.display()))?.path());
    }
    let plan = plan(&files, from, &to, &diffs);
    for file in &plan.stale {
        std::fs::remove_file(file).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    match plan.start {
        Some((_, day)) if day == to => progress.log(format!("Reusing the planet of {from} with the diffs up to {to}")),
        Some((base, day)) => {
            progress.log(format!("Updating the planet of {from} with the diffs up to {day}"));
            osmium.apply_changes(base, true, &plan.diffs, &path, progress)?;
        }
        None => osmium.apply_changes(planet, false, &plan.diffs, &path, progress)?,
    }
    Ok((path, to, Some(applied)))
}

/// Refuse a planet that ends, at `end`, before the state that the first diff starts from: the
/// changes between would be missing. Overlap is fine, because a diff applies over newer data.
fn check_gap(from: &str, end: &str, start: &str) -> Result<(), String> {
    if end >= start {
        return Ok(());
    }
    Err(format!(
        "invalid data: the `osm-planet` planet of {from} ends at {end}, but the `osm-replication` diffs \
         start at {start}, so the changes between are missing. Pin a planet that ends at or after the \
         start of its daily diff: `obc data refresh osm-planet --env live`"
    ))
}

/// What an update to `planet-<from>+<to>.osm.pbf` does with the `files` of its folder.
#[derive(Debug, PartialEq, Eq)]
struct Plan<'a> {
    /// The newest result from the same planet up to `to`, and its day; `None` starts from the
    /// planet. A start of the day `to` is the result itself.
    start: Option<(&'a Path, &'a str)>,
    /// The diffs after the day of the start.
    diffs: Vec<&'a Path>,
    /// Every other file: older or newer results, results of another planet and `.part` files.
    stale: Vec<&'a Path>,
}

/// `diffs` holds each diff with the day of its state, in order.
fn plan<'a>(files: &'a [PathBuf], from: &str, to: &str, diffs: &[(&'a Path, String)]) -> Plan<'a> {
    let prefix = format!("planet-{from}+");
    let day_of = |path: &'a Path| -> Option<&'a str> {
        let day = path.file_name()?.to_str()?.strip_prefix(prefix.as_str())?.strip_suffix(".osm.pbf")?;
        (day <= to).then_some(day)
    };
    let start = files.iter().filter_map(|path| Some((path.as_path(), day_of(path)?))).max_by_key(|(_, day)| *day);
    let stale = files.iter().map(PathBuf::as_path).filter(|path| start.is_none_or(|(kept, _)| path != &kept)).collect();
    let after = start.map_or("", |(_, day)| day);
    let diffs = diffs.iter().filter(|(_, day)| day.as_str() > after).map(|(diff, _)| *diff).collect();
    Plan { start, diffs, stale }
}

/// The `timestamp` of an Osmosis replication `state.txt`, which escapes each `:`.
fn state_timestamp(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let timestamp = text.lines().find_map(|line| line.strip_prefix("timestamp="));
    let timestamp = timestamp.map(|t| t.trim().replace('\\', "")).filter(|t| t.len() >= 10);
    timestamp.ok_or_else(|| format!("{}: a replication state with no timestamp", path.display()))
}

fn local_snapshot(path: &Path) -> Result<String, String> {
    let binary = std::env::var_os("OBC_OSMIUM").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("osmium"));
    if let Ok(output) =
        Command::new(binary).args(["fileinfo", "-g", "header.option.osmosis_replication_timestamp"]).arg(path).output()
    {
        if output.status.success() {
            let timestamp = String::from_utf8_lossy(&output.stdout);
            let timestamp = timestamp.trim();
            if timestamp.len() >= 10 && obc_pack::catalog::validate_date(&timestamp[..10]).is_ok() {
                return Ok(timestamp[..10].to_string());
            }
        }
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .ok_or_else(|| format!("{}: no modification time to date the planet source", path.display()))?;
    Ok(obc_pack::catalog::format_timestamp(secs)[..10].to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LeafId {
    pub i: i64,
    pub j: i64,
}

impl LeafId {
    pub fn cell(self) -> CellId {
        CellId::new(SOURCE_LEAF_LOG2, self.i, self.j).expect("planned leaf is in the grid")
    }

    /// The box that Osmium extracts the leaf with: its square and a halo.
    pub fn extract_bbox(self) -> [f64; 4] {
        LeafRect { i0: self.i, i1: self.i + 1, j0: self.j, j1: self.j + 1 }.extract_bbox()
    }
}

#[derive(Debug, Clone)]
pub struct PlanetLeaf {
    pub id: LeafId,
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub logical_bbox: UBox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LeafRect {
    i0: i64,
    i1: i64,
    j0: i64,
    j1: i64,
}

impl LeafRect {
    fn root() -> Self {
        let south_west = CellId::containing(SOURCE_LEAF_LOG2, -90_000_000, -180_000_000);
        let north_east = CellId::containing(SOURCE_LEAF_LOG2, 90_000_000, 180_000_000);
        LeafRect { i0: south_west.i, i1: north_east.i + 1, j0: south_west.j, j1: north_east.j + 1 }
    }

    fn is_leaf(self) -> bool {
        self.i1 - self.i0 == 1 && self.j1 - self.j0 == 1
    }

    fn split(self) -> [Self; 2] {
        let rows = self.i1 - self.i0;
        let cols = self.j1 - self.j0;
        if cols >= rows && cols > 1 {
            let mid = self.j0 + cols / 2;
            [LeafRect { j1: mid, ..self }, LeafRect { j0: mid, ..self }]
        } else {
            let mid = self.i0 + rows / 2;
            [LeafRect { i1: mid, ..self }, LeafRect { i0: mid, ..self }]
        }
    }

    fn leaves(self, out: &mut Vec<LeafId>) {
        for i in self.i0..self.i1 {
            for j in self.j0..self.j1 {
                out.push(LeafId { i, j });
            }
        }
    }

    fn bbox(self) -> UBox {
        let size = 1i64 << SOURCE_LEAF_LOG2;
        (
            GRID_ORIGIN + self.j0 * size,
            GRID_ORIGIN + self.i0 * size,
            GRID_ORIGIN + self.j1 * size,
            GRID_ORIGIN + self.i1 * size,
        )
    }

    fn extract_bbox(self) -> [f64; 4] {
        let (w, s, e, n) = self.bbox();
        [
            ((w - SHARD_HALO_UDEG).max(-180_000_000) as f64) / 1e6,
            ((s - SHARD_HALO_UDEG).max(-90_000_000) as f64) / 1e6,
            ((e + SHARD_HALO_UDEG).min(180_000_000) as f64) / 1e6,
            ((n + SHARD_HALO_UDEG).min(90_000_000) as f64) / 1e6,
        ]
    }

    fn slug(self) -> String {
        format!("{}-{}_{}-{}", self.i0, self.i1, self.j0, self.j1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeafState {
    state_version: u32,
    source_sha256: String,
    leaf_sha256: String,
    bytes: u64,
    logical_bbox: [i64; 4],
}

#[derive(Debug, Clone)]
pub struct ExtractRequest {
    pub output: String,
    pub bbox: [f64; 4],
}

/// Injectable because CI must prove the hierarchy without downloading or
/// requiring Osmium. Production uses [`OsmiumRunner`].
pub trait ShardRunner: Sync {
    fn check(&self) -> Result<(), String>;
    fn split(
        &self,
        input: &Path,
        output_dir: &Path,
        requests: &[ExtractRequest],
        progress: &Progress,
    ) -> Result<(), String>;
}

pub struct OsmiumRunner {
    binary: PathBuf,
}

impl Default for OsmiumRunner {
    fn default() -> Self {
        Self { binary: std::env::var_os("OBC_OSMIUM").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("osmium")) }
    }
}

impl ShardRunner for OsmiumRunner {
    fn check(&self) -> Result<(), String> {
        let out = Command::new(&self.binary)
            .arg("--version")
            .output()
            .map_err(|e| format!("{} is required for `obc bake --all`: {e}", self.binary.display()))?;
        if !out.status.success() {
            return Err(format!("{} --version failed with {}", self.binary.display(), out.status));
        }
        Ok(())
    }

    fn split(
        &self,
        input: &Path,
        output_dir: &Path,
        requests: &[ExtractRequest],
        _progress: &Progress,
    ) -> Result<(), String> {
        #[derive(Serialize)]
        struct Config<'a> {
            extracts: Vec<ConfigExtract<'a>>,
        }
        #[derive(Serialize)]
        struct ConfigExtract<'a> {
            output: &'a str,
            bbox: [f64; 4],
        }
        std::fs::create_dir_all(output_dir).map_err(|e| format!("{}: {e}", output_dir.display()))?;
        let config =
            Config { extracts: requests.iter().map(|r| ConfigExtract { output: &r.output, bbox: r.bbox }).collect() };
        let config_path = output_dir.join("extracts.json");
        write_json(&config_path, &config)?;
        // `-F pbf`: a planet from the store is an object without a file extension.
        let status = Command::new(&self.binary)
            .args(["extract", "-F", "pbf", "--config"])
            .arg(&config_path)
            .args(["--directory"])
            .arg(output_dir)
            .args(["--strategy", "smart", "--set-bounds", "--overwrite", "--verbose"])
            .arg(input)
            .status()
            .map_err(|e| format!("run {} extract: {e}", self.binary.display()))?;
        if !status.success() {
            return Err(format!("{} extract failed with {status}", self.binary.display()));
        }
        for request in requests {
            let path = output_dir.join(&request.output);
            if !path.is_file() {
                return Err(format!("{} did not produce {}", self.binary.display(), path.display()));
            }
        }
        Ok(())
    }
}

impl OsmiumRunner {
    /// The first line of `osmium --version`.
    pub fn version(&self) -> Result<String, String> {
        let out = Command::new(&self.binary).arg("--version").output();
        let out = out.map_err(|e| format!("{} --version: {e}", self.binary.display()))?;
        if !out.status.success() {
            return Err(format!("{} --version failed with {}", self.binary.display(), out.status));
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string())
    }

    /// Write `base` with `diffs` applied to `out`, in passes of [`DIFFS_PER_PASS`] diffs. A pass
    /// writes a `.part` file, so `out` exists only when it is whole. With `delete_base`, `base`
    /// goes when the first pass is done.
    pub fn apply_changes(
        &self,
        base: &Path,
        delete_base: bool,
        diffs: &[&Path],
        out: &Path,
        progress: &Progress,
    ) -> Result<(), String> {
        let part = |pass: usize| {
            let mut name = out.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".{}.part", pass % 2));
            out.with_file_name(name)
        };
        let passes = || -> Result<(), String> {
            let mut input = base.to_path_buf();
            for (pass, batch) in diffs.chunks(DIFFS_PER_PASS).enumerate() {
                progress.check()?;
                let first = pass * DIFFS_PER_PASS + 1;
                progress.log(format!("Applying daily diffs {first}–{} of {}", first + batch.len() - 1, diffs.len()));
                let output = part(pass);
                let status = Command::new(&self.binary)
                    .args(["apply-changes", "-F", "pbf", "--change-file-format", "osc.gz", "-f", "pbf"])
                    .args(["--fsync", "--overwrite", "-o"])
                    .arg(&output)
                    .arg(&input)
                    .args(batch)
                    .status()
                    .map_err(|e| format!("run {} apply-changes: {e}", self.binary.display()))?;
                if !status.success() {
                    return Err(format!("{} apply-changes failed with {status}", self.binary.display()));
                }
                if input != base || delete_base {
                    std::fs::remove_file(&input).map_err(|e| format!("{}: {e}", input.display()))?;
                }
                input = output;
            }
            std::fs::rename(&input, out).map_err(|e| format!("install {}: {e}", out.display()))
        };
        let result = passes();
        if result.is_err() {
            let _ = std::fs::remove_file(part(0));
            let _ = std::fs::remove_file(part(1));
        }
        result
    }

    /// The `osmosis_replication_timestamp` in the header of a PBF: the time its data ends.
    fn replication_timestamp(&self, path: &Path) -> Result<String, String> {
        let output = Command::new(&self.binary)
            .args(["fileinfo", "-F", "pbf", "-g", "header.option.osmosis_replication_timestamp"])
            .arg(path)
            .output()
            .map_err(|e| format!("run {} fileinfo: {e}", self.binary.display()))?;
        let timestamp = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || timestamp.is_empty() {
            return Err(format!("{}: no osmosis_replication_timestamp in its header", path.display()));
        }
        Ok(timestamp)
    }
}

pub struct PlanetSharder<'a> {
    pub input: &'a PlanetInput,
    pub cache: &'a Path,
    pub runner: &'a dyn ShardRunner,
}

pub struct PlanetShardRun {
    pub leaves: Vec<PlanetLeaf>,
    pub reused: usize,
    pub refreshed: usize,
    pub changed: usize,
}

#[derive(Default)]
struct ShardChanges {
    refreshed: usize,
    changed: usize,
}

struct ShardProgress<'a> {
    previous: &'a BTreeMap<LeafId, PlanetLeaf>,
    current: BTreeSet<LeafId>,
    changes: ShardChanges,
}

impl PlanetSharder<'_> {
    pub fn run(&self, progress: &Progress) -> Result<PlanetShardRun, String> {
        self.runner.check()?;
        let root = LeafRect::root();
        let mut expected = Vec::new();
        root.leaves(&mut expected);
        let mut previous = BTreeMap::new();
        let mut current = BTreeSet::new();
        for id in &expected {
            if let Some((leaf, state)) = self.read_stored(*id, true)? {
                if state.source_sha256 == self.input.sha256 {
                    current.insert(*id);
                }
                previous.insert(*id, leaf);
            }
        }
        let current_at_start = current.len();
        let mut shard_progress = ShardProgress { previous: &previous, current, changes: ShardChanges::default() };
        progress.log(format!(
            "planet source shards: {} current, {} to compare/create (2^{} µdeg leaves)",
            shard_progress.current.len(),
            expected.len() - shard_progress.current.len(),
            SOURCE_LEAF_LOG2
        ));
        if shard_progress.current.len() != expected.len() {
            let work = self.cache.join("planet-shards/.work");
            std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
            self.ensure(root, &self.input.path, false, &mut shard_progress, progress)?;
        }

        let leaves = expected
            .into_iter()
            .map(|id| {
                self.read_current(id, false)?
                    .ok_or_else(|| format!("planet source leaf {}/{} is missing after sharding", id.i, id.j))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if current_at_start + shard_progress.changes.refreshed + shard_progress.changes.changed != leaves.len() {
            return Err(format!(
                "planet source-shard outcome mismatch: {} current + {} refreshed + {} changed != {} leaves",
                current_at_start,
                shard_progress.changes.refreshed,
                shard_progress.changes.changed,
                leaves.len()
            ));
        }
        Ok(PlanetShardRun {
            reused: current_at_start,
            refreshed: shard_progress.changes.refreshed,
            changed: shard_progress.changes.changed,
            leaves,
        })
    }

    fn ensure(
        &self,
        rect: LeafRect,
        input: &Path,
        input_owned: bool,
        shard_progress: &mut ShardProgress<'_>,
        progress: &Progress,
    ) -> Result<(), String> {
        let mut ids = Vec::new();
        rect.leaves(&mut ids);
        if ids.iter().all(|id| shard_progress.current.contains(id)) {
            if input_owned {
                let _ = std::fs::remove_file(input);
            }
            return Ok(());
        }
        if rect.is_leaf() {
            let id = ids[0];
            self.install_leaf(
                id,
                rect,
                input,
                input_owned,
                shard_progress.previous.get(&id),
                &mut shard_progress.changes,
            )?;
            shard_progress.current.insert(id);
            return Ok(());
        }

        let children = rect.split();
        let needed: Vec<LeafRect> = children
            .into_iter()
            .filter(|child| {
                let mut leaves = Vec::new();
                child.leaves(&mut leaves);
                leaves.iter().any(|id| !shard_progress.current.contains(id))
            })
            .collect();
        let dir = self.cache.join("planet-shards/.work").join(rect.slug());
        let _ = std::fs::remove_dir_all(&dir);
        let requests: Vec<ExtractRequest> = needed
            .iter()
            .enumerate()
            .map(|(k, child)| ExtractRequest { output: format!("child-{k}.osm.pbf"), bbox: child.extract_bbox() })
            .collect();
        progress.log(format!(
            "  sharding {} into {} child extract(s) ({} leaf/leaves remain)",
            rect.slug(),
            requests.len(),
            ids.iter().filter(|id| !shard_progress.current.contains(id)).count()
        ));
        self.runner.split(input, &dir, &requests, progress)?;
        for (k, child) in needed.iter().enumerate() {
            self.ensure(*child, &dir.join(format!("child-{k}.osm.pbf")), true, shard_progress, progress)?;
        }
        let _ = std::fs::remove_dir_all(&dir);
        if input_owned {
            let _ = std::fs::remove_file(input);
        }
        Ok(())
    }

    fn install_leaf(
        &self,
        id: LeafId,
        rect: LeafRect,
        input: &Path,
        input_owned: bool,
        previous: Option<&PlanetLeaf>,
        changes: &mut ShardChanges,
    ) -> Result<(), String> {
        let path = self.leaf_path(id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let (bytes, leaf_sha256) = crate::hash::file(input)?;
        let unchanged = previous.is_some_and(|leaf| leaf.bytes == bytes && leaf.sha256 == leaf_sha256);
        if unchanged {
            if input_owned {
                let _ = std::fs::remove_file(input);
            }
            changes.refreshed += 1;
        } else {
            if input_owned {
                std::fs::rename(input, &path).map_err(|e| format!("{} -> {}: {e}", input.display(), path.display()))?;
            } else {
                std::fs::copy(input, &path).map_err(|e| format!("{} -> {}: {e}", input.display(), path.display()))?;
            }
            changes.changed += 1;
        }
        let bbox = rect.bbox();
        let state = LeafState {
            state_version: SHARD_STATE_VERSION,
            source_sha256: self.input.sha256.clone(),
            leaf_sha256,
            bytes,
            logical_bbox: [bbox.0, bbox.1, bbox.2, bbox.3],
        };
        write_json(&self.leaf_state_path(id), &state)
    }

    fn read_current(&self, id: LeafId, verify_hash: bool) -> Result<Option<PlanetLeaf>, String> {
        Ok(self
            .read_stored(id, verify_hash)?
            .filter(|(_, state)| state.source_sha256 == self.input.sha256)
            .map(|(leaf, _)| leaf))
    }

    fn read_stored(&self, id: LeafId, verify_hash: bool) -> Result<Option<(PlanetLeaf, LeafState)>, String> {
        let path = self.leaf_path(id);
        let state_path = self.leaf_state_path(id);
        let Ok(text) = std::fs::read_to_string(&state_path) else { return Ok(None) };
        let Ok(state) = serde_json::from_str::<LeafState>(&text) else { return Ok(None) };
        if state.state_version != SHARD_STATE_VERSION || !path.is_file() {
            return Ok(None);
        }
        let expected_bbox = id.cell().square();
        if state.logical_bbox != [expected_bbox.0, expected_bbox.1, expected_bbox.2, expected_bbox.3] {
            return Ok(None);
        }
        let bytes = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if bytes != state.bytes {
            return Ok(None);
        }
        if verify_hash {
            let (_, hash) = crate::hash::file(&path)?;
            if hash != state.leaf_sha256 {
                return Ok(None);
            }
        }
        Ok(Some((
            PlanetLeaf {
                id,
                path,
                bytes,
                sha256: state.leaf_sha256.clone(),
                logical_bbox: (
                    state.logical_bbox[0],
                    state.logical_bbox[1],
                    state.logical_bbox[2],
                    state.logical_bbox[3],
                ),
            },
            state,
        )))
    }

    fn leaf_path(&self, id: LeafId) -> PathBuf {
        self.cache.join("planet-shards/leaves").join(id.i.to_string()).join(format!("{}.osm.pbf", id.j))
    }

    fn leaf_state_path(&self, id: LeafId) -> PathBuf {
        self.cache.join("planet-shards/leaves").join(id.i.to_string()).join(format!("{}.json", id.j))
    }
}

const PLANET_BAKE_STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct PlanetRunSummary {
    pub tree: PathBuf,
    pub source: PathBuf,
    pub source_snapshot: String,
    pub source_bytes: u64,
    pub replication: Option<Replication>,
    pub leaves: usize,
    pub source_leaves_reused: usize,
    pub source_leaves_refreshed: usize,
    pub source_leaves_changed: usize,
    pub leaves_cut: usize,
    pub leaves_unchanged: usize,
    pub leaves_refreshed: usize,
    pub artifacts_cut: usize,
    pub known_empty_cut: usize,
    pub bytes_written: u64,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlanetRegionPreset {
    region: Region,
    poly: String,
    cells: BTreeMap<String, Vec<String>>,
}

/// Resolve every small selection polygon before the planet download or any
/// long-running sharding. A typo or missing upstream polygon must fail in seconds,
/// not at the end of a multi-day bake.
pub fn resolve_region_presets(
    regions: &[Region],
    source: &dyn ExtractSource,
    bands: &BandTable,
    progress: &Progress,
) -> Result<Vec<PlanetRegionPreset>, String> {
    let sizes: BTreeSet<u32> = bands.bands.iter().map(|band| band.cell_log2).collect();
    let mut out = Vec::new();
    for region in regions {
        progress.log(format!("\n--- resolving region preset {} ({}) ---", region.id, region.name));
        let poly = source.fetch_poly(region, progress)?;
        let coverage = Coverage::parse_poly(&poly).map_err(|error| format!("{}.poly: {error}", region.id))?;
        let by_size: BTreeMap<u32, Vec<String>> = sizes
            .iter()
            .map(|log2| (*log2, coverage.cells(*log2).into_iter().map(|cell| cell.to_string()).collect()))
            .collect();
        let cells = bands
            .bands
            .iter()
            .map(|band| (band.id.clone(), by_size.get(&band.cell_log2).cloned().unwrap_or_default()))
            .collect();
        out.push(PlanetRegionPreset { region: region.clone(), poly, cells });
    }
    Ok(out)
}

impl PlanetRunSummary {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(out, "\n=== planet bake summary ({}) ===", self.tree.display());
        let _ = writeln!(
            out,
            "source {} ({}; {})",
            self.source.display(),
            self.source_snapshot,
            human_bytes(self.source_bytes)
        );
        if let Some(replication) = &self.replication {
            let _ = writeln!(
                out,
                "replication: {} → {}, {} daily diff(s) applied",
                replication.from, replication.to, replication.diffs
            );
        }
        let _ = writeln!(
            out,
            "source shards: {} current, {} byte-identical, {} changed/new",
            self.source_leaves_reused, self.source_leaves_refreshed, self.source_leaves_changed
        );
        let _ = writeln!(
            out,
            "{} leaves: {} cut, {} metadata-only, {} unchanged",
            self.leaves, self.leaves_cut, self.leaves_refreshed, self.leaves_unchanged
        );
        let _ = writeln!(
            out,
            "{} artifact cell(s), {} known-empty cell(s), {} written",
            self.artifacts_cut,
            self.known_empty_cut,
            human_bytes(self.bytes_written)
        );
        for failure in &self.failures {
            let _ = writeln!(out, "  FAILED: {failure}");
        }
        out
    }
}

pub struct PlanetBake<'a> {
    pub input: &'a PlanetInput,
    pub leaves: &'a [PlanetLeaf],
    pub regions: &'a [PlanetRegionPreset],
    pub schema: &'a StyleDoc,
    pub skins: &'a [&'a StyleDoc],
    pub cutter: &'a dyn CellCutter,
    pub source_leaves_reused: usize,
    pub source_leaves_refreshed: usize,
    pub source_leaves_changed: usize,
    pub opts: CellBakeOptions,
}

impl PlanetBake<'_> {
    pub fn run(&self, progress: &Progress) -> Result<PlanetRunSummary, String> {
        self.validate_planet_leaves()?;
        self.validate_configuration()?;
        self.write_status(false)?;
        let summary = self.run_inner(progress)?;
        if summary.ok() {
            self.write_status(true)?;
        }
        Ok(summary)
    }

    fn run_inner(&self, progress: &Progress) -> Result<PlanetRunSummary, String> {
        crate::cells::write_schema_and_skins(&self.opts.out, self.schema, self.skins, &self.opts)?;
        let mut empties = KnownEmptyIndex::load(&self.opts.out, &self.opts.bands, self.opts.schema_revision)?;
        let mut summary = PlanetRunSummary {
            tree: self.opts.out.clone(),
            source: self.input.path.clone(),
            source_snapshot: self.input.snapshot.clone(),
            source_bytes: self.input.bytes,
            replication: self.input.replication.clone(),
            leaves: self.leaves.len(),
            source_leaves_reused: self.source_leaves_reused,
            source_leaves_refreshed: self.source_leaves_refreshed,
            source_leaves_changed: self.source_leaves_changed,
            leaves_cut: 0,
            leaves_unchanged: 0,
            leaves_refreshed: 0,
            artifacts_cut: 0,
            known_empty_cut: 0,
            bytes_written: 0,
            failures: Vec::new(),
        };

        progress.log(format!(
            "planet bakery: {} source leaves, schema `{}`; one leaf is ingested at a time",
            self.leaves.len(),
            self.opts.schema_id
        ));
        let peak_key = obc_pack::peak_map::fingerprint(&self.opts.peaks.clone().into_iter().collect::<Vec<_>>())?;
        // Each region's compiled landmark content in this tree, keyed once: the key reads the
        // content and verifies every photo it declares, and a leaf asks for it per cut.
        let mut artifacts = Vec::new();
        for preset in self.regions {
            if let Some(path) = crate::landmarks::in_tree(&self.opts.out, &preset.region) {
                let key = obc_pack::landmark_map::fingerprint(std::slice::from_ref(&path))?;
                let selected: BTreeSet<&String> = preset.cells.values().flatten().collect();
                artifacts.push((selected, path, key));
            }
        }
        for (number, leaf) in self.leaves.iter().enumerate() {
            let cells = leaf_cells(leaf.id, &self.opts.bands);
            // The regions this leaf's ground lies in — a leaf that spans a border gets both.
            let ids: BTreeSet<String> = cells.values().flatten().map(CellId::to_string).collect();
            // Walk the leaf's cells, never the region's: a leaf is a bounded ~1300 cells and a
            // region's set is a country's, and most pairs of the two do not meet at all.
            let reaching: Vec<_> =
                artifacts.iter().filter(|(selected, _, _)| ids.iter().any(|c| selected.contains(c))).collect();
            let mut landmarks: Vec<PathBuf> = reaching.iter().map(|(_, path, _)| path.clone()).collect();
            landmarks.sort();
            let mut landmark_keys: Vec<&str> = reaching.iter().map(|(_, _, key)| key.as_str()).collect();
            landmark_keys.sort_unstable();
            let landmark_key = format!(
                "{}\npeaks={peak_key}",
                if landmark_keys.is_empty() { "none".to_string() } else { landmark_keys.join(",") }
            );
            let pack_key = self.pack_key(leaf, &landmark_key);
            progress.log(format!(
                "\n--- planet leaf {}/{} ({}/{}, {}, {} cell-band outputs) ---",
                number + 1,
                self.leaves.len(),
                leaf.id.i,
                leaf.id.j,
                human_bytes(leaf.bytes),
                cells.values().map(Vec::len).sum::<usize>()
            ));
            match self.reuse_leaf(leaf, &cells, &pack_key, &mut empties) {
                Ok(LeafReuse::Unchanged) if !self.opts.force => {
                    summary.leaves_unchanged += 1;
                    progress.log("    every cell current — not ingesting");
                    continue;
                }
                Ok(LeafReuse::Refreshed) if !self.opts.force => {
                    empties.write_all(&self.opts.out, self.opts.schema_revision)?;
                    self.write_leaf_state(leaf.id, &pack_key)?;
                    summary.leaves_refreshed += 1;
                    progress.log("    bytes current — refreshed planet snapshot metadata only");
                    continue;
                }
                Ok(_) => {}
                Err(error) => progress.warn(format!("    cached leaf state is stale: {error}")),
            }

            match self.cut_leaf(leaf, &cells, &pack_key, &landmarks, &mut empties, progress) {
                Ok(stats) => {
                    empties.write_all(&self.opts.out, self.opts.schema_revision)?;
                    self.write_leaf_state(leaf.id, &pack_key)?;
                    summary.leaves_cut += 1;
                    summary.artifacts_cut += stats.artifacts;
                    summary.known_empty_cut += stats.known_empty;
                    summary.bytes_written += stats.bytes;
                }
                Err(error) => {
                    let message = format!("leaf {}/{}: {error}", leaf.id.i, leaf.id.j);
                    progress.warn(format!("    FAILED: {message}"));
                    summary.failures.push(message);
                    if self.opts.fail_fast {
                        break;
                    }
                }
            }
        }

        self.write_regions(progress)?;
        Ok(summary)
    }

    fn validate_configuration(&self) -> Result<(), String> {
        self.opts.bands.validate(self.schema.config.lods.len())?;
        if self.opts.schema_revision == 0 {
            return Err("--schema-revision starts at 1 — a cell store has no revision zero".into());
        }
        for skin in self.skins {
            obc_pack::catalog::check_skin_document(&skin.json, &skin.path.display().to_string())?;
            obc_pack::catalog::check_skin(&self.schema.config, &skin.config).map_err(|error| {
                format!(
                    "skin `{}` ({}) is not a skin over schema `{}` ({}): {error}",
                    skin.id,
                    skin.path.display(),
                    self.opts.schema_id,
                    self.schema.path.display()
                )
            })?;
        }
        crate::cells::check_schema_id(self.schema, &self.opts)
    }

    fn validate_planet_leaves(&self) -> Result<(), String> {
        let mut expected = Vec::new();
        LeafRect::root().leaves(&mut expected);
        let actual: BTreeSet<LeafId> = self.leaves.iter().map(|leaf| leaf.id).collect();
        let expected: BTreeSet<LeafId> = expected.into_iter().collect();
        if actual != expected || actual.len() != self.leaves.len() {
            return Err(format!(
                "planet bakery received {} distinct leaf/leaves ({} raw), expected {} — refusing to mark partial \
                 coverage complete",
                actual.len(),
                self.leaves.len(),
                expected.len()
            ));
        }
        if self.source_leaves_reused + self.source_leaves_refreshed + self.source_leaves_changed != self.leaves.len() {
            return Err(format!(
                "planet source-shard summary says {} current + {} byte-identical + {} changed/new, but the bakery \
                 received {} leaves",
                self.source_leaves_reused,
                self.source_leaves_refreshed,
                self.source_leaves_changed,
                self.leaves.len()
            ));
        }
        for leaf in self.leaves {
            if leaf.logical_bbox != leaf.id.cell().square() {
                return Err(format!(
                    "planet leaf {}/{} has logical bbox {:?}, expected {:?}",
                    leaf.id.i,
                    leaf.id.j,
                    leaf.logical_bbox,
                    leaf.id.cell().square()
                ));
            }
        }
        Ok(())
    }

    fn write_status(&self, complete: bool) -> Result<(), String> {
        let bands = serde_json::to_string(&self.opts.bands).map_err(|error| error.to_string())?;
        write_json(
            &self.opts.out.join(PLANET_STATUS_FILE),
            &PlanetBakeStatus {
                state_version: PLANET_BAKE_STATE_VERSION,
                complete,
                source_sha256: self.input.sha256.clone(),
                source_snapshot: self.input.snapshot.clone(),
                schema_id: self.opts.schema_id.clone(),
                schema_revision: self.opts.schema_revision,
                schema_sha256: self.schema.body_sha256.clone(),
                bands_sha256: crate::hash::text(&bands),
                cutter_recipe: self.cutter.recipe(),
                leaves: self.leaves.len(),
            },
        )
    }

    fn pack_key(&self, leaf: &PlanetLeaf, landmark_key: &str) -> String {
        let bands = serde_json::to_string(&self.opts.bands).unwrap_or_default();
        // `terrain` is here for the same reason it is in the curated key: the nav graph's per-edge
        // ascent is integrated from that raster, so a terrain re-bake really does move these bytes.
        let terrain = self.opts.terrain.as_ref().map_or("none".to_string(), |t| t.revision.to_string());
        crate::hash::text(&format!(
            "planet-recipe={PLANET_BAKE_STATE_VERSION}\ncell-recipe={}\nobcm={}\ncutter={}\nschema={}\nrevision={}\nbands={bands}\nterrain={terrain}\nlandmarks={landmark_key}\nleaf={}\nextent={:?}\n",
            crate::cells::CELL_RECIPE_VERSION,
            obc_formats::obcm::VERSION,
            self.cutter.recipe(),
            self.schema.body_sha256,
            self.opts.schema_revision,
            leaf.sha256,
            leaf.logical_bbox,
        ))
    }

    fn reuse_leaf(
        &self,
        leaf: &PlanetLeaf,
        cells: &BTreeMap<String, Vec<CellId>>,
        pack_key: &str,
        empties: &mut KnownEmptyIndex,
    ) -> Result<LeafReuse, String> {
        let state_path = self.leaf_bake_state_path(leaf.id);
        let text = std::fs::read_to_string(&state_path).map_err(|_| "no leaf bake state".to_string())?;
        let state: LeafBakeState = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", state_path.display()))?;
        if state.state_version != PLANET_BAKE_STATE_VERSION || state.pack_key != pack_key {
            return Err("pack key changed".into());
        }
        let source = self.planet_source();
        let mut refresh = state.source_snapshot != self.input.snapshot;
        for (band, ids) in cells {
            for id in ids {
                let known_empty = empties.fact(band, *id);
                let artifact = read_cell_state(&self.opts.out, band, *id, pack_key)?;
                match (known_empty, artifact) {
                    (Some(_), Some(_)) => return Err(format!("{band} {id} is both an artifact and known-empty")),
                    (None, None) => return Err(format!("{band} {id} has no coverage state")),
                    (Some(fact), None) => refresh |= fact.sources != vec![source.clone()],
                    (None, Some(cell)) => refresh |= cell.sidecar.sources != vec![source.clone()],
                }
            }
        }
        if !refresh {
            return Ok(LeafReuse::Unchanged);
        }

        let mut changes: BTreeMap<String, Vec<EmptyChange>> = BTreeMap::new();
        for (band, ids) in cells {
            for id in ids {
                if let Some(fact) = empties.fact(band, *id).cloned() {
                    changes.entry(band.clone()).or_default().push(EmptyChange {
                        id: *id,
                        fact: Some(EmptyFact { built_at: fact.built_at, sources: vec![source.clone()] }),
                    });
                } else if let Some(mut cell) = read_cell_state(&self.opts.out, band, *id, pack_key)? {
                    cell.sidecar.sources = vec![source.clone()];
                    let (_, sidecar_path, state_path) = cell_paths(&self.opts.out, band, *id);
                    write_json(&sidecar_path, &cell.sidecar)?;
                    write_json(&state_path, &cell)?;
                }
            }
        }
        for (band, band_changes) in changes {
            empties.apply(&band, &band_changes)?;
        }
        Ok(LeafReuse::Refreshed)
    }

    fn cut_leaf(
        &self,
        leaf: &PlanetLeaf,
        cells: &BTreeMap<String, Vec<CellId>>,
        pack_key: &str,
        landmarks: &[PathBuf],
        empties: &mut KnownEmptyIndex,
        progress: &Progress,
    ) -> Result<LeafStats, String> {
        let tmp = self.opts.out.join(format!(".planet-cut-{}-{}", leaf.id.i, leaf.id.j));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut selected: Vec<CellId> = cells.values().flatten().copied().collect();
        selected.sort_unstable();
        selected.dedup();
        let source = self.planet_source();
        let opts = CutOptions {
            bands: self.opts.bands.clone(),
            select: selected,
            only_bands: Vec::new(),
            sources: vec![SourceExtent {
                id: source.extract_id.clone(),
                snapshot: Some(source.snapshot.clone()),
                coverage: None,
            }],
            chunk_size: None,
            no_land: false,
            land: None,
            // The terrain published in this tree, or nothing. A tree with no terrain writes
            // `Ascent M = 0`, which is a decode-valid map.
            terrain: self.opts.terrain.as_ref().map(|t| t.dir.clone()),
            landmarks: landmarks.to_vec(),
            peaks: self.opts.peaks.clone().into_iter().collect(),
            bbox: None,
            source_extent: Some(leaf.logical_bbox),
        };
        let pbfs = vec![leaf.path.to_string_lossy().into_owned()];
        let cut = self.cutter.cut(&pbfs, &self.schema.config, &tmp, &opts, progress).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&tmp);
        })?;
        let expected: BTreeSet<(String, CellId)> =
            cells.iter().flat_map(|(band, ids)| ids.iter().map(|id| (band.clone(), *id))).collect();
        let produced: BTreeSet<(String, CellId)> =
            cut.cells.iter().map(|artifact| (artifact.band.clone(), artifact.id)).collect();
        if produced != expected || cut.cells.len() != expected.len() {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!(
                "cutter returned {} distinct cell-band output(s) for {} expected ({} raw); refusing a planet tree \
                 with silent holes or duplicates",
                produced.len(),
                expected.len(),
                cut.cells.len()
            ));
        }

        let built_at = obc_pack::catalog::now_timestamp();
        let mut changes: BTreeMap<String, Vec<EmptyChange>> = BTreeMap::new();
        let mut stats = LeafStats::default();
        for artifact in &cut.cells {
            let fact = EmptyFact { built_at: built_at.clone(), sources: vec![source.clone()] };
            if artifact.empty {
                remove_artifact(&self.opts.out, &artifact.band, artifact.id)?;
                let _ = std::fs::remove_file(obc_pack::cut::artifact_path(&tmp, artifact));
                changes
                    .entry(artifact.band.clone())
                    .or_default()
                    .push(EmptyChange { id: artifact.id, fact: Some(fact) });
                stats.known_empty += 1;
            } else {
                install_artifact(
                    &self.opts.out,
                    &tmp,
                    artifact,
                    &fact,
                    pack_key,
                    self.opts.schema_revision,
                    self.opts.terrain.as_ref().map(|t| t.revision),
                )?;
                changes.entry(artifact.band.clone()).or_default().push(EmptyChange { id: artifact.id, fact: None });
                stats.artifacts += 1;
                stats.bytes += artifact.bytes;
            }
        }
        for (band, band_changes) in changes {
            empties.apply(&band, &band_changes)?;
        }
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(stats)
    }

    fn write_regions(&self, progress: &Progress) -> Result<(), String> {
        for preset in self.regions {
            progress.log(format!("\n--- writing region preset {} ({}) ---", preset.region.id, preset.region.name));
            crate::cells::write_region_selection(&self.opts.out, &preset.region, &preset.poly, preset.cells.clone())?;
        }
        Ok(())
    }

    fn planet_source(&self) -> CellSource {
        CellSource { extract_id: "planet".into(), snapshot: self.input.snapshot.clone() }
    }

    fn leaf_bake_state_path(&self, id: LeafId) -> PathBuf {
        self.opts.out.join(".planet-bake/leaves").join(id.i.to_string()).join(format!("{}.json", id.j))
    }

    fn write_leaf_state(&self, id: LeafId, pack_key: &str) -> Result<(), String> {
        write_json(
            &self.leaf_bake_state_path(id),
            &LeafBakeState {
                state_version: PLANET_BAKE_STATE_VERSION,
                pack_key: pack_key.to_string(),
                source_snapshot: self.input.snapshot.clone(),
            },
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeafBakeState {
    state_version: u32,
    pack_key: String,
    source_snapshot: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanetBakeStatus {
    state_version: u32,
    complete: bool,
    source_sha256: String,
    source_snapshot: String,
    schema_id: String,
    schema_revision: u32,
    schema_sha256: String,
    bands_sha256: String,
    cutter_recipe: String,
    leaves: usize,
}

/// Refuse to publish or verify a tree while a whole-planet bake is incomplete.
///
/// A catalog cannot infer global holes from the artifacts that happen to exist, so
/// planet mode keeps this local completion marker as the missing negative fact.
/// Curated-only trees have no marker and retain their existing behavior.
pub fn check_publishable_tree(tree: &Path) -> Result<(), String> {
    let state_dir = tree.join(".planet-bake");
    if !state_dir.exists() {
        return Ok(());
    }
    let path = tree.join(PLANET_STATUS_FILE);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        format!("{}: {error} — the planet bake has no completion record; rerun `obc bake --all`", path.display())
    })?;
    let status: PlanetBakeStatus =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if status.state_version != PLANET_BAKE_STATE_VERSION {
        return Err(format!(
            "{}: planet bake state version {} is not supported; rerun `obc bake --all`",
            path.display(),
            status.state_version
        ));
    }
    if !status.complete {
        return Err(format!(
            "{}: planet bake is incomplete; rerun `obc bake --all` before verify or publish",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeafReuse {
    Refreshed,
    Unchanged,
}

#[derive(Default)]
struct LeafStats {
    artifacts: usize,
    known_empty: usize,
    bytes: u64,
}

fn install_artifact(
    out: &Path,
    tmp: &Path,
    artifact: &CellArtifact,
    fact: &EmptyFact,
    pack_key: &str,
    schema_revision: u32,
    terrain_revision: Option<u32>,
) -> Result<(), String> {
    let src = obc_pack::cut::artifact_path(tmp, artifact);
    crate::verify::verify_cell(&src, artifact.id.square())?;
    let (dest, sidecar_path, state_path) = cell_paths(out, &artifact.band, artifact.id);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let sidecar = CellSidecar {
        schema_revision,
        built_at: fact.built_at.clone(),
        sources: fact.sources.clone(),
        partial: false,
        terrain_revision,
    };
    write_json(&sidecar_path, &sidecar)?;
    std::fs::rename(&src, &dest).map_err(|e| format!("{} -> {}: {e}", src.display(), dest.display()))?;
    write_json(
        &state_path,
        &CellState { pack_key: pack_key.to_string(), sha256: artifact.sha256.clone(), bytes: artifact.bytes, sidecar },
    )
}

fn remove_artifact(out: &Path, band: &str, id: CellId) -> Result<(), String> {
    let (artifact, sidecar, state) = cell_paths(out, band, id);
    for path in [artifact, sidecar, state] {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn leaf_cells(id: LeafId, bands: &BandTable) -> BTreeMap<String, Vec<CellId>> {
    bands
        .bands
        .iter()
        .map(|band| {
            let factor = 1i64 << (SOURCE_LEAF_LOG2 - band.cell_log2);
            let mut cells = Vec::with_capacity((factor * factor) as usize);
            for i in id.i * factor..(id.i + 1) * factor {
                for j in id.j * factor..(id.j + 1) * factor {
                    cells.push(CellId::new(band.cell_log2, i, j).expect("leaf child lies in world grid"));
                }
            }
            (band.id.clone(), cells)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn repo(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
    }

    #[test]
    fn planet_leaf_grid_covers_geography_and_aligns_to_every_band() {
        let root = LeafRect::root();
        let mut leaves = Vec::new();
        root.leaves(&mut leaves);
        assert!(leaves.len() > 900 && leaves.len() < 1100, "unexpected leaf count {}", leaves.len());
        for id in leaves {
            let bbox = id.cell().square();
            for band_log2 in [18, 19, 20] {
                let step = 1i64 << band_log2;
                assert_eq!((bbox.0 - GRID_ORIGIN) % step, 0);
                assert_eq!((bbox.1 - GRID_ORIGIN) % step, 0);
                assert_eq!((bbox.2 - GRID_ORIGIN) % step, 0);
                assert_eq!((bbox.3 - GRID_ORIGIN) % step, 0);
            }
        }
    }

    #[test]
    fn hierarchy_never_requests_more_than_two_extracts() {
        let mut stack = vec![LeafRect::root()];
        while let Some(rect) = stack.pop() {
            if rect.is_leaf() {
                continue;
            }
            let children = rect.split();
            assert_eq!(children.len(), 2);
            for child in children {
                assert!(child.i0 >= rect.i0 && child.i1 <= rect.i1);
                assert!(child.j0 >= rect.j0 && child.j1 <= rect.j1);
                stack.push(child);
            }
        }
    }

    struct FakeRunner {
        calls: Mutex<Vec<usize>>,
        mutated_leaf: Mutex<Option<[f64; 4]>>,
    }

    impl ShardRunner for FakeRunner {
        fn check(&self) -> Result<(), String> {
            Ok(())
        }

        fn split(
            &self,
            _input: &Path,
            output_dir: &Path,
            requests: &[ExtractRequest],
            _progress: &Progress,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(requests.len());
            let mutated_leaf = *self.mutated_leaf.lock().unwrap();
            std::fs::create_dir_all(output_dir).unwrap();
            for request in requests {
                let suffix = if mutated_leaf == Some(request.bbox) { " changed" } else { "" };
                std::fs::write(output_dir.join(&request.output), format!("{:?}{suffix}", request.bbox)).unwrap();
            }
            Ok(())
        }
    }

    #[test]
    fn installed_osmium_accepts_the_generated_binary_split_config() {
        let runner = OsmiumRunner::default();
        if runner.check().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("obc-osmium-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let requests = [
            ExtractRequest { output: "teningen.osm.pbf".into(), bbox: [7.0, 47.0, 8.5, 49.0] },
            ExtractRequest { output: "empty.osm.pbf".into(), bbox: [20.0, 20.0, 21.0, 21.0] },
        ];
        runner.split(&repo("builder/tests/corpus/data/tiny.osm.pbf"), &dir, &requests, &Progress::silent()).unwrap();
        assert!(requests.iter().all(|request| dir.join(&request.output).is_file()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_osmium_applies_diffs_in_passes_to_an_object_without_an_extension() {
        let runner = OsmiumRunner::default();
        if runner.check().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("obc-osmium-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let osmium = |args: &[&str]| {
            assert!(Command::new(&runner.binary).args(args).current_dir(&dir).status().unwrap().success())
        };
        let tiny = repo("builder/tests/corpus/data/tiny.osm.pbf");
        std::fs::copy(&tiny, dir.join("base")).unwrap();
        osmium(&["getid", tiny.to_str().unwrap(), "n1", "n2", "n3", "-o", "kept.osm.pbf"]);
        osmium(&["derive-changes", tiny.to_str().unwrap(), "kept.osm.pbf", "-o", "diff.osc.gz"]);
        // Store objects have no extension. Eight diffs take two passes.
        std::fs::rename(dir.join("diff.osc.gz"), dir.join("diff")).unwrap();
        let diff = dir.join("diff");
        let diffs = vec![diff.as_path(); DIFFS_PER_PASS + 1];
        std::fs::create_dir(dir.join("planet")).unwrap();
        let out = dir.join("planet/applied.osm.pbf");
        runner.apply_changes(&dir.join("base"), false, &diffs, &out, &Progress::silent()).unwrap();

        let opl = |path: &Path| {
            let output =
                Command::new(&runner.binary).args(["cat", "-F", "pbf", "-f", "opl"]).arg(path).output().unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        assert_eq!(opl(&out), opl(&dir.join("kept.osm.pbf")));
        let left: Vec<_> =
            std::fs::read_dir(dir.join("planet")).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(left, ["applied.osm.pbf"], "no pass leaves a part file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_starts_from_the_newest_result_of_the_same_planet() {
        let files = |names: &[&str]| names.iter().map(|name| PathBuf::from("planet").join(name)).collect::<Vec<_>>();
        let (d2, d3, d4) = (Path::new("d2"), Path::new("d3"), Path::new("d4"));
        let diffs = [(d2, "2026-08-02".to_string()), (d3, "2026-08-03".into()), (d4, "2026-08-04".into())];
        let path = |name: &str| PathBuf::from("planet").join(name);
        let run = |names: &[&str], to: &str| {
            let files = files(names);
            // The record of `to` holds the diffs up to `to`.
            let diffs: Vec<_> = diffs.iter().filter(|(_, day)| day.as_str() <= to).cloned().collect();
            let plan = plan(&files, "2026-08-01", to, &diffs);
            let start = plan.start.map(|(path, day)| (path.to_path_buf(), day.to_string()));
            let stale: Vec<PathBuf> = plan.stale.iter().map(|path| path.to_path_buf()).collect();
            (start, plan.diffs.clone().into_iter().map(Path::to_path_buf).collect::<Vec<_>>(), stale)
        };

        // No earlier result: every diff applies to the planet.
        assert_eq!(run(&[], "2026-08-04"), (None, vec![d2.into(), d3.into(), d4.into()], vec![]));
        // An earlier result is the start; only the newer diffs apply. A stale part, an older result
        // and a result of another planet go.
        let names =
            ["planet-2026-08-01+2026-08-02.osm.pbf", "planet-2026-08-01+2026-08-03.osm.pbf", "x.osm.pbf.0.part"];
        let other = "planet-2026-07-25+2026-08-03.osm.pbf";
        assert_eq!(
            run(&[names[0], names[1], names[2], other], "2026-08-04"),
            (
                Some((path(names[1]), "2026-08-03".into())),
                vec![d4.into()],
                vec![path(names[0]), path(names[2]), path(other)]
            )
        );
        // The result of `to` is there: nothing applies.
        assert_eq!(run(&[names[1]], "2026-08-03"), (Some((path(names[1]), "2026-08-03".into())), vec![], vec![]));
        // The pin moved back: a newer result goes and the planet is the start.
        assert_eq!(run(&[names[1]], "2026-08-02"), (None, vec![d2.into()], vec![path(names[1])]));
    }

    #[test]
    fn a_planet_that_ends_before_the_first_diff_is_refused() {
        assert!(check_gap("2026-09-28", "2026-09-28T00:00:04Z", "2026-09-28T00:00:00Z").is_ok());
        assert!(check_gap("2026-09-28", "2026-09-28T00:00:00Z", "2026-09-28T00:00:00Z").is_ok());
        let error = check_gap("2026-09-28", "2026-09-27T23:59:59Z", "2026-09-28T00:00:00Z").unwrap_err();
        assert!(error.contains("invalid data") && error.contains("obc data refresh osm-planet"), "{error}");
    }

    #[test]
    fn a_replication_state_gives_its_timestamp_without_the_escapes() {
        let path = std::env::temp_dir().join(format!("obc-state-{}.txt", std::process::id()));
        std::fs::write(
            &path,
            "#Mon Oct 05 00:18:17 UTC 2026\nsequenceNumber=5136\ntimestamp=2026-10-05T00\\:00\\:00Z\n",
        )
        .unwrap();
        assert_eq!(state_timestamp(&path).unwrap(), "2026-10-05T00:00:00Z");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn installed_osmium_updates_an_earlier_result_with_only_the_newer_diffs() {
        let runner = OsmiumRunner::default();
        if runner.check().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("obc-osmium-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let osmium = |args: &[&str]| {
            assert!(Command::new(&runner.binary).args(args).current_dir(&dir).status().unwrap().success())
        };
        let tiny = repo("builder/tests/corpus/data/tiny.osm.pbf");
        let tiny = tiny.to_str().unwrap();
        for (planet, end) in [("planet", "2026-08-01T00:00:04Z"), ("early", "2026-07-31T23:00:00Z")] {
            let header = format!("--output-header=osmosis_replication_timestamp={end}");
            osmium(&["cat", tiny, "-f", "pbf", "-o", planet, &header]);
        }
        // Each day deletes one more node.
        let days: [&[&str]; 3] = [&["n1", "n2", "n3", "n4", "n5"], &["n1", "n2", "n3", "n4"], &["n1", "n2", "n3"]];
        let mut previous = tiny.to_string();
        let mut files = vec![("state0".to_string(), "timestamp=2026-08-01T00\\:00\\:00Z\n".to_string())];
        for (day, ids) in days.iter().enumerate() {
            let kept = format!("kept{}.osm.pbf", day + 1);
            osmium(&[&["getid", tiny], *ids, &["-o", &kept]].concat());
            osmium(&["derive-changes", &previous, &kept, "-o", &format!("diff{}.osc.gz", day + 1)]);
            files.push((format!("diff{}.osc.gz", day + 1), String::new()));
            files.push((format!("state{}", day + 1), format!("timestamp=2026-08-0{}T00\\:00\\:00Z\n", day + 2)));
            previous = kept;
        }
        for (name, text) in &files {
            if !text.is_empty() {
                std::fs::write(dir.join(name), text).unwrap();
            }
        }
        let fetched = |to: &str, count: usize| obc_data::fetch::Fetched {
            snapshot: obc_data::store::Snapshot {
                source: "osm-replication".into(),
                version: to.into(),
                files: files[..count]
                    .iter()
                    .map(|(name, _)| obc_data::store::FileRecord {
                        name: name.clone(),
                        url: name.clone(),
                        size: 0,
                        sha256: String::new(),
                        retrieved: String::new(),
                    })
                    .collect(),
            },
            paths: files[..count].iter().map(|(name, _)| dir.join(name)).collect(),
        };
        let opl = |path: &Path| {
            let output =
                Command::new(&runner.binary).args(["cat", "-F", "pbf", "-f", "opl"]).arg(path).output().unwrap();
            String::from_utf8(output.stdout).unwrap()
        };
        let cache = dir.join("cache");
        let silent = Progress::silent();

        let (first, to, _) =
            update(&dir.join("planet"), "2026-08-01", &fetched("2026-08-03", 5), &cache, &runner, &silent).unwrap();
        assert_eq!(
            (first.file_name().unwrap(), to.as_str()),
            ("planet-2026-08-01+2026-08-03.osm.pbf".as_ref(), "2026-08-03")
        );
        assert_eq!(opl(&first), opl(&dir.join("kept2.osm.pbf")));

        // The earlier result is the start: the diffs it holds are not read again.
        std::fs::remove_file(dir.join("diff1.osc.gz")).unwrap();
        std::fs::remove_file(dir.join("diff2.osc.gz")).unwrap();
        std::fs::write(cache.join("stale.part"), "").unwrap();
        let (second, _, applied) =
            update(&dir.join("planet"), "2026-08-01", &fetched("2026-08-04", 7), &cache, &runner, &silent).unwrap();
        assert_eq!(opl(&second), opl(&dir.join("kept3.osm.pbf")));
        assert_eq!(applied.unwrap().diffs, 3);
        let left: Vec<_> = std::fs::read_dir(&cache).unwrap().map(|entry| entry.unwrap().path()).collect();
        assert_eq!(left, [second], "the earlier result and the stale part file are gone");

        let error = update(&dir.join("early"), "2026-08-01", &fetched("2026-08-04", 7), &cache, &runner, &silent)
            .expect_err("a planet that ends before the first diff starts leaves a gap");
        assert!(error.contains("invalid data") && error.contains("obc data refresh osm-planet"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_osmium_keeps_identical_leaf_bytes_across_replication_headers() {
        let runner = OsmiumRunner::default();
        if runner.check().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("obc-osmium-replication-headers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = repo("builder/tests/corpus/data/tiny.osm.pbf");
        let make = |name: &str, sequence: &str, timestamp: &str| {
            let path = dir.join(name);
            let status = Command::new(&runner.binary)
                .arg("cat")
                .arg(&source)
                .arg("-o")
                .arg(&path)
                .arg("--overwrite")
                .arg(format!("--output-header=osmosis_replication_sequence_number={sequence}"))
                .arg(format!("--output-header=osmosis_replication_timestamp={timestamp}"))
                .arg("--output-header=osmosis_replication_base_url=https://planet.openstreetmap.org/replication/hour/")
                .status()
                .unwrap();
            assert!(status.success());
            path
        };
        let first = make("first.osm.pbf", "10", "2026-08-01T10:00:00Z");
        let second = make("second.osm.pbf", "11", "2026-08-01T11:00:00Z");
        let request = [ExtractRequest { output: "leaf.osm.pbf".into(), bbox: [7.0, 47.0, 8.5, 49.0] }];
        let first_out = dir.join("first");
        let second_out = dir.join("second");
        runner.split(&first, &first_out, &request, &Progress::silent()).unwrap();
        runner.split(&second, &second_out, &request, &Progress::silent()).unwrap();
        assert_eq!(
            crate::hash::file(&first_out.join("leaf.osm.pbf")).unwrap(),
            crate::hash::file(&second_out.join("leaf.osm.pbf")).unwrap(),
            "replication-only source header changes must not invalidate every geographic leaf"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sharding_is_resumable_and_hash_checks_leaves() {
        let dir = std::env::temp_dir().join(format!("obc-planet-shards-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let input_path = dir.join("planet.osm.pbf");
        std::fs::write(&input_path, b"planet").unwrap();
        let input = PlanetInput {
            path: input_path,
            bytes: 6,
            sha256: crate::hash::text("fixture-planet"),
            snapshot: "2026-08-01".into(),
            replication: None,
        };
        let runner = FakeRunner { calls: Mutex::new(Vec::new()), mutated_leaf: Mutex::new(None) };
        let sharder = PlanetSharder { input: &input, cache: &dir, runner: &runner };
        let first = sharder.run(&Progress::silent()).unwrap();
        assert!(runner.calls.lock().unwrap().iter().all(|count| *count <= 2));
        let calls = runner.calls.lock().unwrap().len();
        let second = sharder.run(&Progress::silent()).unwrap();
        assert_eq!(first.leaves.len(), second.leaves.len());
        assert_eq!(first.changed, first.leaves.len());
        assert_eq!(second.reused, second.leaves.len());
        assert_eq!(runner.calls.lock().unwrap().len(), calls, "a current hierarchy performs no extraction");

        let next_input = PlanetInput {
            path: input.path.clone(),
            bytes: input.bytes,
            sha256: crate::hash::text("fixture-planet-next-snapshot"),
            snapshot: "2026-08-02".into(),
            replication: None,
        };
        let next_sharder = PlanetSharder { input: &next_input, cache: &dir, runner: &runner };
        let refreshed = next_sharder.run(&Progress::silent()).unwrap();
        assert_eq!(refreshed.reused, 0);
        assert_eq!(refreshed.refreshed, refreshed.leaves.len());
        assert_eq!(refreshed.changed, 0, "snapshot headers must not invalidate byte-identical leaves");
        assert_eq!(
            first.leaves.iter().map(|leaf| &leaf.sha256).collect::<Vec<_>>(),
            refreshed.leaves.iter().map(|leaf| &leaf.sha256).collect::<Vec<_>>()
        );

        let changed_id = refreshed.leaves[0].id;
        *runner.mutated_leaf.lock().unwrap() = Some(
            LeafRect { i0: changed_id.i, i1: changed_id.i + 1, j0: changed_id.j, j1: changed_id.j + 1 }.extract_bbox(),
        );
        let changed_input = PlanetInput {
            path: input.path.clone(),
            bytes: input.bytes,
            sha256: crate::hash::text("fixture-planet-with-one-change"),
            snapshot: "2026-08-03".into(),
            replication: None,
        };
        let changed_sharder = PlanetSharder { input: &changed_input, cache: &dir, runner: &runner };
        let changed = changed_sharder.run(&Progress::silent()).unwrap();
        assert_eq!(changed.changed, 1);
        assert_eq!(changed.refreshed, changed.leaves.len() - 1);

        std::fs::write(&first.leaves[0].path, b"corrupt").unwrap();
        let repaired = changed_sharder.run(&Progress::silent()).unwrap();
        assert_eq!(repaired.changed, 1);
        assert!(runner.calls.lock().unwrap().len() > calls, "a changed leaf is recreated through its branch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_bands() -> BandTable {
        use obc_pack::grid::{Band, BandRole};
        BandTable {
            bands: vec![
                Band {
                    id: "coarse".into(),
                    cell_log2: 22,
                    lods: vec![0, 1, 2, 3, 4],
                    sections: vec![],
                    role: BandRole::Coarse,
                },
                Band { id: "mid".into(), cell_log2: 22, lods: vec![5, 6], sections: vec![], role: BandRole::Geometry },
                Band { id: "fine".into(), cell_log2: 22, lods: vec![7, 8], sections: vec![], role: BandRole::Geometry },
                Band {
                    id: "network".into(),
                    cell_log2: 22,
                    lods: vec![],
                    sections: vec!["nav".into(), "poi".into()],
                    role: BandRole::Core,
                },
            ],
        }
    }

    /// Keep the planet bookkeeping fixture deliberately small. Its assertions exercise leaf reuse,
    /// known-empty ranges, and deletion; making it bake every production LOD turns one bookkeeping
    /// test into a multi-minute geometry benchmark whenever the shipped ladder grows.
    fn test_schema(out: &Path) -> crate::presets::StyleDoc {
        let source = std::fs::read_to_string(repo("builder/presets/schema.json")).unwrap();
        let mut document: serde_json::Value = serde_json::from_str(&source).unwrap();
        let lods = document.get_mut("lods").and_then(serde_json::Value::as_array_mut).unwrap();
        lods.truncate(9);
        for lod in lods {
            let lod = lod.as_object_mut().unwrap();
            lod.shift_remove("semantic_coverage");
            lod.shift_remove("merge_line_trails");
            lod.shift_remove("line_simplify");
        }
        let presets = out.join(".fixture-presets");
        std::fs::create_dir_all(&presets).unwrap();
        std::fs::write(presets.join(crate::presets::SCHEMA_DOC), serde_json::to_string_pretty(&document).unwrap())
            .unwrap();
        crate::presets::load_schema(&presets).unwrap()
    }

    struct AllEmptyCutter;

    impl CellCutter for AllEmptyCutter {
        fn recipe(&self) -> String {
            "all-empty-fixture".into()
        }

        fn cut(
            &self,
            _pbfs: &[String],
            _config: &obc_pack::config::Config,
            _out_dir: &Path,
            opts: &CutOptions,
            _progress: &Progress,
        ) -> Result<obc_pack::cut::CutSummary, String> {
            let cells = opts
                .bands
                .bands
                .iter()
                .flat_map(|band| {
                    opts.select.iter().copied().map(|id| CellArtifact {
                        id,
                        band: band.id.clone(),
                        path: format!("cells/{}/{}/{}.obcm", band.id, id.i, id.j),
                        bytes: 0,
                        sha256: crate::hash::text(""),
                        partial: false,
                        dropped: 0,
                        pois: 0,
                        nav_nodes: 0,
                        nav_edges: 0,
                        empty: true,
                    })
                })
                .collect();
            Ok(obc_pack::cut::CutSummary { cells, bytes: 0, dropped: 0, partial: 0 })
        }
    }

    #[test]
    fn a_leaf_bake_replaces_empty_artifacts_with_ranges_skips_and_applies_deletions() {
        let dir = std::env::temp_dir().join(format!("obc-planet-bake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pbf = repo("builder/tests/corpus/data/tiny.osm.pbf");
        let (_, sha256) = crate::hash::file(&pbf).unwrap();
        let leaf_cell = CellId::containing(SOURCE_LEAF_LOG2, 47_990_000, 7_805_000);
        let input = PlanetInput {
            path: pbf.clone(),
            bytes: std::fs::metadata(&pbf).unwrap().len(),
            sha256: sha256.clone(),
            snapshot: "2026-08-01".into(),
            replication: None,
        };
        let leaf = PlanetLeaf {
            id: LeafId { i: leaf_cell.i, j: leaf_cell.j },
            path: pbf,
            bytes: input.bytes,
            sha256,
            logical_bbox: leaf_cell.square(),
        };
        let schema = test_schema(&dir);
        let cutter = crate::cells::ObcCutter { no_land: true, land: Default::default(), chunk_size: None };
        let run = || PlanetBake {
            input: &input,
            leaves: std::slice::from_ref(&leaf),
            regions: &[],
            schema: &schema,
            skins: &[],
            cutter: &cutter,
            source_leaves_reused: 0,
            source_leaves_refreshed: 0,
            source_leaves_changed: 1,
            opts: CellBakeOptions {
                out: dir.clone(),
                force: false,
                fail_fast: true,
                bands: test_bands(),
                schema_id: "bikepacking".into(),
                schema_revision: 1,
                terrain: None,
                peaks: None,
            },
        };
        let first = run().run_inner(&Progress::silent()).unwrap();
        assert_eq!(first.leaves_cut, 1, "{:?}", first.failures);
        assert!(first.artifacts_cut > 0, "fixture content produces real cell artifacts");
        assert!(first.known_empty_cut > 0, "quiet child cells become compact zero-byte coverage");
        assert!(dir.join("cells/fine/.known-empty.json").is_file());

        let second = run().run_inner(&Progress::silent()).unwrap();
        assert_eq!(second.leaves_unchanged, 1, "the source leaf is not ingested twice");
        assert_eq!(second.leaves_cut, 0);

        let deleted_input = PlanetInput {
            path: input.path.clone(),
            bytes: input.bytes,
            sha256: crate::hash::text("post-deletion-planet"),
            snapshot: "2026-08-02".into(),
            replication: None,
        };
        let deleted_leaf = PlanetLeaf { sha256: crate::hash::text("post-deletion-leaf"), ..leaf.clone() };
        let deleted = PlanetBake {
            input: &deleted_input,
            leaves: std::slice::from_ref(&deleted_leaf),
            regions: &[],
            schema: &schema,
            skins: &[],
            cutter: &AllEmptyCutter,
            source_leaves_reused: 0,
            source_leaves_refreshed: 0,
            source_leaves_changed: 1,
            opts: CellBakeOptions {
                out: dir.clone(),
                force: false,
                fail_fast: true,
                bands: test_bands(),
                schema_id: "bikepacking".into(),
                schema_revision: 1,
                terrain: None,
                peaks: None,
            },
        }
        .run_inner(&Progress::silent())
        .unwrap();
        assert_eq!(deleted.leaves_cut, 1);
        assert_eq!(deleted.artifacts_cut, 0);
        assert_eq!(deleted.known_empty_cut, 16);
        let empties = KnownEmptyIndex::load(&dir, &test_bands(), 1).unwrap();
        for (band, ids) in leaf_cells(leaf.id, &test_bands()) {
            for id in ids {
                assert!(empties.fact(&band, id).is_some(), "{band} {id} records the deletion as known-empty");
                assert!(!cell_paths(&dir, &band, id).0.exists(), "{band} {id} stale artifact was removed");
            }
        }

        let error = run().validate_planet_leaves().expect_err("one fixture leaf is not a complete planet");
        assert!(error.contains("refusing to mark partial coverage complete"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_incomplete_planet_marker_blocks_the_shared_gate() {
        let dir = std::env::temp_dir().join(format!("obc-planet-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let status = PlanetBakeStatus {
            state_version: PLANET_BAKE_STATE_VERSION,
            complete: false,
            source_sha256: "0".repeat(64),
            source_snapshot: "2026-08-01".into(),
            schema_id: "bikepacking".into(),
            schema_revision: 1,
            schema_sha256: "1".repeat(64),
            bands_sha256: "2".repeat(64),
            cutter_recipe: "fixture".into(),
            leaves: 999,
        };
        write_json(&dir.join(PLANET_STATUS_FILE), &status).unwrap();
        let error = check_publishable_tree(&dir).expect_err("partial planet coverage is not publishable");
        assert!(error.contains("incomplete") && error.contains("obc bake --all"), "{error}");

        let mut status = status;
        status.complete = true;
        write_json(&dir.join(PLANET_STATUS_FILE), &status).unwrap();
        check_publishable_tree(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_configuration_does_not_poison_an_existing_planet_tree() {
        let dir = std::env::temp_dir().join(format!("obc-planet-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ids = Vec::new();
        LeafRect::root().leaves(&mut ids);
        let leaves: Vec<PlanetLeaf> = ids
            .into_iter()
            .map(|id| PlanetLeaf {
                id,
                path: PathBuf::from("unused.osm.pbf"),
                bytes: 0,
                sha256: "0".repeat(64),
                logical_bbox: id.cell().square(),
            })
            .collect();
        let input = PlanetInput {
            path: PathBuf::from("unused.osm.pbf"),
            bytes: 0,
            sha256: "0".repeat(64),
            snapshot: "2026-08-01".into(),
            replication: None,
        };
        let schema = crate::presets::load_schema(&repo("builder/presets")).unwrap();
        let cutter = crate::cells::ObcCutter { no_land: true, land: Default::default(), chunk_size: None };
        let error = PlanetBake {
            input: &input,
            leaves: &leaves,
            regions: &[],
            schema: &schema,
            skins: &[],
            cutter: &cutter,
            source_leaves_reused: leaves.len(),
            source_leaves_refreshed: 0,
            source_leaves_changed: 0,
            opts: CellBakeOptions {
                out: dir.clone(),
                force: false,
                fail_fast: true,
                bands: BandTable::recommended(),
                schema_id: "typo".into(),
                schema_revision: 1,
                terrain: None,
                peaks: None,
            },
        }
        .run(&Progress::silent())
        .expect_err("schema mismatch fails in preflight");
        assert!(error.contains("--schema-id"), "{error}");
        assert!(!dir.join(PLANET_STATUS_FILE).exists(), "no incomplete marker was written before preflight passed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
