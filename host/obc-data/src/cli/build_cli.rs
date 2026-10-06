//! `obc data plan ENV` and `obc data build ENV`: the steps of every product for an environment,
//! what a build would fetch and build, and the build into the store. Nothing uploads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::runs_cli::{bytes, duration};
use super::{cells, fetched, live_versions, print_json, print_table, registry, Code, Error};
use crate::engine::plan::{self, Estimate, Group, Plan};
use crate::engine::release;
use crate::engine::runs::{Context, Limits, Run};
use crate::engine::Step;
use crate::env::{Env, LiveVersions};
use crate::fetch::{self, http::Http, Request};
use crate::product::{Product, Unplanned, Wanted};
use crate::regions::Regions;
use crate::sources::{Refresh, Source};
use crate::store::Store;

#[derive(Args)]
pub struct PlanArgs {
    /// The environment: `data/env/ENV.toml`.
    env: String,
    /// Only these groups, by id.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Read this version of a source instead of the version of live; without a version, the newest
    /// version upstream. A `manual` source moves only this way.
    #[arg(long = "move", value_name = "SOURCE[@VERSION]")]
    moves: Vec<String>,
}

#[derive(Args)]
pub struct BuildArgs {
    /// The environment: `data/env/ENV.toml`.
    env: String,
    /// Only these groups, by id.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Build the groups of this output of `plan ENV --json`, with its versions. Exit status 3 when
    /// the plan of now differs.
    #[arg(long)]
    plan: Option<PathBuf>,
    /// Read this version of a source instead of the version of live; without a version, the newest
    /// version upstream.
    #[arg(long = "move", value_name = "SOURCE[@VERSION]", conflicts_with = "plan")]
    moves: Vec<String>,
}

/// What a build of an environment would fetch and build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvPlan {
    pub env: String,
    pub region: String,
    pub layers: Vec<String>,
    /// The version of each source that `--move` names. A `--move SOURCE` has the newest version
    /// upstream that the plan fetched.
    pub moves: BTreeMap<String, String>,
    /// The version of each fetch that the step lists read. `build --plan` reads exactly these.
    pub versions: Vec<FetchVersion>,
    /// The groups that `--only` selected, or none for every group.
    pub only: Vec<String>,
    pub groups: Vec<Group>,
    /// The products that give no steps for the environment. The others plan without them.
    pub blocked: Vec<BlockedProduct>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockedProduct {
    pub product: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchVersion {
    pub source: String,
    /// The `NAME=VALUE`s of the fetch, sorted.
    pub params: Vec<(String, String)>,
    pub version: String,
}

/// What a build did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Built {
    /// `None` when there was nothing to fetch or build.
    pub run: Option<String>,
    /// The layers of the run, in dependency order.
    pub layers: Vec<BuiltLayer>,
    /// The release of each product whose every layer is built.
    pub releases: Vec<BuiltRelease>,
    /// The products that give no steps for the environment; nothing of them is built.
    pub blocked: Vec<BlockedProduct>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BuiltLayer {
    pub step: String,
    pub key: String,
    pub reused: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BuiltRelease {
    pub product: String,
    /// The SHA-256 of `releases/<product>/<id>.json` in the store.
    pub id: String,
}

pub fn plan(root: &Path, products: &[&dyn Product], args: PlanArgs, json: bool) -> Result<(), Error> {
    let (store, http) = (Store::open()?, Http::new());
    let mut loaded = load(root, &args.env)?;
    loaded.env.moves = moves(&loaded.sources, &args.moves)?;
    loaded.env.live = live(root, products, &store, &args.env)?;
    let fetch = fetcher(&store, &http, &loaded.sources, &loaded.env);
    let (_, plan, blocked) = planned(root, &store, products, &mut loaded.env, &loaded.regions, fetch)?;
    let plan = env_plan(&loaded.env, &args.only, select(&plan, &args.only)?, blocked);
    if json {
        return print_json(&plan);
    }
    let layers = if plan.layers.is_empty() { "—".into() } else { plan.layers.join(", ") };
    println!("PLAN {} · region {} · layers {layers}", plan.env, plan.region);
    for blocked in &plan.blocked {
        println!("blocked {}: {}", blocked.product, blocked.reason);
    }
    if plan.groups.is_empty() {
        println!("The store has every layer.");
        return Ok(());
    }
    let mut table = vec![cells(["GROUP", "FETCH", "BUILD", "TIME", "OUTPUT"])];
    for group in &plan.groups {
        let fetch = group.fetches.iter().map(|fetch| fetch.bytes).sum::<Option<u64>>();
        let estimates: Option<Vec<Estimate>> = group.builds.iter().map(|build| build.estimate).collect();
        let total = |cost: fn(&Estimate) -> u64| estimates.as_ref().map(|all| all.iter().map(cost).sum::<u64>());
        table.push(vec![
            group.id.clone(),
            if group.fetches.is_empty() { String::new() } else { fetch.map_or("—".into(), bytes) },
            group.builds.iter().map(|build| build.step.as_str()).collect::<Vec<_>>().join(", "),
            total(|estimate| estimate.wall_ms).map_or("—".into(), duration),
            total(|estimate| estimate.bytes_out).map_or("—".into(), bytes),
        ]);
    }
    print_table(&table);
    Ok(())
}

pub fn build(root: &Path, products: &[&dyn Product], args: BuildArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    // A saved plan names every version, so live does not matter.
    let live = match args.plan {
        Some(_) => LiveVersions::new(),
        None => live(root, products, &store, &args.env)?,
    };
    let built = run_build(root, &store, &Http::new(), products, &args, live)?;
    if json {
        return print_json(&built);
    }
    for layer in &built.layers {
        let done = if layer.reused { "reused" } else { "built" };
        println!("{done:<7} {}  {}", &layer.key[..12], layer.step);
    }
    for release in &built.releases {
        println!("release {}  {}", &release.id[..8], release.product);
    }
    for blocked in &built.blocked {
        println!("blocked {}: {}", blocked.product, blocked.reason);
    }
    Ok(())
}

fn run_build(
    root: &Path,
    store: &Store,
    http: &Http,
    products: &[&dyn Product],
    args: &BuildArgs,
    live: LiveVersions,
) -> Result<Built, Error> {
    let mut loaded = load(root, &args.env)?;
    loaded.env.live = live;
    let saved = args.plan.as_deref().map(|file| read_plan(file, &loaded.env).map(|plan| (file, plan))).transpose()?;
    match &saved {
        Some((_, saved)) => {
            let versions = saved.versions.iter().map(|v| ((v.source.clone(), v.params.clone()), v.version.clone()));
            loaded.env.planned = Some(versions.collect());
        }
        None => loaded.env.moves = moves(&loaded.sources, &args.moves)?,
    }
    let mut fetch = fetcher(store, http, &loaded.sources, &loaded.env);
    let (env, regions) = (&mut loaded.env, &loaded.regions);
    let (steps, now, blocked) = match &saved {
        None => planned(root, store, products, env, regions, fetch)?,
        // A fetch without a version is one that the plan does not name.
        Some((file, _)) => planned(root, store, products, env, regions, |wanted| match &wanted.version {
            None => Err(outdated(file)),
            Some(version) => fetch(wanted).map_err(|e| match e.fix == e.code.fix() {
                true => {
                    let (plan, source) = (file.display(), &wanted.source);
                    let fix =
                        format!("{} Or plan again: {plan} reads `{source}@{version}`, which the store lacks.", e.fix);
                    e.fix(fix)
                }
                false => e,
            }),
        })?,
    };
    let wanted = match saved {
        None => select(&now, &args.only)?,
        Some((file, saved)) => {
            let groups = Plan { groups: saved.groups };
            if saved.blocked != blocked || !select(&now, &saved.only).is_ok_and(|now| now.same_work(&groups)) {
                return Err(outdated(file));
            }
            select(&groups, &args.only)?
        }
    };
    if !products.is_empty() && blocked.len() == products.len() {
        let reasons = blocked.iter().map(|b| format!("product `{}`: {}", b.product, b.reason)).collect::<Vec<_>>();
        return Err(Code::Blocked.error(reasons.join("; ")));
    }
    let mut built = Built { run: None, layers: Vec::new(), releases: Vec::new(), blocked };
    if !wanted.groups.is_empty() {
        let mut run = Run::create(store, &format!("build {}", loaded.env.name))?;
        let id = run.id().to_string();
        eprintln!("obc data: run {id}; `obc data runs {id} --follow` shows its events");
        let context = Context { store, root, sources: &loaded.sources, http, limits: Limits::machine() };
        let result = run.build(&context, &steps, &wanted);
        run.finish(result.as_ref().err().map(String::as_str))?;
        let layers = result.map_err(|e| Code::RunFailed.error(format!("run {id}: {e}")))?;
        built.layers = layers
            .into_iter()
            .map(|layer| BuiltLayer { step: layer.receipt.step, key: layer.receipt.key, reused: layer.reused })
            .collect();
        built.run = Some(id);
    }
    for product in products.iter().filter(|product| !built.blocked.iter().any(|b| b.product == product.name())) {
        if let Some(release) = release::release(store, root, product.name(), &steps)? {
            built.releases.push(BuiltRelease { product: product.name().into(), id: release.write(store)? });
        }
    }
    Ok(built)
}

/// The output of `plan ENV --json` in `file`, when it is a plan of `env`.
fn read_plan(file: &Path, env: &Env) -> Result<EnvPlan, Error> {
    let text = std::fs::read_to_string(file).map_err(|e| Code::Usage.error(format!("{}: {e}", file.display())))?;
    let saved: EnvPlan = serde_json::from_str(&text).map_err(|e| {
        Code::Usage.error(format!("{} is not the output of `obc data plan --json`: {e}", file.display()))
    })?;
    if (&saved.env, &saved.region, &saved.layers) != (&env.name, &env.region, &env.layers) {
        return Err(outdated(file));
    }
    Ok(saved)
}

fn outdated(file: &Path) -> Error {
    Code::PlanOutdated.error(format!("{} is not the plan of now", file.display()))
}

fn env_plan(env: &Env, only: &[String], plan: Plan, blocked: Vec<BlockedProduct>) -> EnvPlan {
    let moves = env.moves.iter().filter_map(|(source, version)| Some((source.clone(), version.clone()?)));
    let read = env.read.borrow();
    let versions = read.iter().map(|((source, params), version)| FetchVersion {
        source: source.clone(),
        params: params.clone(),
        version: version.clone(),
    });
    EnvPlan {
        env: env.name.clone(),
        region: env.region.clone(),
        layers: env.layers.clone(),
        moves: moves.collect(),
        versions: versions.collect(),
        only: only.to_vec(),
        groups: plan.groups,
        blocked,
    }
}

fn select(plan: &Plan, only: &[String]) -> Result<Plan, Error> {
    if only.is_empty() {
        return Ok(plan.clone());
    }
    plan.only(only).map_err(|e| Code::Usage.error(e))
}

/// An environment, with the sources and the regions that it was read with.
pub(super) struct Loaded {
    pub(super) env: Env,
    pub(super) sources: Vec<Source>,
    pub(super) regions: Regions,
}

pub(super) fn load(root: &Path, name: &str) -> Result<Loaded, Error> {
    let registry = registry(root)?;
    if !crate::is_kebab(name) || !Env::path(root, name).is_file() {
        return Err(Code::Usage.error(format!("no environment `{name}` in data/env/")));
    }
    let regions = Regions::load(root).map_err(|e| Code::InvalidData.error(e))?;
    let env = Env::load(root, name, &regions).map_err(|e| Code::InvalidData.error(e))?;
    Ok(Loaded { env, sources: registry.sources, regions })
}

/// The versions of each fetch that the live releases read, for the environment `live`. Another
/// environment has no live release.
fn live(root: &Path, products: &[&dyn Product], store: &Store, env: &str) -> Result<LiveVersions, Error> {
    match env {
        "live" => live_versions(root, products, store),
        _ => Ok(BTreeMap::new()),
    }
}

/// The `--move SOURCE[@VERSION]` of a plan or a build.
fn moves(sources: &[Source], moves: &[String]) -> Result<BTreeMap<String, Option<String>>, Error> {
    let mut parsed = BTreeMap::new();
    for arg in moves {
        let (id, version) = match arg.split_once('@') {
            Some((id, version)) => (id, Some(version.to_string())),
            None => (arg.as_str(), None),
        };
        let usage = |message: String| Code::Usage.error(format!("--move {arg}: {message}"));
        let source = sources.iter().find(|source| source.id == id).ok_or_else(|| usage(format!("no source `{id}`")))?;
        version.as_deref().map(|version| fetch::check_version(source, version)).transpose().map_err(usage)?;
        if parsed.insert(id.to_string(), version).is_some() {
            return Err(usage(format!("`{id}` moves twice")));
        }
    }
    Ok(parsed)
}

/// The steps of every product, the plan of all of them, and the products without steps.
fn planned(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    env: &mut Env,
    regions: &Regions,
    fetch: impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<(Vec<Step>, Plan, Vec<BlockedProduct>), Error> {
    let (steps, blocked) = steps(products, env, regions, store, fetch)?;
    let plan = plan::plan(store, root, &steps)?;
    Ok((steps, plan, blocked))
}

/// Fetch what a product names, and give the version fetched; with the code of a failed fetch:
/// `fetch_failed` or `blocked`. A `manual` source is fetched at the newest version upstream only
/// when `env` moves it there.
pub(super) fn fetcher<'a>(
    store: &'a Store,
    http: &'a Http,
    sources: &'a [Source],
    env: &Env,
) -> impl FnMut(&Wanted) -> Result<String, Error> + 'a {
    let newest: BTreeSet<String> = env.moves.keys().filter(|source| env.moves_to_newest(source)).cloned().collect();
    let moved: BTreeSet<String> = env.moves.keys().cloned().collect();
    let reads = env.live.iter().flat_map(|((source, _), read)| read.iter().map(move |version| (source, version)));
    let live: BTreeSet<(String, String)> = reads.map(|(source, version)| (source.clone(), version.clone())).collect();
    move |wanted| {
        let source = sources
            .iter()
            .find(|source| source.id == wanted.source)
            .ok_or_else(|| Code::InvalidData.error(format!("no source `{}` in data/sources.toml", wanted.source)))?;
        let pick = format!("Plan with `--move {}@VERSION`.", source.id);
        if wanted.version.is_none() && source.refresh == Refresh::Manual && !newest.contains(&source.id) {
            let message =
                format!("source `{}` is manual, and neither live nor the store has a version of it", source.id);
            return Err(Code::Blocked.error(message).fix(pick));
        }
        let unnamed = fetch::params(source).into_iter().find(|name| wanted.params.iter().all(|(n, _)| n != name));
        if let Some(name) = unnamed.filter(|_| wanted.version.is_none()) {
            let message = format!("source `{}` is fetched per `{name}=`: it has no one newest version", source.id);
            return Err(Code::Usage.error(message).fix(pick));
        }
        let request = Request { source, version: wanted.version.clone(), params: wanted.params.clone() };
        let of_live =
            |version: &String| !moved.contains(&source.id) && live.contains(&(source.id.clone(), version.clone()));
        fetched(source, fetch::fetch(store, http, &request)).map(|snapshot| snapshot.version).map_err(|e| {
            match e.code == Code::FetchFailed && wanted.version.as_ref().is_some_and(of_live) {
                true => e.fix(format!(
                    "Upstream can stop serving an old version: plan with `--move {}` to read the newest.",
                    source.id
                )),
                false => e,
            }
        })
    }
}

/// An error in the code of product `name`, not in `data/`.
fn product_bug(name: &str, message: String) -> Error {
    let fix = format!("The code of product `{name}` is wrong, not the data: correct the product in its crate.");
    Code::Failed.error(message).fix(fix)
}

/// The fetch rounds of one product: a loop guard, far above the two that the planner needs.
const ROUNDS: usize = 8;

/// The steps of every product, and the products that give `Unplanned::Invalid`. A product whose
/// step list reads snapshots that the store lacks gets them fetched, as [`product_steps`] says. The
/// fetch for a `--move SOURCE` names its version in `env`, so every product reads that one version.
pub(super) fn steps(
    products: &[&dyn Product],
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    mut fetch: impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<(Vec<Step>, Vec<BlockedProduct>), Error> {
    check_layers(products, env)?;
    let (mut all, mut blocked) = (Vec::new(), Vec::new());
    for product in products {
        match product_steps(*product, env, regions, store, &mut fetch)? {
            Ok(steps) => all.extend(steps),
            Err(reason) => blocked.push(BlockedProduct { product: product.name().into(), reason }),
        }
    }
    let read = env.read.borrow();
    if let Some(source) = env.moves.keys().find(|moved| !read.keys().any(|(source, _)| source == *moved)) {
        let message = format!("--move {source}: no step list of `{}` reads `{source}`", env.name);
        return Err(Code::Usage.error(message));
    }
    Ok((all, blocked))
}

/// Refuse an optional layer of `env` that no product has.
pub(super) fn check_layers(products: &[&dyn Product], env: &Env) -> Result<(), Error> {
    let offered = |layer: &&String| products.iter().any(|product| product.optional().contains(&layer.as_str()));
    if let Some(layer) = env.layers.iter().find(|layer| !offered(layer)) {
        let message = format!("data/env/{}.toml: no product has the optional layer `{layer}`", env.name);
        return Err(Code::InvalidData.error(message));
    }
    Ok(())
}

/// The steps of one product, after the fetches that its step list needs: it is asked again while
/// each round names only new fetches, at most [`ROUNDS`] times. `Ok(Err(reason))` when the
/// product is blocked (`Unplanned::Invalid`).
pub(super) fn product_steps(
    product: &dyn Product,
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    fetch: &mut impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<Result<Vec<Step>, String>, Error> {
    let name = product.name();
    // A refusal belongs to the product that is listed now.
    env.refused.borrow_mut().clear();
    let mut listed = product.steps(env, regions, store);
    // A fetch can name the next one, such as the `.poly` that gives the box of a capture.
    let mut fetched: Vec<Wanted> = Vec::new();
    for _ in 0..ROUNDS {
        let Err(Unplanned::NeedsFetch(fetches)) = &listed else { break };
        if fetches.iter().any(|wanted| fetched.contains(wanted)) {
            break;
        }
        for wanted in fetches {
            let version = fetch(wanted)?;
            if env.moves_to_newest(&wanted.source) {
                env.moves.insert(wanted.source.clone(), Some(version));
            }
        }
        fetched.extend(fetches.iter().cloned());
        listed = product.steps(env, regions, store);
    }
    let steps = match listed {
        Ok(steps) => steps,
        Err(Unplanned::NeedsFetch(fetches)) => {
            let wanted = fetches
                .iter()
                .map(|f| format!("{}@{}", f.source, f.version.as_deref().unwrap_or("newest")))
                .collect::<Vec<_>>();
            return Err(product_bug(
                name,
                format!("product `{name}` still needs {} after the fetch", wanted.join(", ")),
            ));
        }
        Err(Unplanned::Invalid(e) | Unplanned::Failed(e)) if !env.refused.borrow().is_empty() => {
            let source = env.refused.borrow().first().cloned().unwrap_or_default();
            let fix = format!("Plan with `--move {source}@VERSION`.");
            return Err(Code::Blocked.error(format!("product `{name}`: {e}")).fix(fix));
        }
        Err(Unplanned::Invalid(reason)) => return Ok(Err(reason)),
        Err(Unplanned::Failed(e)) => return Err(Code::Failed.error(format!("product `{name}`: {e}"))),
    };
    if let Some(step) = steps.iter().find(|step| !step.name.starts_with(&format!("{name}/"))) {
        return Err(product_bug(name, format!("step `{}` of product `{name}` is not named `{name}/…`", step.name)));
    }
    Ok(Ok(steps))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::engine::tests::{fixture, pipeline, write, JOIN};
    use crate::engine::{snapshot_files, Input};

    /// The test pipeline, listed once the store has the snapshot `index@1`.
    struct Indexed;

    impl Product for Indexed {
        fn name(&self) -> &'static str {
            "test"
        }

        fn optional(&self) -> &'static [&'static str] {
            &["extra"]
        }

        fn steps(&self, _: &Env, _: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
            match snapshot_files(store, "index", "1", &[], &[]).map_err(Unplanned::Failed)? {
                Some(_) => Ok(pipeline()),
                None => Err(Unplanned::NeedsFetch(vec![Wanted {
                    source: "index".into(),
                    version: Some("1".into()),
                    params: Vec::new(),
                }])),
            }
        }
    }

    fn env(layers: &[&str]) -> Env {
        let layers = layers.iter().map(|layer| layer.to_string()).collect();
        Env { name: "live".into(), region: "monaco".into(), layers, ..Env::default() }
    }

    #[test]
    fn a_step_list_that_reads_a_snapshot_is_listed_again_after_its_fetch() {
        let (fixture, empty) = (fixture("cli-two-stage"), fixture("cli-two-stage-empty"));
        let regions = Regions::new(Vec::new()).unwrap();
        let fetches = Cell::new(0);
        let fetch = |wanted: &Wanted| {
            fixture.fetched(&wanted.source, "index.txt", b"index\n");
            fetches.set(fetches.get() + 1);
            Ok("1".into())
        };
        let (listed, _) = steps(&[&Indexed], &mut env(&["extra"]), &regions, &fixture.store, fetch).unwrap();
        assert_eq!((listed.len(), fetches.get()), (3, 1));
        let (listed, _) = steps(&[&Indexed], &mut env(&[]), &regions, &fixture.store, |_| unreachable!()).unwrap();
        assert_eq!(listed.len(), 3, "the store has it now");

        let err = steps(&[&Indexed], &mut env(&[]), &regions, &empty.store, |_| Ok("1".into())).err().unwrap();
        assert_eq!(err.message, "product `test` still needs index@1 after the fetch");
        assert!(err.fix.contains("product `test`"), "a product bug points at its code: {}", err.fix);
        let blocked = |_: &Wanted| Err(Code::Blocked.error("credential missing"));
        let err = steps(&[&Indexed], &mut env(&[]), &regions, &empty.store, blocked).err().unwrap();
        assert_eq!(err.code, Code::Blocked, "a failed fetch keeps its code");
        let err = steps(&[&Indexed], &mut env(&["snow"]), &regions, &fixture.store, |_| Ok("1".into())).err().unwrap();
        let message = "data/env/live.toml: no product has the optional layer `snow`";
        assert_eq!((err.code, err.message.as_str()), (Code::InvalidData, message));
    }

    /// The test pipeline, once the store has `index@1` and then `box@1`, which only the index names.
    struct Chained;

    impl Product for Chained {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
            Indexed.steps(env, regions, store)?;
            match snapshot_files(store, "box", "1", &[], &[]).map_err(Unplanned::Failed)? {
                Some(_) => Ok(pipeline()),
                None => Err(Unplanned::NeedsFetch(vec![Wanted {
                    source: "box".into(),
                    version: Some("1".into()),
                    params: Vec::new(),
                }])),
            }
        }
    }

    #[test]
    fn a_fetch_that_names_the_next_fetch_gets_another_round() {
        let fixture = fixture("cli-two-rounds");
        let regions = Regions::new(Vec::new()).unwrap();
        let fetched = std::cell::RefCell::new(Vec::new());
        let fetch = |wanted: &Wanted| {
            fixture.fetched(&wanted.source, "file.txt", wanted.source.as_bytes());
            fetched.borrow_mut().push(wanted.source.clone());
            Ok("1".into())
        };
        let (listed, _) = steps(&[&Chained], &mut env(&[]), &regions, &fixture.store, fetch).unwrap();
        assert_eq!((listed.len(), fetched.into_inner()), (3, vec!["index".to_string(), "box".to_string()]));
    }

    /// No steps, once the store has the outline of `area=europe/monaco`.
    struct Outlined;

    impl Product for Outlined {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, env: &Env, _: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
            let area = [("area".to_string(), "europe/monaco".to_string())];
            match crate::product::read(env, store, "land", &area).map_err(Unplanned::Failed)? {
                Ok(_) => Ok(Vec::new()),
                Err(wanted) => Err(Unplanned::NeedsFetch(vec![wanted])),
            }
        }
    }

    #[test]
    fn a_source_that_nothing_names_is_fetched_at_the_newest_version_unless_it_is_manual() {
        use crate::fetch::tests::{quick, serve, source, whole};
        let (url, log) = serve(|_, _| whole(b"outline"));
        let fixture = fixture("cli-newest");
        let manual = source(&url.replace("data/file.bin", "{area}.poly"), "date");
        let (regions, http) = (Regions::new(Vec::new()).unwrap(), quick());
        let mut live = env(&[]);
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&manual), &live);
        let err = steps(&[&Outlined], &mut live, &regions, &fixture.store, fetch).err().unwrap();
        assert_eq!(
            (err.code, err.fix.as_str()),
            (Code::Blocked, "Plan with `--move land@VERSION`."),
            "{}",
            err.message
        );
        assert!(log.lock().unwrap().is_empty(), "a manual source does not move by itself");

        live.moves.insert("land".into(), None);
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&manual), &live);
        steps(&[&Outlined], &mut live, &regions, &fixture.store, fetch).unwrap();
        assert_eq!(
            live.moves["land"].as_deref(),
            Some("2026-10-05"),
            "the fetch names the move: the Last-Modified day"
        );
        let requests = log.lock().unwrap().len();

        let land = Source { refresh: Refresh::Days(30), ..manual };
        let mut live = env(&[]);
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&land), &live);
        steps(&[&Outlined], &mut live, &regions, &fixture.store, fetch).unwrap();
        assert_eq!(log.lock().unwrap().len(), requests, "without a move or live, the store serves");

        live.moves.insert("qrank".into(), None);
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&land), &live);
        let err = steps(&[&Outlined], &mut live, &regions, &fixture.store, fetch).err().unwrap();
        assert_eq!(
            (err.code, err.message.as_str()),
            (Code::Usage, "--move qrank: no step list of `live` reads `qrank`")
        );
        let mut fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&land), &live);
        let err = fetch(&Wanted { source: "land".into(), version: None, params: Vec::new() }).unwrap_err();
        assert!(err.message.contains("fetched per `area=`"), "{}", err.message);
    }

    /// The test pipeline, with `head` at the version that `product::version` gives.
    struct Versioned;

    impl Product for Versioned {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, env: &Env, _: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
            let head = crate::product::version(env, store, "head", &[]).map_err(Unplanned::Invalid)?;
            let head = head.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            let mut steps = pipeline();
            for input in steps.iter_mut().flat_map(|step| &mut step.inputs) {
                if let Input::Snapshot { source, version, .. } = input {
                    if source == "head" {
                        version.clone_from(&head);
                    }
                }
            }
            Ok(steps)
        }
    }

    #[test]
    fn a_move_chooses_between_two_versions_that_live_reads() {
        let fixture = fixture("cli-conflict");
        let regions = Regions::new(Vec::new()).unwrap();
        let mut live = env(&[]);
        live.live.insert(("head".into(), Vec::new()), ["1".to_string(), "2".to_string()].into());
        let err = steps(&[&Versioned], &mut live.clone(), &regions, &fixture.store, |_| unreachable!()).err().unwrap();
        assert_eq!(
            (err.code, err.fix.as_str()),
            (Code::Blocked, "Plan with `--move head@VERSION`."),
            "{}",
            err.message
        );

        live.moves.insert("head".into(), Some("1".into()));
        let (listed, _) = steps(&[&Versioned], &mut live, &regions, &fixture.store, |_| unreachable!()).unwrap();
        assert_eq!(listed.len(), 3);
    }

    #[test]
    fn a_build_of_a_plan_reads_the_versions_of_the_plan_and_not_a_newer_one() {
        let fixture = fixture("cli-versions");
        let root = fixture.root();
        write(&root.join("data/sources.toml"), include_str!("../../../../data/sources.toml"));
        write(&root.join("data/regions/monaco.toml"), "name = \"Monaco\"\nkind = \"geofabrik\"\n");
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        let http = Http::new();
        let build = |plan: Option<PathBuf>| {
            let args = BuildArgs { env: "live".into(), only: Vec::new(), plan, moves: Vec::new() };
            run_build(&root, &fixture.store, &http, &[&Versioned], &args, Default::default()).unwrap().releases
        };
        let first = build(None)[0].id.clone();
        let mut loaded = load(&root, "live").unwrap();
        let (env, regions) = (&mut loaded.env, &loaded.regions);
        let (_, plan, blocked) =
            planned(&root, &fixture.store, &[&Versioned], env, regions, |_| unreachable!()).unwrap();
        let saved = env_plan(&loaded.env, &[], plan, blocked);
        assert_eq!(saved.versions, [FetchVersion { source: "head".into(), params: Vec::new(), version: "1".into() }]);
        let file = root.join("plan.json");
        write(&file, &serde_json::to_string(&saved).unwrap());

        // Another checkout fetches a newer `head` into the store and builds it.
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        assert_ne!(build(None)[0].id, first);
        assert_eq!(build(Some(file))[0].id, first, "the plan reads head@1");
    }

    #[test]
    fn a_build_refuses_an_outdated_plan_and_writes_the_release_when_every_layer_is_built() {
        let fixture = fixture("cli-build");
        let root = fixture.root();
        write(&root.join("data/sources.toml"), include_str!("../../../../data/sources.toml"));
        write(&root.join("data/regions/monaco.toml"), "name = \"Monaco\"\nkind = \"geofabrik\"\n");
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        fixture.fetched("index", "index.txt", b"index\n");
        let http = Http::new();
        let mut loaded = load(&root, "live").unwrap();
        let (env, regions) = (&mut loaded.env, &loaded.regions);
        let (_, plan, blocked) = planned(&root, &fixture.store, &[&Indexed], env, regions, |_| unreachable!()).unwrap();
        let saved = env_plan(&loaded.env, &[], plan, blocked);
        let file = root.join("plan.json");
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: Some(file.clone()), moves: Vec::new() };
        let build = || run_build(&root, &fixture.store, &http, &[&Indexed], &args, Default::default());
        let refused = |saved: &EnvPlan, why: &str| {
            write(&file, &serde_json::to_string(saved).unwrap());
            let err = build().unwrap_err();
            assert_eq!((err.code, err.code.exit()), (Code::PlanOutdated, 3), "{why}");
            assert!(!fixture.store.root().join("layers").exists(), "nothing is built");
        };

        refused(&EnvPlan { groups: Vec::new(), ..saved.clone() }, "a group that is new since the plan");
        write(&root.join("steps/src/lib.rs"), "// Another key.\n");
        refused(&saved, "another key");
        write(&root.join("steps/src/lib.rs"), "");
        write(&root.join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        refused(&saved, "the code of a step that waits for another build");

        write(&root.join("join.py"), JOIN);
        write(&file, &serde_json::to_string(&saved).unwrap());
        let built = build().unwrap();
        let steps: Vec<&str> = built.layers.iter().map(|layer| layer.step.as_str()).collect();
        assert_eq!(steps, ["test/upper", "test/join", "test/count"]);
        assert!(fixture.store.release("test", &built.releases[0].id).is_file());
    }

    /// A product that the environment does not suit.
    struct Refused;

    impl Product for Refused {
        fn name(&self) -> &'static str {
            "other"
        }

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
            Err(Unplanned::Invalid("no box region".into()))
        }
    }

    #[test]
    fn a_product_without_steps_is_blocked_and_the_others_plan_and_build() {
        let fixture = fixture("cli-blocked");
        let root = fixture.root();
        write(&root.join("data/sources.toml"), include_str!("../../../../data/sources.toml"));
        write(&root.join("data/regions/monaco.toml"), "name = \"Monaco\"\nkind = \"geofabrik\"\n");
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        fixture.fetched("index", "index.txt", b"index\n");
        let (http, products): (_, [&dyn Product; 2]) = (Http::new(), [&Indexed, &Refused]);
        let mut loaded = load(&root, "live").unwrap();
        let (env, regions) = (&mut loaded.env, &loaded.regions);
        let (_, plan, blocked) = planned(&root, &fixture.store, &products, env, regions, |_| unreachable!()).unwrap();
        assert_eq!(blocked, [BlockedProduct { product: "other".into(), reason: "no box region".into() }]);
        assert_eq!(plan.groups.len(), 1, "the test product plans");

        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() };
        let built = run_build(&root, &fixture.store, &http, &products, &args, Default::default()).unwrap();
        assert_eq!((built.layers.len(), built.releases.len(), &built.blocked), (3, 1, &blocked));
        assert_eq!(built.releases[0].product, "test", "a blocked product has no release");
        let again = run_build(&root, &fixture.store, &http, &products, &args, Default::default()).unwrap();
        assert_eq!((again.run, again.releases[0].id == built.releases[0].id), (None, true), "up to date");
        let err = run_build(&root, &fixture.store, &http, &[&Refused], &args, Default::default()).unwrap_err();
        assert_eq!((err.code.exit(), err.message.as_str()), (4, "product `other`: no box region"), "no product suits");
    }
}
