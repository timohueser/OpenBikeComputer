//! Current product declarations share the normal build and portable reuse boundary.

use super::*;
use obc_data::engine::release::Release;
use obc_data::product::{Product, Steps};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Planner,
    Maps,
}

impl Kind {
    pub fn product(self) -> &'static dyn Product {
        match self {
            Self::Planner => &super::super::Planner,
            Self::Maps => &crate::maps::Maps,
        }
    }

    fn reviewed(self, env: &mut Env, plan: &obc_data::cli::EnvPlan) {
        env.planned = Some(
            plan.versions
                .iter()
                .filter(|read| read.product.as_deref() == Some(self.product().name()))
                .map(|read| ((read.source.clone(), read.params.clone()), read.version.clone()))
                .collect(),
        );
    }

    pub fn declarations(
        self,
        root: &Path,
        env: &Env,
        regions: &Regions,
        store: &Store,
        tool: Result<Option<obc_data::engine::Library>, String>,
    ) -> Result<Steps, Unplanned> {
        match self {
            Self::Planner => super::super::Planner.declarations(root, env, regions, store, tool, false),
            Self::Maps => crate::maps::Maps.declarations(env, regions, store, tool),
        }
    }
}

pub(super) struct Inputs<'a> {
    pub root: &'a Path,
    pub store: &'a Store,
    pub regions: &'a Regions,
    pub registry: &'a Registry,
}

fn required(release: &Release) -> BTreeMap<String, Vec<String>> {
    release
        .layers
        .iter()
        .filter(|layer| !layer.client.is_none())
        .map(|layer| {
            let extra = release
                .named
                .iter()
                .filter(|named| layer.files.iter().any(|file| file.sha256 == named.sha256))
                .filter_map(|named| {
                    layer
                        .files
                        .iter()
                        .find(|file| file.sha256 == named.sha256 && file.path == named.path)
                        .or_else(|| layer.files.iter().find(|file| file.sha256 == named.sha256))
                        .map(|file| file.path.clone())
                })
                .collect();
            (layer.step.clone(), extra)
        })
        .collect()
}

impl Inputs<'_> {
    pub(super) fn build(
        &self,
        env: Env,
        kind: Kind,
        refresh_live: bool,
        reviewed: Option<&obc_data::cli::EnvPlan>,
        run: &mut runs::Run,
    ) -> Result<Release, String> {
        let Self { root, store, regions, registry } = *self;
        let remote = Remote::from_env()?;
        let (env, live, mut steps) = self.metadata(env, kind, refresh_live, reviewed, Some(&remote), Some(run))?;
        let http = Http::new();
        let copies = obc_data::input_copy::Restore { remote: &remote, live: &live };
        let prior = live.releases().next().map(|(_, _, release)| release);
        let reused = if let Some(prior) = prior {
            obc_data::local::reuse(root, store, &remote, kind.product(), prior, &steps)?
        } else {
            BTreeMap::new()
        };
        if steps
            .iter()
            .any(|step| !reused.contains_key(&step.name) && step.code.crates.iter().any(|name| name == "obc-osm"))
        {
            let tool = obc_osm::OsmiumRunner::default().binding()?;
            let declared = kind
                .declarations(root, &env, regions, store, Ok(Some(tool)))
                .map_err(|e| format!("Local execution declarations changed: {e:?}"))?;
            if !declared.blocked.is_empty() {
                return Err(format!("Local planner is blocked: {:?}", declared.blocked));
            }
            steps = declared.steps;
        }
        run.record(&Event::Phase { phase: Phase::Build })?;
        run.reuse_layers(&reused);
        let work = obc_data::engine::plan::plan_reusing(store, root, &steps, &reused)?;
        run.build(
            &runs::Context {
                store,
                root,
                sources: &registry.sources,
                http: &http,
                copies: Some(&copies),
                limits: runs::Limits::machine(),
            },
            &steps,
            &work,
        )?;
        let optional: Vec<_> =
            env.layers.iter().filter(|name| kind.product().optional().contains(&name.as_str())).cloned().collect();
        let mut original = if let Some(prior) = prior {
            release::release_reusing(store, root, &env.region, &optional, &steps, prior, &reused)?
        } else {
            release::release(store, root, kind.product().name(), &env.region, &optional, &steps)?
        }
        .ok_or("Local build has no complete planner release")?;
        original.name_files(kind.product().named(&original)?)?;
        original.write(store)?;
        let required = required(&original);
        let selection = obc_data::local::plan(root, store, kind.product(), &original, &steps, &required)?;
        if !selection.blocked.is_empty() {
            return Err(format!("Local data does not match this request: {:?}", selection.blocked));
        }
        run.record(&Event::Phase { phase: Phase::Verify })?;
        obc_data::local::adopt(root, store, &remote, kind.product(), &original, &steps, &required, &selection)?;
        Ok(original)
    }

    pub(super) fn metadata(
        &self,
        mut env: Env,
        kind: Kind,
        refresh_live: bool,
        reviewed: Option<&obc_data::cli::EnvPlan>,
        remote: Option<&Remote>,
        mut run: Option<&mut runs::Run>,
    ) -> Result<(Env, Live, Vec<Step>), String> {
        let Self { root, store, regions, registry } = *self;
        let reviewed_product =
            reviewed.and_then(|plan| plan.live.iter().find(|product| product.product == kind.product().name()));
        let pinned = if let Some(product) = reviewed_product {
            product.release.as_ref().map(|id| Release::read(store, kind.product().name(), id)).transpose()?
        } else if refresh_live {
            None
        } else {
            obc_data::local::saved(store)?
                .into_iter()
                .find(|saved| saved.original.product == kind.product().name())
                .map(|saved| saved.original)
        };

        let live = if let Some(original) = pinned {
            Live {
                products: vec![obc_data::live::LiveProduct {
                    product: kind.product().name().into(),
                    prefix: kind.product().prefix().into(),
                    release: Some((original.id(), original)),
                    applied: None,
                    commit: None,
                    document: None,
                    observed: None,
                }],
                inputs: Default::default(),
            }
        } else {
            match remote.filter(|_| reviewed_product.is_none()) {
                Some(remote) => Live::read_products(
                    remote,
                    &[(kind.product().name(), kind.product().prefix())],
                    &registry.sources,
                    store,
                )?,
                None => Live { products: Vec::new(), inputs: Default::default() },
            }
        };
        env.live = live.versions();
        if let Some(plan) = reviewed {
            kind.reviewed(&mut env, plan);
        }
        env.retained = obc_data::input_copy::retained(&live, store)?;
        let http = Http::new();
        let copies = remote.map(|remote| obc_data::input_copy::Restore { remote, live: &live });
        let steps = loop {
            if let Some(run) = run.as_deref() {
                run.check_stop(store)?;
            }
            match kind.declarations(root, &env, regions, store, Ok(None)) {
                Ok(steps) if steps.blocked.is_empty() => break steps.steps,
                Ok(steps) => return Err(format!("Local planner is blocked: {:?}", steps.blocked)),
                Err(Unplanned::NeedsFetch(wanted)) => {
                    let Some(run) = run.as_deref_mut() else {
                        return Err(format!(
                            "Prepare Local metadata before review: {}",
                            wanted.iter().map(|wanted| wanted.source.as_str()).collect::<Vec<_>>().join(", ")
                        ));
                    };
                    if wanted.is_empty() {
                        return Err("Local metadata discovery made no progress".into());
                    }
                    for wanted in wanted {
                        let source =
                            registry.sources.iter().find(|s| s.id == wanted.source).ok_or("unknown Local source")?;
                        let fetched = run.fetch_request(
                            root,
                            store,
                            &http,
                            copies.as_ref(),
                            &obc_data::fetch::Request {
                                source,
                                version: wanted.version,
                                params: wanted.params.clone(),
                            },
                            &[],
                        )?;
                        env.resolved
                            .insert((source.id.clone(), obc_data::store::sorted(&wanted.params)), fetched.version);
                    }
                }
                Err(Unplanned::Invalid(reason) | Unplanned::Failed(reason)) => return Err(reason),
            }
        };
        Ok((env, live, steps))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_source_requests_keep_distinct_product_pins_without_latest_fallback() {
        let scratch = tempfile::tempdir().unwrap();
        let store = Store::at(scratch.path());
        let params = vec![("area".into(), "europe/test".into())];
        let plan = obc_data::cli::EnvPlan {
            versions: [("planner", "2026-10-01"), ("maps", "2026-10-02")]
                .into_iter()
                .map(|(product, version)| obc_data::cli::FetchVersion {
                    product: Some(product.into()),
                    source: "geofabrik-extracts".into(),
                    params: params.clone(),
                    version: version.into(),
                })
                .collect(),
            ..Default::default()
        };
        for (kind, expected) in [(Kind::Planner, "2026-10-01"), (Kind::Maps, "2026-10-02")] {
            let mut env = Env::default();
            env.live.insert(("geofabrik-extracts".into(), params.clone()), [expected.into()].into());
            kind.reviewed(&mut env, &plan);
            let selected = obc_data::product::version(&env, &store, "geofabrik-extracts", &params).unwrap().unwrap();
            assert_eq!(selected, expected, "the shared review must not overwrite a saved product's pin");
            assert_eq!(env.read.borrow()[&("geofabrik-extracts".into(), params.clone())], expected);
            assert!(
                obc_data::product::version(&env, &store, "geofabrik-extracts", &[("area".into(), "another".into())])
                    .unwrap()
                    .is_err(),
                "an unreviewed request cannot select a latest version"
            );
        }
    }
}
