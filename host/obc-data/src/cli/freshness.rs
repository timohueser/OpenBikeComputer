//! The fetches that the steps of a target read, and whether a newer upstream version is due.

use std::collections::BTreeSet;
use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use super::{listing, Code, Error};
use crate::engine::state::Newer;
use crate::fetch::{
    http::Http,
    upstream::{self, Observation},
};
use crate::product::{Inputs, Product, RequestKey, Versions};
use crate::regions::Regions;
use crate::sources::{self, Refresh, Source, State};
use crate::store::Store;

#[derive(Clone, Serialize, JsonSchema)]
pub(super) struct RequestStatus {
    pub params: Vec<(String, String)>,
    /// The version that the serving release reads.
    pub serving: Option<String>,
    pub observation: Observation,
    pub state: State,
    pub reason: Option<String>,
    pub age_days: Option<i64>,
}

/// The fetches that the step lists of `inputs` ask for, with small metadata fetched and no bulk
/// data. Nothing of `inputs` changes.
pub(super) fn discover(
    root: &Path,
    products: &[&dyn Product],
    inputs: &Inputs,
    regions: &Regions,
    store: &Store,
    http: &Http,
    sources: &[Source],
) -> Result<BTreeSet<RequestKey>, Error> {
    let mut inventory =
        Inputs { settings: inputs.settings.clone(), serving: inputs.serving.clone(), ..Inputs::default() };
    let fetch = listing::fetcher(root, store, http, sources, None, false);
    match listing::steps(root, products, &mut inventory, regions, store, true, fetch) {
        Ok(_) => Ok(inventory.requests.into_inner()),
        Err(error) if matches!(error.code, Code::Blocked | Code::FetchFailed) => Ok(inventory.requests.into_inner()),
        Err(error) => Err(error),
    }
}

/// The state of each request of `source` among `requests`, against the version that `serving`
/// reads. A request is stale when its policy is due and upstream has a newer version.
pub(super) fn requests(
    store: &Store,
    http: &Http,
    source: &Source,
    requests: &BTreeSet<RequestKey>,
    serving: &Versions,
    check_now: bool,
) -> Vec<RequestStatus> {
    let (today, now) = (crate::date::today(), crate::date::now());
    let max_age = if check_now { 0 } else { upstream::CACHE };
    requests
        .iter()
        .filter(|(id, _)| id == &source.id)
        .map(|key| {
            let read = serving.get(key).cloned();
            let observation = upstream::observe(store, http, source, &key.1, max_age, now);
            let base = source.fetch.from.as_ref().and_then(|id| serving.get(&(id.clone(), key.1.clone())));
            let status = sources::status(source, read.as_deref(), base.map(String::as_str), &observation.result, today);
            RequestStatus {
                params: key.1.clone(),
                serving: read,
                observation,
                state: status.state,
                reason: status.reason,
                age_days: status.age_days,
            }
        })
        .collect()
}

/// The stale requests of `requests`, each with the newer version upstream when it has a name.
pub(super) fn newer(
    store: &Store,
    http: &Http,
    sources: &[Source],
    requests: &BTreeSet<RequestKey>,
    serving: &Versions,
) -> Newer {
    let mut newer = Newer::new();
    for source in sources.iter().filter(|source| source.refresh != Refresh::Manual) {
        for request in self::requests(store, http, source, requests, serving, false) {
            if request.state == State::Stale {
                let upstream = request.observation.result.version().map(str::to_string);
                newer.insert((source.id.clone(), request.params), upstream);
            }
        }
    }
    newer
}
