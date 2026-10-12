//! `obc data status`: what is live, the state of its layers, and what needs attention. `--check`
//! also compares R2 with live.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::build_cli::{check_layers, fetcher, load, status_steps};
use super::{bytes, cells, print_json, print_table, read_live, registry, remote, source_rows, Code, Error};
use crate::engine::release::Layer;
use crate::engine::state::{self, Environment, LayerState, Newer, Verdict};
use crate::engine::Step;
use crate::env::Env;
use crate::fetch::http::Http;
use crate::live::{Check, Remote};
use crate::product::{BlockedLayer, Product, Wanted};
use crate::regions::Regions;
use crate::sources::State;
use crate::store::Store;

#[derive(Args)]
pub struct StatusArgs {
    /// Also compare R2 with live. Exit status 1 for R2 drift or leftovers.
    #[arg(long)]
    pub check: bool,
}

/// What `status` writes.
#[derive(Clone, Serialize, JsonSchema)]
pub struct Status {
    /// Where live was read: the bucket, or its public URL.
    pub from: String,
    pub products: Vec<ProductStatus>,
    pub attention: Vec<Attention>,
    /// Only with `--check`.
    pub check: Option<Check>,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct ProductStatus {
    pub product: String,
    /// The id of the live release; `None` when nothing is live.
    pub release: Option<String>,
    /// When an apply made the release live, `YYYY-MM-DDTHH:MM:SSZ`; `None` when nothing is live or
    /// the pointer has no time.
    pub applied: Option<String>,
    /// The commit that the apply of the release ran; `None` when nothing is live.
    pub commit: Option<String>,
    /// The size of the objects of the live release.
    pub bytes: Option<u64>,
    /// The optional layers of the product, which `layer live NAME on|off` switches.
    pub optional: Vec<String>,
    /// Each operator layer that the steps of the product make or that its release holds, in
    /// dependency order, then its optional layers that are off; `None` when a fetch that its step
    /// list needs failed, and `attention` says why.
    pub layers: Option<Vec<LayerState>>,
}

/// Something that needs a person.
#[derive(Clone, Serialize, JsonSchema)]
pub struct Attention {
    pub kind: AttentionKind,
    /// The source, the directory, the product, or `R2`.
    pub about: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// A source that is stale.
    Stale,
    /// A source that is blocked, or a step that cannot be planned.
    Blocked,
    /// The checkout is not a pushed commit; an apply refuses it.
    Unpushed,
    /// Keys that live uses and R2 lacks, or holds with another size.
    Drift,
    /// Keys of earlier releases that no live release uses.
    Leftovers,
    /// A fetch that the step list of a product needs failed, so its layer states are unknown.
    Unreachable,
}

impl AttentionKind {
    pub fn text(self) -> &'static str {
        match self {
            AttentionKind::Stale => "stale",
            AttentionKind::Blocked => "blocked",
            AttentionKind::Unpushed => "not pushed",
            AttentionKind::Drift => "drift",
            AttentionKind::Leftovers => "leftovers",
            AttentionKind::Unreachable => "unreachable",
        }
    }
}

pub fn status(root: &Path, products: &[&dyn Product], check: bool, json: bool) -> Result<ExitCode, Error> {
    let status = read(root, products, check)?;
    let problems = status.check.as_ref().is_some_and(|check| !check.drift.is_empty() || !check.leftovers.is_empty());
    if json {
        print_json(&status)?;
    } else {
        print_status(&status);
    }
    Ok(if problems { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// What `status` writes; with `check`, what `status --check` writes.
fn read(root: &Path, products: &[&dyn Product], check: bool) -> Result<Status, Error> {
    let (store, mut registry) = (Store::open()?, registry(root)?);
    let remote = remote()?;
    if check && matches!(remote, Remote::Public(_)) {
        let error = Code::Blocked.error("`--check` lists R2, and a listing needs `OBC_R2_BUCKET` and its key");
        return Err(error);
    }
    let live = read_live(&remote, &registry, products, &store)?;
    crate::settings::observe(&store, &live)?;
    let mut loaded = load(root, "live", &store)?;
    registry.sources = loaded.sources.clone();
    loaded.env.live = live.versions();
    let http = Http::new();
    let copies = crate::input_copy::Restore { remote: &remote, live: &live };
    loaded.env.retained = crate::input_copy::retained(&live, &store)?;
    let inventory = super::freshness::discover(
        root,
        products,
        &loaded.env,
        &loaded.regions,
        &store,
        &http,
        &loaded.sources,
        Some(&copies),
        None,
    )?;
    let rows = source_rows(&registry, Some(&live.by_source()), Some(&inventory), false)?;
    let newer: Newer = rows
        .iter()
        .flat_map(|row| row.requests.iter().map(move |request| (row, request)))
        .filter(|(_, request)| request.state == State::Stale)
        .map(|(row, request)| {
            let upstream =
                request.observation.result.version().filter(|version| Some(*version) != request.live.as_deref());
            ((row.source.id.clone(), request.params.clone()), upstream.map(str::to_string))
        })
        .collect();
    let fetch = discovery_fetch(fetcher(root, &store, &http, &loaded.sources, &loaded.env, Some(&copies)), false);
    let environment = Environment { release: live.layers(), newer };
    let (mut layers, blocked) =
        layer_states(root, &store, products, &mut loaded.env, &loaded.regions, &environment, fetch)?;
    let mut attention = Vec::new();
    if let Err(error) = super::apply_cli::pushed_commit(root) {
        let reason = format!("{} {}", error.message, error.fix);
        attention.push(Attention { kind: AttentionKind::Unpushed, about: "checkout".into(), reason });
    }
    // `Live::read` gives one live product per product, in their order.
    let products = live.products.iter().zip(products).map(|(product, offered)| {
        let release = product.release.as_ref().map(|(id, _)| id.clone());
        let bytes = product.release.as_ref().map(|(_, release)| release.objects().values().sum());
        let optional = offered.optional().iter().map(|layer| layer.to_string()).collect();
        let layers = match layers.remove(&product.product).unwrap_or(Ok(Vec::new())) {
            Ok(layers) => Some(layers),
            Err(reason) => {
                let about = product.product.clone();
                attention.push(Attention { kind: AttentionKind::Unreachable, about, reason });
                None
            }
        };
        let (applied, commit) = (product.applied.clone(), product.commit.clone());
        ProductStatus { product: product.product.clone(), release, applied, commit, bytes, optional, layers }
    });
    let products: Vec<ProductStatus> = products.collect();
    let mut steps: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for BlockedLayer { step, layer, reason } in blocked {
        steps.entry((layer, reason)).or_default().push(step);
    }
    for ((about, reason), steps) in steps {
        let steps = if steps.len() == 1 { steps[0].clone() } else { format!("{} steps", steps.len()) };
        attention.push(Attention { kind: AttentionKind::Blocked, about, reason: format!("{steps}: {reason}") });
    }
    for row in &rows {
        let kind = match row.state {
            State::Stale => AttentionKind::Stale,
            State::Blocked => AttentionKind::Blocked,
            _ => continue,
        };
        let reason = row.reason.clone().unwrap_or_default();
        attention.push(Attention { kind, about: row.source.id.clone(), reason });
    }
    for source in loaded.env.refused.borrow().iter() {
        let reads = loaded.env.live.iter().filter(|((id, _), _)| id == source).flat_map(|(_, read)| read);
        let versions = reads.map(String::as_str).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
        let reason = format!("live reads it at {}: plan with `--move {source}@VERSION`", versions.join(" and "));
        attention.push(Attention { kind: AttentionKind::Blocked, about: source.clone(), reason });
    }
    let check = check.then(|| live.check(&remote, &store)).transpose().map_err(|e| Code::R2Failed.error(e))?;
    if let Some(check) = &check {
        let about = "R2".to_string();
        if !check.drift.is_empty() {
            let reason = format!("{} of live missing or with another size", keys(check.drift.len()));
            attention.push(Attention { kind: AttentionKind::Drift, about: about.clone(), reason });
        }
        if !check.leftovers.is_empty() {
            let size = bytes(check.leftovers.iter().map(|object| object.bytes).sum());
            let reason =
                format!("{}, {size}, of earlier releases that no live release uses", keys(check.leftovers.len()));
            attention.push(Attention { kind: AttentionKind::Leftovers, about, reason });
        }
    }
    Ok(Status { from: remote.describe().into(), products, attention, check })
}

/// Status can prepare the small files that enumerate a region, but never bulk product inputs.
pub(super) fn discovery_fetch(
    mut fetch: impl FnMut(&Wanted) -> Result<String, Error>,
    prepare: bool,
) -> impl FnMut(&Wanted) -> Result<String, Error> {
    move |wanted| {
        if !prepare
            && !matches!(wanted.source.as_str(), "geofabrik-poly" | "geofabrik-index" | "copernicus-glo-30-tiles")
        {
            return Err(Code::Blocked
                .error(format!(
                    "source `{}` is not prepared; status and ordinary plans do not fetch bulk data",
                    wanted.source
                ))
                .fix("Run `obc data prepare ENV` or a build to fetch it."));
        }
        fetch(wanted)
    }
}

/// The layers of each product, or why they are unknown.
type Found = BTreeMap<String, Result<Vec<LayerState>, String>>;

/// The state of each layer of `live`, by product, and the steps that cannot be planned. `Err` with
/// the reason for a product that is blocked, whose step list needs a fetch that fails, or that
/// reads a layer of such a product: the rest of `status` does not need its steps.
fn layer_states(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    env: &mut Env,
    regions: &Regions,
    environment: &Environment,
    mut fetch: impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<(Found, Vec<BlockedLayer>), Error> {
    check_layers(products, env)?;
    env.fetch_failures.clear();
    env.stale.extend(environment.newer.keys().map(|(id, _)| id.clone()));
    env.stale_requests.extend(environment.newer.keys().cloned());
    let (mut listed, mut found, mut refused, mut blocked) = (Vec::new(), BTreeMap::new(), BTreeSet::new(), Vec::new());
    for product in products {
        let steps = status_steps(root, *product, env, regions, store, &mut fetch);
        refused.extend(env.refused.borrow().iter().cloned());
        match steps {
            Ok(Ok(steps)) => {
                blocked.extend(steps.blocked);
                found.insert(product.name().to_string(), Ok(Vec::new()));
                listed.push((product.name().to_string(), steps.steps));
            }
            Ok(Err(reason)) => {
                found.insert(product.name().to_string(), Err(reason));
            }
            Err(e) if matches!(e.code, Code::FetchFailed | Code::Blocked) => {
                let reason = format!("a fetch that the step list needs failed: {}", e.message);
                found.insert(product.name().to_string(), Err(reason));
            }
            Err(e) => return Err(e),
        }
    }
    // Each product clears the refusals of the one before.
    *env.refused.borrow_mut() = refused;
    while let Some((name, reason)) = reads_unknown(&listed, &found) {
        listed.retain(|(product, _)| *product != name);
        found.insert(name, Err(reason));
    }
    let mut steps = Vec::new();
    for (name, listed) in listed {
        let product = products.iter().find(|product| product.name() == name).expect("listed product");
        super::build_cli::add_steps(products, *product, &mut steps, listed)?;
    }
    // The judge reads the live layers of the listed products. `attention` lists a blocked step.
    let owner = |step: &str| step.split('/').next().unwrap_or_default().to_string();
    let judged = |step: &str| matches!(found.get(&owner(step)), Some(Ok(_))) && !blocked.iter().any(|b| b.step == step);
    let release = environment.release.iter().filter(|(step, _)| judged(step));
    let release: BTreeMap<String, Layer> = release.map(|(step, layer)| (step.clone(), layer.clone())).collect();
    let environment = Environment { release, newer: environment.newer.clone() };
    // A layer is listed for each product whose release holds one of its steps.
    let mut owners: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for step in &steps {
        owners.entry(step.layer.name).or_default().insert(owner(&step.name));
    }
    for layer in environment.release.values() {
        owners.entry(&layer.layer).or_default().insert(owner(&layer.step));
    }
    for layer in state::state(store, &steps, &environment)? {
        for product in &owners[layer.layer.as_str()] {
            if let Some(Ok(layers)) = found.get_mut(product) {
                layers.push(layer.clone());
            }
        }
    }
    for product in products {
        let Some(Ok(layers)) = found.get_mut(product.name()) else { continue };
        for name in product.optional().iter().filter(|name| !env.layers.iter().any(|on| on == *name)) {
            let layer = format!("{}/{name}", product.name());
            if !layers.iter().any(|known| known.layer == layer) {
                layers.push(LayerState { layer, state: Verdict::Off, cause: None, text: None });
            }
        }
    }
    Ok((found, blocked))
}

/// A listed product that reads a layer of a product whose layers are unknown, with that reason.
fn reads_unknown(listed: &[(String, Vec<Step>)], found: &Found) -> Option<(String, String)> {
    let made: HashSet<&str> = listed.iter().flat_map(|(_, steps)| steps).map(|step| step.name.as_str()).collect();
    listed.iter().find_map(|(product, steps)| {
        let missing = steps.iter().flat_map(Step::layers).find(|layer| !made.contains(layer))?;
        let owner = missing.split('/').next().unwrap_or_default();
        Some((product.clone(), found.get(owner)?.as_ref().err()?.clone()))
    })
}

fn print_status(status: &Status) {
    println!("LIVE from {}", status.from);
    let mut table = Vec::new();
    for product in &status.products {
        let release = product
            .release
            .as_ref()
            .map_or("nothing live · not owned until the first apply".into(), |id| format!("release {}", &id[..8]));
        let size = product.bytes.map_or("—".into(), bytes);
        let Some(layers) = &product.layers else {
            table.push(vec![product.product.clone(), release, size, "unknown".into()]);
            continue;
        };
        let mut counts: Vec<(Verdict, usize)> = Vec::new();
        for layer in layers {
            match counts.iter_mut().find(|(state, _)| *state == layer.state) {
                Some((_, n)) => *n += 1,
                None => counts.push((layer.state, 1)),
            }
        }
        let counts: Vec<String> = counts.into_iter().map(|(state, n)| format!("{n} {state}")).collect();
        table.push(vec![product.product.clone(), release, size, counts.join(" · ")]);
    }
    if !table.is_empty() {
        table.insert(0, cells(["PRODUCT", "RELEASE", "SIZE", "LAYERS"]));
        print_table(&table);
    }
    let stale = status.products.iter().flat_map(|product| {
        let layers = product.layers.iter().flatten().filter(|layer| layer.state == Verdict::Stale);
        layers.map(|layer| vec![format!("  {}", layer.layer), layer.text.clone().unwrap_or_default()])
    });
    let stale: Vec<Vec<String>> = stale.collect::<BTreeSet<_>>().into_iter().collect();
    if !stale.is_empty() {
        println!("STALE");
        print_table(&stale);
    }
    println!("NEEDS ATTENTION");
    let rows = status.attention.iter().map(|a| vec![format!("  {}", a.kind.text()), a.about.clone(), a.reason.clone()]);
    let rows: Vec<Vec<String>> = rows.collect();
    if rows.is_empty() {
        println!("  nothing");
    }
    print_table(&rows);
    let Some(check) = &status.check else { return };
    println!("CHECK {}", check.prefixes.iter().map(|prefix| format!("{prefix}/")).collect::<Vec<_>>().join(", "));
    let mut table = Vec::new();
    for drift in &check.drift {
        let size = |size: Option<u64>| size.map_or("—".into(), |size| format!("{size} B"));
        let what = if drift.found.is_none() { "missing" } else { "size" };
        table.push(vec![format!("  {what}"), drift.key.clone(), size(drift.found), size(drift.expected)]);
    }
    let leftovers: Vec<_> = check
        .leftovers
        .iter()
        .map(|object| crate::live::Removal { key: object.key.clone(), bytes: object.bytes })
        .collect();
    for (prefix, (count, size)) in crate::live::by_prefix(&leftovers) {
        table.push(vec!["  leftovers".into(), prefix.into(), keys(count), bytes(size)]);
    }
    if table.is_empty() {
        println!("  live and R2 agree");
    }
    print_table(&table);
}

/// `1 key`, `2 keys`.
pub(super) fn keys(count: usize) -> String {
    format!("{count} key{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::engine::tests::step;
    use crate::engine::{Input, Run};
    use crate::product::Unplanned;
    use crate::store::tests::Scratch;

    /// A product whose step list always needs a fetch.
    struct Fetching;

    /// A product with no steps.
    struct Listed;

    impl Product for Listed {
        fn name(&self) -> &'static str {
            "listed"
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Ok(Vec::new().into())
        }
    }

    impl Product for Fetching {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            let wanted = Wanted { source: "index".into(), version: None, params: Vec::new() };
            Err(Unplanned::NeedsFetch(vec![wanted]))
        }
    }

    /// A product whose step reads a layer of `test`.
    struct Reading;

    impl Product for Reading {
        fn name(&self) -> &'static str {
            "reading"
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            let run = Run::Command(vec!["true".into()]);
            Ok(vec![step("reading/one", vec![Input::layer("test/one")], "out", run)].into())
        }
    }

    struct Bulk;

    impl Product for Bulk {
        fn name(&self) -> &'static str {
            "test"
        }
        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, crate::product::Unplanned> {
            Err(crate::product::Unplanned::NeedsFetch(vec![Wanted {
                source: "geofabrik-extracts".into(),
                version: None,
                params: vec![("area".into(), "europe/test".into())],
            }]))
        }
    }

    #[test]
    fn status_reports_an_unprepared_bulk_source_without_starting_its_download() {
        let scratch = Scratch::new("status-bulk");
        let store = Store::at(scratch.0.join("store"));
        let regions = Regions::new(Vec::new()).unwrap();
        let environment = Environment { release: BTreeMap::new(), newer: Newer::new() };
        let fetch = discovery_fetch(|_| panic!("a status must not download an extract"), false);
        let (found, _) =
            layer_states(&scratch.0, &store, &[&Bulk], &mut Env::default(), &regions, &environment, fetch).unwrap();
        assert!(found["test"].as_ref().unwrap_err().contains("geofabrik-extracts` is not prepared"));
        assert!(!store.root().join("snapshots").exists());
    }

    struct Partial;

    impl Product for Partial {
        fn name(&self) -> &'static str {
            "test"
        }
        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, crate::product::Unplanned> {
            let run = Run::Command(vec!["true".into()]);
            let mut listed = crate::product::Steps {
                steps: vec![
                    step("test/usable", vec![], "out", Run::Command(vec!["true".into()])),
                    step("test/reader", vec![Input::layer("test/capture")], "out", run),
                ],
                blocked: vec![crate::product::BlockedLayer::new(
                    "test/capture",
                    &crate::engine::tests::TEST,
                    "plan with `--move wikidata`",
                )],
            };
            listed.block_dependents();
            Ok(listed)
        }
    }

    #[test]
    fn status_keeps_usable_layers_and_reports_precise_blocked_dependents_without_fetching() {
        let scratch = Scratch::new("status-partial");
        let store = Store::at(scratch.0.join("store"));
        let regions = Regions::new(Vec::new()).unwrap();
        let environment = Environment { release: BTreeMap::new(), newer: Newer::new() };
        let fetch = |_: &Wanted| panic!("no capture fetch");
        let (found, blocked) =
            layer_states(&scratch.0, &store, &[&Partial], &mut Env::default(), &regions, &environment, fetch).unwrap();
        let layers = found["test"].as_ref().unwrap();
        assert_eq!((layers.len(), layers[0].state, layers[0].text.as_deref()), (1, Verdict::Stale, Some("new")));
        let blocked: Vec<(&str, bool)> =
            blocked.iter().map(|b| (b.step.as_str(), b.reason.contains("--move wikidata"))).collect();
        assert_eq!(blocked, [("test/capture", true), ("test/reader", true)]);
    }

    #[test]
    fn a_failed_fetch_makes_its_product_and_the_products_that_read_it_unknown() {
        let scratch = Scratch::new("status-unreachable");
        let store = Store::at(scratch.0.join("store"));
        let regions = Regions::new(Vec::new()).unwrap();
        let environment = Environment { release: BTreeMap::new(), newer: Newer::new() };
        let states = |code: Code| {
            let fetch = |_: &Wanted| Err(code.error("GET https://example.org/index: unreachable"));
            let mut env = Env { name: "live".into(), region: "monaco".into(), ..Env::default() };
            let products: &[&dyn Product] = &[&Fetching, &Listed, &Reading];
            layer_states(&scratch.0, &store, products, &mut env, &regions, &environment, fetch)
        };
        let (found, _) = states(Code::FetchFailed).unwrap();
        let reason = "a fetch that the step list needs failed: GET https://example.org/index: unreachable";
        assert_eq!(found["test"], Err(reason.to_string()));
        assert_eq!(found["reading"], Err(reason.to_string()), "it reads a layer of `test`");
        assert_eq!(found["listed"], Ok(Vec::new()));
        assert!(states(Code::Failed).is_err(), "only a failed fetch is unknown layers");
    }
}
