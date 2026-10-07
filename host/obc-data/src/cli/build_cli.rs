//! `obc data plan ENV` and `obc data build ENV`: the steps of every product for an environment,
//! what a build would fetch and build, and the build into the store. Nothing uploads. A plan of
//! `live` compares the steps with the live releases, and has one group per cause.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::runs_cli::{bytes, duration};
use super::{cells, fetched, print_json, print_table, registry, Code, Error};
use crate::engine::changes::{self, Against};
use crate::engine::plan::{self, Cause, Estimate, Group, Plan};
use crate::engine::release::{self, Release};
use crate::engine::runs::{Context, Event, Limits, Phase, Run};
#[cfg(test)]
use crate::engine::Input;
use crate::engine::Step;
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
    pub(super) env: String,
    /// Only these groups, by id, or `none` for no group. Against `live`, only these moves.
    #[arg(long, value_delimiter = ',')]
    pub(super) only: Vec<String>,
    /// Read this version of a source instead of the version of live; without a version, the newest
    /// version upstream. A `manual` source moves only this way.
    #[arg(long = "move", value_name = "SOURCE[@VERSION]")]
    pub(super) moves: Vec<String>,
}

#[derive(Args)]
pub struct BuildArgs {
    /// The environment: `data/env/ENV.toml`.
    pub(super) env: String,
    /// Only these groups, by id, or `none` for no group. Against `live`, only these moves.
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvPlan {
    pub env: String,
    pub region: String,
    pub layers: Vec<String>,
    /// Source move intent: an explicit version, or each request's newest version. Exact resolved
    /// versions are in `versions`; fetching never changes this intent.
    pub moves: BTreeMap<String, Option<String>>,
    /// The version of each fetch that the step lists read. `build --plan` reads exactly these.
    pub versions: Vec<FetchVersion>,
    /// For `live`: the release that each product has live now. Empty for another environment.
    pub live: Vec<LiveRelease>,
    /// For `live`: what the environment file changes against the live releases.
    pub edits: Vec<Edit>,
    /// The groups that `--only` selected: none for every group, `["none"]` for no group.
    pub only: Vec<String>,
    pub groups: Vec<Group>,
    /// Incomplete products, with unavailable layers, or an empty layer list when the whole product is blocked.
    pub blocked: Vec<BlockedProduct>,
    /// For `live`: the keys that an apply of the plan removes from R2.
    pub remove: Vec<Removal>,
    /// For `live`: whether the plan listed R2, which needs the bucket. Without a listing, the plan
    /// has no `repair` group, and `remove` lacks the leftovers and the files that a client finds
    /// by name.
    pub listed: bool,
    /// Discovery could not resolve the step graph. Prepare and review a new plan before replay.
    pub needs_prepare: bool,
    /// Complete manual publication reviews this owner record separately from source refreshes.
    pub approval: Option<crate::approval::Review>,
}

/// Explicit preparation resolves inputs. Save `plan` after reviewing it, not this envelope.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Prepared {
    pub run: String,
    pub plan: EnvPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockedProduct {
    pub product: String,
    pub reason: String,
    pub layers: Vec<crate::product::BlockedLayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchVersion {
    /// Local product whose saved release supplies this pin; absent for the shared Live plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
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
    /// SHA-256 of the actual client document, excluding only `release` and `applied`.
    pub pointer: Option<String>,
    /// SHA-256 of the exact pointer bytes that consent observed; None means absent.
    pub observed: Option<String>,
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
    /// The operation journal, including preparation and release creation.
    pub run: String,
    /// The layers of the run, in dependency order.
    pub layers: Vec<BuiltLayer>,
    /// The release of each product whose every layer is built. For `live`, the release of each
    /// product that the groups change: its live layers with the layers of the groups.
    pub releases: Vec<BuiltRelease>,
    /// Incomplete products. Usable layers can build, but these products get no complete release.
    pub blocked: Vec<BlockedProduct>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BuiltLayer {
    pub step: String,
    pub key: String,
    pub reused: bool,
}

#[derive(Debug, serde::Deserialize, Serialize, JsonSchema)]
pub struct BuiltRelease {
    pub product: String,
    /// The SHA-256 of `releases/<product>/<id>.json` in the store.
    pub id: String,
}

pub fn plan(root: &Path, products: &[&dyn Product], args: PlanArgs, json: bool) -> Result<(), Error> {
    let (store, http) = (Store::open()?, Http::new());
    let remote = live_remote(&args.env)?;
    let basis = Basis::Moves(&args.moves);
    let plan = planned(root, &store, &http, remote.as_ref(), products, &args.env, &args.only, basis, false)?.plan;
    if json {
        return print_json(&plan);
    }
    print_plan(&plan);
    Ok(())
}

pub fn prepare(root: &Path, products: &[&dyn Product], args: PlanArgs, json: bool) -> Result<(), Error> {
    let (store, http, remote) = (Store::open()?, Http::new(), live_remote(&args.env)?);
    let mut run = super::api::start_run(&store, &format!("prepare {}", args.env))?;
    let result = prepare_plan(root, &store, &http, remote.as_ref(), products, &args, &mut run);
    let prepared = super::api::finish_run(run, result, None)?;
    if json {
        return print_json(&prepared);
    }
    println!("run {}", prepared.run);
    print_plan(&prepared.plan);
    Ok(())
}

fn prepare_plan(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: Option<&Remote>,
    products: &[&dyn Product],
    args: &PlanArgs,
    run: &mut Run,
) -> Result<Prepared, Error> {
    run.record(&Event::Phase { phase: Phase::Prepare })?;
    let prepared = planned_run(
        root,
        store,
        http,
        remote,
        products,
        &args.env,
        &args.only,
        Basis::Moves(&args.moves),
        true,
        Some(run),
    )?;
    if let Some((wanted, message)) = prepared.loaded.env.fetch_failures.first() {
        let source = prepared
            .loaded
            .sources
            .iter()
            .find(|source| source.id == wanted.source)
            .ok_or_else(|| Code::InvalidData.error(format!("no source `{}`", wanted.source)))?;
        return Err(fetched(source, Err(format!("source `{}`: {message}", wanted.source))).unwrap_err());
    }
    Ok(Prepared { run: run.id().into(), plan: prepared.plan })
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
    if let Some(review) = &plan.approval {
        println!("{}", review.summary());
    }
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
        Some(Cause::Pointer { product, .. }) => format!("client document of {product}"),
        Some(Cause::Repair { keys }) => format!("{} keys that R2 lacks", keys.len()),
    }
}

pub fn build(root: &Path, products: &[&dyn Product], args: BuildArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let remote = live_remote(&args.env)?;
    let built = run_build(root, &store, &Http::new(), remote.as_ref(), products, &args)?;
    let blocked = built.blocked.iter().filter(|blocked| !blocked.layers.is_empty()).collect::<Vec<_>>();
    let incomplete = || {
        let reasons = blocked.iter().map(|b| format!("{}: {}", b.product, b.reason)).collect::<Vec<_>>().join("; ");
        let mut error = Code::Blocked.error(format!("build is incomplete; required layers are blocked: {reasons}"));
        error.run = Some(built.run.clone());
        error
    };
    if json && !blocked.is_empty() {
        return Err(incomplete());
    }
    if json {
        print_json(&built)?;
    } else {
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
    }
    if !blocked.is_empty() {
        return Err(incomplete());
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
    complete(saved.as_ref())?;
    let mut run = super::api::start_run(store, &format!("build {}", args.env))?;
    let result = build_env(root, store, http, remote, products, args, saved.as_ref(), &mut run).map(|built| built.0);
    let incomplete = result
        .as_ref()
        .ok()
        .filter(|built| built.blocked.iter().any(|blocked| !blocked.layers.is_empty()))
        .map(|_| "required layers are blocked");
    super::api::finish_run(run, result, incomplete)
}

pub(super) fn complete(saved: Option<&EnvPlan>) -> Result<(), Error> {
    if saved.is_some_and(|plan| plan.needs_prepare) {
        return Err(Code::PlanOutdated
            .error("the preview has unresolved inputs; nothing fetched or built")
            .fix("Run `obc data prepare ENV --json`, review its plan, and save only the .plan object."));
    }
    Ok(())
}

/// The complete desired publication after the build.
pub(super) struct Applying {
    pub(super) next: Live,
    pub(super) plan: EnvPlan,
}

/// Recheck the approval after verification, including code for reused layers and unused acquisition.
pub(super) fn recheck_approval(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: &Remote,
    products: &[&dyn Product],
    saved: &EnvPlan,
    run: &mut Run,
) -> Result<(), Error> {
    let current = planned_run(
        root,
        store,
        http,
        Some(remote),
        products,
        &saved.env,
        &saved.only,
        Basis::Saved(saved),
        false,
        Some(run),
    )?
    .plan;
    if saved.approval != current.approval || saved.live != current.live {
        return Err(outdated());
    }
    Ok(())
}

/// Build `args.env`, or the plan `saved`. A build of live also gives what an apply of it changes.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_env(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: Option<&Remote>,
    products: &[&dyn Product],
    args: &BuildArgs,
    saved: Option<&EnvPlan>,
    run: &mut Run,
) -> Result<(Built, Option<Applying>), Error> {
    complete(saved)?;
    run.record(&Event::Phase { phase: Phase::Prepare })?;
    let (basis, only) = match saved {
        Some(saved) => (Basis::Saved(saved), &saved.only),
        None => (Basis::Moves(&args.moves), &args.only),
    };
    let Planned { loaded, steps, plan, live } =
        planned_run(root, store, http, remote, products, &args.env, only, basis, true, Some(run))?;
    if let Some(saved) = saved {
        let unchanged =
            (&saved.env, &saved.region, &saved.layers, &saved.blocked, &saved.live, &saved.edits, &saved.approval)
                == (&plan.env, &plan.region, &plan.layers, &plan.blocked, &plan.live, &plan.edits, &plan.approval);
        if !unchanged || !(Plan { groups: plan.groups.clone() }).same_work(&Plan { groups: saved.groups.clone() }) {
            return Err(outdated());
        }
    }
    suits(products, &plan)?;
    let mut built =
        Built { run: run.id().into(), layers: Vec::new(), releases: Vec::new(), blocked: plan.blocked.clone() };
    let work = Plan { groups: plan.groups.clone() };
    run.record(&Event::Phase { phase: Phase::Build })?;
    if work.builds().next().is_some() || !work.fetches().is_empty() {
        let id = run.id().to_string();
        let copies = remote.zip(live.as_ref()).map(|(remote, live)| crate::input_copy::Restore { remote, live });
        let context =
            Context { store, root, sources: &loaded.sources, http, copies: copies.as_ref(), limits: Limits::machine() };
        let result = run.build(&context, &steps, &work);
        let layers = result.map_err(|e| Code::RunFailed.error(format!("run {id}: {e}")))?;
        built.layers = layers
            .into_iter()
            .map(|layer| BuiltLayer { step: layer.receipt.step, key: layer.receipt.key, reused: layer.reused })
            .collect();
    }
    run.record(&Event::Phase { phase: Phase::Verify })?;
    let Some(live) = live else {
        crate::worker::check(root)?;
        let unblocked = products.iter().filter(|product| !plan.blocked.iter().any(|b| b.product == product.name()));
        for product in unblocked {
            let (name, optional) = (product.name(), optional(*product, &plan.layers));
            if let Some(mut release) = release::release(store, root, name, &plan.region, &optional, &steps)? {
                release.name_files(product.named(&release)?)?;
                built.releases.push(BuiltRelease { product: name.into(), id: release.write(store)? });
            }
        }
        return Ok((built, None));
    };
    let (mut next, missing) = next_reusing(root, store, products, &live, &steps, &plan, &run.originals)?;
    retain_input_copies(store, &loaded.sources, &live, &mut next)?;
    if let Some(layer) = missing.first() {
        return Err(Code::Failed.error(format!("the store lacks the layer `{layer}` after the build")));
    }
    for (next, live) in next.products.iter().zip(&live.products) {
        if let Some((id, release)) = next.release.as_ref().filter(|_| changed(live, next)) {
            release.write(store)?;
            built.releases.push(BuiltRelease { product: release.product.clone(), id: id.clone() });
        }
    }
    Ok((built, Some(Applying { next, plan })))
}

/// Fail with `blocked` when no product suits the environment of `plan`.
pub(super) fn suits(products: &[&dyn Product], plan: &EnvPlan) -> Result<(), Error> {
    if !products.is_empty() && plan.blocked.len() == products.len() && plan.groups.is_empty() {
        let reasons = plan.blocked.iter().map(|b| format!("product `{}`: {}", b.product, b.reason)).collect::<Vec<_>>();
        return Err(Code::Blocked.error(reasons.join("; ")));
    }
    Ok(())
}

/// Whether the release id or the actual client document differs.
pub(super) fn changed(live: &LiveProduct, next: &LiveProduct) -> bool {
    let id = |product: &LiveProduct| product.release.as_ref().map(|(id, _)| id.clone());
    id(live) != id(next) || live.document != next.document
}

fn document_digest(product: &LiveProduct) -> Option<String> {
    let document = product.document.as_ref()?;
    let value = crate::engine::sorted(serde_json::to_value(document).expect("JSON values serialize"));
    Some(crate::store::sha256_hex(&serde_json::to_vec(&value).expect("JSON values serialize")))
}

/// The plan of `live` now against the live releases that `remote` holds: `plan live --only ONLY
/// --json`, which the TUI shows and an apply without `--plan` applies.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_live(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: &Remote,
    products: &[&dyn Product],
    only: &[String],
    prepare: bool,
) -> Result<EnvPlan, Error> {
    plan_live_moves(root, store, http, remote, products, only, &[], prepare)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_live_moves(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: &Remote,
    products: &[&dyn Product],
    only: &[String],
    moves: &[String],
    prepare: bool,
) -> Result<EnvPlan, Error> {
    Ok(planned(root, store, http, Some(remote), products, "live", only, Basis::Moves(moves), prepare)?.plan)
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

/// `--only none`: no group, or against live no move.
pub(super) const NONE: &str = "none";

/// The groups that `only` names. Against live, `only` names moves, and the plan keeps every group:
/// a move that it does not name does not move, and the rest of live follows `data/` and the code.
fn select(plan: &Plan, only: &[String], live: bool) -> Result<Plan, Error> {
    if only.is_empty() {
        return Ok(plan.clone());
    }
    if only.iter().any(|id| id == NONE) {
        if only.iter().any(|id| id != NONE) {
            return Err(Code::Usage.error(format!("`--only {}`: `none` names no other group", only.join(","))));
        }
        return Ok(if live { plan.clone() } else { Plan { groups: Vec::new() } });
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
    prepare: bool,
) -> Result<Planned, Error> {
    planned_run(root, store, http, remote, products, name, only, basis, prepare, None)
}

#[allow(clippy::too_many_arguments)]
fn planned_run(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: Option<&Remote>,
    products: &[&dyn Product],
    name: &str,
    only: &[String],
    basis: Basis,
    prepare: bool,
    mut run: Option<&mut Run>,
) -> Result<Planned, Error> {
    let mut loaded = load(root, name)?;
    if let Some(run) = run.as_deref_mut().filter(|run| run.requires_committed_code()) {
        for product in products {
            if let Some(owner) = product.planning_code(&loaded.env).map_err(|error| Code::Blocked.error(error))? {
                run.check_owner(root, &owner).map_err(|error| Code::Usage.error(error))?;
            }
        }
    }
    let live = remote.map(|remote| Live::read(remote, products, &loaded.sources, store)).transpose();
    let live = live.map_err(|e| Code::R2Failed.error(e))?;
    if let (Some(live), Some(remote)) = (&live, remote) {
        live.restore_named(remote, store).map_err(|e| Code::VerifyFailed.error(e))?;
    }
    let env = &mut loaded.env;
    let copies = remote.zip(live.as_ref()).map(|(remote, live)| crate::input_copy::Restore { remote, live });
    env.retained = live.as_ref().map(|live| crate::input_copy::retained(live, store)).transpose()?.unwrap_or_default();
    if let (Some(approved), Some(remote), Some(live)) =
        (run.as_deref().and_then(|run| run.automatic.as_ref()), remote, live.as_ref())
    {
        let mut original = env.clone();
        original.live = live.versions();
        original.moves.clear();
        let fetch = |wanted: &Wanted| {
            Err(Code::Blocked
                .error(format!("automatic admission lacks retained planning inputs for `{}`", wanted.source,)))
        };
        let (current, blocked) = list_steps(root, products, &mut original, &loaded.regions, store, true, fetch, true)?;
        if !blocked.is_empty() {
            return Err(Code::Blocked
                .error("automatic admission needs the complete retained product declarations")
                .fix("Restore the retained planning inputs with a manual prepare, then retry."));
        }
        let review = crate::approval::review(
            root,
            &original,
            &loaded.regions,
            &current,
            products,
            &loaded.sources,
            remote.describe(),
            Ok(approved.observation.clone()),
        );
        approved.check(&review).map_err(|reason| Code::Blocked.error(reason))?;
    }
    match basis {
        Basis::Saved(saved) => {
            let versions = saved.versions.iter().map(|v| ((v.source.clone(), v.params.clone()), v.version.clone()));
            env.planned = Some(versions.collect());
            env.moves = saved.moves.clone();
        }
        Basis::Moves(args) => {
            env.moves = moves(&loaded.sources, args)?;
            if let Some(live) = &live {
                env.live = live.versions();
                let inventory = super::freshness::discover(
                    root,
                    products,
                    env,
                    &loaded.regions,
                    store,
                    http,
                    &loaded.sources,
                    copies.as_ref(),
                    run.as_deref_mut(),
                )?;
                for source in &loaded.sources {
                    if source.refresh == Refresh::Manual || env.moves.contains_key(&source.id) {
                        continue;
                    }
                    for request in super::freshness::requests(store, http, source, &inventory, false) {
                        if run.as_deref().is_some_and(|run| run.automatic.is_some())
                            && request.state == crate::sources::State::Blocked
                        {
                            return Err(Code::Blocked.error(format!(
                                "required refresh `{}`: {}",
                                source.id,
                                request.reason.as_deref().unwrap_or("upstream check failed")
                            )));
                        }
                        if request.state == crate::sources::State::Stale {
                            env.stale_requests.insert((source.id.clone(), request.params.clone()));
                            env.moves.insert(source.id.clone(), None);
                            env.stale.insert(source.id.clone());
                            if let Some(version) = request.observation.result.version() {
                                env.resolved.insert((source.id.clone(), request.params), version.into());
                            }
                        }
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
                    env.resolved.retain(|(source, _), _| env.moves.contains_key(source));
                    env.stale.retain(|source| env.moves.contains_key(source));
                    env.stale_requests.retain(|(source, _)| env.moves.contains_key(source));
                }
            }
        }
    }
    let fetch = super::status_cli::discovery_fetch(
        fetcher_recorded(root, store, http, &loaded.sources, env, copies.as_ref(), run.as_deref_mut()),
        prepare,
    );
    let (steps, blocked) = match basis {
        Basis::Moves(_) => list_steps(root, products, env, &loaded.regions, store, live.is_some(), fetch, !prepare)?,
        // A fetch without a version is one that the plan does not name.
        Basis::Saved(_) => {
            let mut fetch = fetch;
            steps(root, products, env, &loaded.regions, store, live.is_some(), |wanted| match &wanted.version {
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
        crate::worker::check(root)?;
        let plan = env_plan(env, only, select(&all, only, false)?, blocked, None);
        return Ok(Planned { loaded, steps, plan, live: None });
    };
    let mut originals = BTreeMap::new();
    if let Some(run) = run.filter(|run| run.automatic.is_some()) {
        let approved = run.automatic.as_ref().expect("filtered above");
        if !blocked.is_empty() {
            return Err(Code::Blocked.error("automatic product declarations are incomplete"));
        }
        let review = crate::approval::review(
            root,
            env,
            &loaded.regions,
            &steps,
            products,
            &loaded.sources,
            remote.expect("Live has a remote").describe(),
            Ok(approved.observation.clone()),
        );
        approved.check(&review).map_err(|reason| Code::Blocked.error(reason))?;
        for (product, published) in products.iter().zip(&live.products) {
            if let Some((_, release)) = &published.release {
                originals.extend(crate::local::reuse(
                    root,
                    store,
                    remote.expect("Live has a remote"),
                    *product,
                    release,
                    &steps,
                )?);
            }
        }
        run.originals = originals.clone();
    }
    let edits = edits(products, env, &live, &blocked);
    let mut listed = match remote {
        Some(remote @ Remote::Bucket(_)) => Some(live.list(remote).map_err(|e| Code::R2Failed.error(e))?),
        _ => None,
    };
    let check = listed.as_deref().map(|listed| live.check(listed));
    let against = against(&live, env, &edits, &blocked, check.as_ref(), store);
    let all = changes::changes_reusing(store, root, &steps, &against, &originals)?;
    let mut plan = env_plan(env, only, select(&all, only, true)?, blocked, Some((&live, edits)));
    let (mut next, _) = next_reusing(root, store, products, &live, &steps, &plan, &originals)?;
    retain_input_copies(store, &loaded.sources, &live, &mut next)?;
    for (now, next) in live.products.iter().zip(&next.products) {
        let layered = plan.groups.iter().any(|group| {
            group
                .layers
                .iter()
                .map(|layer| &layer.step)
                .chain(&group.drops)
                .any(|layer| layer.split('/').next() == Some(next.product.as_str()))
        });
        if !layered && next.document.is_some() && changed(now, next) {
            let document = document_digest(next).expect("a desired document exists");
            plan.groups.push(Group {
                id: format!("pointer:{}", next.product),
                cause: Some(Cause::Pointer {
                    product: next.product.clone(),
                    release: next.release.as_ref().expect("a desired document has a release").0.clone(),
                    document,
                }),
                layers: Vec::new(),
                drops: Vec::new(),
                fetches: Vec::new(),
                builds: Vec::new(),
            });
        }
    }
    // An apply also removes what it finds under the prefixes that it makes live, and the retired ones.
    if let (Some(listed), Some(remote)) = (listed.as_mut(), remote) {
        let owned = live.prefixes();
        for prefix in next.swept().into_iter().filter(|prefix| !owned.contains(prefix)) {
            listed.extend(remote.list(&prefix).map_err(|e| Code::R2Failed.error(e))?);
        }
    }
    (plan.remove, plan.listed) = (live.removed(&next, listed.as_deref()), listed.is_some());
    let prior = super::commit_cli::approval_observation(store);
    plan.approval = Some(if plan.needs_prepare || !plan.blocked.is_empty() {
        crate::approval::Review::Unavailable {
            owner: prior.ok().map(|prior| prior.owner),
            reason: "the complete product graph is not available for automatic approval".into(),
        }
    } else {
        crate::approval::review(
            root,
            &loaded.env,
            &loaded.regions,
            &steps,
            products,
            &loaded.sources,
            remote.expect("Live has a remote").describe(),
            prior,
        )
    });
    crate::worker::check(root)?;
    Ok(Planned { loaded, steps, plan, live: Some(live) })
}

pub(super) fn env_plan(
    env: &Env,
    only: &[String],
    plan: Plan,
    blocked: Vec<BlockedProduct>,
    live: Option<(&Live, Vec<Edit>)>,
) -> EnvPlan {
    let read = env.read.borrow();
    let versions = read.iter().map(|((source, params), version)| FetchVersion {
        product: None,
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
                pointer: document_digest(product),
                observed: product.observed.clone(),
            });
            (releases.collect(), edits)
        }
    };
    EnvPlan {
        env: env.name.clone(),
        region: env.region.clone(),
        layers: env.layers.clone(),
        moves: env.moves.clone(),
        versions: versions.collect(),
        live: releases,
        edits,
        only: only.to_vec(),
        groups: plan.groups,
        blocked,
        remove: Vec::new(),
        listed: false,
        needs_prepare: !env.fetch_failures.is_empty(),
        approval: None,
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
    store: &Store,
) -> Against<'a> {
    let planned = live.products.iter().filter(|product| !blocked.iter().any(|b| b.product == product.product));
    let releases = planned.filter_map(|product| product.release.as_ref().map(|(_, release)| release));
    let by_source = live.by_source();
    let moves = env.moves.iter().filter_map(|(source, version)| {
        let from = by_source.get(source).cloned().unwrap_or_default();
        let to = version.clone().or_else(|| {
            let read = env.read.borrow();
            let versions: BTreeSet<_> =
                read.iter().filter(|((id, _), _)| id == source).map(|(_, v)| v.as_str()).collect();
            (!versions.is_empty()).then(|| versions.into_iter().collect::<Vec<_>>().join(", "))
        })?;
        Some((source.clone(), (from, to)))
    });
    let drift = check.map(|check| {
        let keys: Vec<String> = check.drift.iter().map(|drift| drift.key.clone()).collect();
        let local_named: BTreeSet<String> = live
            .releases()
            .flat_map(|(prefix, id, release)| {
                release
                    .named
                    .iter()
                    .filter(|file| store.object(&file.sha256).is_file())
                    .map(move |file| format!("{prefix}/releases/{id}/{}", file.path))
            })
            .collect();
        let unavailable: Vec<_> = keys.iter().filter(|key| !local_named.contains(*key)).cloned().collect();
        let owners = live.owners(&unavailable);
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
/// layers of the groups that the store lacks.
fn next_reusing(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    live: &Live,
    steps: &[Step],
    plan: &EnvPlan,
    originals: &BTreeMap<String, release::Layer>,
) -> Result<(Live, Vec<String>), Error> {
    crate::worker::check(root)?;
    let taken: BTreeSet<&str> = plan.groups.iter().flat_map(|group| &group.layers).map(|l| l.step.as_str()).collect();
    let stored = release::stored_reusing(store, root, steps, &taken, originals)?;
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
        let release = if plan.blocked.iter().any(|b| b.product == name)
            || (new.is_empty() && dropped.is_empty() && !edited)
        {
            now.release.clone()
        } else {
            let live = now.release.as_ref().map(|(_, release)| release);
            let release = Release::compose(name, &plan.region, &optional(*product, &plan.layers), live, new, &dropped);
            Some((release.id(), release))
        };
        let blocked = plan.blocked.iter().any(|b| b.product == name);
        let release = release
            .map(|(_, mut release)| -> Result<_, Error> {
                if !blocked && !missing.iter().any(|layer| layer.split('/').next() == Some(name)) {
                    release.bind_producers(store)?;
                    release.name_files(product.named(&release)?)?;
                }
                Ok((release.id(), release))
            })
            .transpose()?;
        let document = match &release {
            Some((_, release))
                if !blocked
                    && release.named.iter().all(|file| store.object(&file.sha256).is_file())
                    && !missing.iter().any(|layer| layer.split('/').next() == Some(name)) =>
            {
                let pointer = product.pointer().expect("unblocked live product has a pointer");
                let mut pointer = pointer(root, release, store).map_err(|e| Code::VerifyFailed.error(e))?;
                if name == "planner" {
                    crate::vps::document(&mut pointer.document, now.document.as_ref())
                        .map_err(|e| Code::VerifyFailed.error(e))?;
                }
                if pointer.document.contains_key("release") || pointer.document.contains_key("applied") {
                    return Err(product_bug(name, "pointer includes publication fields".into()));
                }
                Some(pointer.document)
            }
            _ => now.document.clone(),
        };
        let prefix = now.prefix.clone();
        next.products.push(LiveProduct {
            product: name.into(),
            prefix,
            release,
            applied: None,
            observed: None,
            document,
        });
    }
    Ok((next, missing))
}

fn retain_input_copies(store: &Store, sources: &[Source], live: &Live, next: &mut Live) -> Result<(), Error> {
    for read in crate::input_copy::reads(next)? {
        if !live.inputs.contains_key(&read.key)
            && !sources.iter().any(|s| s.id == read.key.source && s.r2_copy && s.redistribute)
        {
            continue;
        }
        let record = match live.inputs.get(&read.key) {
            Some(Some(record)) => {
                record.check_local(store)?;
                Some(record.clone())
            }
            _ => crate::input_copy::Record::local(store, &read)?,
        };
        next.inputs.insert(read.key, record);
    }
    Ok(())
}

/// The `--move SOURCE[@VERSION]` of a plan or a build.
pub(super) fn moves(sources: &[Source], moves: &[String]) -> Result<BTreeMap<String, Option<String>>, Error> {
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
    root: &'a Path,
    store: &'a Store,
    http: &'a Http,
    sources: &'a [Source],
    env: &Env,
    copies: Option<&'a crate::input_copy::Restore<'a>>,
) -> impl FnMut(&Wanted) -> Result<String, Error> + 'a {
    fetcher_recorded(root, store, http, sources, env, copies, None)
}

pub(super) fn fetcher_recorded<'a>(
    root: &'a Path,
    store: &'a Store,
    http: &'a Http,
    sources: &'a [Source],
    env: &Env,
    copies: Option<&'a crate::input_copy::Restore<'a>>,
    mut run: Option<&'a mut Run>,
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
        let snapshot = match run.as_deref_mut() {
            Some(run) => run.fetch_request(root, store, http, copies, &request, &[]),
            None => crate::input_copy::fetch(root, store, http, copies, &request, &[]),
        };
        fetched(source, snapshot).map(|snapshot| snapshot.version).map_err(|e| {
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
#[allow(clippy::too_many_arguments)]
pub(super) fn steps(
    root: &Path,
    products: &[&dyn Product],
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    live: bool,
    fetch: impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<(Vec<Step>, Vec<BlockedProduct>), Error> {
    list_steps(root, products, env, regions, store, live, fetch, false)
}

#[allow(clippy::too_many_arguments)]
fn list_steps(
    root: &Path,
    products: &[&dyn Product],
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    live: bool,
    mut fetch: impl FnMut(&Wanted) -> Result<String, Error>,
    preview: bool,
) -> Result<(Vec<Step>, Vec<BlockedProduct>), Error> {
    check_layers(products, env)?;
    env.fetch_failures.clear();
    let (mut all, mut blocked) = (Vec::new(), Vec::new());
    for product in products {
        if live && product.pointer().is_none() {
            let reason = "no client document yet, so an apply cannot make its release live".into();
            blocked.push(BlockedProduct { product: product.name().into(), reason, layers: Vec::new() });
            continue;
        }
        let before = env.fetch_failures.len();
        let listed = match product_steps(root, *product, env, regions, store, &mut fetch) {
            Err(error)
                if preview
                    && env.fetch_failures.len() > before
                    && matches!(error.code, Code::Blocked | Code::FetchFailed) =>
            {
                blocked.push(BlockedProduct {
                    product: product.name().into(),
                    reason: format!("{}; {}", error.message, error.fix),
                    layers: Vec::new(),
                });
                continue;
            }
            result => result?,
        };
        match listed {
            Ok(listed) => {
                if !listed.blocked.is_empty() {
                    let reason = listed
                        .blocked
                        .iter()
                        .map(|layer| format!("{}: {}", layer.layer, layer.reason))
                        .collect::<Vec<_>>()
                        .join("; ");
                    blocked.push(BlockedProduct { product: product.name().into(), reason, layers: listed.blocked });
                }
                all.extend(listed.steps);
            }
            Err(reason) => blocked.push(BlockedProduct { product: product.name().into(), reason, layers: Vec::new() }),
        }
    }
    let read = env.read.borrow();
    let unread = |moved: &&String| {
        !env.stale.contains(*moved)
            && !read.keys().any(|(source, _)| source == *moved)
            && !env.fetch_failures.iter().any(|(wanted, _)| wanted.source == **moved)
    };
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
    root: &Path,
    product: &dyn Product,
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    fetch: &mut impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<Result<crate::product::Steps, String>, Error> {
    listed_steps(root, product, env, regions, store, fetch, false)
}

pub(super) fn status_steps(
    root: &Path,
    product: &dyn Product,
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    fetch: &mut impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<Result<crate::product::Steps, String>, Error> {
    listed_steps(root, product, env, regions, store, fetch, true)
}

#[allow(clippy::too_many_arguments)]
fn listed_steps(
    root: &Path,
    product: &dyn Product,
    env: &mut Env,
    regions: &Regions,
    store: &Store,
    fetch: &mut impl FnMut(&Wanted) -> Result<String, Error>,
    observation: bool,
) -> Result<Result<crate::product::Steps, String>, Error> {
    let declarations = |env: &Env| {
        if observation {
            product.status_steps(root, env, regions, store)
        } else {
            product.steps(root, env, regions, store)
        }
    };
    let name = product.name();
    let mut failure = None;
    // A refusal belongs to the product that is listed now.
    env.refused.borrow_mut().clear();
    crate::worker::check(root)?;
    let mut listed = declarations(env);
    // A fetch can name the next one, such as the `.poly` that gives the box of a capture.
    let mut fetched: Vec<Wanted> = Vec::new();
    for _ in 0..ROUNDS {
        let Err(Unplanned::NeedsFetch(fetches)) = &listed else { break };
        if fetches.iter().any(|wanted| fetched.contains(wanted)) {
            break;
        }
        for wanted in fetches {
            let version = match fetch(wanted) {
                Ok(version) => version,
                Err(error) if matches!(error.code, Code::FetchFailed | Code::Blocked) => {
                    env.fetch_failures.push((wanted.clone(), error.message.clone()));
                    failure = Some(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            env.resolved.insert((wanted.source.clone(), crate::store::sorted(&wanted.params)), version);
        }
        fetched.extend(fetches.iter().cloned());
        crate::worker::check(root)?;
        listed = declarations(env);
    }
    let steps = match listed {
        Ok(steps) => steps,
        Err(Unplanned::NeedsFetch(fetches)) => {
            if let Some(error) = failure {
                return Err(error);
            }
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
    if let Some(step) = steps.steps.iter().find(|step| !step.name.starts_with(&format!("{name}/"))) {
        return Err(product_bug(name, format!("step `{}` of product `{name}` is not named `{name}/…`", step.name)));
    }
    Ok(Ok(steps))
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::engine::Client;
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

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            store: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            match snapshot_files(store, "index", "1", &[], &[]).map_err(Unplanned::Failed)? {
                Some(_) => Ok(pipeline().into()),
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
        planned(root, store, &Http::new(), None, products, "live", &[], Basis::Moves(&[]), true).unwrap().plan
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
        let (listed, _) =
            steps(&fixture.root(), &[&Indexed], &mut env(&["extra"]), &regions, &fixture.store, false, fetch).unwrap();
        assert_eq!((listed.len(), fetches.get()), (3, 1));
        let (listed, _) =
            steps(&fixture.root(), &[&Indexed], &mut env(&[]), &regions, &fixture.store, false, |_| unreachable!())
                .unwrap();
        assert_eq!(listed.len(), 3, "the store has it now");

        let err = steps(&fixture.root(), &[&Indexed], &mut env(&[]), &regions, &empty.store, false, |_| Ok("1".into()))
            .err()
            .unwrap();
        assert_eq!(err.message, "product `test` still needs index@1 after the fetch");
        assert!(err.fix.contains("product `test`"), "a product bug points at its code: {}", err.fix);
        let blocked = |_: &Wanted| Err(Code::Blocked.error("credential missing"));
        let err =
            steps(&fixture.root(), &[&Indexed], &mut env(&[]), &regions, &empty.store, false, blocked).err().unwrap();
        assert_eq!(err.code, Code::Blocked, "a failed fetch keeps its code");
        let err = steps(&fixture.root(), &[&Indexed], &mut env(&["snow"]), &regions, &fixture.store, false, |_| {
            Ok("1".into())
        })
        .err()
        .unwrap();
        let message = "data/env/live.toml: no product has the optional layer `snow`";
        assert_eq!((err.code, err.message.as_str()), (Code::InvalidData, message));
    }

    #[test]
    fn an_explicit_move_preview_stays_read_only_until_preparation_resolves_its_graph() {
        use crate::fetch::tests::{serve, whole};
        struct MovedIndex;
        impl Product for MovedIndex {
            fn name(&self) -> &'static str {
                "test"
            }
            fn steps(
                &self,
                root: &Path,
                env: &Env,
                regions: &Regions,
                store: &Store,
            ) -> Result<crate::product::Steps, Unplanned> {
                assert_eq!(
                    crate::product::version(env, store, "index", &[]).map_err(Unplanned::Failed)?,
                    Ok("1".into())
                );
                Indexed.steps(root, env, regions, store)
            }
        }
        let (actual, requests) = serve(|_, _| whole(b"index\n"));
        let actual = actual.replacen("/data/", "/data/1/", 1);
        let url = actual.replacen("http://", "https://", 1).replacen("/data/1/", "/data/{version}/", 1);
        let fixture = fixture("cli-preview-prepare");
        fixture.with_acquisition();
        let root = fixture.root();
        write(&root.join("data/sources.toml"), &format!(
            "[[source]]\nid = \"index\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Index\"\nfetch = {{ kind = \"http\", url = \"{url}\" }}\nversion = \"release\"\nrefresh = \"manual\"\nredistribute = false\n"));
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        let http = Http::loopback(&actual);
        let moves = ["index@1".into()];
        let preview =
            planned(&root, &fixture.store, &http, None, &[&MovedIndex], "live", &[], Basis::Moves(&moves), false)
                .unwrap()
                .plan;
        assert!(preview.needs_prepare);
        assert!(!preview.blocked.is_empty());
        assert!(requests.lock().unwrap().is_empty(), "--move does not authorize bulk discovery");
        assert!(!fixture.store.root().join("runs").exists(), "a preview creates no operation journal");
        let saved = root.join("preview.json");
        write(&saved, &serde_json::to_string(&preview).unwrap());
        let args = BuildArgs { env: "live".into(), only: Vec::new(), moves: Vec::new(), plan: Some(saved) };
        let error = run_build(&root, &fixture.store, &http, None, &[&MovedIndex], &args).unwrap_err();
        assert_eq!(error.code, Code::PlanOutdated);
        assert!(requests.lock().unwrap().is_empty(), "replay refuses before preparation");

        let mut run = Run::create(&fixture.store, "prepare live").unwrap();
        let id = run.id().to_string();
        run.record(&Event::Phase { phase: Phase::Prepare }).unwrap();
        let prepared = planned_run(
            &root,
            &fixture.store,
            &http,
            None,
            &[&MovedIndex],
            "live",
            &[],
            Basis::Moves(&moves),
            true,
            Some(&mut run),
        )
        .unwrap();
        assert!(!prepared.plan.needs_prepare);
        assert_eq!(prepared.steps.len(), 3);
        assert_eq!(requests.lock().unwrap().len(), 1);
        run.finish(None).unwrap();
        let events = crate::engine::runs::events(&fixture.store, &id).unwrap();
        assert!(matches!(events.last(), Some(Event::Finished { ok: true, .. })));
        assert_eq!(events.iter().filter(|event| matches!(event, Event::FetchFinished { .. })).count(), 1);
        assert!(!fixture.store.root().join("layers").exists(), "prepare makes no layer receipts");
    }

    #[test]
    fn preparation_fails_when_a_product_keeps_usable_steps_after_a_failed_fetch() {
        use crate::fetch::tests::{not_found, serve};
        struct Softened;
        impl Product for Softened {
            fn name(&self) -> &'static str {
                "test"
            }
            fn steps(&self, _: &Path, env: &Env, _: &Regions, _: &Store) -> Result<crate::product::Steps, Unplanned> {
                if env.fetch_failures.is_empty() {
                    return Err(Unplanned::NeedsFetch(vec![Wanted {
                        source: "index".into(),
                        version: Some("1".into()),
                        params: Vec::new(),
                    }]));
                }
                let mut listed: crate::product::Steps = pipeline().into();
                listed.blocked.push(crate::product::BlockedLayer {
                    layer: "test/content".into(),
                    reason: env.fetch_failures[0].1.clone(),
                });
                Ok(listed)
            }
        }
        let (actual, requests) = serve(|_, _| not_found());
        let actual = actual.replacen("/data/", "/data/1/", 1);
        let url = actual.replacen("http://", "https://", 1).replacen("/data/1/", "/data/{version}/", 1);
        let fixture = fixture("prepare-softened-fetch");
        fixture.with_acquisition();
        let root = fixture.root();
        write(&root.join("data/sources.toml"), &format!(
            "[[source]]\nid = \"index\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Index\"\nfetch = {{ kind = \"http\", url = \"{url}\" }}\nversion = \"release\"\nrefresh = \"manual\"\nredistribute = false\n"));
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        let mut run = Run::create(&fixture.store, "prepare live").unwrap();
        let id = run.id().to_string();
        let args = PlanArgs { env: "live".into(), only: Vec::new(), moves: vec!["index@1".into()] };
        let result = prepare_plan(&root, &fixture.store, &Http::loopback(&actual), None, &[&Softened], &args, &mut run);
        let error = super::super::api::finish_run(run, result, None).unwrap_err();
        assert_eq!((error.code, error.code.exit()), (Code::FetchFailed, 1));
        assert_eq!(error.run.as_ref(), Some(&id));
        assert!(error.message.contains("index") && error.message.contains("404"), "{}", error.message);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let events = crate::engine::runs::events(&fixture.store, &id).unwrap();
        assert!(matches!(events.last(), Some(Event::Finished { ok: false, .. })));
        assert_eq!(events.iter().filter(|event| matches!(event, Event::Finished { .. })).count(), 1);
        assert!(!events.iter().any(|event| matches!(event, Event::StepStarted { .. })));
    }

    /// The test pipeline, once the store has `index@1` and then `box@1`, which only the index names.
    struct Chained;

    impl Product for Chained {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(
            &self,
            root: &std::path::Path,
            env: &Env,
            regions: &Regions,
            store: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Indexed.steps(root, env, regions, store)?;
            match snapshot_files(store, "box", "1", &[], &[]).map_err(Unplanned::Failed)? {
                Some(_) => Ok(pipeline().into()),
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
        let (listed, _) =
            steps(&fixture.root(), &[&Chained], &mut env(&[]), &regions, &fixture.store, false, fetch).unwrap();
        assert_eq!((listed.len(), fetched.into_inner()), (3, vec!["index".to_string(), "box".to_string()]));
    }

    /// No steps, once the store has the outline of `area=europe/monaco`.
    struct Outlined;

    impl Product for Outlined {
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
            let area = [("area".to_string(), "europe/monaco".to_string())];
            match crate::product::read(env, store, "land", &area).map_err(Unplanned::Failed)? {
                Ok(_) => Ok(Vec::new().into()),
                Err(wanted) => Err(Unplanned::NeedsFetch(vec![wanted])),
            }
        }
    }

    #[test]
    fn one_source_move_resolves_each_area_and_saved_pins_keep_those_versions() {
        struct Areas;
        impl Product for Areas {
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
                let mut wanted = Vec::new();
                for area in ["a", "b"] {
                    if let Err(request) = crate::product::version(env, store, "land", &[("area".into(), area.into())])
                        .map_err(Unplanned::Failed)?
                    {
                        wanted.push(request);
                    }
                }
                if wanted.is_empty() {
                    Ok(Vec::new().into())
                } else {
                    Err(Unplanned::NeedsFetch(wanted))
                }
            }
        }
        let fixture = fixture("cli-area-versions");
        let regions = Regions::new(Vec::new()).unwrap();
        let mut env = env(&[]);
        env.moves.insert("land".into(), None);
        let mut fetched = Vec::new();
        let mut fetch = |wanted: &Wanted| {
            fetched.push(wanted.clone());
            Ok(if wanted.params[0].1 == "a" { "2026-10-01" } else { "2026-10-02" }.into())
        };
        product_steps(&fixture.root(), &Areas, &mut env, &regions, &fixture.store, &mut fetch).unwrap().unwrap();
        assert_eq!(fetched.len(), 2);
        assert_eq!(env.moves["land"], None, "source intent stays unresolved");
        assert_eq!(env.read.borrow().len(), 2);
        let pins = env.read.borrow().clone();
        let mut saved = env.clone();
        saved.resolved.clear();
        saved.planned = Some(pins);
        product_steps(&fixture.root(), &Areas, &mut saved, &regions, &fixture.store, &mut |_| {
            panic!("saved exact pins fetch no new versions")
        })
        .unwrap()
        .unwrap();
        assert_eq!(saved.read, env.read);
        env.moves.insert("land".into(), Some("2026-10-03".into()));
        assert_eq!(env.version("land", &[("area".into(), "a".into())]), Ok(Some("2026-10-03")));
        assert_eq!(env.version("land", &[("area".into(), "b".into())]), Ok(Some("2026-10-03")));
    }

    #[test]
    fn a_source_that_nothing_names_is_fetched_at_the_newest_version_unless_it_is_manual() {
        use crate::fetch::tests::{quick, serve, source, whole};
        let (url, log) = serve(|_, _| whole(b"outline"));
        let fixture = fixture("cli-newest");
        let root = fixture.root();
        let manual = source(&url.replace("data/file.bin", "{area}.poly"), "date");
        let (regions, http) = (Regions::new(Vec::new()).unwrap(), quick());
        let mut live = env(&[]);
        let fetch = fetcher(&root, &fixture.store, &http, std::slice::from_ref(&manual), &live, None);
        let err =
            steps(&fixture.root(), &[&Outlined], &mut live, &regions, &fixture.store, false, fetch).err().unwrap();
        assert_eq!(
            (err.code, err.fix.as_str()),
            (Code::Blocked, "Plan with `--move land@VERSION`."),
            "{}",
            err.message
        );
        assert!(log.lock().unwrap().is_empty(), "a manual source does not move by itself");

        live.moves.insert("land".into(), None);
        let fetch = fetcher(&root, &fixture.store, &http, std::slice::from_ref(&manual), &live, None);
        steps(&fixture.root(), &[&Outlined], &mut live, &regions, &fixture.store, false, fetch).unwrap();
        assert_eq!(
            live.resolved[&("land".into(), vec![("area".into(), "europe/monaco".into())])].as_str(),
            "2026-10-05",
            "the fetch resolves only this request"
        );
        let requests = log.lock().unwrap().len();

        let land = Source { refresh: Refresh::Days(30), ..manual };
        let mut live = env(&[]);
        let fetch = fetcher(&root, &fixture.store, &http, std::slice::from_ref(&land), &live, None);
        steps(&fixture.root(), &[&Outlined], &mut live, &regions, &fixture.store, false, fetch).unwrap();
        assert_eq!(log.lock().unwrap().len(), requests, "without a move or live, the store serves");

        live.moves.insert("qrank".into(), None);
        let fetch = fetcher(&root, &fixture.store, &http, std::slice::from_ref(&land), &live, None);
        let err =
            steps(&fixture.root(), &[&Outlined], &mut live, &regions, &fixture.store, false, fetch).err().unwrap();
        assert_eq!(
            (err.code, err.message.as_str()),
            (Code::Usage, "--move qrank: no step list of `live` reads `qrank`")
        );
        let mut fetch = fetcher(&root, &fixture.store, &http, std::slice::from_ref(&land), &live, None);
        let err = fetch(&Wanted { source: "land".into(), version: None, params: Vec::new() }).unwrap_err();
        assert!(err.message.contains("fetched per `area=`"), "{}", err.message);
    }

    /// The test pipeline, with `head` at the version that `product::version` gives. Its pointer
    /// document is `{"schema": 1}`, and its release has a `COUNT.txt`.
    pub(crate) struct Versioned;

    impl Product for Versioned {
        fn name(&self) -> &'static str {
            "test"
        }

        fn pointer(&self) -> Option<crate::product::PointerFn> {
            Some(|_, _, _| {
                let document = [("schema".to_string(), 1.into())].into_iter().collect();
                Ok(crate::product::Pointer { document })
            })
        }

        fn named(&self, release: &Release) -> Result<Vec<crate::engine::LayerFile>, String> {
            let mut file =
                release.layers.iter().find(|layer| layer.step == "test/count").ok_or("no count layer")?.files[0]
                    .clone();
            file.path = "COUNT.txt".into();
            Ok(vec![file])
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            env: &Env,
            _: &Regions,
            store: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
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
            Ok(steps.into())
        }
    }

    #[test]
    fn a_move_chooses_between_two_versions_that_live_reads() {
        let fixture = fixture("cli-conflict");
        let regions = Regions::new(Vec::new()).unwrap();
        let mut live = env(&[]);
        live.live.insert(("head".into(), Vec::new()), ["1".to_string(), "2".to_string()].into());
        let err = steps(
            &fixture.root(),
            &[&Versioned],
            &mut live.clone(),
            &regions,
            &fixture.store,
            false,
            |_| unreachable!(),
        )
        .err()
        .unwrap();
        assert_eq!(
            (err.code, err.fix.as_str()),
            (Code::Blocked, "Plan with `--move head@VERSION`."),
            "{}",
            err.message
        );

        live.moves.insert("head".into(), Some("1".into()));
        let (listed, _) =
            steps(&fixture.root(), &[&Versioned], &mut live, &regions, &fixture.store, false, |_| unreachable!())
                .unwrap();
        assert_eq!(listed.len(), 3);
    }

    #[test]
    fn a_build_of_a_plan_reads_the_versions_of_the_plan_and_not_a_newer_one() {
        let fixture = fixture("cli-versions");
        let root = fixture.root();
        write(&root.join("data/sources.toml"), include_str!("../../../../data/sources.toml"));
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        let http = Http::new();
        let build = |plan: Option<PathBuf>| {
            let args = BuildArgs { env: "live".into(), only: Vec::new(), plan, moves: Vec::new() };
            run_build(&root, &fixture.store, &http, None, &[&Versioned], &args).unwrap().releases
        };
        let first = build(None)[0].id.clone();
        let saved = plan_of(&root, &fixture.store, &[&Versioned]);
        assert_eq!(
            saved.versions,
            [FetchVersion { product: None, source: "head".into(), params: Vec::new(), version: "1".into() }]
        );
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
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
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

    pub(crate) struct Partial;

    impl Product for Partial {
        fn name(&self) -> &'static str {
            "test"
        }
        fn pointer(&self) -> Option<crate::product::PointerFn> {
            Versioned.pointer()
        }
        fn steps(
            &self,
            root: &std::path::Path,
            env: &Env,
            regions: &Regions,
            store: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            let mut listed = Versioned.steps(root, env, regions, store)?;
            listed.blocked.push(crate::product::BlockedLayer {
                layer: "test/missing".into(),
                reason: "capture needs `--move wikidata`".into(),
            });
            Ok(listed)
        }
    }

    #[test]
    fn usable_layers_build_but_an_incomplete_product_has_no_release() {
        let fixture = fixture("cli-partial");
        let root = fixture.root();
        write(&root.join("data/sources.toml"), SOURCES);
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        let preview =
            planned(&root, &fixture.store, &Http::new(), None, &[&Partial], "live", &[], Basis::Moves(&[]), false)
                .unwrap()
                .plan;
        assert!(!preview.needs_prepare, "blocked readiness does not make the graph unresolved");
        let file = root.join("partial.json");
        write(&file, &serde_json::to_string(&preview).unwrap());
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: Some(file), moves: Vec::new() };
        let built = run_build(&root, &fixture.store, &Http::new(), None, &[&Partial], &args).unwrap();
        assert_eq!(built.layers.len(), 3);
        assert!(built.releases.is_empty());
        assert_eq!(built.blocked[0].layers[0].layer, "test/missing");
        let events = crate::engine::runs::events(&fixture.store, &built.run).unwrap();
        assert!(matches!(events.last(), Some(Event::Finished { ok: false, .. })));
    }

    /// A product that the environment does not suit.
    struct Refused;

    impl Product for Refused {
        fn name(&self) -> &'static str {
            "other"
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Err(Unplanned::Invalid("no box region".into()))
        }
    }

    #[test]
    fn a_product_without_steps_is_blocked_and_the_others_plan_and_build() {
        let fixture = fixture("cli-blocked");
        let root = fixture.root();
        write(&root.join("data/sources.toml"), include_str!("../../../../data/sources.toml"));
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        fixture.fetched("index", "index.txt", b"index\n");
        let (http, products): (_, [&dyn Product; 2]) = (Http::new(), [&Indexed, &Refused]);
        let plan = plan_of(&root, &fixture.store, &products);
        let blocked = plan.blocked;
        assert_eq!(
            blocked,
            [BlockedProduct { product: "other".into(), reason: "no box region".into(), layers: Vec::new() }]
        );
        assert_eq!(plan.groups.len(), 1, "the test product plans");

        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() };
        let built = run_build(&root, &fixture.store, &http, None, &products, &args).unwrap();
        assert_eq!((built.layers.len(), built.releases.len(), &built.blocked), (3, 1, &blocked));
        assert_eq!(built.releases[0].product, "test", "a blocked product has no release");
        let again = run_build(&root, &fixture.store, &http, None, &products, &args).unwrap();
        assert_eq!((again.layers.is_empty(), again.releases[0].id == built.releases[0].id), (true, true), "up to date");
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
        let registry = crate::sources::Registry::load(&fixture.root()).unwrap();
        let source = registry.sources.iter().find(|s| s.id == source).unwrap();
        crate::fetch::upstream::seed(&fixture.store, source, &[], version);
    }

    /// Make `release` live in the local bucket, with the input copy of `head@2020-01-01`.
    fn publish(fixture: &Fixture, release: &Release) {
        let (bucket, id) = (fixture.scratch.0.join("bucket"), release.id());
        write(&bucket.join("test/catalog.json"), &format!("{{\"schema\": 1, \"release\": \"{id}\"}}"));
        write(&bucket.join(format!("test/releases/{id}.json")), &String::from_utf8(release.canonical()).unwrap());
        let head = fixture.store.snapshot("head", "2020-01-01").unwrap().unwrap();
        let live = Live {
            products: vec![LiveProduct {
                product: "test".into(),
                prefix: "test".into(),
                release: Some((id.clone(), release.clone())),
                applied: None,
                observed: None,
                document: None,
            }],
            ..Live::default()
        };
        for read in crate::input_copy::reads(&live).unwrap().into_iter().filter(|r| r.key.source == "head") {
            let copy = crate::input_copy::Record::local(&fixture.store, &read).unwrap().unwrap();
            write(&bucket.join(read.key.path()), &String::from_utf8(copy.canonical()).unwrap());
        }
        for file in &release.named {
            let to = bucket.join(format!("test/releases/{id}/{}", file.path));
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(fixture.store.object(&file.sha256), to).unwrap();
        }
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
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
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
        Ok(planned(&root, &fixture.store, &http, Some(remote), &[&Versioned], "live", &only, moves, true)?.plan)
    }

    #[test]
    fn a_plan_of_live_has_one_group_per_cause_and_the_keys_that_an_apply_removes() {
        let (fixture, remote, release) = live("cli-live-plan");
        let plan = live_plan(&fixture, &remote, &[]).unwrap();
        assert_eq!(
            plan.live,
            [LiveRelease {
                product: "test".into(),
                release: Some(release.id()),
                pointer: Some(crate::store::sha256_hex(b"{\"schema\":1}")),
                observed: Some(crate::store::sha256_hex(
                    format!("{{\"schema\": 1, \"release\": \"{}\"}}", release.id()).as_bytes()
                ))
            }]
        );
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
        assert_eq!((moved.cause.as_ref(), &plan.moves), (Some(&cause), &BTreeMap::from([("head".into(), None)])));
        let steps = |group: &Group| group.layers.iter().map(|layer| layer.step.clone()).collect::<Vec<_>>();
        assert_eq!((steps(moved).join(" "), moved.builds.len()), ("test/upper test/join test/count".into(), 3));
        assert_eq!(steps(code), ["test/join", "test/count"]);
        assert_eq!(repair.cause, Some(Cause::Repair { keys: vec![count.clone()] }));
        assert!(repair.builds.is_empty(), "the groups make count again");
        let manifest = format!("test/releases/{}.json", release.id());
        let records = crate::input_copy::reads(&Live {
            products: vec![LiveProduct {
                product: "test".into(),
                prefix: "test".into(),
                release: Some((release.id(), release.clone())),
                applied: None,
                observed: None,
                document: None,
            }],
            ..Live::default()
        })
        .unwrap()
        .into_iter()
        .find(|r| r.key.source == "head")
        .unwrap()
        .key
        .path();
        let mut every = vec![
            records,
            join,
            upper,
            "test/objects/old".into(),
            manifest,
            format!("test/releases/{}/COUNT.txt", release.id()),
            format!("inputs/objects/{}", crate::store::sha256_hex(b"head\n")),
        ];
        every.sort();
        let removed = |plan: &EnvPlan| plan.remove.iter().map(|removal| removal.key.clone()).collect::<Vec<_>>();
        assert_eq!(
            removed(&plan),
            every,
            "count is absent; an unread file of the new snapshot does not keep the old input object"
        );

        let err = live_plan(&fixture, &remote, &["code:test/join"]).unwrap_err();
        assert_eq!(err.code, Code::Usage, "against live, --only selects moves only: {}", err.message);
        let plan = live_plan(&fixture, &remote, &["move:head"]).unwrap();
        assert_eq!((plan.groups.len(), plan.moves.len()), (3, 1), "every group, and the move");
        let plan = live_plan(&fixture, &remote, &["none"]).unwrap();
        let ids: Vec<&str> = plan.groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!((ids, plan.moves.len()), (vec!["code:test/join", "repair"], 0), "no move");

        write(
            &fixture.root().join("data/regions/andorra.toml"),
            "name = \"Andorra\"\nkind = \"geofabrik\"\nareas = [\"andorra\"]\n",
        );
        write(&fixture.root().join("data/env/live.toml"), "region = \"andorra\"\n");
        let edit = Edit::Region { product: "test".into(), from: Some("monaco".into()), to: "andorra".into() };
        assert_eq!(live_plan(&fixture, &remote, &[]).unwrap().edits, [edit], "no layer reads the region");
    }

    #[test]
    fn a_live_intermediate_layer_that_a_client_now_reads_gets_its_files_on_r2() {
        let (fixture, remote, release) = live("cli-live-client");
        let mut lean = release.clone();
        lean.layers[1].client = Client::None;
        let join = format!("test/objects/{}", release.layers[1].files[0].sha256);
        std::fs::remove_file(fixture.scratch.0.join("bucket").join(&join)).unwrap();
        publish(&fixture, &lean);
        let plan = live_plan(&fixture, &remote, &[]).unwrap();
        let ids: Vec<&str> = plan.groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, ["code:test/join"], "the step says that a client reads join");

        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() };
        let built = run_build(&fixture.root(), &fixture.store, &Http::new(), Some(&remote), &[&Versioned], &args);
        assert_eq!(built.unwrap().releases[0].id, release.id(), "the release has the files of join again");
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
        assert_eq!(
            (again.layers.is_empty(), &again.releases[0].id),
            (true, &built.releases[0].id),
            "a retry of the same plan"
        );

        publish(&fixture, &next);
        let err = build().unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::PlanOutdated, 3), "{}", err.message);
    }
}
