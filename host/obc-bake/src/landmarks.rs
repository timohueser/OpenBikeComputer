//! The landmark stage: a region's coverage polygon in, its compiled landmark artifact out.
//!
//! ```text
//! regions.toml ──▶ .poly ──▶ coverage ──▶ boundary.geojson
//!                                              │
//!                     policy.json ─────────────┤
//!                                              ▼
//!                         <cache>/landmarks/<region>/     raw capture (network)
//!                                              │
//!                                              ▼
//!                         <tree>/landmarks/<region id>/content.json + photos
//!                         <tree>/landmarks/<region id>/landmarks.json
//! ```
//!
//! A third artifact class beside the cells and the terrain, on the same shape as terrain: its own
//! cache, its own skip key, its own command. It is not a step of the cell bake because its inputs
//! are neither the OSM extract nor the schema — a schema bump must not re-enter it, and a Wikipedia
//! edit must not re-bake a cell.
//!
//! # The network step
//!
//! Capture is the one non-reproducible step in the whole bakery: it reads live Wikidata, Wikipedia
//! and Commons, and a country takes hours. Everything after it is a pure function of the bytes it
//! wrote. So the stage states in its output when it is about to go out, and `--no-capture` runs the
//! deterministic half alone over whatever the cache already holds.
//!
//! The capture tool owns resumption: each request is content-addressed and an interrupted run
//! reuses every byte it already has. The stage only decides whether to call it, by reading the
//! recipe the capture stamped — a capture made for a different boundary or a different policy is
//! never compiled, it is re-captured into its own directory.
//!
//! # What "unchanged" means here
//!
//! One key over the capture's own source digests, the boundary and the policy: exactly the bytes
//! the compiler reads. The compiler's digest of itself is recorded beside the artifact rather than
//! keyed on, because it is only known after a compile; [`LANDMARK_RECIPE_VERSION`] is what an
//! operator moves when the compiled bytes must change for unchanged sources.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_pack::progress::Progress;
use serde::{Deserialize, Serialize};

use crate::coverage::Coverage;
use crate::regions::Region;
use crate::source::ExtractSource;
use crate::util::write_json;

/// Bumped when a change in this stage alters a published landmark artifact for unchanged inputs.
pub const LANDMARK_RECIPE_VERSION: u32 = 1;

/// The reserved directory name, in the cache and in the tree.
pub const LANDMARKS_DIR: &str = "landmarks";
/// The compiled artifact the packer reads.
pub const CONTENT_DOC: &str = "content.json";
/// The artifact's own declaration, beside it.
pub const LANDMARK_DOC: &str = "landmarks.json";
const CAPTURE_MANIFEST: &str = "manifest.json";
const CAPTURE_RECIPE: &str = "recipe.json";
const POLICY_DOC: &str = "policy.json";

/// `landmarks.json`: what this artifact was compiled from, and at which key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandmarkDoc {
    pub region_id: String,
    pub recipe_version: u32,
    /// The skip key: the capture's source digests, the boundary and the policy.
    pub fingerprint: String,
    pub policy_sha256: String,
    pub boundary_sha256: String,
    /// The article languages the artifact stores, as the compiler reports them.
    pub languages: Vec<String>,
    /// The compiler's own digest of its rules, recorded for provenance rather than keyed on.
    pub compiler_policy_sha256: String,
    pub content_sha256: String,
    pub built_at: String,
}

/// What captures a region's raw sources.
///
/// A trait for the reason [`crate::terrain::TerrainCutter`] is one: the stage's real content is the
/// boundary derivation, the skip key and the tree layout, none of which should need the live
/// Wikidata API to test.
pub trait LandmarkCapture {
    /// Where captures come from, for the run header.
    fn describe(&self) -> String;
    /// Capture `boundary` under `policy` into `out`, resuming whatever `out` already holds.
    fn capture(&self, boundary: &Path, policy: &Path, out: &Path, progress: &Progress) -> Result<(), String>;
}

/// The real capture: `tools/landmark_capture.py`.
///
/// Spawned rather than linked, the way the planet bake spawns `pyosmium-up-to-date`: the tool is
/// the repository's one rate-limited, resumable API client, and a second implementation of its
/// politeness and its content addressing is the last thing this stage should own.
pub struct PythonCapture {
    python: PathBuf,
    script: PathBuf,
    /// The binary that selects candidates before assets are acquired — this one, so capture and
    /// compile can never be two different builds of the same policy.
    compiler: PathBuf,
}

impl PythonCapture {
    pub fn from_env() -> Result<Self, String> {
        let script = std::env::var_os("OBC_LANDMARK_CAPTURE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("tools/landmark_capture.py"));
        if !script.is_file() {
            return Err(format!(
                "{} is not there — set OBC_LANDMARK_CAPTURE to tools/landmark_capture.py, or run `obc bake \
                 landmarks`, which sets it",
                script.display()
            ));
        }
        let compiler = std::env::current_exe().map_err(|e| format!("this binary's own path: {e}"))?;
        let python = std::env::var_os("OBC_PYTHON").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("python3"));
        Ok(Self { python, script, compiler })
    }
}

impl LandmarkCapture for PythonCapture {
    fn describe(&self) -> String {
        format!("{} {}", self.python.display(), self.script.display())
    }

    fn capture(&self, boundary: &Path, policy: &Path, out: &Path, progress: &Progress) -> Result<(), String> {
        progress.check()?;
        let status = Command::new(&self.python)
            .arg(&self.script)
            .arg("--boundary")
            .arg(boundary)
            .arg("--policy")
            .arg(policy)
            .arg("--out")
            .arg(out)
            .arg("--select-with")
            .arg(&self.compiler)
            .status()
            .map_err(|e| format!("run {}: {e}", self.script.display()))?;
        match status.code() {
            Some(0) => Ok(()),
            // The tool's own "captured, but not everything the boundary asks for". Its cache keeps
            // what it got, so the next run resumes rather than starts over.
            Some(2) => Err("the capture is incomplete — re-run to resume it".into()),
            _ => Err(format!("{} failed with {status}", self.script.display())),
        }
    }
}

/// How a landmark run is scoped and where it writes.
#[derive(Debug, Clone)]
pub struct LandmarkBakeOptions {
    pub out: PathBuf,
    /// Where raw captures live, one directory per region.
    pub cache: PathBuf,
    /// Re-compile even when the key says nothing changed.
    pub force: bool,
    /// Never call the capture tool: compile what the cache already holds, and report the regions it
    /// does not.
    pub no_capture: bool,
}

/// How one region's artifact ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LandmarkStatus {
    /// The capture tool ran: this region went to the network, and the artifact was compiled after.
    Captured,
    /// The cached capture was current and the artifact was compiled from it.
    Compiled,
    /// The key matched and the artifact on disk still matches its recorded digest.
    Unchanged,
    /// The cache holds no current capture and this run may not make one.
    CaptureMissing,
}

#[derive(Debug, Clone, Serialize)]
pub struct LandmarkOutcome {
    pub region_id: String,
    pub status: LandmarkStatus,
    pub records: usize,
    pub photos: usize,
    pub bytes: u64,
}

/// Everything a landmark run did.
#[derive(Debug, Clone, Serialize)]
pub struct LandmarkRunSummary {
    pub tree: PathBuf,
    pub cache: PathBuf,
    pub recipe_version: u32,
    pub regions: Vec<LandmarkOutcome>,
    pub warnings: Vec<String>,
}

impl LandmarkRunSummary {
    pub fn bytes(&self) -> u64 {
        self.regions.iter().map(|r| r.bytes).sum()
    }

    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "\n=== landmark bake summary ({}) ===", self.tree.display());
        let _ = writeln!(s, "capture cache {} — recipe v{}", self.cache.display(), self.recipe_version);
        let count = |want: LandmarkStatus| self.regions.iter().filter(|r| r.status == want).count();
        let _ = writeln!(
            s,
            "{} region(s): {} captured from the network, {} compiled from the cache, {} unchanged, {} without a \
             capture, {}",
            self.regions.len(),
            count(LandmarkStatus::Captured),
            count(LandmarkStatus::Compiled),
            count(LandmarkStatus::Unchanged),
            count(LandmarkStatus::CaptureMissing),
            crate::util::human_bytes(self.bytes())
        );
        for region in &self.regions {
            let _ = writeln!(
                s,
                "  {:<44} {:>6} record(s), {:>5} photo(s)  {:?}",
                region.region_id, region.records, region.photos, region.status
            );
        }
        for w in &self.warnings {
            let _ = writeln!(s, "\nwarning: {w}");
        }
        s
    }

    /// Whether every selected region has a compiled artifact.
    pub fn ok(&self) -> bool {
        self.regions.iter().all(|r| r.status != LandmarkStatus::CaptureMissing) && self.warnings.is_empty()
    }
}

/// A configured landmark run.
pub struct LandmarkBakery<'a> {
    pub regions: &'a [Region],
    pub source: &'a dyn ExtractSource,
    pub capture: &'a dyn LandmarkCapture,
    pub opts: LandmarkBakeOptions,
}

impl LandmarkBakery<'_> {
    /// Capture and compile every selected region's landmark artifact.
    pub fn run(&self, progress: &Progress) -> Result<LandmarkRunSummary, String> {
        if self.regions.is_empty() {
            return Err("no regions — a landmark bake is per region, so it needs one".into());
        }
        progress.log(format!("landmark bakery: {} region(s)", self.regions.len()));
        progress.log(format!("  tree:    {}", self.opts.out.display()));
        progress.log(format!("  cache:   {}", self.opts.cache.display()));
        progress.log(format!("  capture: {}", self.capture.describe()));

        let policy = self.write_policy()?;
        let policy_sha256 = crate::hash::bytes(obc_pack::landmarks::POLICY_BYTES);

        let mut warnings = Vec::new();
        let mut outcomes = Vec::new();
        for region in self.regions {
            match self.one(region, &policy, &policy_sha256, progress) {
                Ok(outcome) => outcomes.push(outcome),
                Err(e) => {
                    progress.warn(format!("  {}: {e}", region.id));
                    warnings.push(format!("{}: {e}", region.id));
                }
            }
        }
        Ok(LandmarkRunSummary {
            tree: self.opts.out.clone(),
            cache: self.opts.cache.clone(),
            recipe_version: LANDMARK_RECIPE_VERSION,
            regions: outcomes,
            warnings,
        })
    }

    /// The curated policy, written into the cache so the capture tool reads the same bytes the
    /// compiler is built with rather than a copy in the source tree.
    fn write_policy(&self) -> Result<PathBuf, String> {
        let path = self.opts.cache.join(LANDMARKS_DIR).join(POLICY_DOC);
        let dir = path.parent().expect("the policy path has a parent");
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&path, obc_pack::landmarks::POLICY_BYTES).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    fn one(
        &self,
        region: &Region,
        policy: &Path,
        policy_sha256: &str,
        progress: &Progress,
    ) -> Result<LandmarkOutcome, String> {
        let poly = self.source.fetch_poly(region, progress)?;
        let coverage = Coverage::parse_poly(&poly).map_err(|e| format!("{}.poly: {e}", region.id))?;
        let boundary_text = coverage.geojson();
        let boundary_sha256 = crate::hash::text(&boundary_text);
        let boundary = self.opts.cache.join(LANDMARKS_DIR).join(format!("{}.geojson", flat(region)));
        std::fs::write(&boundary, &boundary_text).map_err(|e| format!("{}: {e}", boundary.display()))?;

        let capture_dir = self.opts.cache.join(LANDMARKS_DIR).join(flat(region));
        let mut status = LandmarkStatus::Compiled;
        if !capture_current(&capture_dir, &boundary_sha256, policy_sha256)? {
            if self.opts.no_capture {
                progress.log(format!("  {}: no current capture in {}", region.id, capture_dir.display()));
                return Ok(LandmarkOutcome {
                    region_id: region.id.clone(),
                    status: LandmarkStatus::CaptureMissing,
                    records: 0,
                    photos: 0,
                    bytes: 0,
                });
            }
            progress.log(format!(
                "  {}: capturing into {} — this is the network step, and the only step of any bake that two runs \
                 can disagree on",
                region.id,
                capture_dir.display()
            ));
            self.capture.capture(&boundary, policy, &capture_dir, progress)?;
            if !capture_current(&capture_dir, &boundary_sha256, policy_sha256)? {
                return Err(format!("{} captured no usable sources for this boundary", capture_dir.display()));
            }
            status = LandmarkStatus::Captured;
        }

        let manifest = capture_dir.join(CAPTURE_MANIFEST);
        let fingerprint = fingerprint(&manifest, &boundary_sha256, policy_sha256)?;
        let artifact = self.artifact_dir(region);
        let skippable = !self.opts.force && status != LandmarkStatus::Captured;
        if skippable && read_current(&artifact, &fingerprint)? {
            let (bytes, records, photos) = measure(&artifact)?;
            return Ok(LandmarkOutcome {
                region_id: region.id.clone(),
                status: LandmarkStatus::Unchanged,
                records,
                photos,
                bytes,
            });
        }

        // The compiler refuses a directory that is not empty, and rightly so: a stale photo beside
        // a fresh `content.json` is an artifact no run can account for. So it compiles beside the
        // artifact and the finished directory replaces it.
        let staging = artifact.with_extension("part");
        let _ = std::fs::remove_dir_all(&staging);
        let content = obc_pack::landmarks::compile(&manifest, &boundary, &staging)?;
        let content_sha256 = crate::hash::file(&staging.join(CONTENT_DOC))?.1;
        write_json(
            &staging.join(LANDMARK_DOC),
            &LandmarkDoc {
                region_id: region.id.clone(),
                recipe_version: LANDMARK_RECIPE_VERSION,
                fingerprint,
                policy_sha256: policy_sha256.to_string(),
                boundary_sha256,
                languages: content.languages.clone(),
                compiler_policy_sha256: content.policy_sha256.clone(),
                content_sha256,
                built_at: obc_pack::catalog::now_timestamp(),
            },
        )?;
        let _ = std::fs::remove_dir_all(&artifact);
        if let Some(parent) = artifact.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::rename(&staging, &artifact)
            .map_err(|e| format!("{} -> {}: {e}", staging.display(), artifact.display()))?;

        let (bytes, records, photos) = measure(&artifact)?;
        Ok(LandmarkOutcome { region_id: region.id.clone(), status, records, photos, bytes })
    }

    /// `<tree>/landmarks/<id segments>/`, the same nesting the tree gives a region document.
    fn artifact_dir(&self, region: &Region) -> PathBuf {
        let mut path = self.opts.out.join(LANDMARKS_DIR);
        for segment in region.segments() {
            path = path.join(segment);
        }
        path
    }
}

/// The capture cache's directory name: the id flattened, like every other per-region cache entry.
fn flat(region: &Region) -> String {
    region.id.replace('/', "_")
}

/// What the capture stamped about itself.
#[derive(Debug, Deserialize)]
struct CaptureRecipe {
    boundary_sha256: String,
    policy_sha256: String,
}

#[derive(Debug, Deserialize)]
struct CaptureManifest {
    sources: Vec<CaptureSource>,
    coverage: CaptureCoverage,
}

#[derive(Debug, Deserialize)]
struct CaptureSource {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct CaptureCoverage {
    /// The tool's own verdict that it asked everything the boundary and the policy ask for.
    country_complete: bool,
}

/// Whether the cache holds a finished capture of exactly this boundary under exactly this policy.
///
/// The recipe check is the one that matters: a capture directory is named after a region, and a
/// region's border or the policy's roots can move under it. Compiling such a capture would publish
/// landmarks for ground the artifact no longer claims.
fn capture_current(dir: &Path, boundary_sha256: &str, policy_sha256: &str) -> Result<bool, String> {
    let recipe_path = dir.join(CAPTURE_RECIPE);
    let Ok(text) = std::fs::read_to_string(&recipe_path) else { return Ok(false) };
    let recipe: CaptureRecipe = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", recipe_path.display()))?;
    if recipe.boundary_sha256 != boundary_sha256 || recipe.policy_sha256 != policy_sha256 {
        return Ok(false);
    }
    Ok(read_manifest(&dir.join(CAPTURE_MANIFEST))?.is_some_and(|m| m.coverage.country_complete))
}

fn read_manifest(path: &Path) -> Result<Option<CaptureManifest>, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(None) };
    serde_json::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

/// The skip key: every source byte the compiler will read, plus the boundary and the policy.
///
/// The digests are the capture's own, over the raw API responses, so a re-capture that fetched the
/// same revisions again is not a re-compile.
fn fingerprint(manifest: &Path, boundary_sha256: &str, policy_sha256: &str) -> Result<String, String> {
    let manifest =
        read_manifest(manifest)?.ok_or_else(|| format!("{}: no capture manifest to key on", manifest.display()))?;
    let sources: BTreeMap<String, String> = manifest.sources.into_iter().map(|s| (s.path, s.sha256)).collect();
    let mut key =
        format!("landmark-recipe={LANDMARK_RECIPE_VERSION}\nboundary={boundary_sha256}\npolicy={policy_sha256}\n");
    for (path, sha256) in sources {
        key.push_str(&format!("{path}={sha256}\n"));
    }
    Ok(crate::hash::text(&key))
}

/// Whether the tree already holds this artifact, compiled from this key, with its bytes intact.
fn read_current(artifact: &Path, fingerprint: &str) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(artifact.join(LANDMARK_DOC)) else { return Ok(false) };
    let Ok(doc) = serde_json::from_str::<LandmarkDoc>(&text) else { return Ok(false) };
    let content = artifact.join(CONTENT_DOC);
    if doc.fingerprint != fingerprint || !content.is_file() {
        return Ok(false);
    }
    Ok(crate::hash::file(&content)?.1 == doc.content_sha256)
}

/// An artifact's size on disk, and what its `content.json` says it holds.
fn measure(artifact: &Path) -> Result<(u64, usize, usize), String> {
    let path = artifact.join(CONTENT_DOC);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let content: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let count = |key: &str| content["counts"][key].as_u64().unwrap_or(0) as usize;
    let mut bytes = 0;
    for entry in std::fs::read_dir(artifact).map_err(|e| format!("{}: {e}", artifact.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", artifact.display()))?;
        bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
    }
    Ok((bytes, content["records"].as_array().map_or(0, Vec::len), count("images")))
}
