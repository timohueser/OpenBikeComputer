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
    /// The environment does not give what the product needs, such as a pin.
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

/// The files of the fetch of `source` with `params` that a step list reads: at the version of
/// `env`, or else at the newest version of that fetch in the store. `Err(Wanted)` while the store
/// lacks them.
pub fn read(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
) -> Result<Result<BTreeMap<String, PathBuf>, Wanted>, String> {
    let pinned = env.version(source).map(str::to_string);
    let version = match &pinned {
        Some(version) => Some(version.clone()),
        None if params.is_empty() => store.snapshots(source)?.into_iter().map(|snapshot| snapshot.version).max(),
        None => store.requests(source, params)?.into_iter().map(|request| request.version).max(),
    };
    let files = match &version {
        Some(version) => snapshot_files(store, source, version, params, &[])?,
        None => None,
    };
    Ok(files.ok_or(Wanted { source: source.into(), version: pinned, params: params.to_vec() }))
}
