//! A product, such as the planner or the device maps: the steps that make its layers for the
//! settings of a target. The steps of a product live in the crate that makes their bytes; the
//! `obc data` binary passes each product to `cli::main`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::engine::{snapshot_files, LayerFile, Step};
use crate::regions::Regions;
use crate::release::{Output, Release};
use crate::settings::Settings;
use crate::sources::compare_versions;
use crate::store::{sorted, Store};

pub trait Product: Sync {
    /// Kebab-case. Each of its step names starts with `<name>/`.
    fn name(&self) -> &'static str;

    /// The optional layers that the settings of a target can switch on.
    fn optional(&self) -> &'static [&'static str] {
        &[]
    }

    /// Its steps for `inputs`, with tool paths relative to `root`. A step list that reads a
    /// snapshot, such as the `.poly` of a region or the Geofabrik index, gives
    /// `Unplanned::NeedsFetch` while the store lacks it.
    fn steps(&self, root: &Path, inputs: &Inputs, regions: &Regions, store: &Store) -> Result<Steps, Unplanned>;

    /// Its steps for a read-only overview, without checks of the tools that the steps run.
    fn status_steps(&self, root: &Path, inputs: &Inputs, regions: &Regions, store: &Store) -> Result<Steps, Unplanned> {
        self.steps(root, inputs, regions, store)
    }

    /// The pointer that its clients read first: one of [`crate::release::POINTERS`].
    fn pointer(&self) -> Pointer;

    /// The files that its clients find by name: each path in its folder, and the client file of an
    /// output that has its bytes.
    fn named(&self, _outputs: &[Output]) -> Result<Vec<(String, LayerFile)>, String> {
        Ok(Vec::new())
    }

    /// Check the files of `release` in the store before a target serves it. `root` locates the
    /// offline verification tools.
    fn verify(
        &self,
        _root: &Path,
        _previous: Option<&Release>,
        _release: &Release,
        _store: &Store,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// A pointer key, and the client file of a step that a client reads there.
pub struct Pointer {
    pub key: &'static str,
    pub step: &'static str,
    pub path: &'static str,
}

/// Usable steps and the layers whose inputs are unavailable. A blocked layer prevents an update,
/// but does not prevent independent steps from listing.
#[derive(Default)]
pub struct Steps {
    pub steps: Vec<Step>,
    pub blocked: Vec<BlockedLayer>,
}

impl Steps {
    /// Remove steps that read a blocked layer, including their dependents.
    pub fn block_dependents(&mut self) {
        while let Some((at, reason)) = self.steps.iter().enumerate().find_map(|(at, step)| {
            let input = step.reads().find_map(|name| self.blocked.iter().find(|b| b.step == name))?;
            Some((at, format!("reads the blocked step `{}`: {}", input.step, input.reason)))
        }) {
            let step = self.steps.remove(at);
            self.blocked.push(BlockedLayer::new(step.name, step.layer, reason));
        }
    }
}

impl From<Vec<Step>> for Steps {
    fn from(steps: Vec<Step>) -> Self {
        Self { steps, blocked: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockedLayer {
    /// The step that cannot be planned.
    pub step: String,
    /// The layer of the step.
    pub layer: String,
    pub reason: String,
}

impl BlockedLayer {
    pub fn new(step: impl Into<String>, layer: &crate::engine::Layer, reason: impl Into<String>) -> Self {
        BlockedLayer { step: step.into(), layer: layer.name.into(), reason: reason.into() }
    }
}

/// Why a product gives no steps.
#[derive(Debug, PartialEq, Eq)]
pub enum Unplanned {
    /// The step list reads these snapshots. `obc data` fetches them and asks once more.
    NeedsFetch(Vec<Wanted>),
    /// The product does not suit the settings, such as a kind of region that it does not read.
    /// `obc data` reports the product as blocked and lists the others.
    Invalid(String),
    /// The store, a file or the data of a fetch failed. The command fails.
    Failed(String),
}

/// A fetch that a step list needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub source: String,
    /// `None` for the newest version upstream.
    pub version: Option<String>,
    pub params: Vec<(String, String)>,
}

/// A fetch: a source id with its `NAME=VALUE`s, sorted.
pub type RequestKey = (String, Vec<(String, String)>);
/// The version of each fetch.
pub type Versions = BTreeMap<RequestKey, String>;

/// What a step list reads: the settings of a target and the version of each fetch.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub settings: Settings,
    /// The version of each fetch that the serving release of the target read.
    pub serving: Versions,
    /// The versions that this update chose: the newer upstream version of a due fetch, and the
    /// version that a fetch of the newest version gave.
    pub resolved: Versions,
    /// The fetches whose refresh policy is due and whose newer upstream version has no name, such
    /// as a capture: a step list reads the newest version upstream.
    pub due: BTreeSet<RequestKey>,
    /// Sources with a `manual` policy: a fetch of one of them keeps one version for all requests.
    pub manual: BTreeSet<String>,
    /// Every fetch that a step list asked for, also one without files in the store.
    pub requests: RefCell<BTreeSet<RequestKey>>,
    /// The fetches that failed in this listing. A product blocks only the layers that need them.
    pub fetch_failures: Vec<(Wanted, String)>,
}

impl Inputs {
    /// The fetch to run for `wanted`. A `manual` source moves as a whole: a request without a
    /// version takes the version that the first fetch of the source gave, so a model fetched tile
    /// by tile over midnight keeps one version.
    pub fn pinned(&self, wanted: &Wanted) -> Wanted {
        let version =
            wanted.version.clone().or_else(|| self.resolved.get(&(wanted.source.clone(), Vec::new())).cloned());
        Wanted { version, ..wanted.clone() }
    }

    /// Record the `version` that a fetch of `wanted` gave. The first of a `manual` source is the
    /// version of the source too.
    pub fn resolve(&mut self, wanted: &Wanted, version: String) {
        if self.manual.contains(&wanted.source) {
            self.resolved.entry((wanted.source.clone(), Vec::new())).or_insert_with(|| version.clone());
        }
        self.resolved.insert((wanted.source.clone(), sorted(&wanted.params)), version);
    }
}

/// The version of the fetch of `source` with `params` that a step list reads:
///
/// 1. For a due fetch, the version that this update took upstream; before that fetch,
///    `Err(Wanted)` names a fetch of the newest version upstream.
/// 2. Otherwise the newest version that this machine knows ([`known`]).
/// 3. Otherwise `Err(Wanted)`: the newest version upstream.
///
/// Every step list gets its versions here, and `inputs` records each fetch that it asks for.
pub fn version(
    inputs: &Inputs,
    store: &Store,
    source: &str,
    params: &[(String, String)],
) -> Result<Result<String, Wanted>, String> {
    let key = (source.to_string(), sorted(params));
    inputs.requests.borrow_mut().insert(key.clone());
    let version = match inputs.resolved.get(&key) {
        Some(version) => Some(version.clone()),
        None if inputs.due.contains(&key) => None,
        None => known(inputs, store, source, params)?,
    };
    Ok(version.ok_or(Wanted { source: source.into(), version: None, params: params.to_vec() }))
}

/// The newest of the serving version of the fetch, its versions in the store and the `start` of
/// its source, by the order of the source's versions. The serving version of a fetch without
/// params, or of any fetch of a source with a `start`, is the newest serving version of the source:
/// one version names all its requests, such as the GLO-30 tiles of every region. A `commit` or a
/// `digest` has no order: its `start`, so that a change of `data/sources.toml` moves it; else the
/// serving version; else the one version in the store, when there is one.
fn known(inputs: &Inputs, store: &Store, source: &str, params: &[(String, String)]) -> Result<Option<String>, String> {
    let declared = crate::sources::all().iter().find(|s| s.id == source);
    let start = declared.and_then(|s| s.start.clone());
    let newest = |versions: &mut dyn Iterator<Item = String>| versions.max_by(|a, b| compare_versions(a, b));
    let serving = match inputs.serving.get(&(source.to_string(), sorted(params))) {
        Some(version) => Some(version.clone()),
        None if params.is_empty() || start.is_some() => {
            newest(&mut inputs.serving.iter().filter(|((id, _), _)| id == source).map(|(_, version)| version.clone()))
        }
        None => None,
    };
    let stored: Vec<String> = match params {
        [] => store.snapshots(source)?.into_iter().map(|snapshot| snapshot.version).collect(),
        params => store.requests(source, params)?.into_iter().map(|request| request.version).collect(),
    };
    if declared.is_some_and(|source| !source.version.ordered()) {
        let only = <[String; 1]>::try_from(stored).ok().map(|[version]| version);
        return Ok(start.or(serving).or(only));
    }
    Ok(newest(&mut serving.into_iter().chain(stored).chain(start)))
}

/// The files of the fetch of `source` with `params` that a step list reads, at its [`version`].
/// `Err(Wanted)` while the store lacks them.
pub fn read(
    inputs: &Inputs,
    store: &Store,
    source: &str,
    params: &[(String, String)],
) -> Result<Result<BTreeMap<String, PathBuf>, Wanted>, String> {
    let version = match version(inputs, store, source, params)? {
        Ok(version) => version,
        Err(wanted) => return Ok(Err(wanted)),
    };
    let files = snapshot_files(store, source, &version, params, &[])?;
    Ok(files.ok_or(Wanted { source: source.into(), version: Some(version), params: params.to_vec() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fixture;

    #[test]
    fn a_fetch_reads_the_newest_version_that_this_machine_knows() {
        let fixture = fixture("product-version");
        let area = |area: &str| vec![("area".to_string(), area.to_string())];
        let mut inputs = Inputs::default();
        inputs.serving.insert(("head".into(), Vec::new()), "1".into());
        inputs.serving.insert(("hansen-gfc".into(), area("a")), "v1.12".into());
        let version = |inputs: &Inputs, source: &str, params: &[(String, String)]| {
            version(inputs, &fixture.store, source, params).unwrap().map_err(|wanted| wanted.version)
        };
        assert_eq!(version(&inputs, "head", &[]), Ok("1".into()), "the serving release");
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        assert_eq!(version(&inputs, "head", &[]), Ok("2".into()), "a newer version in the store");
        assert_eq!(version(&inputs, "hansen-gfc", &area("b")), Ok("v1.12".into()), "a new request of a `start`");
        assert_eq!(version(&inputs, "copernicus-glo-30", &area("a")), Ok("2022-05-09".into()), "the `start`");
        assert_eq!(version(&inputs, "absent", &area("a")), Err(None), "the newest upstream");

        // `query-model` starts at `v2`: a newer version in the store is newer than the start, and
        // the update after the one that took it reads it again.
        fixture.fetched_version("query-model", "v3", "model.zip", b"model\n");
        assert_eq!(version(&inputs, "query-model", &[]), Ok("v3".into()), "newer than the `start`");
        inputs.serving.insert(("query-model".into(), Vec::new()), "v3".into());
        assert_eq!(version(&inputs, "query-model", &[]), Ok("v3".into()), "the next update");
        fixture.fetched_version("nominatim-country-data", "5.10.0", "data.whl", b"data\n");
        inputs.serving.insert(("nominatim-country-data".into(), Vec::new()), "5.3.2".into());
        assert_eq!(version(&inputs, "nominatim-country-data", &[]), Ok("5.10.0".into()), "by number, not text");
        // A commit has no order: its `start`, else the serving version, else the one in the store.
        let start = crate::sources::embedded("protomaps-assets").start.clone().unwrap();
        inputs.serving.insert(("protomaps-assets".into(), Vec::new()), "a".into());
        fixture.fetched_version("protomaps-assets", "b", "assets.zip", b"assets\n");
        assert_eq!(version(&inputs, "protomaps-assets", &[]), Ok(start.clone()), "a reviewed `start` moves it");
        inputs.serving.insert(("protomaps-assets".into(), Vec::new()), start.clone());
        assert_eq!(version(&inputs, "protomaps-assets", &[]), Ok(start), "the serving version is the `start`");
        fixture.fetched_version("natural-earth", "c", "earth.zip", b"earth\n");
        assert_eq!(version(&inputs, "natural-earth", &[]), Ok("c".into()), "the one digest in the store");
        inputs.serving.insert(("natural-earth".into(), Vec::new()), "d".into());
        fixture.fetched_version("natural-earth", "e", "earth.zip", b"other earth\n");
        assert_eq!(version(&inputs, "natural-earth", &[]), Ok("d".into()), "a digest keeps its serving version");

        inputs.due.insert(("head".into(), Vec::new()));
        assert_eq!(version(&inputs, "head", &[]), Err(None), "a due fetch without a newer name");
        inputs.resolve(&Wanted { source: "head".into(), version: None, params: Vec::new() }, "3".into());
        assert_eq!(version(&inputs, "head", &[]), Ok("3".into()), "the version that this update chose");
    }
}
