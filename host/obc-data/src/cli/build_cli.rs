//! `obc data plan ENV` and `obc data build ENV`: the steps of every product for an environment,
//! what a build would fetch and build, and the build into the store. Nothing uploads. A plan of
//! `live` compares the steps with the live releases, and has one group per cause.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::runs_cli::{bytes, duration};
use super::{cells, fetched, print_json, print_table, registry, stale, Code, Error};
use crate::engine::changes::{self, Against};
use crate::engine::plan::{self, Cause, Estimate, Group, Plan};
use crate::engine::release::{self, Release};
use crate::engine::runs::{Context, Limits, Run};
use crate::engine::{Input, Step};
use crate::env::Env;
use crate::fetch::{self, http::Http, Request};
use crate::live::{Check, Live, LiveProduct, Remote, Removal};
use crate::product::{Product, Unplanned, Wanted};
use crate::regions::Regions;
use crate::sources::{Refresh, Source};
use crate::store::Store;

#[derive(Args)]
pub struct PlanArgs {
    /// The environment: `data/env/ENV.toml`.
    env: String,
    /// Only these groups, by id. Against `live`, only these moves.
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
    pub(super) env: String,
    /// Only these groups, by id. Against `live`, only these moves.
    #[arg(long, value_delimiter = ',', conflicts_with = "plan")]
    pub(super) only: Vec<String>,
    /// Build the groups of this output of `plan ENV --json`, with its versions. Exit status 3 when
    /// live or the plan of now differs.
    #[arg(long)]
    pub(super) plan: Option<PathBuf>,
    /// Read this version of a source instead of the version of live; without a version, the newest
    /// version upstream.
    #[arg(long = "move", value_name = "SOURCE[@VERSION]", conflicts_with = "plan")]
    pub(super) moves: Vec<String>,
}

/// What a build of an environment would fetch and build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvPlan {
    pub env: String,
    pub region: String,
    pub layers: Vec<String>,
    /// The version of each source that the plan moves: each `--move`, and for `live` each stale
    /// source that the step lists read. A move without a version has the version that its fetch
    /// gave.
    pub moves: BTreeMap<String, String>,
    /// The version of each fetch that the step lists read. `build --plan` reads exactly these.
    pub versions: Vec<FetchVersion>,
    /// For `live`: the release that each product has live now. Empty for another environment.
    pub live: Vec<LiveRelease>,
    /// For `live`: what the environment file changes against the live releases.
    pub edits: Vec<Edit>,
    /// The groups that `--only` selected, or none for every group.
    pub only: Vec<String>,
    pub groups: Vec<Group>,
    /// The products that give no steps for the environment. The others plan without them.
    pub blocked: Vec<BlockedProduct>,
    /// For `live`: the keys that an apply of the plan removes from R2.
    pub remove: Vec<Removal>,
    /// For `live`: whether the plan listed R2, which needs the bucket. Without a listing, the plan
    /// has no `repair` group, and `remove` lacks the leftovers and the files that a client finds
    /// by name.
    pub listed: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LiveRelease {
    pub product: String,
    /// `None` when nothing is live.
    pub release: Option<String>,
}

/// What the environment file changes against the live release of a product.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    /// `from` is `None` when the product has nothing live.
    Region { product: String, from: Option<String>, to: String },
    /// The optional layers that the environment switches on and off.
    Layers { product: String, on: Vec<String>, off: Vec<String> },
}

/// What a build did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Built {
    /// `None` when there was nothing to fetch or build.
    pub run: Option<String>,
    /// The layers of the run, in dependency order.
    pub layers: Vec<BuiltLayer>,
    /// The release of each product whose every layer is built. For `live`, the release of each
    /// product that the groups change: its live layers with the layers of the groups.
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
    let remote = live_remote(&args.env)?;
    let basis = Basis::Moves(&args.moves);
    let plan = planned(root, &store, &http, remote.as_ref(), products, &args.env, &args.only, basis)?.plan;
    if json {
        return print_json(&plan);
    }
    print_plan(&plan);
    Ok(())
}

/// The plan as text.
pub(super) fn print_plan(plan: &EnvPlan) {
    let layers = if plan.layers.is_empty() { "—".into() } else { plan.layers.join(", ") };
    println!("PLAN {} · region {} · layers {layers}", plan.env, plan.region);
    for live in &plan.live {
        let release = live.release.as_ref().map_or("nothing".into(), |id| format!("release {}", &id[..8]));
        println!("live {}: {release}", live.product);
    }
    for blocked in &plan.blocked {
        println!("blocked {}: {}", blocked.product, blocked.reason);
    }
    if plan.groups.is_empty() {
        println!("{}", if plan.live.is_empty() { "The store has every layer." } else { "Live has every change." });
    } else {
        let mut table = vec![cells(["GROUP", "CHANGE", "FETCH", "BUILD", "TIME", "OUTPUT"])];
        for group in &plan.groups {
            let cost = Cost::of(std::slice::from_ref(group));
            table.push(vec![
                group.id.clone(),
                change(group, &plan.edits),
                if group.fetches.is_empty() { String::new() } else { cost.fetch.map_or("—".into(), bytes) },
                group.builds.iter().map(|build| build.step.as_str()).collect::<Vec<_>>().join(", "),
                cost.wall_ms.map_or("—".into(), duration),
                cost.bytes_out.map_or("—".into(), bytes),
            ]);
        }
        print_table(&table);
    }
    if !plan.live.is_empty() {
        let size = plan.remove.iter().map(|removal| removal.bytes).sum::<Option<u64>>().map_or("—".into(), bytes);
        let unlisted = if plan.listed { "" } else { "; R2 was not listed, so leftovers are unknown" };
        println!("REMOVE FROM R2 {} keys, {size}{unlisted}", plan.remove.len());
    }
}

/// The plan of `live` that the TUI shows: `plan live --only ONLY --json`.
pub(super) fn plan_live(root: &Path, products: &[&dyn Product], only: &[String]) -> Result<EnvPlan, Error> {
    let (store, http) = (Store::open()?, Http::new());
    let remote = super::remote()?;
    Ok(planned(root, &store, &http, Some(&remote), products, "live", only, Basis::Moves(&[]))?.plan)
}

/// What groups cost: the download, the build time and the output; `None` when the store does not
/// know one of the parts. A fetch or a build that two groups need counts once.
pub(super) struct Cost {
    pub fetch: Option<u64>,
    pub wall_ms: Option<u64>,
    pub bytes_out: Option<u64>,
}

impl Cost {
    pub fn of(groups: &[Group]) -> Cost {
        let plan = Plan { groups: groups.to_vec() };
        let mut steps = BTreeSet::new();
        let builds = plan.builds().filter(|build| steps.insert(build.step.as_str()));
        let estimates: Option<Vec<Estimate>> = builds.map(|build| build.estimate).collect();
        let total = |cost: fn(&Estimate) -> u64| estimates.as_ref().map(|all| all.iter().map(cost).sum::<u64>());
        Cost {
            fetch: plan.fetches().iter().map(|fetch| fetch.bytes).sum(),
            wall_ms: total(|estimate| estimate.wall_ms),
            bytes_out: total(|estimate| estimate.bytes_out),
        }
    }
}

/// What a group changes, in a few words.
pub(super) fn change(group: &Group, edits: &[Edit]) -> String {
    let edits = |region: bool| {
        let edits = edits.iter().filter(|edit| matches!(edit, Edit::Region { .. }) == region);
        edits.map(Edit::text).collect::<Vec<_>>().join("; ")
    };
    match &group.cause {
        None => String::new(),
        Some(Cause::Region) => edits(true),
        Some(Cause::Layers) => edits(false),
        Some(Cause::Move { source, from, to }) => {
            format!("move {source} {} → {to}", if from.is_empty() { "—".into() } else { from.join(", ") })
        }
        Some(Cause::Code { paths, crates }) if paths.is_empty() && crates.is_empty() => "no step makes it".into(),
        Some(Cause::Code { paths, crates }) => format!("code of {}", [&paths[..], crates].concat().join(", ")),
        Some(Cause::Repair { keys }) => format!("{} keys that R2 lacks", keys.len()),
    }
}

pub fn build(root: &Path, products: &[&dyn Product], args: BuildArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let remote = live_remote(&args.env)?;
    let built = run_build(root, &store, &Http::new(), remote.as_ref(), products, &args)?;
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

/// Where live is read for the environment `env`: only `live` has a live release.
fn live_remote(env: &str) -> Result<Option<Remote>, Error> {
    (env == "live").then(super::remote).transpose()
}

/// `remote` is where live is read, for a build of live.
fn run_build(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: Option<&Remote>,
    products: &[&dyn Product],
    args: &BuildArgs,
) -> Result<Built, Error> {
    let saved = args.plan.as_deref().map(read_plan).transpose()?;
    Ok(build_env(root, store, http, remote, products, args, saved.as_ref())?.0)
}

/// Live, and live after an apply of the plan of a build.
pub(super) struct Applying {
    pub(super) live: Live,
    pub(super) next: Live,
}

/// Build `args.env`, or the plan `saved`. A build of live also gives what an apply of it changes.
pub(super) fn build_env(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: Option<&Remote>,
    products: &[&dyn Product],
    args: &BuildArgs,
    saved: Option<&EnvPlan>,
) -> Result<(Built, Option<Applying>), Error> {
    let (basis, only) = match saved {
        Some(saved) => (Basis::Saved(saved), &saved.only),
        None => (Basis::Moves(&args.moves), &args.only),
    };
    let Planned { loaded, steps, plan, live } = planned(root, store, http, remote, products, &args.env, only, basis)?;
    if let Some(saved) = saved {
        let unchanged = (&saved.env, &saved.region, &saved.layers, &saved.blocked, &saved.live, &saved.edits)
            == (&plan.env, &plan.region, &plan.layers, &plan.blocked, &plan.live, &plan.edits);
        if !unchanged || !(Plan { groups: plan.groups.clone() }).same_work(&Plan { groups: saved.groups.clone() }) {
            return Err(outdated());
        }
    }
    suits(products, &plan)?;
    let mut built = Built { run: None, layers: Vec::new(), releases: Vec::new(), blocked: plan.blocked.clone() };
    let work = Plan { groups: plan.groups.clone() };
    if work.builds().next().is_some() || !work.fetches().is_empty() {
        let mut run = Run::create(store, &format!("build {}", loaded.env.name))?;
        let id = run.id().to_string();
        eprintln!("obc data: run {id}; `obc data runs {id} --follow` shows its events");
        let context = Context { store, root, sources: &loaded.sources, http, limits: Limits::machine() };
        let result = run.build(&context, &steps, &work);
        run.finish(result.as_ref().err().map(String::as_str))?;
        let layers = result.map_err(|e| Code::RunFailed.error(format!("run {id}: {e}")))?;
        built.layers = layers
            .into_iter()
            .map(|layer| BuiltLayer { step: layer.receipt.step, key: layer.receipt.key, reused: layer.reused })
            .collect();
        built.run = Some(id);
    }
    let Some(live) = live else {
        let unblocked = products.iter().filter(|product| !plan.blocked.iter().any(|b| b.product == product.name()));
        for product in unblocked {
            let (name, optional) = (product.name(), optional(*product, &plan.layers));
            if let Some(release) = release::release(store, root, name, &plan.region, &optional, &steps)? {
                built.releases.push(BuiltRelease { product: name.into(), id: release.write(store)? });
            }
        }
        return Ok((built, None));
    };
    let (next, missing) = next(root, store, products, &loaded.sources, &live, &steps, &plan)?;
    if let Some(layer) = missing.first() {
        return Err(Code::Failed.error(format!("the store lacks the layer `{layer}` after the build")));
    }
    for (next, live) in next.products.iter().zip(&live.products) {
        if let Some((id, release)) = next.release.as_ref().filter(|_| changed(live, next)) {
            release.write(store)?;
            built.releases.push(BuiltRelease { product: release.product.clone(), id: id.clone() });
        }
    }
    Ok((built, Some(Applying { live, next })))
}

/// Fail with `blocked` when no product suits the environment of `plan`.
pub(super) fn suits(products: &[&dyn Product], plan: &EnvPlan) -> Result<(), Error> {
    if !products.is_empty() && plan.blocked.len() == products.len() {
        let reasons = plan.blocked.iter().map(|b| format!("product `{}`: {}", b.product, b.reason)).collect::<Vec<_>>();
        return Err(Code::Blocked.error(reasons.join("; ")));
    }
    Ok(())
}

/// Whether `next` names another release than `live`.
pub(super) fn changed(live: &LiveProduct, next: &LiveProduct) -> bool {
    let id = |product: &LiveProduct| product.release.as_ref().map(|(id, _)| id.clone());
    id(live) != id(next)
}

/// The plan of `live` now, against the live releases that `remote` holds.
pub(super) fn plan_live(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: &Remote,
    products: &[&dyn Product],
) -> Result<EnvPlan, Error> {
    Ok(planned(root, store, http, Some(remote), products, "live", &[], Basis::Moves(&[]))?.plan)
}

/// The output of `plan ENV --json` in `file`.
pub(super) fn read_plan(file: &Path) -> Result<EnvPlan, Error> {
    let text = std::fs::read_to_string(file).map_err(|e| Code::Usage.error(format!("{}: {e}", file.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| Code::Usage.error(format!("{} is not the output of `obc data plan --json`: {e}", file.display())))
}

fn outdated() -> Error {
    Code::PlanOutdated.error("the plan is not the plan of now")
}

/// The groups that `only` names. Against live, `only` names moves, and the plan keeps every group:
/// a move that it does not name does not move, and the rest of live follows `data/` and the code.
fn select(plan: &Plan, only: &[String], live: bool) -> Result<Plan, Error> {
    if only.is_empty() {
        return Ok(plan.clone());
    }
    if let Some(id) = only.iter().find(|id| live && !id.starts_with("move:")) {
        return Err(Code::Usage.error(format!("`--only {id}`: against live, `--only` selects moves only")));
    }
    let selected = plan.only(only).map_err(|e| Code::Usage.error(e))?;
    Ok(if live { plan.clone() } else { selected })
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

/// Where the versions of a plan come from.
#[derive(Clone, Copy)]
enum Basis<'a> {
    /// The `--move`s, and for live the versions that live reads.
    Moves(&'a [String]),
    /// The plan in a file: its versions and its moves.
    Saved(&'a EnvPlan),
}

/// The plan of now, with what it was made from.
struct Planned {
    loaded: Loaded,
    /// The steps of every product that is not blocked.
    steps: Vec<Step>,
    plan: EnvPlan,
    /// What is live, for a plan against live.
    live: Option<Live>,
}

/// The plan of the environment `name`, of the groups that `only` selects. With `remote`, it reads
/// live there and plans against it.
#[allow(clippy::too_many_arguments)]
fn planned(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: Option<&Remote>,
    products: &[&dyn Product],
    name: &str,
    only: &[String],
    basis: Basis,
) -> Result<Planned, Error> {
    let mut loaded = load(root, name)?;
    let live = remote.map(|remote| Live::read(remote, products, &loaded.sources, store)).transpose();
    let live = live.map_err(|e| Code::R2Failed.error(e))?;
    let env = &mut loaded.env;
    match basis {
        Basis::Saved(saved) => {
            let versions = saved.versions.iter().map(|v| ((v.source.clone(), v.params.clone()), v.version.clone()));
            env.planned = Some(versions.collect());
            env.moves = saved.moves.iter().map(|(source, version)| (source.clone(), Some(version.clone()))).collect();
        }
        Basis::Moves(args) => {
            env.moves = moves(&loaded.sources, args)?;
            if let Some(live) = &live {
                env.live = live.versions();
                for (source, version) in stale(store, http, &loaded.sources, &live.by_source()) {
                    if !env.moves.contains_key(&source) {
                        env.moves.insert(source.clone(), version);
                        env.stale.insert(source);
                    }
                }
                if !only.is_empty() {
                    let (stale, args) = (&env.stale, &env.moves);
                    for source in args.keys().filter(|source| !stale.contains(*source)) {
                        if !only.contains(&format!("move:{source}")) {
                            eprintln!("obc data: `--only` leaves out `move:{source}`: `--move {source}` is dropped");
                        }
                    }
                    env.moves.retain(|source, _| only.contains(&format!("move:{source}")));
                }
            }
        }
    }
    let fetch = fetcher(store, http, &loaded.sources, env);
    let (steps, blocked) = match basis {
        Basis::Moves(_) => steps(products, env, &loaded.regions, store, live.is_some(), fetch)?,
        // A fetch without a version is one that the plan does not name.
        Basis::Saved(_) => {
            let mut fetch = fetch;
            steps(products, env, &loaded.regions, store, live.is_some(), |wanted| match &wanted.version {
                None => Err(outdated()),
                Some(version) => fetch(wanted).map_err(|e| match e.fix == e.code.fix() {
                    true => {
                        let source = &wanted.source;
                        let fix = format!(
                            "{} Or plan again: the plan reads `{source}@{version}`, which the store lacks.",
                            e.fix
                        );
                        e.fix(fix)
                    }
                    false => e,
                }),
            })?
        }
    };
    let read = env.read.borrow().clone();
    env.moves.retain(|source, _| read.keys().any(|(id, _)| id == source));

    let Some(live) = live else {
        let all = plan::plan(store, root, &steps)?;
        let plan = env_plan(env, only, select(&all, only, false)?, blocked, None);
        return Ok(Planned { loaded, steps, plan, live: None });
    };
    let edits = edits(products, env, &live, &blocked);
    let mut listed = match remote {
        Some(remote @ Remote::Bucket(_)) => Some(live.list(remote).map_err(|e| Code::R2Failed.error(e))?),
        _ => None,
    };
    let check = listed.as_deref().map(|listed| live.check(listed));
    let against = against(&live, env, &edits, &blocked, check.as_ref());
    let all = changes::changes(store, root, &steps, &against)?;
    let mut plan = env_plan(env, only, select(&all, only, true)?, blocked, Some((&live, edits)));
    let (next, _) = next(root, store, products, &loaded.sources, &live, &steps, &plan)?;
    // An apply also removes what it finds under the prefixes that it makes live, and the retired ones.
    if let (Some(listed), Some(remote)) = (listed.as_mut(), remote) {
        let owned = live.prefixes();
        for prefix in next.swept().into_iter().filter(|prefix| !owned.contains(prefix)) {
            listed.extend(remote.list(&prefix).map_err(|e| Code::R2Failed.error(e))?);
        }
    }
    (plan.remove, plan.listed) = (live.removed(&next, listed.as_deref()), listed.is_some());
    Ok(Planned { loaded, steps, plan, live: Some(live) })
}

fn env_plan(
    env: &Env,
    only: &[String],
    plan: Plan,
    blocked: Vec<BlockedProduct>,
    live: Option<(&Live, Vec<Edit>)>,
) -> EnvPlan {
    let moves = env.moves.iter().filter_map(|(source, version)| Some((source.clone(), version.clone()?)));
    let read = env.read.borrow();
    let versions = read.iter().map(|((source, params), version)| FetchVersion {
        source: source.clone(),
        params: params.clone(),
        version: version.clone(),
    });
    let (releases, edits) = match live {
        None => (Vec::new(), Vec::new()),
        Some((live, edits)) => {
            let releases = live.products.iter().map(|product| LiveRelease {
                product: product.product.clone(),
                release: product.release.as_ref().map(|(id, _)| id.clone()),
            });
            (releases.collect(), edits)
        }
    };
    EnvPlan {
        env: env.name.clone(),
        region: env.region.clone(),
        layers: env.layers.clone(),
        moves: moves.collect(),
        versions: versions.collect(),
        live: releases,
        edits,
        only: only.to_vec(),
        groups: plan.groups,
        blocked,
        remove: Vec::new(),
        listed: false,
    }
}

/// The optional layers of `product` that `layers` of an environment switch on, sorted.
fn optional(product: &dyn Product, layers: &[String]) -> Vec<String> {
    let on = product.optional().iter().filter(|layer| layers.iter().any(|name| name == *layer));
    let mut on: Vec<String> = on.map(|layer| layer.to_string()).collect();
    on.sort();
    on
}

/// What `env` changes against the live release of each product that is not blocked.
fn edits(products: &[&dyn Product], env: &Env, live: &Live, blocked: &[BlockedProduct]) -> Vec<Edit> {
    let mut edits = Vec::new();
    for (product, live) in products.iter().zip(&live.products) {
        if blocked.iter().any(|b| b.product == product.name()) {
            continue;
        }
        let (name, to) = (product.name().to_string(), env.region.clone());
        let Some((_, release)) = &live.release else {
            edits.push(Edit::Region { product: name, from: None, to });
            continue;
        };
        if release.region != env.region {
            edits.push(Edit::Region { product: name.clone(), from: Some(release.region.clone()), to });
        }
        let now = optional(*product, &env.layers);
        if release.optional != now {
            let on = now.iter().filter(|layer| !release.optional.contains(layer)).cloned().collect();
            let off = release.optional.iter().filter(|layer| !now.contains(layer)).cloned().collect();
            edits.push(Edit::Layers { product: name, on, off });
        }
    }
    edits
}

/// What the steps of `env` are compared with: the live layers of the products that are not
/// blocked, the edits, the moves and the drift.
fn against<'a>(
    live: &'a Live,
    env: &Env,
    edits: &[Edit],
    blocked: &[BlockedProduct],
    check: Option<&Check>,
) -> Against<'a> {
    let planned = live.products.iter().filter(|product| !blocked.iter().any(|b| b.product == product.product));
    let releases = planned.filter_map(|product| product.release.as_ref().map(|(_, release)| release));
    let by_source = live.by_source();
    let moves = env.moves.iter().filter_map(|(source, version)| {
        let from = by_source.get(source).cloned().unwrap_or_default();
        Some((source.clone(), (from, version.clone()?)))
    });
    let drift = check.map(|check| {
        let keys: Vec<String> = check.drift.iter().map(|drift| drift.key.clone()).collect();
        let owners = live.owners(&keys);
        (keys, owners)
    });
    Against {
        layers: releases.flat_map(|release| &release.layers).map(|layer| (layer.step.as_str(), layer)).collect(),
        region: edits.iter().filter(|edit| matches!(edit, Edit::Region { .. })).map(|e| e.product().into()).collect(),
        optional: edits.iter().filter(|edit| matches!(edit, Edit::Layers { .. })).map(|e| e.product().into()).collect(),
        moves: moves.collect(),
        drift,
    }
}

impl Edit {
    fn product(&self) -> &str {
        match self {
            Edit::Region { product, .. } | Edit::Layers { product, .. } => product,
        }
    }

    fn text(&self) -> String {
        match self {
            Edit::Region { product, from, to } => format!("{product} {} → {to}", from.as_deref().unwrap_or("—")),
            Edit::Layers { product, on, off } => {
                let on = on.iter().map(|layer| format!("+{layer}"));
                format!(
                    "{product} {}",
                    on.chain(off.iter().map(|layer| format!("-{layer}"))).collect::<Vec<_>>().join(" ")
                )
            }
        }
    }
}

/// Live after an apply of `plan`, as far as the store knows it: the release of each product that
/// the plan changes, of its live layers and the layers of the groups that the store has; and the
/// input copies that the releases and the layers to build read. Also the layers of the groups that
/// the store lacks.
fn next(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    sources: &[Source],
    live: &Live,
    steps: &[Step],
    plan: &EnvPlan,
) -> Result<(Live, Vec<String>), Error> {
    let taken: BTreeSet<&str> = plan.groups.iter().flat_map(|group| &group.layers).map(|l| l.step.as_str()).collect();
    let stored = release::stored(store, root, steps, &taken)?;
    let missing: Vec<String> =
        taken.into_iter().filter(|name| !stored.contains_key(*name)).map(str::to_string).collect();
    let mut next = Live::default();
    for (product, now) in products.iter().zip(&live.products) {
        let name = product.name();
        let mine = |layer: &&str| layer.split('/').next() == Some(name);
        let drops = plan.groups.iter().flat_map(|group| &group.drops);
        // A layer that the store lacks goes with its live layer: its new objects are not known.
        let dropped: BTreeSet<String> =
            drops.chain(&missing).map(String::as_str).filter(mine).map(str::to_string).collect();
        let new: Vec<_> = stored.values().filter(|layer| mine(&layer.step.as_str())).cloned().collect();
        let edited = plan.edits.iter().any(|edit| edit.product() == name);
        let release = if new.is_empty() && dropped.is_empty() && !edited {
            now.release.clone()
        } else {
            let live = now.release.as_ref().map(|(_, release)| release);
            let release = Release::compose(name, &plan.region, &optional(*product, &plan.layers), live, new, &dropped);
            Some((release.id(), release))
        };
        next.products.push(LiveProduct { product: name.into(), prefix: now.prefix.clone(), release });
    }
    let unbuilt = steps.iter().filter(|step| missing.contains(&step.name)).flat_map(|step| &step.inputs);
    let mut reads = next.snapshots();
    reads.extend(unbuilt.filter_map(|input| match input {
        Input::Snapshot { source, version, .. } => Some((source.clone(), version.clone())),
        Input::Layer { .. } => None,
    }));
    // A record that R2 holds stays. A read of a source with `r2_copy` whose record R2 lacks, such
    // as a moved version, has the record of the store.
    for (source, version) in reads {
        let record = match live.inputs.get(&(source.clone(), version.clone())) {
            Some(Some(record)) => Some(record.clone()),
            _ if sources.iter().any(|s| s.id == source && s.r2_copy) => store.snapshot(&source, &version)?,
            _ => continue,
        };
        next.inputs.insert((source, version), record);
    }
    Ok((next, missing))
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

/// The steps of every product, and the products that give `Unplanned::Invalid`; against `live`,
/// also the products without a pointer, whose release an apply cannot make live. A product whose
/// step list reads snapshots that the store lacks gets them fetched, as [`product_steps`] says. The
/// fetch for a `--move SOURCE` names its version in `env`, so every product reads that one version.
pub(super) fn steps(
    products: &[&dyn Product],
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    live: bool,
    mut fetch: impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<(Vec<Step>, Vec<BlockedProduct>), Error> {
    check_layers(products, env)?;
    let (mut all, mut blocked) = (Vec::new(), Vec::new());
    for product in products {
        if live && product.pointer().is_none() {
            let reason = "no client document yet, so an apply cannot make its release live".into();
            blocked.push(BlockedProduct { product: product.name().into(), reason });
            continue;
        }
        match product_steps(*product, env, regions, store, &mut fetch)? {
            Ok(steps) => all.extend(steps),
            Err(reason) => blocked.push(BlockedProduct { product: product.name().into(), reason }),
        }
    }
    let read = env.read.borrow();
    let unread = |moved: &&String| !env.stale.contains(*moved) && !read.keys().any(|(source, _)| source == *moved);
    if let Some(source) = env.moves.keys().find(unread) {
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
pub(crate) mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::engine::snapshot_files;
    use crate::engine::tests::{fixture, pipeline, write, Fixture, JOIN};

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

    /// The plan of `live` without live, as `plan live --json` writes it.
    fn plan_of(root: &Path, store: &Store, products: &[&dyn Product]) -> EnvPlan {
        planned(root, store, &Http::new(), None, products, "live", &[], Basis::Moves(&[])).unwrap().plan
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
        let (listed, _) = steps(&[&Indexed], &mut env(&["extra"]), &regions, &fixture.store, false, fetch).unwrap();
        assert_eq!((listed.len(), fetches.get()), (3, 1));
        let (listed, _) =
            steps(&[&Indexed], &mut env(&[]), &regions, &fixture.store, false, |_| unreachable!()).unwrap();
        assert_eq!(listed.len(), 3, "the store has it now");

        let err = steps(&[&Indexed], &mut env(&[]), &regions, &empty.store, false, |_| Ok("1".into())).err().unwrap();
        assert_eq!(err.message, "product `test` still needs index@1 after the fetch");
        assert!(err.fix.contains("product `test`"), "a product bug points at its code: {}", err.fix);
        let blocked = |_: &Wanted| Err(Code::Blocked.error("credential missing"));
        let err = steps(&[&Indexed], &mut env(&[]), &regions, &empty.store, false, blocked).err().unwrap();
        assert_eq!(err.code, Code::Blocked, "a failed fetch keeps its code");
        let err =
            steps(&[&Indexed], &mut env(&["snow"]), &regions, &fixture.store, false, |_| Ok("1".into())).err().unwrap();
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
        let (listed, _) = steps(&[&Chained], &mut env(&[]), &regions, &fixture.store, false, fetch).unwrap();
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
        let err = steps(&[&Outlined], &mut live, &regions, &fixture.store, false, fetch).err().unwrap();
        assert_eq!(
            (err.code, err.fix.as_str()),
            (Code::Blocked, "Plan with `--move land@VERSION`."),
            "{}",
            err.message
        );
        assert!(log.lock().unwrap().is_empty(), "a manual source does not move by itself");

        live.moves.insert("land".into(), None);
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&manual), &live);
        steps(&[&Outlined], &mut live, &regions, &fixture.store, false, fetch).unwrap();
        assert_eq!(
            live.moves["land"].as_deref(),
            Some("2026-10-05"),
            "the fetch names the move: the Last-Modified day"
        );
        let requests = log.lock().unwrap().len();

        let land = Source { refresh: Refresh::Days(30), ..manual };
        let mut live = env(&[]);
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&land), &live);
        steps(&[&Outlined], &mut live, &regions, &fixture.store, false, fetch).unwrap();
        assert_eq!(log.lock().unwrap().len(), requests, "without a move or live, the store serves");

        live.moves.insert("qrank".into(), None);
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&land), &live);
        let err = steps(&[&Outlined], &mut live, &regions, &fixture.store, false, fetch).err().unwrap();
        assert_eq!(
            (err.code, err.message.as_str()),
            (Code::Usage, "--move qrank: no step list of `live` reads `qrank`")
        );
        let mut fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&land), &live);
        let err = fetch(&Wanted { source: "land".into(), version: None, params: Vec::new() }).unwrap_err();
        assert!(err.message.contains("fetched per `area=`"), "{}", err.message);
    }

    /// The test pipeline, with `head` at the version that `product::version` gives. Its pointer
    /// document is `{"schema": 1}`, and its release has a `LICENSE.txt`.
    pub(crate) struct Versioned;

    impl Product for Versioned {
        fn name(&self) -> &'static str {
            "test"
        }

        fn pointer(&self) -> Option<crate::product::PointerFn> {
            Some(|_, _| {
                let document = [("schema".to_string(), 1.into())].into_iter().collect();
                Ok(crate::product::Pointer { document, named: [("LICENSE.txt".into(), b"CC0-1.0\n".to_vec())].into() })
            })
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
        let err =
            steps(&[&Versioned], &mut live.clone(), &regions, &fixture.store, false, |_| unreachable!()).err().unwrap();
        assert_eq!(
            (err.code, err.fix.as_str()),
            (Code::Blocked, "Plan with `--move head@VERSION`."),
            "{}",
            err.message
        );

        live.moves.insert("head".into(), Some("1".into()));
        let (listed, _) = steps(&[&Versioned], &mut live, &regions, &fixture.store, false, |_| unreachable!()).unwrap();
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
            run_build(&root, &fixture.store, &http, None, &[&Versioned], &args).unwrap().releases
        };
        let first = build(None)[0].id.clone();
        let saved = plan_of(&root, &fixture.store, &[&Versioned]);
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
        let saved = plan_of(&root, &fixture.store, &[&Indexed]);
        let file = root.join("plan.json");
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: Some(file.clone()), moves: Vec::new() };
        let build = || run_build(&root, &fixture.store, &http, None, &[&Indexed], &args);
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
        let plan = plan_of(&root, &fixture.store, &products);
        let blocked = plan.blocked;
        assert_eq!(blocked, [BlockedProduct { product: "other".into(), reason: "no box region".into() }]);
        assert_eq!(plan.groups.len(), 1, "the test product plans");

        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() };
        let built = run_build(&root, &fixture.store, &http, None, &products, &args).unwrap();
        assert_eq!((built.layers.len(), built.releases.len(), &built.blocked), (3, 1, &blocked));
        assert_eq!(built.releases[0].product, "test", "a blocked product has no release");
        let again = run_build(&root, &fixture.store, &http, None, &products, &args).unwrap();
        assert_eq!((again.run, again.releases[0].id == built.releases[0].id), (None, true), "up to date");
        let err = run_build(&root, &fixture.store, &http, None, &[&Refused], &args).unwrap_err();
        assert_eq!((err.code.exit(), err.message.as_str()), (4, "product `other`: no box region"), "no product suits");
    }

    pub(crate) const SOURCES: &str = r#"
        [[source]]
        id = "head"
        kind = "data"
        licence = "CC0-1.0"
        fetch = { kind = "http", url = "https://example.org/head.txt" }
        version = "date"
        refresh = 7
        redistribute = true
        r2_copy = true

        [[source]]
        id = "tail"
        kind = "data"
        licence = "CC0-1.0"
        fetch = { kind = "http", url = "https://example.org/tail.txt" }
        version = "date"
        refresh = "manual"
        redistribute = true
    "#;

    /// What upstream has of `source`, as a check of now.
    pub(crate) fn upstream(fixture: &Fixture, source: &str, version: &str) {
        let check = format!("{{\"checked\": {}, \"version\": \"{version}\"}}", crate::date::now());
        write(&fixture.store.root().join(format!("upstream/{source}.json")), &check);
    }

    /// Make `release` live in the local bucket, with the input copy of `head@2020-01-01`.
    fn publish(fixture: &Fixture, release: &Release) {
        let (bucket, id) = (fixture.scratch.0.join("bucket"), release.id());
        write(&bucket.join("test/catalog.json"), &format!("{{\"release\": \"{id}\"}}"));
        write(&bucket.join(format!("test/releases/{id}.json")), &String::from_utf8(release.canonical()).unwrap());
        let head = fixture.store.snapshot("head", "2020-01-01").unwrap().unwrap();
        write(&bucket.join("inputs/records/head/2020-01-01.json"), &serde_json::to_string(&head).unwrap());
        let layers = release.objects().into_keys().map(|sha256| ("test", sha256));
        for (prefix, sha256) in layers.chain(head.files.iter().map(|file| ("inputs", file.sha256.as_str()))) {
            let to = bucket.join(format!("{prefix}/objects/{sha256}"));
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            if !to.exists() {
                std::fs::copy(fixture.store.object(sha256), to).unwrap();
            }
        }
    }

    /// A repository whose live release is the test pipeline with `head@2020-01-01`, built from
    /// nothing live and published in a local bucket.
    fn live(name: &str) -> (Fixture, Remote, Release) {
        let fixture = fixture(name);
        let root = fixture.root();
        write(&root.join("data/sources.toml"), SOURCES);
        write(&root.join("data/regions/monaco.toml"), "name = \"Monaco\"\nkind = \"geofabrik\"\n");
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        fixture.fetched_version("head", "2020-01-01", "head.txt", b"head\n");
        std::fs::create_dir_all(fixture.scratch.0.join("bucket")).unwrap();
        let remote = Remote::Bucket(crate::r2::Bucket::local(&fixture.scratch.0.join("bucket")));
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() };
        let built = run_build(&root, &fixture.store, &Http::new(), Some(&remote), &[&Versioned], &args).unwrap();
        let release = Release::read(&fixture.store, "test", &built.releases[0].id).unwrap();
        assert_eq!((release.region.as_str(), release.layers.len()), ("monaco", 3), "the first release has every layer");
        publish(&fixture, &release);
        upstream(&fixture, "head", "2020-01-01");
        upstream(&fixture, "tail", "1");
        (fixture, remote, release)
    }

    fn live_plan(fixture: &Fixture, remote: &Remote, only: &[&str]) -> Result<EnvPlan, Error> {
        let only: Vec<String> = only.iter().map(|id| id.to_string()).collect();
        let (root, http, moves) = (fixture.root(), Http::new(), Basis::Moves(&[]));
        Ok(planned(&root, &fixture.store, &http, Some(remote), &[&Versioned], "live", &only, moves)?.plan)
    }

    #[test]
    fn a_plan_of_live_has_one_group_per_cause_and_the_keys_that_an_apply_removes() {
        let (fixture, remote, release) = live("cli-live-plan");
        let plan = live_plan(&fixture, &remote, &[]).unwrap();
        assert_eq!(plan.live, [LiveRelease { product: "test".into(), release: Some(release.id()) }]);
        assert_eq!((plan.groups.len(), plan.edits.len(), plan.remove.len(), plan.listed), (0, 0, 0, true));

        // Head is stale, and its new version keeps the file of the old one beside a new file. The
        // code of join changed, R2 lost the object of count and has an old one.
        upstream(&fixture, "head", "2020-02-01");
        fixture.fetched_version("head", "2020-02-01", "head.txt", b"newer\n");
        fixture.fetched_version("head", "2020-02-01", "kept.txt", b"head\n");
        write(&fixture.root().join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        let bucket = fixture.scratch.0.join("bucket");
        let object = |layer: usize| format!("test/objects/{}", release.layers[layer].files[0].sha256);
        let (count, join, upper) = (object(0), object(1), object(2));
        std::fs::remove_file(bucket.join(&count)).unwrap();
        write(&bucket.join("test/objects/old"), "old");

        let plan = live_plan(&fixture, &remote, &[]).unwrap();
        let ids: Vec<&str> = plan.groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, ["move:head", "code:test/join", "repair"]);
        let (moved, code, repair) = (&plan.groups[0], &plan.groups[1], &plan.groups[2]);
        let to = "2020-02-01".to_string();
        let cause = Cause::Move { source: "head".into(), from: vec!["2020-01-01".into()], to: to.clone() };
        assert_eq!((moved.cause.as_ref(), &plan.moves), (Some(&cause), &BTreeMap::from([("head".into(), to)])));
        let steps = |group: &Group| group.layers.iter().map(|layer| layer.step.clone()).collect::<Vec<_>>();
        assert_eq!((steps(moved).join(" "), moved.builds.len()), ("test/upper test/join test/count".into(), 3));
        assert_eq!(steps(code), ["test/join", "test/count"]);
        assert_eq!(repair.cause, Some(Cause::Repair { keys: vec![count.clone()] }));
        assert!(repair.builds.is_empty(), "the groups make count again");
        let manifest = format!("test/releases/{}.json", release.id());
        let records = "inputs/records/head/2020-01-01.json".to_string();
        let mut every = vec![records, join, upper, "test/objects/old".into(), manifest];
        every.sort();
        let removed = |plan: &EnvPlan| plan.remove.iter().map(|removal| removal.key.clone()).collect::<Vec<_>>();
        assert_eq!(removed(&plan), every, "not count, which R2 lacks, nor the object that both heads have");

        let err = live_plan(&fixture, &remote, &["code:test/join"]).unwrap_err();
        assert_eq!(err.code, Code::Usage, "against live, --only selects moves only: {}", err.message);
        let plan = live_plan(&fixture, &remote, &["move:head"]).unwrap();
        assert_eq!((plan.groups.len(), plan.moves.len()), (3, 1), "every group, and the move");

        write(&fixture.root().join("data/regions/andorra.toml"), "name = \"Andorra\"\nkind = \"geofabrik\"\n");
        write(&fixture.root().join("data/env/live.toml"), "region = \"andorra\"\n");
        let edit = Edit::Region { product: "test".into(), from: Some("monaco".into()), to: "andorra".into() };
        assert_eq!(live_plan(&fixture, &remote, &[]).unwrap().edits, [edit], "no layer reads the region");
    }

    #[test]
    fn a_build_of_a_plan_of_live_composes_the_release_and_refuses_the_plan_once_live_changed() {
        let (fixture, remote, release) = live("cli-live-build");
        let root = fixture.root();
        write(&root.join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        let file = root.join("plan.json");
        write(&file, &serde_json::to_string(&live_plan(&fixture, &remote, &[]).unwrap()).unwrap());
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: Some(file), moves: Vec::new() };
        let build = || run_build(&root, &fixture.store, &Http::new(), Some(&remote), &[&Versioned], &args);

        let built = build().unwrap();
        let steps: Vec<&str> = built.layers.iter().map(|layer| layer.step.as_str()).collect();
        assert_eq!(steps, ["test/join", "test/count"]);
        let next = Release::read(&fixture.store, "test", &built.releases[0].id).unwrap();
        assert_eq!(next.layers[2], release.layers[2], "upper stays the live layer");
        assert_ne!(next.layers[1], release.layers[1]);
        let again = build().unwrap();
        assert_eq!((again.run, &again.releases[0].id), (None, &built.releases[0].id), "a retry of the same plan");

        publish(&fixture, &next);
        let err = build().unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::PlanOutdated, 3), "{}", err.message);
    }
}
