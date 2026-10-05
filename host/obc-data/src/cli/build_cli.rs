//! `obc data plan ENV` and `obc data build ENV`: the steps of every product for an environment,
//! what a build would fetch and build, and the build into the store. Nothing uploads.

use std::path::{Path, PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::runs_cli::{bytes, duration};
use super::{cells, fetched, print_json, print_table, registry, Code, Error};
use crate::engine::plan::{self, Estimate, Fetch, Group, Plan};
use crate::engine::release;
use crate::engine::runs::{Context, Limits, Run};
use crate::engine::Step;
use crate::env::Env;
use crate::fetch::{self, http::Http, Request};
use crate::product::{Product, Unplanned};
use crate::regions::Regions;
use crate::sources::{Kind, Source};
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
    pub groups: Vec<Group>,
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
    let Planned { env, plan, .. } = planned(root, &store, &http, products, &args.env)?;
    let plan = env_plan(&env, select(&plan, &args.only)?);
    if json {
        return print_json(&plan);
    }
    let layers = if plan.layers.is_empty() { "—".into() } else { plan.layers.join(", ") };
    println!("PLAN {} · region {} · layers {layers}", plan.env, plan.region);
    if plan.groups.is_empty() {
        println!("The store has every layer.");
        return Ok(());
    }
    let mut table = vec![cells(["GROUP", "FETCH", "BUILD", "TIME", "OUTPUT", "BLOCKED"])];
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
            group.blocked.clone().unwrap_or_default(),
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
    Ok(())
}

fn run_build(
    root: &Path,
    store: &Store,
    http: &Http,
    products: &[&dyn Product],
    args: &BuildArgs,
) -> Result<Built, Error> {
    let Planned { env, sources, steps, plan: now } = planned(root, store, http, products, &args.env)?;
    let wanted = match &args.plan {
        None => select(&now, &args.only)?,
        Some(file) => {
            let text =
                std::fs::read_to_string(file).map_err(|e| Code::Usage.error(format!("{}: {e}", file.display())))?;
            let EnvPlan { env: name, region, layers, groups } = serde_json::from_str(&text).map_err(|e| {
                Code::Usage.error(format!("{} is not the output of `obc data plan --json`: {e}", file.display()))
            })?;
            let saved = select(&Plan { groups }, &args.only)?;
            let ids: Vec<String> = saved.groups.iter().map(|group| group.id.clone()).collect();
            let same_env = (name, region, layers) == (env.name.clone(), env.region.clone(), env.layers.clone());
            if !same_env || !now.only(&ids).is_ok_and(|now| now.same_work(&saved)) {
                return Err(Code::PlanOutdated.error(format!("{} is not the plan of now", file.display())));
            }
            saved
        }
    };
    if let Some(group) = wanted.groups.iter().find(|group| group.blocked.is_some()) {
        let reason = group.blocked.as_deref().unwrap_or_default();
        return Err(Code::Blocked
            .error(format!("group `{}` is blocked: {reason}", group.id))
            .fix("Install the tool, or leave the group out with `--only`."));
    }
    let mut built = Built { run: None, layers: Vec::new(), releases: Vec::new() };
    if !wanted.groups.is_empty() {
        let mut run = Run::create(store, &format!("build {}", env.name))?;
        let id = run.id().to_string();
        eprintln!("obc data: run {id}; `obc data runs {id} --follow` shows its events");
        let context = Context { store, root, sources: &sources, http, limits: Limits::machine() };
        let result = run.build(&context, &steps, &wanted);
        run.finish(result.as_ref().err().map(String::as_str))?;
        let layers = result.map_err(|e| Code::RunFailed.error(format!("run {id}: {e}")))?;
        built.layers = layers
            .into_iter()
            .map(|layer| BuiltLayer { step: layer.receipt.step, key: layer.receipt.key, reused: layer.reused })
            .collect();
        built.run = Some(id);
    }
    for product in products {
        if let Some(release) = release::release(store, root, product.name(), &steps)? {
            built.releases.push(BuiltRelease { product: product.name().into(), id: release.write(store)? });
        }
    }
    Ok(built)
}

fn env_plan(env: &Env, plan: Plan) -> EnvPlan {
    EnvPlan { env: env.name.clone(), region: env.region.clone(), layers: env.layers.clone(), groups: plan.groups }
}

fn select(plan: &Plan, only: &[String]) -> Result<Plan, Error> {
    if only.is_empty() {
        return Ok(plan.clone());
    }
    plan.only(only).map_err(|e| Code::Usage.error(e))
}

/// An environment, the steps of every product and the plan of all of them.
struct Planned {
    env: Env,
    sources: Vec<Source>,
    steps: Vec<Step>,
    plan: Plan,
}

fn planned(root: &Path, store: &Store, http: &Http, products: &[&dyn Product], name: &str) -> Result<Planned, Error> {
    let registry = registry(root)?;
    if !crate::is_kebab(name) || !root.join("data/env").join(format!("{name}.toml")).is_file() {
        return Err(Code::Usage.error(format!("no environment `{name}` in data/env/")));
    }
    let regions = Regions::load(root).map_err(|e| Code::InvalidData.error(e))?;
    let env = Env::load(root, name, &registry.sources, &regions).map_err(|e| Code::InvalidData.error(e))?;
    let fetch = |wanted: &Fetch| {
        let source = find_source(&registry.sources, &wanted.source)?;
        let request = Request { source, version: Some(wanted.version.clone()), params: wanted.params.clone() };
        fetched(source, fetch::fetch(store, http, &request)).map(drop)
    };
    let steps = steps(products, &env, &regions, store, &registry.sources, fetch)?;
    let plan = plan::plan(store, root, &steps)?;
    Ok(Planned { env, sources: registry.sources, steps, plan })
}

fn find_source<'a>(sources: &'a [Source], id: &str) -> Result<&'a Source, Error> {
    sources
        .iter()
        .find(|source| source.id == id)
        .ok_or_else(|| Code::InvalidData.error(format!("no source `{id}` in data/sources.toml")))
}

/// The steps of every product. A product whose step list reads snapshots that the store lacks
/// gets them fetched, and is asked once more.
fn steps(
    products: &[&dyn Product],
    env: &Env,
    regions: &Regions,
    store: &Store,
    sources: &[Source],
    mut fetch: impl FnMut(&Fetch) -> Result<(), Error>,
) -> Result<Vec<Step>, Error> {
    let offered = |layer: &&String| products.iter().any(|product| product.optional().contains(&layer.as_str()));
    if let Some(layer) = env.layers.iter().find(|layer| !offered(layer)) {
        let message = format!("data/env/{}.toml: no product has the optional layer `{layer}`", env.name);
        return Err(Code::InvalidData.error(message));
    }
    let mut all = Vec::new();
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
                let wanted = fetches.iter().map(|f| format!("{}@{}", f.source, f.version)).collect::<Vec<_>>();
                let message = format!("product `{name}` still needs {} after the fetch", wanted.join(", "));
                return Err(Code::Failed.error(message));
            }
            Err(Unplanned::Invalid(e)) => return Err(Code::InvalidData.error(format!("product `{name}`: {e}"))),
        };
        for step in &steps {
            if !step.name.starts_with(&format!("{name}/")) {
                return Err(
                    Code::Failed.error(format!("step `{}` of product `{name}` is not named `{name}/…`", step.name))
                );
            }
            let tool = |id: &String| sources.iter().any(|source| &source.id == id && source.kind == Kind::Tool);
            if let Some(id) = step.tools.iter().find(|id| !tool(id)) {
                return Err(Code::Failed.error(format!("step `{}`: `{id}` is no `kind = \"tool\"` source", step.name)));
            }
        }
        all.extend(steps);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use super::*;
    use crate::engine::snapshot_files;
    use crate::engine::tests::{fixture, pipeline, write};

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
                None => Err(Unplanned::NeedsFetch(vec![Fetch {
                    source: "index".into(),
                    version: "1".into(),
                    params: Vec::new(),
                    files: Vec::new(),
                    bytes: None,
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
        let fetch = |wanted: &Fetch| {
            fixture.fetched(&wanted.source, "index.txt", b"index\n");
            fetches.set(fetches.get() + 1);
            Ok(())
        };
        let listed = steps(&[&Indexed], &env(&["extra"]), &regions, &fixture.store, &[], fetch).unwrap();
        assert_eq!((listed.len(), fetches.get()), (3, 1));
        let listed = steps(&[&Indexed], &env(&[]), &regions, &fixture.store, &[], |_| unreachable!()).unwrap();
        assert_eq!(listed.len(), 3, "the store has it now");

        let err = steps(&[&Indexed], &env(&[]), &regions, &empty.store, &[], |_| Ok(())).err().unwrap();
        assert_eq!(err.message, "product `test` still needs index@1 after the fetch");
        let err = steps(&[&Indexed], &env(&["snow"]), &regions, &fixture.store, &[], |_| Ok(())).err().unwrap();
        let message = "data/env/live.toml: no product has the optional layer `snow`";
        assert_eq!((err.code, err.message.as_str()), (Code::InvalidData, message));
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
        let Planned { env, plan, .. } = planned(&root, &fixture.store, &http, &[&Indexed], "live").unwrap();
        let file = root.join("plan.json");
        write(&file, &serde_json::to_string(&env_plan(&env, plan)).unwrap());
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: Some(file) };

        write(&root.join("steps/src/lib.rs"), "// Another key.\n");
        let err = run_build(&root, &fixture.store, &http, &[&Indexed], &args).unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::PlanOutdated, 3));
        assert!(!fixture.store.root().join("layers").exists(), "nothing is built");

        write(&root.join("steps/src/lib.rs"), "");
        let built = run_build(&root, &fixture.store, &http, &[&Indexed], &args).unwrap();
        let steps: Vec<&str> = built.layers.iter().map(|layer| layer.step.as_str()).collect();
        assert_eq!(steps, ["test/upper", "test/join", "test/count"]);
        assert!(fixture.store.release("test", &built.releases[0].id).is_file());
    }
}
