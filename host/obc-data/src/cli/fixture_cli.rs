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
use crate::product::{Product, Steps, Unplanned, Wanted};
use crate::regions::Regions;
use crate::store::{hash_file, sha256_hex, Store};

fn provider(collection: Option<&FixtureCollection>) -> Result<&FixtureCollection, Error> {
    collection.ok_or_else(|| {
        Code::Blocked
            .error("this binary has no device-map fixture producers")
            .fix("Run fixture preparation through `obc data` in the producer checkout.")
    })
}

struct CapturedMaps<'a> {
    collection: &'a FixtureCollection,
    inputs: &'a fixtures::Inputs,
}

impl Product for CapturedMaps<'_> {
    fn name(&self) -> &'static str {
        "maps"
    }
    fn steps(&self, _: &Path, env: &Env, regions: &Regions, store: &Store) -> Result<Steps, Unplanned> {
        (self.collection.recipes)(env, regions, store, self.inputs)
    }
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
    let declarations = Collection::load(root, &Regions::load(root)?)?;
    if request.only.iter().any(|id| !declarations.packages.contains_key(id)) {
        return Err(Code::Usage.error("--only names a fixture package, not a layer group"));
    }
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
    let terrain = hash_file(&root.join("fixtures/sources/ride-assistant/assistant-terrain.json"))?;
    Ok(sha256_hex(&serde_json::to_vec(&(collection, selected, sources, osm, terrain)).map_err(|e| e.to_string())?))
}

fn environment(id: &str, package: &fixtures::Package) -> Result<Env, String> {
    for source in ["copernicus-glo-30", "land-polygons"] {
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

fn licences<'a>(
    ids: impl Iterator<Item = &'a str>,
    sources: &[crate::sources::Source],
) -> Result<std::collections::BTreeSet<String>, String> {
    ids.map(|id| {
        sources.iter().find(|source| source.id == id).and_then(|source| source.licence.clone()).ok_or_else(|| {
            format!("{id}: fixture input has no confirmed licence; review the exact retained source notices")
        })
    })
    .collect()
}

fn inspect(root: &Path, store: &Store, collection: &FixtureCollection, only: &[String]) -> Result<Plan, Error> {
    let regions = Regions::load(root)?;
    let declarations = Collection::load(root, &regions)?;
    if only.iter().any(|id| !declarations.packages.contains_key(id)) {
        return Err(Code::Usage.error("--only names a fixture package, not a layer group"));
    }
    let mut packages = BTreeMap::new();
    let catalog = Catalog::read(root)?;
    let sources = super::registry(root)?.sources;
    for (id, package) in declarations.packages.iter().filter(|(id, _)| only.is_empty() || only.contains(id)) {
        let (bootstrap, original) = package.bootstrap(root, &regions)?;
        let mut env = Env { name: format!("fixtures-{id}"), region: package.region.clone(), ..Default::default() };
        let inputs = fixtures::inputs(root, store, package, &original);
        let assets = catalog.assets(root, id, &package.map)?;
        let listed = (|| {
            env = environment(id, package)?;
            let inputs = inputs.as_ref().map_err(Clone::clone)?;
            let steps = recipes(collection, package, &env, &regions, store, inputs)?;
            let ids = steps.steps.iter().flat_map(|step| &step.inputs).filter_map(|input| match input {
                crate::engine::Input::Snapshot { source, .. } => Some(source.as_str()),
                _ => None,
            });
            licences(ids.chain(inputs.historical.values().map(|input| input.source.as_str())), &sources)?;
            if !assets.is_empty() {
                let asset_id =
                    package.asset_source.as_deref().ok_or("tracked fixture assets need a registered source")?;
                if !sources.iter().any(|source| source.id == asset_id && source.redistribute) {
                    return Err("tracked fixture asset redistribution is not confirmed".to_string());
                }
                licences(
                    std::iter::once(
                        package.asset_source.as_deref().ok_or("tracked fixture assets need a registered source")?,
                    ),
                    &sources,
                )?;
            }
            Ok::<_, String>(steps)
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
        packages.insert(id.clone(), PackagePlan { bootstrap, inputs: inputs.ok(), assets, plan });
    }
    let packaging = match fixtures::packaging(root) {
        Ok(packaging) => packaging,
        Err(reason) => {
            for package in packages.values_mut() {
                package.plan.blocked.push(build_cli::BlockedProduct {
                    product: "maps".into(),
                    reason: format!("fixture packaging: {reason}"),
                    layers: Vec::new(),
                });
                package.plan.needs_prepare = true;
            }
            BTreeMap::new()
        }
    };
    Ok(Plan {
        catalog: catalog.file,
        configuration: configuration(root, &declarations, &regions)?,
        destination: crate::r2::fixture_destination()?,
        worker: crate::worker::bound_code(root)?,
        packaging,
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
                fixtures::archives(root, store, &http, &catalog, &bootstrap, package, &sources)
                    .map_err(|e| Code::Blocked.error(format!("{id}: {e}")))?;
                if let Some(version) = package.sources.get("geofabrik-extracts") {
                    let area = package.osm.strip_suffix("-latest.osm.pbf").ok_or_else(|| {
                        Code::InvalidData.error("exact Geofabrik fixture recovery needs its recorded area path")
                    })?;
                    let env = environment(id, package)?;
                    build_cli::fetcher_recorded(root, store, &http, &sources, &env, None, Some(&mut run))(
                        &Wanted { source: "geofabrik-extracts".into(), version: Some(version.clone()), params: vec![("area".into(), area.into())] }
                    ).map_err(|error| error.fix("Restore the exact recorded Geofabrik bytes, then prepare fixtures again. A newest extract is not a substitute."))?;
                }
                let inputs = fixtures::inputs(root, store, package, &bootstrap)?;
                let mut env = environment(id, package)?;
                let mut fetch = build_cli::fetcher_recorded(root, store, &http, &sources, &env, None, Some(&mut run));
                let mut exact = |wanted: &Wanted| {
                    if wanted.version.as_ref() != package.sources.get(&wanted.source) {
                        return Err(Code::Blocked.error(format!(
                            "{id}: select exact `{}` history in data/env/fixtures.toml before preparation",
                            wanted.source
                        )));
                    }
                    fetch(wanted).map_err(|error| error.fix(format!("Restore `obc data fetch {}@{} {}` from the exact retained input record, then prepare fixtures again. Do not replace missing history with newest or Live.", wanted.source, wanted.version.as_deref().unwrap_or("VERSION"), wanted.params.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "))))
                };
                build_cli::product_steps(
                    root,
                    &CapturedMaps { collection, inputs: &inputs },
                    &mut env,
                    &regions,
                    store,
                    &mut exact,
                )?
                .map_err(|reason| Code::Blocked.error(reason))?;
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
                .ok_or_else(|| Code::RunFailed.error("fixture map release is incomplete"))?;
            release.name_files(collection.maps.named(&release)?)?;
            run.record(&Event::Phase { phase: Phase::Verify })?;
            collection.maps.verify(root, None, &release, store)?;
            let scratch = crate::r2::Scratch::new()?;
            let tree = scratch.0.join("package");
            std::fs::create_dir(&tree).map_err(|e| e.to_string())?;
            (collection.assemble)(&release, store, &tree.join(&declaration.map))?;
            for (destination, asset) in &package.assets {
                let copied = tree.join(destination);
                std::fs::create_dir_all(copied.parent().expect("asset has a parent")).map_err(|e| e.to_string())?;
                std::fs::copy(root.join(&asset.path), &copied).map_err(|e| e.to_string())?;
                if hash_file(&copied)? != (asset.sha256.clone(), asset.size) {
                    return Err(Code::PlanOutdated.error("fixture tracked asset changed after review"));
                }
                let retained = store.partial(&format!("fixture-{}", asset.sha256));
                std::fs::copy(&copied, &retained).map_err(|e| e.to_string())?;
                store.insert(&retained, &asset.sha256)?;
            }
            let (current_bootstrap, original) = declaration.bootstrap(root, &regions)?;
            if current_bootstrap != package.bootstrap
                || fixtures::inputs(root, store, declaration, &original)? != *inputs
            {
                return Err(Code::PlanOutdated.error("fixture bootstrap or historical input changed during the build"));
            }
            let mut retained_assets = package.assets.values().cloned().collect::<Vec<_>>();
            for (destination, input) in &inputs.historical {
                if destination == &declaration.map || package.assets.contains_key(destination) {
                    return Err(Code::InvalidData.error("historical sidecar collides with a current fixture output"));
                }
                let files = crate::engine::snapshot_files(store, &input.source, &input.version, &[], &input.files)?
                    .ok_or_else(|| Code::Blocked.error("historical sidecar input is missing"))?;
                let [path] = files.values().collect::<Vec<_>>()[..] else {
                    return Err(Code::InvalidData.error("historical sidecar must select one file"));
                };
                let copied = tree.join(destination);
                std::fs::create_dir_all(copied.parent().expect("sidecar has a parent")).map_err(|e| e.to_string())?;
                std::fs::copy(path, &copied).map_err(|e| e.to_string())?;
                let (sha256, size) = hash_file(&copied)?;
                let snapshot = store
                    .snapshot(&input.source, &input.version)?
                    .ok_or_else(|| Code::Blocked.error("historical sidecar record is missing"))?;
                if !snapshot
                    .files
                    .iter()
                    .any(|file| input.files.contains(&file.name) && file.sha256 == sha256 && file.size == size)
                {
                    return Err(
                        Code::VerifyFailed.error("historical sidecar bytes differ from their exact retained record")
                    );
                }
                retained_assets.push(crate::engine::LayerFile { path: destination.clone(), sha256, size });
            }
            std::fs::write(
                tree.join(".obc-data.json"),
                serde_json::to_vec(&(&package.bootstrap, inputs, &declaration.asset_source, &package.assets, &release))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let archive = store.partial(&format!("fixture-{id}.tar.gz"));
            crate::worker::check(root)?;
            fixtures::materialize(
                root,
                &["seal".into(), id.clone(), tree.to_string_lossy().into(), archive.to_string_lossy().into()],
                Some(&reviewed.packaging),
            )?;
            let (sha256, size) = hash_file(&archive)?;
            let file =
                crate::engine::LayerFile { path: format!("packages/{sha256}.tar.gz"), sha256: sha256.clone(), size };
            let object = store.insert(&archive, &sha256)?;
            let mut licences = licences(
                release
                    .layers
                    .iter()
                    .flat_map(|layer| layer.snapshots.keys())
                    .map(String::as_str)
                    .chain(inputs.historical.values().map(|input| input.source.as_str())),
                &sources,
            )?;
            if !package.assets.is_empty() {
                let asset_source = declaration
                    .asset_source
                    .as_ref()
                    .ok_or_else(|| Code::Blocked.error("tracked fixture assets need their registered source"))?;
                let source = sources
                    .iter()
                    .find(|source| &source.id == asset_source)
                    .ok_or_else(|| Code::Blocked.error("tracked fixture asset source is unregistered"))?;
                if !source.redistribute {
                    return Err(Code::Blocked.error("tracked fixture asset redistribution is not confirmed"));
                }
                licences.insert(
                    source
                        .licence
                        .clone()
                        .ok_or_else(|| Code::Blocked.error("tracked fixture asset licence is unconfirmed"))?,
                );
            }
            let licence = licences.into_iter().collect::<Vec<_>>().join(" AND ");
            Saved {
                package: id.clone(),
                bootstrap: package.bootstrap.clone(),
                inputs: inputs.clone(),
                release,
                archive: file.clone(),
                assets: retained_assets,
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
        for package in reviewed.packages.values() {
            if hash_file(&root.join(&package.bootstrap.path))?
                != (package.bootstrap.sha256.clone(), package.bootstrap.size)
            {
                return Err(Code::PlanOutdated.error("fixture bootstrap changed before catalog update"));
            }
        }
        catalog.replace(root, &updates)?;
        Ok(reviewed.clone())
    })();
    let plan = api::finish_run(run, result, None)?;
    print_json(&plan)
}
