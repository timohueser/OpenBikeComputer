//! A product, such as the planner or the device maps: the steps that build its release for an
//! environment. The steps of a product live in the crate that makes their bytes; the `obc data`
//! binary passes each product to `cli::main`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::engine::release::Release;
use crate::engine::{snapshot_files, LayerFile, Step};
use crate::env::Env;
use crate::regions::Regions;
use crate::store::{sorted, Store};

pub trait Product {
    /// Kebab-case. Each of its layer names starts with `<name>/`.
    fn name(&self) -> &'static str;

    /// The folder of its releases on R2.
    fn prefix(&self) -> &'static str {
        self.name()
    }

    /// The optional layers that `layers` of an environment can switch on.
    fn optional(&self) -> &'static [&'static str] {
        &[]
    }

    /// Its steps for `env`, with recipe and tooling paths relative to `root`. A step list that reads a snapshot, such as the `.poly` of a region or
    /// the Geofabrik index, gives `Unplanned::NeedsFetch` while the store lacks it.
    fn steps(&self, root: &Path, env: &Env, regions: &Regions, store: &Store) -> Result<Steps, Unplanned>;

    /// What clients read of a release. `None` while the product has no client document: a plan of
    /// `live` leaves the product out, because an apply cannot make its release live.
    fn pointer(&self) -> Option<PointerFn> {
        None
    }

    /// The named publication files, from release receipts. Their identity precedes the release id.
    fn named(&self, _release: &Release) -> Result<Vec<LayerFile>, String> {
        Ok(Vec::new())
    }

    /// Check stored artifacts before publication. `root` locates the offline verification tools.
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

/// Usable steps and the layers whose inputs are unavailable. A blocked required layer prevents
/// a complete release, but does not prevent independent steps from building.
#[derive(Default)]
pub struct Steps {
    pub steps: Vec<Step>,
    pub blocked: Vec<BlockedLayer>,
}

impl Steps {
    /// Remove steps that read a blocked layer, including their dependents.
    pub fn block_dependents(&mut self) {
        while let Some((at, reason)) = self.steps.iter().enumerate().find_map(|(at, step)| {
            let input = step.layers().find_map(|name| self.blocked.iter().find(|b| b.layer == name))?;
            Some((at, format!("reads blocked layer `{}`: {}", input.layer, input.reason)))
        }) {
            let step = self.steps.remove(at);
            self.blocked.push(BlockedLayer { layer: step.name, reason });
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
    pub layer: String,
    pub reason: String,
}

/// Gives the pointer of a release from the store.
pub type PointerFn = fn(&Release, &Store) -> Result<Pointer, String>;

/// What clients read of a release.
pub struct Pointer {
    /// The document of `<prefix>/catalog.json`, without `release` and `applied`: an apply adds them.
    pub document: serde_json::Map<String, serde_json::Value>,
}

/// Why a product gives no steps.
#[derive(Debug, PartialEq, Eq)]
pub enum Unplanned {
    /// The step list reads these snapshots. `obc data` fetches them and asks once more.
    NeedsFetch(Vec<Wanted>),
    /// The product does not suit the environment, such as a kind of region that it does not read.
    /// `obc data` reports the product as blocked and plans the others.
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

/// The version of the fetch of `source` with `params` that a step list reads: the version that
/// `env` names (a saved plan, a `--move`, or the version that live reads), or else the newest
/// version of that fetch in the store. `Err(Wanted)` names a fetch of the newest version upstream:
/// for a `--move SOURCE`, while the store has no fetch of it, or when a saved plan lacks it. Every
/// step list gets its versions here, so one function decides where they come from, and `env`
/// records each version that it gives. `Err` for a fetch that live reads at more versions and
/// that no `--move` names.
pub fn version(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
) -> Result<Result<String, Wanted>, String> {
    env.requests.borrow_mut().insert((source.into(), sorted(params)));
    let named = env.version(source, params).inspect_err(|_| {
        env.refused.borrow_mut().insert(source.into());
    })?;
    let named = named.map(str::to_string);
    let version = match &named {
        Some(version) => Some(version.clone()),
        None if env.planned.is_some() || env.moves_to_newest(source) => None,
        None if params.is_empty() => store.snapshots(source)?.into_iter().map(|snapshot| snapshot.version).max(),
        None => store.requests(source, params)?.into_iter().map(|request| request.version).max(),
    };
    if let Some(version) = &version {
        env.read.borrow_mut().insert((source.into(), sorted(params)), version.clone());
    }
    Ok(version.ok_or(Wanted { source: source.into(), version: named, params: params.to_vec() }))
}

/// The files of the fetch of `source` with `params` that a step list reads, at its [`version`].
/// `Err(Wanted)` while the store lacks them.
pub fn read(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
) -> Result<Result<BTreeMap<String, PathBuf>, Wanted>, String> {
    let version = match version(env, store, source, params)? {
        Ok(version) => version,
        Err(wanted) => return Ok(Err(wanted)),
    };
    let named = env.version(source, params)?.map(str::to_string);
    let wanted = || Wanted { source: source.into(), version: named, params: params.to_vec() };
    Ok(snapshot_files(store, source, &version, params, &[])?.ok_or_else(wanted))
}
