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
use crate::fixtures::{self, Catalog, Collection, FixtureCollection, PackagePlan, Plan, Saved, Selection};
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
        recipes(self.collection, env, regions, store, self.inputs)
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
    let collection = provider(collection)?;
    let store = Store::open()?;
    let mut request = Request {
        kind: kind.unwrap_or(Kind::Prepare),
        env: "fixtures".into(),
        only,
        moves,
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
            None => inspect(root, &store, collection, &request.only, &request.moves, None)?,
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
            request.moves.clear();
            request.fixture = Some(Box::new(plan));
            super::operation_cli::print_handle(&super::operation_cli::start(root, &store, request, None)?, json)?;
        }
    }
    Ok(None)
}

fn configuration(root: &Path, collection: &Collection, regions: &Regions) -> Result<String, String> {
    let selected: Vec<_> = collection.packages.values().map(|p| regions.get(&p.region)).collect();
    let sources = hash_file(&root.join("data/sources.toml"))?;
    Ok(sha256_hex(&serde_json::to_vec(&(collection, selected, sources)).map_err(|e| e.to_string())?))
}

fn environment(id: &str, package: &fixtures::Package, saved: Option<&Saved>) -> Result<Env, String> {
    if let Some(saved) = saved {
        return Ok(saved.selection.environment(&package.region));
    }
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
    env: &Env,
    regions: &Regions,
    store: &Store,
    inputs: &fixtures::Inputs,
) -> Result<crate::product::Steps, Unplanned> {
    let steps = (collection.recipes)(env, regions, store, inputs)?;
    for (source, params) in env.requests.borrow().iter() {
        if !env.moves.contains_key(source) && !env.live.contains_key(&(source.clone(), params.clone())) {
            return Err(Unplanned::Invalid(format!("fixture selection has no exact request for `{source}` {params:?}; explicitly prepare with --move {source}@VERSION")));
        }
    }
    Ok(steps)
}

fn selected_inputs(
    store: &Store,
    env: &Env,
    original: &fixtures::Inputs,
    moves: &BTreeMap<String, Option<String>>,
) -> Result<fixtures::Inputs, Unplanned> {
    let mut inputs = original.clone();
    if moves.contains_key("copernicus-glo-30") {
        inputs.terrain = None;
    }
    for input in std::iter::once(&mut inputs.osm).chain(inputs.content.values_mut()).chain(&mut inputs.terrain) {
        if env.moves.contains_key(&input.source) {
            let version = crate::product::version(env, store, &input.source, &input.params)
                .map_err(Unplanned::Failed)?
                .map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            if crate::engine::snapshot_files(store, &input.source, &version, &input.params, &input.files)
                .map_err(Unplanned::Failed)?
                .is_none()
            {
                return Err(Unplanned::NeedsFetch(vec![Wanted {
                    source: input.source.clone(),
                    version: Some(version),
                    params: input.params.clone(),
                }]));
            }
            input.version = version;
        }
    }
    let paths = crate::engine::snapshot_files(
        store,
        &inputs.osm.source,
        &inputs.osm.version,
        &inputs.osm.params,
        &inputs.osm.files,
    )
    .map_err(Unplanned::Failed)?
    .ok_or_else(|| Unplanned::Invalid("prepare fixtures to restore their exact selected PBF".into()))?;
    let [path] = paths.values().collect::<Vec<_>>()[..] else {
        return Err(Unplanned::Invalid("fixture PBF must select one file".into()));
    };
    let sha256 = hash_file(path).map_err(Unplanned::Failed)?.0;
    if inputs.osm.version == original.osm.version && sha256 != original.osm_sha256 {
        return Err(Unplanned::Invalid("fixture PBF differs from its exact recorded bytes".into()));
    }
    inputs.osm_sha256 = sha256;
    for input in std::iter::once(&inputs.osm).chain(inputs.content.values()).chain(&inputs.terrain) {
        env.read.borrow_mut().insert((input.source.clone(), input.params.clone()), input.version.clone());
    }
    Ok(inputs)
}

fn coverage(package: &fixtures::Package, regions: &Regions, selected: &Selection) -> Result<(), String> {
    let Some(crate::regions::Region { area: crate::regions::Area::Box { bbox }, .. }) = regions.get(&package.region)
    else {
        return Err("fixture region is not a canonical box".into());
    };
    let [west, south, east, north] = selected.coverage;
    if bbox.west < west || bbox.south < south || bbox.east > east || bbox.north > north {
        return Err("fixture region expands beyond its recorded PBF coverage; retain a reviewed input that covers the larger box before rebuilding".into());
    }
    Ok(())
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

fn inspect(
    root: &Path,
    store: &Store,
    collection: &FixtureCollection,
    only: &[String],
    moves: &[String],
    resolved: Option<&BTreeMap<String, crate::env::Versions>>,
) -> Result<Plan, Error> {
    let regions = Regions::load(root)?;
    let declarations = Collection::load(root, &regions)?;
    if only.iter().any(|id| !declarations.packages.contains_key(id)) {
        return Err(Code::Usage.error("--only names a fixture package, not a layer group"));
    }
    let mut packages = BTreeMap::new();
    let catalog = Catalog::read(root)?;
    let sources = super::registry(root)?.sources;
    let requested = build_cli::moves(&sources, moves)?;
    for (id, package) in declarations.packages.iter().filter(|(id, _)| only.is_empty() || only.contains(id)) {
        let basis = Selection::basis(store, &catalog, id);
        let saved = basis.as_ref().ok().and_then(Option::as_ref);
        let selection = saved.map(|saved| saved.selection.identity()).transpose()?;
        let bootstrap = if saved.is_none() && basis.is_ok() { Some(package.bootstrap(root, &regions)?) } else { None };
        let mut env = environment(id, package, saved).unwrap_or_else(|_| Env {
            name: format!("fixtures-{id}"),
            region: package.region.clone(),
            ..Default::default()
        });
        env.moves.extend(requested.clone());
        env.resolved = resolved.and_then(|resolved| resolved.get(id)).cloned().unwrap_or_default();
        let inputs = match saved {
            Some(saved) => coverage(package, &regions, &saved.selection).map(|_| saved.selection.inputs.clone()),
            None => basis.as_ref().map_err(Clone::clone).and_then(|_| {
                bootstrap
                    .as_ref()
                    .ok_or_else(|| "fixture first import needs exact bootstrap inputs".to_string())
                    .and_then(|(_, original)| fixtures::inputs(root, store, package, original))
            }),
        }
        .and_then(|inputs| selected_inputs(store, &env, &inputs, &requested).map_err(|reason| format!("{reason:?}")));
        let assets = catalog.assets(root, id, &package.map)?;
        let listed = (|| {
            let inputs = inputs.as_ref().map_err(Clone::clone)?;
            let steps = recipes(collection, &env, &regions, store, inputs).map_err(|e| format!("{e:?}"))?;
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
        packages.insert(
            id.clone(),
            PackagePlan { bootstrap: bootstrap.map(|(file, _)| file), selection, inputs: inputs.ok(), assets, plan },
        );
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
        moves: moves.to_vec(),
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
    let result = (|| -> Result<fixtures::Outcome, Error> {
        let regions = Regions::load(root)?;
        let declarations = Collection::load(root, &regions)?;
        let catalog = Catalog::read(root)?;
        let sources = super::registry(root)?.sources;
        let http = Http::new();
        if request.kind == Kind::Prepare {
            let requested = build_cli::moves(&sources, &request.moves)?;
            let mut resolved = BTreeMap::new();
            for (id, package) in
                declarations.packages.iter().filter(|(id, _)| request.only.is_empty() || request.only.contains(id))
            {
                run.check_stop(store)?;
                let mut saved = Selection::local(store, id)?;
                if let Some(archive) = catalog.selection(id)? {
                    if saved.as_ref().is_none_or(|saved| saved.archive != archive) {
                        saved = Some(Selection::recover(root, store, &http, &catalog, id, archive)?);
                    }
                }
                let (original, mut env) = match &saved {
                    Some(saved) => {
                        coverage(package, &regions, &saved.selection)?;
                        saved.selection.restore(
                            store,
                            &http,
                            &crate::live::Remote::Public(catalog.base_url.trim_end_matches('/').into()),
                            &requested,
                        )?;
                        (saved.selection.inputs.clone(), saved.selection.environment(&package.region))
                    }
                    None => {
                        let (_, bootstrap) = package.bootstrap(root, &regions)?;
                        fixtures::archives(root, store, &http, &catalog, &bootstrap, package, &sources)?;
                        let env = environment(id, package, None)?;
                        if let Some(version) = package.sources.get("geofabrik-extracts") {
                            let area = package.osm.strip_suffix("-latest.osm.pbf").ok_or_else(|| {
                                "exact fixture recovery needs its recorded Geofabrik area".to_string()
                            })?;
                            build_cli::fetcher_recorded(root, store, &http, &sources, &env, None, Some(&mut run))(
                                &Wanted {
                                    source: "geofabrik-extracts".into(),
                                    version: Some(version.clone()),
                                    params: vec![("area".into(), area.into())],
                                },
                            )?;
                        }
                        (fixtures::inputs(root, store, package, &bootstrap)?, env)
                    }
                };
                env.moves.extend(requested.clone());
                let mut fetch = build_cli::fetcher_recorded(root, store, &http, &sources, &env, None, Some(&mut run));
                let mut fetched = Vec::new();
                let inputs = loop {
                    match selected_inputs(store, &env, &original, &requested) {
                        Ok(inputs) => break inputs,
                        Err(Unplanned::NeedsFetch(wanted)) => {
                            for wanted in wanted {
                                if fetched.contains(&wanted) {
                                    return Err(Code::Blocked.error("refreshed fixture request still lacks its selected files; restore that exact input"));
                                }
                                fetched.push(wanted.clone());
                                let version = fetch(&wanted)?;
                                env.resolved.insert((wanted.source, crate::store::sorted(&wanted.params)), version);
                            }
                        }
                        Err(reason) => return Err(Code::Blocked.error(format!("{id}: {reason:?}"))),
                    }
                };
                let moved = env.moves.clone();
                let retained = env.live.clone();
                let mut exact = |wanted: &Wanted| {
                    if !moved.contains_key(&wanted.source)
                        && !retained.contains_key(&(wanted.source.clone(), crate::store::sorted(&wanted.params)))
                    {
                        return Err(Code::Blocked.error(format!(
                            "{id}: new request {} {:?} requires --move {}@VERSION",
                            wanted.source, wanted.params, wanted.source
                        )));
                    }
                    fetch(wanted)
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
                resolved.insert(id.clone(), env.resolved);
            }
            return inspect(root, store, collection, &request.only, &request.moves, Some(&resolved))
                .map(fixtures::Outcome::Prepared);
        }
        let reviewed =
            request.fixture.as_deref().ok_or_else(|| Code::Usage.error("fixture apply has no reviewed plan"))?;
        let requested = build_cli::moves(&sources, &reviewed.moves)?;
        let selected = reviewed.packages.keys().cloned().collect::<Vec<_>>();
        let resolved = reviewed
            .packages
            .iter()
            .map(|(id, package)| {
                (
                    id.clone(),
                    package
                        .plan
                        .versions
                        .iter()
                        .map(|version| ((version.source.clone(), version.params.clone()), version.version.clone()))
                        .collect(),
                )
            })
            .collect();
        let current = inspect(root, store, collection, &selected, &reviewed.moves, Some(&resolved))?;
        if reviewed != &current {
            return Err(Code::PlanOutdated.error("fixture configuration, inputs, work, catalog or destination changed"));
        }
        reviewed.check()?;
        let bucket = crate::r2::Bucket::from_env(crate::r2::Credentials::Fixtures)?;
        let mut updates = BTreeMap::new();
        let mut completed = Vec::new();
        for (id, package) in &reviewed.packages {
            run.check_stop(store)?;
            let declaration = &declarations.packages[id];
            let saved = Selection::local(store, id)?;
            let mut env = environment(id, declaration, saved.as_ref())?;
            env.moves.extend(package.plan.moves.clone());
            env.planned = resolved.get(id).cloned();
            let inputs = package.inputs.as_ref().expect("reviewed inputs");
            let steps = recipes(collection, &env, &regions, store, inputs)
                .map_err(|e| Code::Blocked.error(format!("{e:?}")))?;
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
            let unchanged =
                Selection::local(store, id)?.map(|saved| saved.selection.identity()).transpose()? == package.selection;
            let bootstrap_unchanged = package
                .bootstrap
                .as_ref()
                .map(|file| hash_file(&root.join(&file.path)).map(|actual| actual == (file.sha256.clone(), file.size)))
                .transpose()?
                .unwrap_or(true);
            if !unchanged
                || !bootstrap_unchanged
                || selected_inputs(store, &env, inputs, &requested).map_err(|error| format!("{error:?}"))? != *inputs
            {
                return Err(Code::PlanOutdated.error("fixture selection changed during the build"));
            }
            for (destination, input) in &inputs.historical {
                if destination == &declaration.map || package.assets.contains_key(destination) {
                    return Err(Code::InvalidData.error("historical sidecar collides with a current fixture output"));
                }
                let files =
                    crate::engine::snapshot_files(store, &input.source, &input.version, &input.params, &input.files)?
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
            }
            let coverage = match &saved {
                Some(saved) => saved.selection.coverage,
                None => declaration.bootstrap(root, &regions)?.1.bounds_lon_lat,
            };
            let selection = Selection::new(
                store,
                id.clone(),
                coverage,
                inputs.clone(),
                release.clone(),
                &env,
                package.assets.clone(),
            )?;
            std::fs::write(tree.join(".obc-data.json"), serde_json::to_vec(&selection).map_err(|e| e.to_string())?)
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
            completed.push(Saved { selection: selection.clone(), archive: file.clone() });
            run.check_stop(store)?;
            reviewed.catalog_unchanged(root)?;
            if crate::r2::fixture_destination()? != reviewed.destination {
                return Err(Code::PlanOutdated.error("fixture destination changed before upload"));
            }
            run.record(&Event::Phase { phase: Phase::Upload })?;
            for copy in &selection.copies {
                for input in &copy.record.files {
                    let object = store.object(&input.sha256);
                    if hash_file(&object)? != (input.sha256.clone(), input.size) {
                        return Err(Code::VerifyFailed.error("fixture selected input changed before upload"));
                    }
                    bucket.put(
                        &object,
                        &format!("{}inputs/objects/{}", catalog.prefix, input.sha256),
                        &crate::r2::Upload { immutable: true, cache_control: None, content_type: None },
                    )?;
                }
            }
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
            if let Some(file) = &package.bootstrap {
                if hash_file(&root.join(&file.path))? != (file.sha256.clone(), file.size) {
                    return Err(Code::PlanOutdated.error("fixture bootstrap changed before catalog update"));
                }
            }
        }
        catalog.replace(root, &updates)?;
        for saved in completed {
            saved.write(store)?;
        }
        Ok(fixtures::Outcome::Applied {
            archives: updates.into_iter().map(|(id, (file, _, _))| (id, file)).collect(),
            catalog: Catalog::read(root)?.file,
        })
    })();
    let plan = api::finish_run(run, result, None)?;
    print_json(&plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{CapturedInput, Inputs};
    use crate::store::{FileRecord, Requested, Snapshot};

    struct Maps;
    impl Product for Maps {
        fn name(&self) -> &'static str {
            "maps"
        }
        fn steps(&self, _: &Path, _: &Env, _: &Regions, _: &Store) -> Result<Steps, Unplanned> {
            Ok(Steps::default())
        }
    }

    #[test]
    fn saved_fixture_requests_stay_exact_and_only_an_explicit_move_admits_new_requests() {
        let temporary = crate::store::tests::Scratch::new("fixture-refresh");
        let store = Store::at(&temporary.0);
        let params = |area: &str| vec![("area".into(), area.into())];
        let mut hashes = Vec::new();
        for (version, bytes) in [("1", b"old".as_slice()), ("2", b"new")] {
            let sha256 = sha256_hex(bytes);
            crate::store::write_atomic(&store.partial("pbf"), bytes).unwrap();
            store.insert(&store.partial("pbf"), &sha256).unwrap();
            store
                .put_snapshot(&Snapshot {
                    source: "geofabrik-extracts".into(),
                    version: version.into(),
                    files: vec![FileRecord {
                        name: "area.osm.pbf".into(),
                        url: format!("https://example.invalid/{version}"),
                        size: bytes.len() as u64,
                        sha256: sha256.clone(),
                        retrieved: version.into(),
                    }],
                })
                .unwrap();
            store
                .put_requested(
                    "geofabrik-extracts",
                    &Requested { version: version.into(), params: params("a"), files: vec!["area.osm.pbf".into()] },
                )
                .unwrap();
            hashes.push(sha256);
        }
        let inputs = Inputs {
            osm: CapturedInput {
                source: "geofabrik-extracts".into(),
                version: "1".into(),
                params: params("a"),
                files: vec!["area.osm.pbf".into()],
            },
            osm_sha256: hashes[0].clone(),
            content: BTreeMap::new(),
            terrain: None,
            historical: BTreeMap::new(),
            empty: BTreeMap::new(),
        };
        let mut env = Env {
            live: [(("geofabrik-extracts".into(), params("a")), ["1".into()].into())].into(),
            ..Default::default()
        };
        assert_eq!(selected_inputs(&store, &env, &inputs, &env.moves).unwrap(), inputs);
        env.moves.insert("geofabrik-extracts".into(), Some("2".into()));
        let next = selected_inputs(&store, &env, &inputs, &env.moves).unwrap();
        assert_eq!(next.osm.version, "2");
        assert_eq!(next.osm.params, inputs.osm.params);
        assert_eq!(next.osm_sha256, hashes[1]);
        let mut captured = inputs.clone();
        captured.terrain = Some(inputs.osm.clone());
        let terrain_move = [("copernicus-glo-30".into(), Some("2026-01-01".into()))].into();
        assert!(
            selected_inputs(&store, &env, &captured, &terrain_move).unwrap().terrain.is_none(),
            "an explicit GLO30 refresh cannot keep old captured TIFFs"
        );
        fn changed_request(env: &Env, _: &Regions, store: &Store, _: &Inputs) -> Result<Steps, Unplanned> {
            crate::product::read(env, store, "geofabrik-extracts", &[("area".into(), "b".into())])
                .map_err(Unplanned::Failed)?
                .map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            Ok(Steps::default())
        }
        store
            .put_requested(
                "geofabrik-extracts",
                &Requested { version: "2".into(), params: params("b"), files: vec!["area.osm.pbf".into()] },
            )
            .unwrap();
        let collection = FixtureCollection { maps: &Maps, recipes: changed_request, assemble: |_, _, _| Ok(()) };
        let regions = Regions::new(Vec::new()).unwrap();
        env.moves.clear();
        assert!(
            matches!(recipes(&collection, &env, &regions, &store, &inputs), Err(Unplanned::Invalid(reason)) if reason.contains("no exact request"))
        );
        env.moves.insert("geofabrik-extracts".into(), Some("2".into()));
        recipes(&collection, &env, &regions, &store, &inputs).unwrap();
    }
}
