//! The landmark stage: a region's coverage polygon in, its compiled landmark artifact out.
//!
//! ```text
//! regions.toml ──▶ .poly ──▶ coverage ──▶ boundary.geojson
//!                .osm.pbf ──▶ wikidata tags ──▶ candidates.json
//!                                              │
//!           policy.json + content-languages ───┤
//!                                              ▼
//!                  <cache>/landmarks/<region>/<recipe>/   raw capture (network)
//!                                              │
//!                                              ▼
//!                         <tree>/landmarks/<region id>/content.json + photos
//!                         <tree>/landmarks/<region id>/landmarks.json
//! ```
//!
//! A third artifact class beside the cells and the terrain, on the same shape as terrain: its own
//! cache, its own skip key, its own command. It reads the region's extract, but only for the QIDs
//! the extract names itself; it is still not a step of the cell bake, because a schema bump must
//! not re-enter it and a Wikipedia edit must not re-bake a cell.
//!
//! # Where candidates come from
//!
//! The extract's explicit `wikidata` tags, and nothing else. Discovery is therefore offline and
//! bounded by the region's own data instead of a bounding box around it, and every landmark has a
//! map object the rider can be routed to. The list is a superset: the compiler applies the exact
//! polygon and the category policy to the captured entities.
//!
//! # The network step
//!
//! Capture is the one non-reproducible step in the whole bakery: it reads live Wikidata, Wikipedia
//! and Commons, and a country takes hours. Everything after it is a pure function of the bytes it
//! wrote. So the stage states in its output when it is about to go out, and `--no-capture` runs the
//! deterministic half alone over whatever the cache already holds.
//!
//! The capture tool owns resumption: each request is content-addressed and an interrupted run
//! reuses every byte it already has. It also refuses to reuse a directory whose recipe moved, so
//! the stage gives each recipe its own directory under the region: a moved border, an edited
//! policy or a new UI language is captured beside the old capture rather than into it, and the
//! bytes an earlier one cost hours to fetch stay where they are.
//!
//! # What "unchanged" means here
//!
//! One key over the capture's own source digests and the four documents that decide what is
//! captured at all: the candidate list, the boundary, the category policy and the shared UI
//! language set. The candidate list itself is cached under the digest of the extract it was read
//! from, so a run with nothing to do digests the extract rather than decoding it. The language set is not a detail of the compiler here — it decides which articles
//! are fetched and which places are eligible, so adding a language must re-capture, not just
//! re-compile. The candidate list is derived, not curated, so an extract that tags one more object
//! is a new recipe and a new capture directory.
//!
//! The compiler's digest of itself is recorded beside the artifact rather than keyed on, because
//! it is only known after a compile; [`LANDMARK_RECIPE_VERSION`] is what an operator moves when
//! the compiled bytes must change for unchanged sources.

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
pub const LANDMARK_RECIPE_VERSION: u32 = 2;

/// The reserved directory name, in the cache and in the tree.
pub const LANDMARKS_DIR: &str = "landmarks";
/// The compiled artifact the packer reads.
pub const CONTENT_DOC: &str = "content.json";
/// The artifact's own declaration, beside it.
pub const LANDMARK_DOC: &str = "landmarks.json";
/// The QID list the capture is pinned to, beside the boundary in the region's cache. One file per
/// extract, because that is what the list is a function of.
const CANDIDATE_STEM: &str = "candidates";
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
    pub language_sha256: String,
    pub candidates_sha256: String,
    /// The article languages the artifact stores, as the compiler reports them.
    pub languages: Vec<String>,
    /// The compiler's own digest of its rules, recorded for provenance rather than keyed on.
    pub compiler_policy_sha256: String,
    /// Over every file of the artifact, `content.json` and the photos alike. The photos are most
    /// of it, so a digest of `content.json` alone would call an artifact with a lost photo intact.
    pub artifact_sha256: String,
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
    /// The candidate QIDs `extract` names, as the `candidates.json` a capture is pinned to.
    /// `extract_sha256` is the stage's digest of the same file, so the extract is read once more,
    /// not twice.
    fn candidates(&self, extract: &Path, extract_sha256: &str) -> Result<Vec<u8>, String>;
    /// Capture the `candidates` inside `boundary` under `policy` into `out`, resuming whatever
    /// `out` already holds.
    fn capture(
        &self,
        boundary: &Path,
        policy: &Path,
        candidates: &Path,
        out: &Path,
        progress: &Progress,
    ) -> Result<(), String>;
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
        let script = std::env::var_os("OBC_LANDMARK_CAPTURE_TOOL")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("tools/landmark_capture.py"));
        if !script.is_file() {
            return Err(format!(
                "{} is not there — set OBC_LANDMARK_CAPTURE_TOOL to tools/landmark_capture.py, or run `obc bake \
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

    /// In this process, not in the tool: discovery is offline and deterministic, so it belongs to
    /// the binary that compiles, and the stage needs its digest before it can name a capture
    /// directory.
    fn candidates(&self, extract: &Path, extract_sha256: &str) -> Result<Vec<u8>, String> {
        let candidates = obc_pack::landmarks::discover::candidates(extract, extract_sha256)?;
        serde_json::to_vec_pretty(&candidates).map_err(|e| e.to_string())
    }

    fn capture(
        &self,
        boundary: &Path,
        policy: &Path,
        candidates: &Path,
        out: &Path,
        progress: &Progress,
    ) -> Result<(), String> {
        progress.check()?;
        let status = Command::new(&self.python)
            .arg(&self.script)
            .arg("--boundary")
            .arg(boundary)
            .arg("--policy")
            .arg(policy)
            .arg("--candidates")
            .arg(candidates)
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
        let mut warnings = Vec::new();
        let mut outcomes = Vec::new();
        for region in self.regions {
            match self.one(region, &policy, progress) {
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

    fn one(&self, region: &Region, policy: &Path, progress: &Progress) -> Result<LandmarkOutcome, String> {
        let poly = self.source.fetch_poly(region, progress)?;
        let coverage = Coverage::parse_poly(&poly).map_err(|e| format!("{}.poly: {e}", region.id))?;
        let boundary_text = coverage.geojson();
        let region_cache = self.opts.cache.join(LANDMARKS_DIR).join(flat(region));
        std::fs::create_dir_all(&region_cache).map_err(|e| format!("{}: {e}", region_cache.display()))?;
        let (candidates, candidate_bytes) = self.candidates(region, &region_cache, progress)?;
        let recipe = Recipe {
            boundary_sha256: crate::hash::text(&boundary_text),
            policy_sha256: crate::hash::bytes(obc_pack::landmarks::POLICY_BYTES),
            language_sha256: crate::hash::bytes(obc_pack::landmarks::LANGUAGE_BYTES),
            candidates_sha256: crate::hash::bytes(&candidate_bytes),
        };
        let capture_dir = region_cache.join(recipe.key());
        let boundary = region_cache.join(format!("{}.geojson", recipe.key()));
        std::fs::write(&boundary, &boundary_text).map_err(|e| format!("{}: {e}", boundary.display()))?;

        let mut status = LandmarkStatus::Compiled;
        if !capture_current(&capture_dir, &recipe)? {
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
            self.capture.capture(&boundary, policy, &candidates, &capture_dir, progress)?;
            if !capture_current(&capture_dir, &recipe)? {
                return Err(format!("{} captured no usable sources for this boundary", capture_dir.display()));
            }
            status = LandmarkStatus::Captured;
        }

        let manifest = capture_dir.join(CAPTURE_MANIFEST);
        let fingerprint = fingerprint(&manifest, &recipe)?;
        let artifact = self.artifact_dir(region);
        let skippable = !self.opts.force && status != LandmarkStatus::Captured;
        if skippable && read_current(&artifact, &fingerprint)? {
            let (records, photos, bytes) = measure(&artifact)?;
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
        write_json(
            &staging.join(LANDMARK_DOC),
            &LandmarkDoc {
                region_id: region.id.clone(),
                recipe_version: LANDMARK_RECIPE_VERSION,
                fingerprint,
                policy_sha256: recipe.policy_sha256,
                boundary_sha256: recipe.boundary_sha256,
                language_sha256: recipe.language_sha256,
                candidates_sha256: recipe.candidates_sha256,
                languages: content.languages.clone(),
                compiler_policy_sha256: content.policy_sha256.clone(),
                artifact_sha256: artifact_digest(&staging)?.0,
                built_at: obc_pack::catalog::now_timestamp(),
            },
        )?;
        let _ = std::fs::remove_dir_all(&artifact);
        if let Some(parent) = artifact.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::rename(&staging, &artifact)
            .map_err(|e| format!("{} -> {}: {e}", staging.display(), artifact.display()))?;

        let (records, photos, bytes) = measure(&artifact)?;
        Ok(LandmarkOutcome { region_id: region.id.clone(), status, records, photos, bytes })
    }

    /// The region's candidate list, and the file it is in.
    ///
    /// Keyed on the extract's digest, because reading a country's `wikidata` tags is a full decode
    /// of a multi-gigabyte file and every run needs the list, even one with nothing to do. The
    /// digest is one read; the decode only happens for an extract this cache has not seen.
    fn candidates(
        &self,
        region: &Region,
        region_cache: &Path,
        progress: &Progress,
    ) -> Result<(PathBuf, Vec<u8>), String> {
        let extract = self.source.fetch(region, progress)?;
        let (_, extract_sha256) = crate::hash::file(&extract.path)?;
        let path = region_cache.join(format!("{CANDIDATE_STEM}-{extract_sha256}.json"));
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok((path, bytes));
        }
        progress.log(format!("  {}: reading the wikidata tags of {}", region.id, extract.path.display()));
        let bytes = self.capture.candidates(&extract.path, &extract_sha256)?;
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((path, bytes))
    }

    /// `<tree>/landmarks/<id segments>/`, the same nesting the tree gives a region document.
    fn artifact_dir(&self, region: &Region) -> PathBuf {
        artifact_dir(&self.opts.out, region)
    }
}

/// `<tree>/landmarks/<id segments>/`, the same nesting the tree gives a region document.
pub fn artifact_dir(tree: &Path, region: &Region) -> PathBuf {
    let mut path = tree.join(LANDMARKS_DIR);
    for segment in region.segments() {
        path = path.join(segment);
    }
    path
}

/// The region's compiled content in this tree, when this stage has put one there.
///
/// Discovered rather than flagged, for the reason the terrain a cell samples is discovered: the
/// landmarks a cell carries must be the landmarks the same catalog publishes, and a flag would be
/// a second place for the two to disagree.
///
/// [`CONTENT_DOC`] alone is the contract, deliberately: the packer reads that document and the
/// photos beside it, and nothing else. A directory a recipe fills by hand, with no [`LANDMARK_DOC`]
/// declaration, is a complete input.
pub fn in_tree(tree: &Path, region: &Region) -> Option<PathBuf> {
    let content = artifact_dir(tree, region).join(CONTENT_DOC);
    content.is_file().then_some(content)
}

/// The capture cache's directory name: the id flattened, like every other per-region cache entry.
fn flat(region: &Region) -> String {
    region.id.replace('/', "_")
}

/// The three documents that decide what a capture asks for.
///
/// Together they are the capture's identity: the capture tool stamps them into its own
/// `recipe.json` and refuses to reuse a directory they moved under, so the stage gives each of
/// them its own directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recipe {
    boundary_sha256: String,
    policy_sha256: String,
    /// `specs/content-languages.json`. It decides which article editions are fetched and which
    /// places are eligible at all, so it belongs here and not only in the compiler's own digest.
    language_sha256: String,
    /// The QIDs read from the region's extract. A retagged object is a different capture.
    candidates_sha256: String,
}

impl Recipe {
    /// The cache directory name: short, because it sits under the region's own directory and only
    /// has to separate one recipe from the next.
    fn key(&self) -> String {
        crate::hash::text(&format!(
            "boundary={}\npolicy={}\nlanguages={}\ncandidates={}\n",
            self.boundary_sha256, self.policy_sha256, self.language_sha256, self.candidates_sha256
        ))[..12]
            .to_string()
    }
}

/// What the capture stamped about itself.
#[derive(Debug, Deserialize)]
struct CaptureRecipe {
    boundary_sha256: String,
    policy_sha256: String,
    language_sha256: String,
    candidates_sha256: String,
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

/// Whether this directory holds a finished capture made under exactly this recipe.
///
/// The recipe is compared even though the directory is named after it: the name is a truncated
/// digest, and a capture whose recipe disagrees must never be compiled — it would publish
/// landmarks for ground or in languages the artifact does not claim.
fn capture_current(dir: &Path, recipe: &Recipe) -> Result<bool, String> {
    let recipe_path = dir.join(CAPTURE_RECIPE);
    let Ok(text) = std::fs::read_to_string(&recipe_path) else { return Ok(false) };
    let captured: CaptureRecipe = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", recipe_path.display()))?;
    if captured.boundary_sha256 != recipe.boundary_sha256
        || captured.policy_sha256 != recipe.policy_sha256
        || captured.language_sha256 != recipe.language_sha256
        || captured.candidates_sha256 != recipe.candidates_sha256
    {
        return Ok(false);
    }
    Ok(read_manifest(&dir.join(CAPTURE_MANIFEST))?.is_some_and(|m| m.coverage.country_complete))
}

fn read_manifest(path: &Path) -> Result<Option<CaptureManifest>, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(None) };
    serde_json::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

/// The skip key: every source byte the compiler will read, plus the recipe they were fetched under.
///
/// The digests are the capture's own, over the raw API responses, so a re-capture that fetched the
/// same revisions again is not a re-compile.
fn fingerprint(manifest: &Path, recipe: &Recipe) -> Result<String, String> {
    let manifest =
        read_manifest(manifest)?.ok_or_else(|| format!("{}: no capture manifest to key on", manifest.display()))?;
    let sources: BTreeMap<String, String> = manifest.sources.into_iter().map(|s| (s.path, s.sha256)).collect();
    let mut key = format!(
        "landmark-recipe={LANDMARK_RECIPE_VERSION}\nboundary={}\npolicy={}\nlanguages={}\ncandidates={}\n",
        recipe.boundary_sha256, recipe.policy_sha256, recipe.language_sha256, recipe.candidates_sha256
    );
    for (path, sha256) in sources {
        key.push_str(&format!("{path}={sha256}\n"));
    }
    Ok(crate::hash::text(&key))
}

/// Whether the tree already holds this artifact, compiled from this key, with every file intact.
fn read_current(artifact: &Path, fingerprint: &str) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(artifact.join(LANDMARK_DOC)) else { return Ok(false) };
    let Ok(doc) = serde_json::from_str::<LandmarkDoc>(&text) else { return Ok(false) };
    if doc.fingerprint != fingerprint || !artifact.join(CONTENT_DOC).is_file() {
        return Ok(false);
    }
    Ok(artifact_digest(artifact)?.0 == doc.artifact_sha256)
}

/// One digest over every file of the artifact but its own declaration, and their total size.
///
/// Name and digest per file, so a photo that is lost, renamed or swapped moves the result. The
/// declaration is excluded because it carries this digest.
fn artifact_digest(artifact: &Path) -> Result<(String, u64), String> {
    let mut files = BTreeMap::new();
    let mut bytes = 0;
    for entry in std::fs::read_dir(artifact).map_err(|e| format!("{}: {e}", artifact.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", artifact.display()))?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if name == LANDMARK_DOC || !path.is_file() {
            continue;
        }
        let (size, sha256) = crate::hash::file(&path)?;
        bytes += size;
        files.insert(name, sha256);
    }
    let listing: String = files.iter().map(|(name, sha256)| format!("{name}={sha256}\n")).collect();
    Ok((crate::hash::text(&listing), bytes))
}

/// What an artifact holds: records, photos, and its size on disk.
fn measure(artifact: &Path) -> Result<(usize, usize, u64), String> {
    let path = artifact.join(CONTENT_DOC);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let content: obc_pack::landmarks::Content =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((content.records.len(), content.counts.images, artifact_digest(artifact)?.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The language set reaches both keys. The capture key is what gives a new language set its own
    /// directory; the fingerprint is what re-compiles when the sources under it did not move —
    /// which is exactly the case a capture with no places produces.
    #[test]
    fn the_language_set_moves_the_capture_key_and_the_fingerprint() {
        let base = Recipe {
            boundary_sha256: "b".into(),
            policy_sha256: "p".into(),
            language_sha256: "en-de-fr-es".into(),
            candidates_sha256: "c".into(),
        };
        let fifth = Recipe { language_sha256: "en-de-fr-es-it".into(), ..base.clone() };
        assert_ne!(base.key(), fifth.key(), "a new language set captures into its own directory");

        let dir = std::env::temp_dir().join(format!("obc-bake-landmark-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join(CAPTURE_MANIFEST);
        std::fs::write(&manifest, r#"{"sources":[],"coverage":{"country_complete":true}}"#).unwrap();
        assert_ne!(fingerprint(&manifest, &base).unwrap(), fingerprint(&manifest, &fifth).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
