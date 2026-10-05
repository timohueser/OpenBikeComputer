//! `obc data plan ENV` and `obc data build ENV`: the steps of every product for an environment,
//! what a build would fetch and build, and the build into the store. Nothing uploads.

use std::path::{Path, PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::runs_cli::{bytes, duration};
use super::{cells, fetched, print_json, print_table, registry, Code, Error};
use crate::engine::plan::{self, Estimate, Group, Plan};
use crate::engine::release;
use crate::engine::runs::{Context, Limits, Run};
use crate::engine::Step;
use crate::env::Env;
use crate::fetch::{self, http::Http, Request};
use crate::product::{Product, Unplanned, Wanted};
use crate::regions::Regions;
use crate::sources::Source;
use crate::store::Store;

#[derive(Args)]
pub struct PlanArgs {
    /// The environment: `data/env/ENV.toml`.
    env: String,
    /// Only these groups, by id.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
}

#[derive(Args)]
pub struct BuildArgs {
    /// The environment: `data/env/ENV.toml`.
    env: String,
    /// Only these groups, by id.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Build the groups of this output of `plan ENV --json`. Exit status 3 when the plan of now
    /// differs.
    #[arg(long)]
    plan: Option<PathBuf>,
}

/// What a build of an environment would fetch and build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvPlan {
    pub env: String,
    pub region: String,
    pub layers: Vec<String>,
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
    let loaded = load(root, &args.env)?;
    let (_, plan, blocked) = planned(root, &store, products, &loaded, fetcher(&store, &http, &loaded.sources))?;
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
    let built = run_build(root, &Store::open()?, &Http::new(), products, &args)?;
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
) -> Result<Built, Error> {
    let loaded = load(root, &args.env)?;
    let saved = args.plan.as_deref().map(|file| read_plan(file, &loaded.env).map(|plan| (file, plan))).transpose()?;
    let (steps, now, blocked) = match &saved {
        None => planned(root, store, products, &loaded, fetcher(store, http, &loaded.sources))?,
        // `plan` fetched what each step list reads: a step list that needs a fetch now is new.
        Some((file, _)) => planned(root, store, products, &loaded, |_| Err(outdated(file)))?,
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
    if wanted.groups.is_empty() && !blocked.is_empty() {
        let reasons = blocked.iter().map(|b| format!("product `{}`: {}", b.product, b.reason)).collect::<Vec<_>>();
        let fix = "Correct what the message names; the other products build without it.";
        return Err(Code::Blocked.error(reasons.join("; ")).fix(fix));
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
    let (env, region, layers) = (env.name.clone(), env.region.clone(), env.layers.clone());
    EnvPlan { env, region, layers, only: only.to_vec(), groups: plan.groups, blocked }
}

fn select(plan: &Plan, only: &[String]) -> Result<Plan, Error> {
    if only.is_empty() {
        return Ok(plan.clone());
    }
    plan.only(only).map_err(|e| Code::Usage.error(e))
}

/// An environment, with the sources and the regions that it was read with.
struct Loaded {
    env: Env,
    sources: Vec<Source>,
    regions: Regions,
}

fn load(root: &Path, name: &str) -> Result<Loaded, Error> {
    let registry = registry(root)?;
    if !crate::is_kebab(name) || !root.join("data/env").join(format!("{name}.toml")).is_file() {
        return Err(Code::Usage.error(format!("no environment `{name}` in data/env/")));
    }
    let regions = Regions::load(root).map_err(|e| Code::InvalidData.error(e))?;
    let env = Env::load(root, name, &registry.sources, &regions).map_err(|e| Code::InvalidData.error(e))?;
    Ok(Loaded { env, sources: registry.sources, regions })
}

/// The steps of every product, the plan of all of them, and the products without steps.
fn planned(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    loaded: &Loaded,
    fetch: impl FnMut(&Wanted) -> Result<(), Error>,
) -> Result<(Vec<Step>, Plan, Vec<BlockedProduct>), Error> {
    let (steps, blocked) = steps(products, &loaded.env, &loaded.regions, store, fetch)?;
    let plan = plan::plan(store, root, &steps)?;
    Ok((steps, plan, blocked))
}

/// Fetch what a product names, with the code of a failed fetch: `fetch_failed` or `blocked`.
fn fetcher<'a>(
    store: &'a Store,
    http: &'a Http,
    sources: &'a [Source],
) -> impl FnMut(&Wanted) -> Result<(), Error> + 'a {
    move |wanted| {
        let source = sources
            .iter()
            .find(|source| source.id == wanted.source)
            .ok_or_else(|| Code::InvalidData.error(format!("no source `{}` in data/sources.toml", wanted.source)))?;
        let request = Request { source, version: wanted.version.clone(), params: wanted.params.clone() };
        fetched(source, fetch::fetch(store, http, &request)).map(drop)
    }
}

/// An error in the code of product `name`, not in `data/`.
fn product_bug(name: &str, message: String) -> Error {
    let fix = format!("The code of product `{name}` is wrong, not the data: correct the product in its crate.");
    Code::Failed.error(message).fix(fix)
}

/// The steps of every product, and the products that give `Unplanned::Invalid`. A product whose
/// step list reads snapshots that the store lacks gets them fetched, and is asked once more.
fn steps(
    products: &[&dyn Product],
    env: &Env,
    regions: &Regions,
    store: &Store,
    mut fetch: impl FnMut(&Wanted) -> Result<(), Error>,
) -> Result<(Vec<Step>, Vec<BlockedProduct>), Error> {
    let offered = |layer: &&String| products.iter().any(|product| product.optional().contains(&layer.as_str()));
    if let Some(layer) = env.layers.iter().find(|layer| !offered(layer)) {
        let message = format!("data/env/{}.toml: no product has the optional layer `{layer}`", env.name);
        return Err(Code::InvalidData.error(message));
    }
    let (mut all, mut blocked) = (Vec::new(), Vec::new());
    for product in products {
        let name = product.name();
        let mut listed = product.steps(env, regions, store);
        if let Err(Unplanned::NeedsFetch(fetches)) = &listed {
            fetches.iter().try_for_each(&mut fetch)?;
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
            Err(Unplanned::Invalid(reason)) => {
                blocked.push(BlockedProduct { product: name.into(), reason });
                continue;
            }
        };
        if let Some(step) = steps.iter().find(|step| !step.name.starts_with(&format!("{name}/"))) {
            return Err(product_bug(name, format!("step `{}` of product `{name}` is not named `{name}/…`", step.name)));
        }
        all.extend(steps);
    }
    Ok((all, blocked))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use super::*;
    use crate::engine::snapshot_files;
    use crate::engine::tests::{fixture, pipeline, write, JOIN};

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
            match snapshot_files(store, "index", "1", &[], &[]).map_err(Unplanned::Invalid)? {
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
        Env { name: "live".into(), region: "monaco".into(), layers, pins: BTreeMap::new() }
    }

    #[test]
    fn a_step_list_that_reads_a_snapshot_is_listed_again_after_its_fetch() {
        let (fixture, empty) = (fixture("cli-two-stage"), fixture("cli-two-stage-empty"));
        let regions = Regions::new(Vec::new()).unwrap();
        let fetches = Cell::new(0);
        let fetch = |wanted: &Wanted| {
            fixture.fetched(&wanted.source, "index.txt", b"index\n");
            fetches.set(fetches.get() + 1);
            Ok(())
        };
        let (listed, _) = steps(&[&Indexed], &env(&["extra"]), &regions, &fixture.store, fetch).unwrap();
        assert_eq!((listed.len(), fetches.get()), (3, 1));
        let (listed, _) = steps(&[&Indexed], &env(&[]), &regions, &fixture.store, |_| unreachable!()).unwrap();
        assert_eq!(listed.len(), 3, "the store has it now");

        let err = steps(&[&Indexed], &env(&[]), &regions, &empty.store, |_| Ok(())).err().unwrap();
        assert_eq!(err.message, "product `test` still needs index@1 after the fetch");
        assert!(err.fix.contains("product `test`"), "a product bug points at its code: {}", err.fix);
        let blocked = |_: &Wanted| Err(Code::Blocked.error("credential missing"));
        let err = steps(&[&Indexed], &env(&[]), &regions, &empty.store, blocked).err().unwrap();
        assert_eq!(err.code, Code::Blocked, "a failed fetch keeps its code");
        let err = steps(&[&Indexed], &env(&["snow"]), &regions, &fixture.store, |_| Ok(())).err().unwrap();
        let message = "data/env/live.toml: no product has the optional layer `snow`";
        assert_eq!((err.code, err.message.as_str()), (Code::InvalidData, message));
    }

    /// No steps, once the store has the outline of `area=europe/monaco`.
    struct Outlined;

    impl Product for Outlined {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, env: &Env, _: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
            let area = [("area".to_string(), "europe/monaco".to_string())];
            match crate::product::read(env, store, "land", &area).map_err(Unplanned::Invalid)? {
                Ok(_) => Ok(Vec::new()),
                Err(wanted) => Err(Unplanned::NeedsFetch(vec![wanted])),
            }
        }
    }

    #[test]
    fn an_unpinned_source_is_fetched_at_the_newest_version_and_then_read_from_the_store() {
        use crate::fetch::tests::{quick, serve, source, whole};
        let (url, log) = serve(|_, _| whole(b"outline"));
        let fixture = fixture("cli-unpinned");
        let land = source(&url.replace("data/file.bin", "{area}.poly"), "date");
        let (regions, http) = (Regions::new(Vec::new()).unwrap(), quick());
        let fetch = fetcher(&fixture.store, &http, std::slice::from_ref(&land));
        steps(&[&Outlined], &env(&[]), &regions, &fixture.store, fetch).unwrap();
        let requests = log.lock().unwrap().len();
        assert_eq!(fixture.store.snapshots("land").unwrap()[0].version, "2026-10-05", "the Last-Modified day");
        steps(&[&Outlined], &env(&[]), &regions, &fixture.store, |_| unreachable!()).unwrap();
        assert_eq!(log.lock().unwrap().len(), requests, "the second ask reads the store");
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
        let loaded = load(&root, "live").unwrap();
        let (_, plan, blocked) = planned(&root, &fixture.store, &[&Indexed], &loaded, |_| unreachable!()).unwrap();
        let saved = env_plan(&loaded.env, &[], plan, blocked);
        let file = root.join("plan.json");
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: Some(file.clone()) };
        let refused = |saved: &EnvPlan, why: &str| {
            write(&file, &serde_json::to_string(saved).unwrap());
            let err = run_build(&root, &fixture.store, &http, &[&Indexed], &args).unwrap_err();
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
        let built = run_build(&root, &fixture.store, &http, &[&Indexed], &args).unwrap();
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
        let loaded = load(&root, "live").unwrap();
        let (_, plan, blocked) = planned(&root, &fixture.store, &products, &loaded, |_| unreachable!()).unwrap();
        assert_eq!(blocked, [BlockedProduct { product: "other".into(), reason: "no box region".into() }]);
        assert_eq!(plan.groups.len(), 1, "the test product plans");

        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None };
        let built = run_build(&root, &fixture.store, &http, &products, &args).unwrap();
        assert_eq!((built.layers.len(), built.releases.len(), built.blocked), (3, 1, blocked));
        assert_eq!(built.releases[0].product, "test", "a blocked product has no release");
        let err = run_build(&root, &fixture.store, &http, &products, &args).unwrap_err();
        assert_eq!(
            (err.code.exit(), err.message.as_str()),
            (4, "product `other`: no box region"),
            "nothing else builds"
        );
    }
}
