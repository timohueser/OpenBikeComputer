//! The bake runner: (region × preset) → a tree `obc-pack catalog` accepts.
//!
//! One job is: resolve the extract, decide whether anything changed, pack, **verify
//! the artifact opens with the real reader**, and only then move it into the tree
//! beside a sidecar recording what the bytes cannot say. Everything interesting
//! about this module is in that ordering and in what "changed" means.
//!
//! ## The artifact is never half-published
//!
//! The packer writes to a dot-prefixed temporary file in the artifact's own
//! directory; [`crate::verify`] reads it back whole; only a verified file is
//! renamed onto the artifact path. So the tree the catalog generator walks contains
//! only artifacts that were, at the moment they landed, fully readable — a corrupt
//! or truncated one is deleted at the temp path and never had a name. The
//! generator's own laws (`OBCC_Spec.md` §8) then keep it honest from the other side:
//! an artifact without a sidecar, or a stray file in a region directory, fails
//! generation rather than being skipped.
//!
//! A failed re-bake **leaves the previous artifact in place**. That is deliberate:
//! partial re-bakes are the normal operation (§3), and dropping last week's
//! perfectly readable Bayern because this week's pack ran out of memory would turn
//! a build failure into a coverage hole. The failure is loud in the summary and in
//! the exit status instead.
//!
//! ## What "unchanged" means
//!
//! The skip key is a hash of everything that can change the bytes:
//!
//! ```text
//! recipe version │ OBCM format version │ SHA-256 of the extract │ SHA-256 of the preset config │ pack options
//! ```
//!
//! Content hashes, never mtimes. A mirror that re-uploads a byte-identical extract
//! with a fresh `Last-Modified` must not cost a twenty-hour re-bake, and a file
//! edited in place must not be missed because its timestamp was preserved. The
//! recorded key sits in a dotfile beside the artifact (invisible to the catalog
//! walk, §8), along with the artifact's own digest — which is re-checked on every
//! run, so an artifact that rotted on disk re-bakes even though its inputs did not.
//!
//! [`RECIPE_VERSION`] is the escape hatch for the case content hashing cannot see:
//! the packer itself changed. Bump it when a packer change alters output bytes and
//! every artifact should be rebuilt.

use std::path::{Path, PathBuf};
use std::time::Instant;

use obc_pack::progress::Progress;
use serde::{Deserialize, Serialize};

use crate::presets::Preset;
use crate::regions::Region;
use crate::source::ExtractSource;
use crate::verify::Verified;

/// Bumped when a packer change alters output bytes for unchanged inputs, forcing a
/// full re-bake that content hashing alone would not.
pub const RECIPE_VERSION: u32 = 1;

const STATE_SUFFIX: &str = ".bake.json";
const ARTIFACT_EXT: &str = ".obcm";
const SIDECAR_EXT: &str = ".obcm.json";

/// What actually packs. A trait so the tests can drive the whole runner — tree
/// layout, verification gate, idempotency, summary — without libGEOS and without a
/// multi-gigabyte extract, and so a test can inject the one artifact this design
/// most needs to prove it rejects: a corrupt one.
pub trait Packer: Sync {
    /// Identifies the pack recipe in the bake key. Two runs whose packers describe
    /// themselves differently must not reuse each other's artifacts.
    fn recipe(&self) -> String;
    /// Pack `pbf` into `out`, returning the bytes written.
    fn pack(&self, pbf: &Path, preset: &Preset, out: &Path, progress: &Progress) -> Result<u64, String>;
}

/// The real thing: `obc_pack::pipeline::pack`, linked in rather than spawned.
///
/// Linked because the pipeline *is* a library API (#906) and the alternative is
/// worse in every dimension that matters here: a subprocess would have to have its
/// stdout scraped for progress, could not be cancelled, and would hand back an exit
/// code where this hands back the byte count. The desktop app made the same call.
pub struct ObcPacker {
    /// Skip land generation. The land dataset is a ~950 MB download; a real bake
    /// wants it (the sea backdrop), the tests never do.
    pub no_land: bool,
    pub chunk_size: Option<usize>,
}

impl Packer for ObcPacker {
    fn recipe(&self) -> String {
        format!("obc-pack no_land={} chunk_size={:?}", self.no_land, self.chunk_size)
    }

    fn pack(&self, pbf: &Path, preset: &Preset, out: &Path, progress: &Progress) -> Result<u64, String> {
        let opts = obc_pack::PackOptions {
            no_land: self.no_land,
            chunk_size: self.chunk_size,
            ..obc_pack::PackOptions::default()
        };
        let pbfs = [pbf.to_string_lossy().into_owned()];
        match obc_pack::pipeline::pack(&pbfs, &preset.config, out, &opts, progress) {
            Ok(summary) => {
                if summary.dropped > 0 {
                    progress.warn(format!("{} features exceeded chunk_size and were dropped", summary.dropped));
                }
                Ok(summary.bytes)
            }
            Err(obc_pack::PackError::Failed(e)) => Err(e),
            Err(obc_pack::PackError::Cancelled) => Err("cancelled".into()),
        }
    }
}

/// How a run is scoped and where it writes.
pub struct BakeOptions {
    /// The bake tree root (`presets/`, `regions/` live under it).
    pub out: PathBuf,
    /// Re-bake even when the key says nothing changed.
    pub force: bool,
    /// Stop at the first failure instead of baking the rest and reporting at the end.
    pub fail_fast: bool,
}

/// The recorded bake, in a dotfile beside the artifact. Invisible to the catalog
/// generator (`OBCC_Spec.md` §8 ignores dotfiles) and never published.
#[derive(Debug, Serialize, Deserialize)]
struct BakeState {
    key: String,
    artifact_sha256: String,
    bytes: u64,
    built_at: String,
    source_snapshot: String,
}

/// The four facts the artifact's bytes cannot state (`OBCC_Spec.md` §8). Written by
/// the job that packed the artifact, and never re-derived afterwards.
#[derive(Debug, Serialize)]
struct Sidecar<'a> {
    region_name: &'a str,
    preset_version: u32,
    built_at: &'a str,
    source_snapshot: &'a str,
}

/// How one (region, preset) job ended.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JobStatus {
    /// Packed, verified, and installed in the tree this run.
    Baked { bytes: u64, seconds: f64, features: u64, lods: usize },
    /// Inputs unchanged and the artifact on disk still matches its recorded digest.
    Unchanged { bytes: u64 },
    /// Loud. The previous artifact, if any, is untouched.
    Failed { error: String },
}

/// One (region, preset) job's result.
#[derive(Debug, Clone, Serialize)]
pub struct JobOutcome {
    pub region_id: String,
    pub region_name: String,
    pub preset_id: String,
    pub source_snapshot: String,
    #[serde(flatten)]
    pub status: JobStatus,
}

impl JobOutcome {
    pub fn failed(&self) -> bool {
        matches!(self.status, JobStatus::Failed { .. })
    }

    /// Bytes this (region, preset) contributes to the published catalog, whether it
    /// was baked this run or left alone.
    pub fn published_bytes(&self) -> u64 {
        match self.status {
            JobStatus::Baked { bytes, .. } | JobStatus::Unchanged { bytes } => bytes,
            JobStatus::Failed { .. } => 0,
        }
    }
}

/// Everything a run did, and everything it did not.
#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub tree: PathBuf,
    pub obcm_version: u8,
    pub recipe_version: u32,
    pub jobs: Vec<JobOutcome>,
    /// Regions the run was asked to cover that ended with no artifact at all — the
    /// failure mode this whole crate is defensive about, because to a user a missing
    /// region is indistinguishable from a curation decision.
    pub uncovered_regions: Vec<String>,
}

impl RunSummary {
    pub fn failures(&self) -> Vec<&JobOutcome> {
        self.jobs.iter().filter(|j| j.failed()).collect()
    }

    pub fn ok(&self) -> bool {
        self.failures().is_empty() && self.uncovered_regions.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.jobs.iter().map(JobOutcome::published_bytes).sum()
    }

    /// The run report, loud end first.
    ///
    /// Sizes are printed per artifact and as a total because the storage estimate for
    /// this catalog was, before the first real bake, an extrapolation from a 1 MB
    /// fixture (#898). The bake is the measurement; this is where it is reported.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "\n=== bake summary ({}) ===", self.tree.display());
        let _ = writeln!(s, "OBCM v{}, recipe v{}", self.obcm_version, self.recipe_version);
        let mut baked = 0;
        let mut unchanged = 0;
        for job in &self.jobs {
            let line = match &job.status {
                JobStatus::Baked { bytes, seconds, features, lods } => {
                    baked += 1;
                    format!("baked      {:>10}  {:>7.1}s  {features} features, {lods} LODs", human(*bytes), seconds)
                }
                JobStatus::Unchanged { bytes } => {
                    unchanged += 1;
                    format!("unchanged  {:>10}", human(*bytes))
                }
                JobStatus::Failed { error } => format!("FAILED     {error}"),
            };
            let _ = writeln!(s, "  {:<42} {:<12} {line}", job.region_id, job.preset_id);
        }
        let _ = writeln!(
            s,
            "\n{baked} baked, {unchanged} unchanged, {} failed — {} published across {} artifacts",
            self.failures().len(),
            human(self.total_bytes()),
            self.jobs.len() - self.failures().len()
        );
        if !self.failures().is_empty() {
            let _ = writeln!(s, "\n!!! FAILED JOBS !!!");
            for job in self.failures() {
                if let JobStatus::Failed { error } = &job.status {
                    let _ = writeln!(s, "  {} [{}]: {error}", job.region_id, job.preset_id);
                }
            }
        }
        if !self.uncovered_regions.is_empty() {
            let _ = writeln!(
                s,
                "\n!!! REGIONS WITH NO ARTIFACT !!! (a curated region that does not ship reads to a user as \
                 \"not covered\")"
            );
            for region in &self.uncovered_regions {
                let _ = writeln!(s, "  {region}");
            }
        }
        s
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A configured run.
pub struct Bakery<'a> {
    pub regions: &'a [Region],
    pub presets: &'a [Preset],
    pub source: &'a dyn ExtractSource,
    pub packer: &'a dyn Packer,
    pub opts: BakeOptions,
}

impl Bakery<'_> {
    /// Bake everything, reporting through `progress`.
    ///
    /// Sequential on purpose. A country pack is memory-bound (the ingest holds the
    /// node table for a whole country) and already rayon-parallel inside; running two
    /// at once is how a workstation gets OOM-killed halfway through a twenty-hour
    /// bake. Parallelism, if it ever pays, belongs across *machines* — the tree is
    /// per-artifact by construction, so two boxes can bake disjoint regions into two
    /// trees and the results merge by copying files.
    pub fn run(&self, progress: &Progress) -> Result<RunSummary, String> {
        progress.log(format!("bakery: {} regions × {} presets", self.regions.len(), self.presets.len()));
        progress.log(format!("  source:  {}", self.source.describe()));
        progress.log(format!("  tree:    {}", self.opts.out.display()));

        let mut jobs: Vec<JobOutcome> = Vec::new();
        let mut uncovered: Vec<String> = Vec::new();

        for region in self.regions {
            let presets: Vec<&Preset> = self
                .presets
                .iter()
                .filter(|p| region.presets.as_ref().is_none_or(|only| only.contains(&p.id)))
                .collect();
            if presets.is_empty() {
                // Not a failure — the caller narrowed the matrix, or the region asks
                // for a preset this run does not have — but never silent: "no preset
                // applied" and "baked nothing" look identical in the tree afterwards.
                progress.warn(format!("{}: no preset in this run applies — skipped", region.id));
                continue;
            }

            // The extract is resolved and hashed once per region, not per preset: it
            // is the same multi-gigabyte file for every preset in the matrix.
            progress.log(format!("\n--- {} ({}) ---", region.id, region.name));
            let resolved = self.source.fetch(region, progress).and_then(|extract| {
                progress.log(format!("  extract {} ({})", human(extract.bytes), extract.snapshot));
                let (_, sha) = crate::hash::file(&extract.path)?;
                Ok((extract, sha))
            });
            let (extract, extract_sha) = match resolved {
                Ok(v) => v,
                Err(e) => {
                    // One failure per preset: the matrix cell is what a reader of the
                    // summary is looking for, and an extract failure loses all of them.
                    for preset in &presets {
                        jobs.push(JobOutcome {
                            region_id: region.id.clone(),
                            region_name: region.name.clone(),
                            preset_id: preset.id.clone(),
                            source_snapshot: String::new(),
                            status: JobStatus::Failed { error: format!("extract: {e}") },
                        });
                    }
                    uncovered.push(region.id.clone());
                    if self.opts.fail_fast {
                        return Ok(self.finish(jobs, uncovered));
                    }
                    continue;
                }
            };

            let mut covered = false;
            for preset in presets {
                progress.log(format!("  [{}] {}", preset.id, region.id));
                let outcome = self.run_job(region, preset, &extract, &extract_sha, progress);
                match &outcome.status {
                    JobStatus::Failed { error } => {
                        progress.warn(format!("  {} [{}] FAILED: {error}", region.id, preset.id))
                    }
                    _ => covered = true,
                }
                let failed = outcome.failed();
                jobs.push(outcome);
                if failed && self.opts.fail_fast {
                    return Ok(self.finish(jobs, uncovered));
                }
            }
            if !covered {
                uncovered.push(region.id.clone());
            }
        }

        Ok(self.finish(jobs, uncovered))
    }

    fn finish(&self, jobs: Vec<JobOutcome>, uncovered: Vec<String>) -> RunSummary {
        RunSummary {
            tree: self.opts.out.clone(),
            obcm_version: obc_formats::obcm::VERSION,
            recipe_version: RECIPE_VERSION,
            jobs,
            uncovered_regions: uncovered,
        }
    }

    fn run_job(
        &self,
        region: &Region,
        preset: &Preset,
        extract: &crate::source::Extract,
        extract_sha: &str,
        progress: &Progress,
    ) -> JobOutcome {
        let started = Instant::now();
        let outcome = |status| JobOutcome {
            region_id: region.id.clone(),
            region_name: region.name.clone(),
            preset_id: preset.id.clone(),
            source_snapshot: extract.snapshot.clone(),
            status,
        };
        match self.bake_one(region, preset, extract, extract_sha, started, progress) {
            Ok(status) => outcome(status),
            Err(error) => outcome(JobStatus::Failed { error }),
        }
    }

    fn bake_one(
        &self,
        region: &Region,
        preset: &Preset,
        extract: &crate::source::Extract,
        extract_sha: &str,
        started: Instant,
        progress: &Progress,
    ) -> Result<JobStatus, String> {
        let dir = region.segments().iter().fold(self.opts.out.join("regions"), |p, seg| p.join(seg));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let artifact = dir.join(format!("{}{ARTIFACT_EXT}", preset.id));
        let sidecar = dir.join(format!("{}{SIDECAR_EXT}", preset.id));
        let state_path = dir.join(format!(".{}{STATE_SUFFIX}", preset.id));
        let tmp = dir.join(format!(".{}.obcm.tmp", preset.id));

        let key = self.bake_key(region, extract_sha, extract, preset);
        if !self.opts.force {
            if let Some(bytes) = unchanged(&state_path, &artifact, &sidecar, &key) {
                progress.log("    unchanged — skipping");
                self.install_preset_config(preset)?;
                return Ok(JobStatus::Unchanged { bytes });
            }
        }

        let _ = std::fs::remove_file(&tmp);
        let pack_result = self.packer.pack(&extract.path, preset, &tmp, progress);
        let verified = pack_result.and_then(|_| {
            progress.log("    verifying with obc-reader");
            crate::verify::verify(&tmp)
        });
        let verified: Verified = match verified {
            Ok(v) => v,
            Err(e) => {
                // Nothing unverified is allowed to exist under a name the catalog
                // generator would walk. The previous artifact, if any, stays.
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };

        let (bytes, artifact_sha) = crate::hash::file(&tmp)?;
        let built_at = obc_pack::catalog::now_timestamp();
        // Sidecar before the rename: an artifact without one fails generation, so the
        // one ordering that must never happen is a visible artifact with no sidecar.
        write_json(
            &sidecar,
            &Sidecar {
                region_name: &region.name,
                preset_version: preset.version,
                built_at: &built_at,
                source_snapshot: &extract.snapshot,
            },
        )?;
        std::fs::rename(&tmp, &artifact).map_err(|e| format!("{} -> {}: {e}", tmp.display(), artifact.display()))?;
        write_json(
            &state_path,
            &BakeState {
                key,
                artifact_sha256: artifact_sha,
                bytes,
                built_at: built_at.clone(),
                source_snapshot: extract.snapshot.clone(),
            },
        )?;
        self.install_preset_config(preset)?;

        progress.log(format!(
            "    {} — {} features, {} LODs, {} POI categories{}",
            human(bytes),
            verified.features,
            verified.lods,
            verified.poi_categories,
            if verified.has_nav_graph { ", nav graph" } else { "" }
        ));
        Ok(JobStatus::Baked {
            bytes,
            seconds: started.elapsed().as_secs_f64(),
            features: verified.features,
            lods: verified.lods,
        })
    }

    /// Everything that can change the artifact's bytes **or its sidecar**, hashed
    /// into one key.
    ///
    /// The region's name and the extract's snapshot date are in here even though
    /// they cannot move a single byte of the `.obcm`: they are recorded in the
    /// sidecar, which is what the manifest reads back, so renaming a region in
    /// `regions.toml` has to re-write that sidecar rather than being skipped as
    /// "unchanged" and leaving the catalog advertising the old name forever.
    fn bake_key(
        &self,
        region: &Region,
        extract_sha: &str,
        extract: &crate::source::Extract,
        preset: &Preset,
    ) -> String {
        crate::hash::text(&format!(
            "recipe={RECIPE_VERSION}\nobcm={}\nregion={}\nname={}\nsnapshot={}\nextract={extract_sha}\npreset={}\n\
             pack={}\n",
            obc_formats::obcm::VERSION,
            region.id,
            region.name,
            extract.snapshot,
            preset.sha256,
            self.packer.recipe(),
        ))
    }

    /// Copy the preset's config verbatim into `<tree>/presets/<id>.json` — the
    /// catalog's description of the preset *as it is now* (§8). Written only for
    /// presets that have an artifact, so a preset whose every job failed does not
    /// make the whole catalog unpublishable ("a preset nobody built").
    fn install_preset_config(&self, preset: &Preset) -> Result<(), String> {
        let dir = self.opts.out.join("presets");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let dest = dir.join(format!("{}.json", preset.id));
        if std::fs::read_to_string(&dest).ok().as_deref() == Some(preset.json.as_str()) {
            return Ok(());
        }
        std::fs::write(&dest, &preset.json).map_err(|e| format!("{}: {e}", dest.display()))
    }
}

/// `Some(bytes)` when the recorded bake still describes what is on disk.
///
/// Three things must agree, and the third is the one that catches rot: the key
/// (inputs unchanged), the presence of both artifact and sidecar, and the
/// artifact's *current* digest against the recorded one. An artifact truncated by a
/// full disk after a good bake therefore re-bakes rather than being skipped forever.
fn unchanged(state_path: &Path, artifact: &Path, sidecar: &Path, key: &str) -> Option<u64> {
    let state: BakeState = serde_json::from_str(&std::fs::read_to_string(state_path).ok()?).ok()?;
    if state.key != key || !artifact.is_file() || !sidecar.is_file() {
        return None;
    }
    let (bytes, sha) = crate::hash::file(artifact).ok()?;
    (sha == state.artifact_sha256 && bytes == state.bytes).then_some(bytes)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| format!("{}: {e}", path.display()))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The tree's per-artifact state files, for a caller that wants to list them.
pub fn state_file_name(preset_id: &str) -> String {
    format!(".{preset_id}{STATE_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024 * 1024), "1.0 MB");
        assert_eq!(human(4_808_626_767), "4.5 GB");
    }

    #[test]
    fn a_summary_with_a_failure_is_not_ok_and_says_so_loudly() {
        let summary = RunSummary {
            tree: PathBuf::from("/tmp/tree"),
            obcm_version: 10,
            recipe_version: RECIPE_VERSION,
            jobs: vec![JobOutcome {
                region_id: "europe/austria".into(),
                region_name: "Austria".into(),
                preset_id: "minimal".into(),
                source_snapshot: "2026-07-28".into(),
                status: JobStatus::Failed { error: "out of memory".into() },
            }],
            uncovered_regions: vec!["europe/austria".into()],
        };
        assert!(!summary.ok());
        let text = summary.render();
        assert!(text.contains("FAILED JOBS"), "{text}");
        assert!(text.contains("REGIONS WITH NO ARTIFACT"), "{text}");
        assert!(text.contains("out of memory"), "{text}");
    }
}
