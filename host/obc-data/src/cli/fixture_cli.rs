//! A fixture collection keeps the ordinary graph and one reviewed Git catalog replacement.

use std::collections::BTreeMap;
use std::path::Path;

use super::{api, build_cli, print_json, Code, Command, Error};
use crate::engine::{
    plan, release,
    runs::{Context, Event, Limits, Phase, Publication},
};
use crate::env::Env;
use crate::fetch::http::Http;
use crate::fixtures::{self, Catalog, Collection, FixtureCollection, PackagePlan, Plan, Saved};
use crate::operation::{Kind, Request};
use crate::regions::Regions;
use crate::store::{hash_file, sha256_hex, Store};

fn provider(collection: Option<&FixtureCollection>) -> Result<&FixtureCollection, Error> {
    collection.ok_or_else(|| {
        Code::Blocked
            .error("this binary has no device-map fixture producers")
            .fix("Run fixture preparation through `obc data` in the producer checkout.")
    })
}

pub(super) fn dispatch(
    root: &Path,
    command: Command,
    collection: Option<&FixtureCollection>,
    json: bool,
) -> Result<Option<Command>, Error> {
    let (kind, only, moves, file, yes) = match &command {
        Command::Plan(args) if args.env == "fixtures" => (None, args.only.clone(), args.moves.clone(), None, false),
        Command::Prepare(args) if args.env == "fixtures" => {
            (Some(Kind::Prepare), args.only.clone(), args.moves.clone(), None, false)
        }
        Command::Apply(args) if args.env == "fixtures" => (
            Some(Kind::Apply),
            Vec::new(),
            Vec::new(),
            args.plan.as_deref(),
            super::apply_cli::consent(args, std::io::IsTerminal::is_terminal(&std::io::stdin()))?,
        ),
        Command::Build(args) if args.env == "fixtures" => {
            return Err(Code::Usage
                .error("fixture preparation imports inputs; reviewed `apply fixtures` builds and seals packages"))
        }
        _ => return Ok(Some(command)),
    };
    if !moves.is_empty() {
        return Err(Code::Usage
            .error("fixture packages have exact independent source selections; edit their collection, not --move"));
    }
    let collection = provider(collection)?;
    let store = Store::open()?;
    let mut request = Request {
        kind: kind.unwrap_or(Kind::Prepare),
        env: "fixtures".into(),
        only,
        moves: Vec::new(),
        plan: None,
        dev: None,
        fixture: None,
    };
    if kind == Some(Kind::Prepare) {
        super::operation_cli::print_handle(&super::operation_cli::start(root, &store, request, None)?, json)?;
    } else {
        let plan: Plan = match file {
            Some(path) => serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| Code::InvalidData.error(e.to_string()))?,
            None => inspect(root, &store, collection, &request.only)?,
        };
        if kind.is_none() {
            print_json(&plan)?;
        } else {
            plan.check().map_err(|e| Code::Blocked.error(e))?;
            api::confirm(
                &format!(
                    "Apply {} fixture packages to the isolated bucket and update only the reviewed Git catalog?",
                    plan.packages.len()
                ),
                yes,
            )?;
            request.only.clear();
            request.fixture = Some(Box::new(plan));
            super::operation_cli::print_handle(&super::operation_cli::start(root, &store, request, None)?, json)?;
        }
    }
    Ok(None)
}

fn configuration(root: &Path, collection: &Collection, regions: &Regions) -> Result<String, String> {
    let selected: Vec<_> = collection.packages.values().map(|p| regions.get(&p.region)).collect();
    let sources = hash_file(&root.join("data/sources.toml"))?;
    let osm = hash_file(&root.join("fixtures/sources/ride-assistant/assistant-osm.json"))?;
    Ok(sha256_hex(&serde_json::to_vec(&(collection, selected, sources, osm)).map_err(|e| e.to_string())?))
}

fn environment(id: &str, package: &fixtures::Package) -> Result<Env, String> {
    for source in ["copernicus-glo-30", "copernicus-glo-30-tiles", "land-polygons"] {
        if !package.sources.contains_key(source) {
            return Err(format!("{id}: declare the exact historical `{source}` version in data/env/fixtures.toml; a baked terrain sidecar or newest source is not a raw input"));
        }
    }
    Ok(Env {
        name: format!("fixtures-{id}"),
        region: package.region.clone(),
        moves: package.sources.iter().map(|(s, v)| (s.clone(), Some(v.clone()))).collect(),
        ..Default::default()
    })
}

fn recipes(
    collection: &FixtureCollection,
    package: &fixtures::Package,
    env: &Env,
    regions: &Regions,
    store: &Store,
    inputs: &fixtures::Inputs,
) -> Result<crate::product::Steps, String> {
    let steps = (collection.recipes)(env, regions, store, inputs).map_err(|e| format!("{e:?}"))?;
    for ((source, _), version) in env.read.borrow().iter() {
        if package.sources.get(source) != Some(version) {
            return Err(format!("fixture planning reads `{source}@{version}` without an exact package selection; declare that historical source version before preparation"));
        }
    }
    Ok(steps)
}

fn inspect(root: &Path, store: &Store, collection: &FixtureCollection, only: &[String]) -> Result<Plan, Error> {
    let regions = Regions::load(root)?;
    let declarations = Collection::load(root, &regions)?;
    if only.iter().any(|id| !declarations.packages.contains_key(id)) {
        return Err(Code::Usage.error("--only names a fixture package, not a layer group"));
    }
    let mut packages = BTreeMap::new();
    for (id, package) in declarations.packages.iter().filter(|(id, _)| only.is_empty() || only.contains(id)) {
        let (bootstrap, original) = package.bootstrap(root, &regions)?;
        let mut env = Env { name: format!("fixtures-{id}"), region: package.region.clone(), ..Default::default() };
        let inputs = fixtures::inputs(root, store, package, &original);
        let listed = (|| {
            env = environment(id, package)?;
            let inputs = inputs.as_ref().map_err(Clone::clone)?;
            recipes(collection, package, &env, &regions, store, inputs)
        })();
        let (work, blocked) = match listed {
            Ok(listed) => (
                plan::plan(store, root, &listed.steps)?,
                listed
                    .blocked
                    .into_iter()
                    .map(|layer| build_cli::BlockedProduct {
                        product: "maps".into(),
                        reason: layer.reason.clone(),
                        layers: vec![layer],
                    })
                    .collect(),
            ),
            Err(reason) => (
                plan::Plan { groups: Vec::new() },
                vec![build_cli::BlockedProduct { product: "maps".into(), reason, layers: Vec::new() }],
            ),
        };
        let mut plan = build_cli::env_plan(&env, &[], work, blocked, None);
        plan.needs_prepare = !plan.blocked.is_empty() || inputs.is_err();
        packages.insert(id.clone(), PackagePlan { bootstrap, inputs: inputs.ok(), plan });
    }
    Ok(Plan {
        catalog: Catalog::read(root)?.file,
        configuration: configuration(root, &declarations, &regions)?,
        destination: crate::r2::fixture_destination()?,
        packages,
    })
}

pub(super) fn perform(
    root: &Path,
    store: &Store,
    request: &Request,
    collection: Option<&FixtureCollection>,
) -> Result<(), Error> {
    let collection = provider(collection)?;
    let mut run = api::start_run(
        store,
        &format!("{} fixtures", if request.kind == Kind::Prepare { "prepare" } else { "apply" }),
    )?;
    let result = (|| -> Result<Plan, Error> {
        let regions = Regions::load(root)?;
        let declarations = Collection::load(root, &regions)?;
        let catalog = Catalog::read(root)?;
        let sources = super::registry(root)?.sources;
        let http = Http::new();
        if request.kind == Kind::Prepare {
            for (id, package) in
                declarations.packages.iter().filter(|(id, _)| request.only.is_empty() || request.only.contains(id))
            {
                run.check_stop(store)?;
                let (_, bootstrap) = package.bootstrap(root, &regions)?;
                fixtures::archives(root, store, &http, &catalog, &bootstrap, &sources)
                    .map_err(|e| Code::Blocked.error(format!("{id}: {e}")))?;
            }
            return inspect(root, store, collection, &request.only);
        }
        let reviewed =
            request.fixture.as_deref().ok_or_else(|| Code::Usage.error("fixture apply has no reviewed plan"))?;
        let selected = reviewed.packages.keys().cloned().collect::<Vec<_>>();
        let current = inspect(root, store, collection, &selected)?;
        if reviewed != &current {
            return Err(Code::PlanOutdated.error("fixture configuration, inputs, work, catalog or destination changed"));
        }
        reviewed.check()?;
        let bucket = crate::r2::Bucket::from_env(crate::r2::Credentials::Fixtures)?;
        let mut updates = BTreeMap::new();
        for (id, package) in &reviewed.packages {
            run.check_stop(store)?;
            let declaration = &declarations.packages[id];
            let env = environment(id, declaration)?;
            let inputs = package.inputs.as_ref().expect("reviewed inputs");
            let steps =
                recipes(collection, declaration, &env, &regions, store, inputs).map_err(|e| Code::Blocked.error(e))?;
            let work = plan::plan(store, root, &steps.steps)?;
            if !work.same_work(&plan::Plan { groups: package.plan.groups.clone() }) {
                return Err(Code::PlanOutdated.error("fixture work changed before build"));
            }
            run.record(&Event::Phase { phase: Phase::Build })?;
            run.build(
                &Context { root, store, sources: &sources, http: &http, copies: None, limits: Limits::machine() },
                &steps.steps,
                &work,
            )?;
            let mut release = release::release(store, root, "maps", &env.region, &env.layers, &steps.steps)?
                .ok_or("fixture map release is incomplete")?;
            release.name_files(collection.maps.named(&release)?)?;
            run.record(&Event::Phase { phase: Phase::Verify })?;
            collection.maps.verify(root, None, &release, store)?;
            let scratch = crate::r2::Scratch::new()?;
            let tree = scratch.0.join("package");
            std::fs::create_dir(&tree).map_err(|e| e.to_string())?;
            (collection.assemble)(&release, store, &tree.join(&declaration.map))?;
            std::fs::write(
                tree.join(".obc-data.json"),
                serde_json::to_vec(&(&package.bootstrap, inputs, &release)).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let archive = store.partial(&format!("fixture-{id}.tar.gz"));
            fixtures::materialize(
                root,
                &["seal".into(), id.clone(), tree.to_string_lossy().into(), archive.to_string_lossy().into()],
            )?;
            let (sha256, size) = hash_file(&archive)?;
            let file =
                crate::engine::LayerFile { path: format!("packages/{sha256}.tar.gz"), sha256: sha256.clone(), size };
            let object = store.insert(&archive, &sha256)?;
            let licence = release
                .layers
                .iter()
                .flat_map(|layer| layer.snapshots.keys())
                .map(|id| {
                    sources
                        .iter()
                        .find(|s| &s.id == id)
                        .and_then(|s| s.licence.clone())
                        .ok_or_else(|| format!("{id}: fixture input has no confirmed licence"))
                })
                .collect::<Result<std::collections::BTreeSet<_>, _>>()?
                .into_iter()
                .collect::<Vec<_>>()
                .join(" AND ");
            Saved {
                package: id.clone(),
                bootstrap: package.bootstrap.clone(),
                inputs: inputs.clone(),
                release,
                archive: file.clone(),
            }
            .write(store)?;
            run.check_stop(store)?;
            reviewed.catalog_unchanged(root)?;
            if crate::r2::fixture_destination()? != reviewed.destination {
                return Err(Code::PlanOutdated.error("fixture destination changed before upload"));
            }
            run.record(&Event::Phase { phase: Phase::Upload })?;
            let key = format!("{}{}", catalog.prefix, file.path);
            bucket.put(
                &object,
                &key,
                &crate::r2::Upload {
                    immutable: true,
                    cache_control: Some("public, max-age=31536000, immutable"),
                    content_type: None,
                },
            )?;
            run.record(&Event::Published { mutation: Publication::Uploaded { key: key.clone() } })?;
            let public = http.download(
                store,
                &format!("{}{}", catalog.base_url, file.path),
                &crate::fetch::http::Expect::default(),
            )?;
            if hash_file(&public.object)? != (sha256, size) {
                return Err(Code::VerifyFailed.error("public fixture archive differs from the sealed package"));
            }
            updates
                .insert(id.clone(), (file, format!("Device map and exact inputs for {}", declaration.region), licence));
        }
        run.check_stop(store)?;
        crate::worker::check(root)?;
        if configuration(root, &Collection::load(root, &Regions::load(root)?)?, &Regions::load(root)?)?
            != reviewed.configuration
            || crate::r2::fixture_destination()? != reviewed.destination
        {
            return Err(Code::PlanOutdated.error("fixture configuration or destination changed before catalog update"));
        }
        catalog.replace(root, &updates)?;
        Ok(reviewed.clone())
    })();
    let plan = api::finish_run(run, result, None)?;
    print_json(&plan)
}
