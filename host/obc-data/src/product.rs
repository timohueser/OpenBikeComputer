//! A product, such as the planner or the device maps: the steps that build its release for an
//! environment. The steps of a product live in the crate that makes their bytes; the `obc data`
//! binary passes each product to `cli::main`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::engine::{snapshot_files, Step};
use crate::env::Env;
use crate::regions::Regions;
use crate::store::Store;

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

    /// Its steps for `env`. A step list that reads a snapshot, such as the `.poly` of a region or
    /// the Geofabrik index, gives `Unplanned::NeedsFetch` while the store lacks it.
    fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned>;
}

/// Why a product gives no steps.
#[derive(Debug, PartialEq, Eq)]
pub enum Unplanned {
    /// The step list reads these snapshots. `obc data` fetches them and asks once more.
    NeedsFetch(Vec<Wanted>),
    /// The environment does not give what the product needs, such as a region of a kind that it
    /// does not read.
    Invalid(String),
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
/// `env` names (a `--move`, or else the version that live reads), or else the newest version of
/// that fetch in the store. `Err(Wanted)` names a fetch of the newest version upstream: for a
/// `--move SOURCE`, or while the store has no fetch of it. Every step list gets its versions here,
/// so one function decides where they come from.
pub fn version(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
) -> Result<Result<String, Wanted>, String> {
    let named = env.version(source).map(str::to_string);
    let version = match &named {
        Some(version) => Some(version.clone()),
        None if env.moves_to_newest(source) => None,
        None if params.is_empty() => store.snapshots(source)?.into_iter().map(|snapshot| snapshot.version).max(),
        None => store.requests(source, params)?.into_iter().map(|request| request.version).max(),
    };
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
    let wanted =
        || Wanted { source: source.into(), version: env.version(source).map(str::to_string), params: params.to_vec() };
    let version = match version(env, store, source, params)? {
        Ok(version) => version,
        Err(wanted) => return Ok(Err(wanted)),
    };
    Ok(snapshot_files(store, source, &version, params, &[])?.ok_or_else(wanted))
}
