//! The landmark artifact class in a bake tree: `landmarks/<region id>/`.
//!
//! The third artifact class, read on its own terms the way terrain is. Nothing a consumer
//! downloads: a cell already carries the landmark sections cut from these artifacts, so the
//! published copies are the provenance and the licence record behind that content
//! (`OBCC_Spec.md` §14).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::landmarks::{artifact_digest, Content, CONTENT_DOC, DECLARATION_DOC};

use super::model::LandmarkArtifactEntry;
use super::validate::{validate_id, validate_timestamp};
use super::{file_name, sorted_entries};

pub const LANDMARKS_DIR: &str = "landmarks";

/// What the stage that compiled an artifact declares beside it. Only the fields the catalog
/// publishes or checks: the stage owns the rest, and a field it adds must not fail generation.
#[derive(Debug, Deserialize)]
struct LandmarkDeclaration {
    region_id: String,
    languages: Vec<String>,
    /// Over every file of the artifact but this one.
    artifact_sha256: String,
    built_at: String,
}

/// Every landmark artifact directory in `tree`, as `(region id, path)`, sorted by id.
///
/// Public because the bake's verify walks the same set from the other side: the catalog says which
/// artifacts it published, and a directory the catalog does not name is one that arrived after
/// generation.
pub fn artifact_dirs(tree: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let root = tree.join(LANDMARKS_DIR);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    collect_artifact_dirs(&root, &mut Vec::new(), &mut found)?;
    found.sort();
    Ok(found)
}

/// Every landmark artifact the tree holds, ready to publish.
pub(super) struct LandmarkStore {
    pub(super) artifacts: Vec<LandmarkArtifactEntry>,
}

/// Walk `landmarks/` into the store, or `None` when the tree holds no artifact.
///
/// `regions` is the catalog's own region set: an artifact is per region, so one for a region the
/// catalog does not offer is a tree someone rearranged, not a catalog to publish.
pub(super) fn read_landmarks(
    tree: &Path,
    regions: &BTreeSet<&str>,
    base_url: &str,
) -> Result<Option<LandmarkStore>, String> {
    let found = artifact_dirs(tree)?;
    if found.is_empty() {
        return Ok(None);
    }
    let mut artifacts = Vec::new();
    for (id, dir) in &found {
        if !regions.contains(id.as_str()) {
            return Err(format!(
                "{}: a landmark artifact for `{id}`, which this catalog publishes no region for — a landmark \
                 artifact is compiled from one region's boundary and belongs to it (OBCC_Spec.md §14)",
                dir.display()
            ));
        }
        artifacts.push(read_artifact(id, dir, base_url)?);
    }
    artifacts.sort_by(|a, b| a.region_id.cmp(&b.region_id));
    Ok(Some(LandmarkStore { artifacts }))
}

fn collect_artifact_dirs(
    dir: &Path,
    segments: &mut Vec<String>,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let mut subdirs = Vec::new();
    let mut is_artifact = false;
    for entry in sorted_entries(dir)? {
        let name = file_name(&entry)?;
        if name.starts_with('.') {
            continue;
        }
        if entry.is_dir() {
            validate_id(&name).map_err(|e| format!("{}: landmark path segment {e}", entry.display()))?;
            subdirs.push((name, entry));
        } else if name == CONTENT_DOC {
            is_artifact = true;
        }
    }
    if is_artifact {
        if segments.is_empty() {
            return Err(format!("{}: a landmark artifact lives under `{LANDMARKS_DIR}/<region id>/`", dir.display()));
        }
        out.push((segments.join("/"), dir.to_path_buf()));
    }
    for (name, path) in subdirs {
        segments.push(name);
        collect_artifact_dirs(&path, segments, out)?;
        segments.pop();
    }
    Ok(())
}

/// One artifact: what it holds, what it costs, and which licences its bytes are under.
///
/// The declaration is required here although the cell bake reads a directory without one: a cut is
/// local and a publication is not, and a published artifact that cannot say what it was compiled
/// from is a licence statement no one can check.
fn read_artifact(id: &str, dir: &Path, base_url: &str) -> Result<LandmarkArtifactEntry, String> {
    let declaration_path = dir.join(DECLARATION_DOC);
    let text = std::fs::read_to_string(&declaration_path).map_err(|e| {
        format!(
            "{}: {e} — a published landmark artifact MUST declare what it was compiled from; run `obc bake \
             landmarks {id}` (OBCC_Spec.md §14)",
            declaration_path.display()
        )
    })?;
    let declaration: LandmarkDeclaration =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", declaration_path.display()))?;
    if declaration.region_id != id {
        return Err(format!(
            "{}: declares region `{}` but lies at `{id}`",
            declaration_path.display(),
            declaration.region_id
        ));
    }
    validate_timestamp(&declaration.built_at).map_err(|e| format!("{}: built_at {e}", declaration_path.display()))?;

    let (sha256, bytes) = artifact_digest(dir)?;
    if sha256 != declaration.artifact_sha256 {
        return Err(format!(
            "{}: the artifact hashes to {sha256} but its declaration says {} — a file beside `{CONTENT_DOC}` was \
             lost, renamed or replaced after the compile",
            dir.display(),
            declaration.artifact_sha256
        ));
    }

    let content_path = dir.join(CONTENT_DOC);
    let content: Content = serde_json::from_str(
        &std::fs::read_to_string(&content_path).map_err(|e| format!("{}: {e}", content_path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", content_path.display()))?;
    let mut licenses = BTreeSet::new();
    let mut photos = 0;
    for record in &content.records {
        for variant in &record.variants {
            licenses.insert(variant.attribution.license_url.clone());
        }
        if let Some(photo) = &record.photo {
            photos += 1;
            licenses.insert(photo.attribution.license_url.clone());
        }
    }
    Ok(LandmarkArtifactEntry {
        region_id: id.to_string(),
        languages: declaration.languages,
        records: content.records.len() as u32,
        photos,
        bytes,
        sha256,
        licenses: licenses.into_iter().collect(),
        url: format!("{base_url}/{LANDMARKS_DIR}/{id}/{CONTENT_DOC}"),
        built_at: declaration.built_at,
    })
}
