//! Active acquisition requests and their upstream observations. Held inputs remain provenance.

use schemars::JsonSchema;
use serde::Serialize;

use super::{build_cli, status_cli, Code, Error};
use crate::env::Env;
use crate::fetch::{
    http::Http,
    upstream::{self, Observation},
};
use crate::product::Product;
use crate::regions::Regions;
use crate::sources::{self, Source, State};
use crate::store::Store;

#[derive(Clone, Serialize, JsonSchema)]
pub(super) struct RequestStatus {
    pub params: Vec<(String, String)>,
    pub live: Option<String>,
    pub observation: Observation,
    pub due: bool,
    pub state: State,
    pub reason: Option<String>,
    pub age_days: Option<i64>,
}

/// Discover only the small metadata needed to enumerate requests, before applying stale moves.
#[allow(clippy::too_many_arguments)]
pub(super) fn discover(
    root: &std::path::Path,
    products: &[&dyn Product],
    env: &Env,
    regions: &Regions,
    store: &Store,
    http: &Http,
    sources: &[Source],
    copies: Option<&crate::input_copy::Restore<'_>>,
    run: Option<&mut crate::engine::runs::Run>,
) -> Result<Env, Error> {
    let mut inventory = env.clone();
    inventory.moves.clear();
    inventory.stale.clear();
    inventory.stale_requests.clear();
    inventory.resolved.clear();
    inventory.planned = None;
    inventory.requests.borrow_mut().clear();
    inventory.read.borrow_mut().clear();
    inventory.fetch_failures.clear();
    let mut fetch = status_cli::discovery_fetch(
        build_cli::fetcher_recorded(root, store, http, sources, &inventory, copies, run),
        false,
    );
    for product in products {
        match build_cli::product_steps(root, *product, &mut inventory, regions, store, &mut fetch) {
            Ok(_) => (),
            Err(error) if matches!(error.code, Code::Blocked | Code::FetchFailed) => (),
            Err(error) => return Err(error),
        }
    }
    Ok(inventory)
}

pub(super) fn requests(store: &Store, http: &Http, source: &Source, env: &Env, check_now: bool) -> Vec<RequestStatus> {
    let today = crate::date::today();
    let now = crate::date::now();
    let max_age = if check_now { 0 } else { upstream::CACHE };
    env.requests
        .borrow()
        .iter()
        .filter(|(id, _)| id == &source.id)
        .map(|key| {
            let live = env.live.get(key).filter(|versions| versions.len() == 1).and_then(|v| v.first()).cloned();
            let observation = upstream::observe(store, http, source, &key.1, max_age, now);
            let base = source
                .fetch
                .from
                .as_ref()
                .and_then(|id| env.live.get(&(id.clone(), key.1.clone())))
                .filter(|versions| versions.len() == 1)
                .and_then(|versions| versions.first());
            let status = sources::status(source, live.as_deref(), base.map(String::as_str), &observation.result, today);
            let conflict = env.live.get(key).filter(|versions| versions.len() > 1);
            RequestStatus {
                params: key.1.clone(),
                due: sources::due(source, live.as_deref(), today),
                live,
                observation,
                state: if conflict.is_some() { State::Blocked } else { status.state },
                reason: conflict
                    .map(|versions| {
                        format!(
                            "active request has conflicting live versions: {}",
                            versions.iter().cloned().collect::<Vec<_>>().join(", ")
                        )
                    })
                    .or(status.reason),
                age_days: status.age_days,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fixture;
    use crate::product::{version, Unplanned};

    #[test]
    fn mutating_discovery_records_its_first_metadata_fetch_but_keeps_the_bulk_guard() {
        use crate::engine::runs::{Event, Run};
        use crate::fetch::tests::{quick, serve, source, whole};
        use crate::product::Wanted;
        struct Metadata;
        impl Product for Metadata {
            fn name(&self) -> &'static str {
                "test"
            }
            fn steps(
                &self,
                _: &std::path::Path,
                _: &Env,
                _: &Regions,
                store: &Store,
            ) -> Result<crate::product::Steps, Unplanned> {
                for source in ["geofabrik-poly", "bulk"] {
                    if crate::engine::snapshot_files(store, source, "1", &[], &[]).map_err(Unplanned::Failed)?.is_none()
                    {
                        return Err(Unplanned::NeedsFetch(vec![Wanted {
                            source: source.into(),
                            version: Some("1".into()),
                            params: Vec::new(),
                        }]));
                    }
                }
                Ok(Vec::new().into())
            }
        }
        let (url, requests) = serve(|_, _| whole(b"metadata"));
        let url = url.replacen("/data/", "/data/{version}/", 1);
        let fixture = fixture("inventory-run-metadata");
        fixture.with_acquisition();
        let sources = [
            Source { id: "geofabrik-poly".into(), ..source(&url, "release") },
            Source { id: "bulk".into(), ..source(&url, "release") },
        ];
        let regions = Regions::new(Vec::new()).unwrap();
        let mut run = Run::create(&fixture.store, "prepare live").unwrap();
        let id = run.id().to_string();
        let inventory = discover(
            &fixture.root(),
            &[&Metadata],
            &Env::default(),
            &regions,
            &fixture.store,
            &quick(),
            &sources,
            None,
            Some(&mut run),
        )
        .unwrap();
        assert_eq!(inventory.fetch_failures[0].0.source, "bulk");
        assert_eq!(requests.lock().unwrap().len(), 1);
        let events = crate::engine::runs::events(&fixture.store, &id).unwrap();
        assert!(events.iter().any(|event| matches!(event, Event::FetchFinished { source, bytes: 8, resolved, .. } if source == "geofabrik-poly" && resolved == "1")));
        assert!(!events.iter().any(|event| matches!(event, Event::FetchStarted { source, .. } if source == "bulk")));
        discover(
            &fixture.root(),
            &[&Metadata],
            &Env::default(),
            &regions,
            &fixture.store,
            &quick(),
            &sources,
            None,
            None,
        )
        .unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1, "a later observational pass reuses this metadata");
        assert_eq!(crate::engine::runs::events(&fixture.store, &id).unwrap(), events);
        run.finish(None).unwrap();
    }

    #[test]
    fn discovery_keeps_unprepared_active_requests_and_excludes_held_inputs() {
        struct Captures;
        impl Product for Captures {
            fn name(&self) -> &'static str {
                "test"
            }
            fn steps(
                &self,
                _root: &std::path::Path,
                env: &Env,
                _: &Regions,
                store: &Store,
            ) -> Result<crate::product::Steps, Unplanned> {
                for collection in ["landmarks", "peaks"] {
                    let _ = version(env, store, "capture", &[("collection".into(), collection.into())])
                        .map_err(Unplanned::Failed)?;
                }
                Err(Unplanned::Invalid("capture bytes not prepared".into()))
            }
        }
        let fixture = fixture("active-request-inventory");
        let mut env = Env::default();
        env.live.insert(("extract".into(), Vec::new()), ["2020-01-01".into()].into());
        env.live.insert(("capture".into(), vec![("collection".into(), "peaks".into())]), ["2026-10-01".into()].into());
        let regions = Regions::new(Vec::new()).unwrap();
        let inventory =
            discover(&fixture.root(), &[&Captures], &env, &regions, &fixture.store, &Http::new(), &[], None, None)
                .unwrap();
        assert_eq!(inventory.requests.borrow().len(), 2);
        assert!(inventory.requests.borrow().iter().all(|(source, _)| source == "capture"));
        assert_eq!(inventory.read.borrow().len(), 1, "missing requests remain inventoried without an invented version");
        assert!(env.requests.borrow().is_empty(), "discovery does not mutate source intent");
    }
}
