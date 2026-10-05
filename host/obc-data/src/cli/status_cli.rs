//! `obc data status`: what is live, the state of its layers, and what needs attention. `--check`
//! also lists the prefixes that live owns on R2.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::build_cli::{check_layers, fetcher, load, product_steps};
use super::{bytes, cells, old_dirs, print_json, print_table, read_live, registry, remote, source_rows, Code, Error};
use crate::engine::state::{self, Environment};
use crate::engine::Step;
use crate::env::Env;
use crate::fetch::http::Http;
use crate::live::{Check, Remote};
use crate::product::{Product, Wanted};
use crate::regions::Regions;
use crate::sources::{self, State};
use crate::store::{import, Store};

#[derive(Args)]
pub struct StatusArgs {
    /// Also list the prefixes that live owns on R2, for drift and leftovers. Exit status 1 when
    /// it finds either.
    #[arg(long)]
    pub check: bool,
}

/// What `status` writes.
#[derive(Serialize, JsonSchema)]
pub struct Status {
    /// Where live was read: the bucket, or its public URL.
    pub from: String,
    pub products: Vec<ProductStatus>,
    pub attention: Vec<Attention>,
    /// Only with `--check`.
    pub check: Option<Check>,
}

#[derive(Serialize, JsonSchema)]
pub struct ProductStatus {
    pub product: String,
    /// The id of the live release; `None` when nothing is live.
    pub release: Option<String>,
    /// Each layer of the environment `live`, in dependency order; `None` when a fetch that its
    /// step list needs failed, and `attention` says why.
    pub layers: Option<Vec<LayerStatus>>,
}

#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub struct LayerStatus {
    pub layer: String,
    pub state: State,
    pub reason: Option<String>,
}

/// Something that needs a person.
#[derive(Serialize, JsonSchema)]
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
    /// A source that is blocked.
    Blocked,
    /// A cache directory of the older bake tools that `clean` moves into the store.
    OldCache,
    /// Keys that live uses and R2 lacks, or holds with another size.
    Drift,
    /// Keys under the owned prefixes that no live release uses.
    Leftovers,
    /// A fetch that the step list of a product needs failed, so its layer states are unknown.
    Unreachable,
}

impl AttentionKind {
    fn text(self) -> &'static str {
        match self {
            AttentionKind::Stale => "stale",
            AttentionKind::Blocked => "blocked",
            AttentionKind::OldCache => "old cache",
            AttentionKind::Drift => "drift",
            AttentionKind::Leftovers => "leftovers",
            AttentionKind::Unreachable => "unreachable",
        }
    }
}

pub fn status(root: &Path, products: &[&dyn Product], check: bool, json: bool) -> Result<ExitCode, Error> {
    let (store, registry, mut loaded) = (Store::open()?, registry(root)?, load(root, "live")?);
    let remote = remote()?;
    if check && matches!(remote, Remote::Public(_)) {
        let error = Code::Blocked.error("`--check` lists R2, and a listing needs `OBC_R2_BUCKET` and its key");
        return Err(error);
    }
    let live = read_live(&remote, &registry, products, &store)?;
    loaded.env.live = live.versions();
    let rows = source_rows(&registry, Some(&live.by_source()), false)?;
    let statuses = rows.iter().map(|row| {
        let status = sources::Status { state: row.state, reason: row.reason.clone(), age_days: row.age_days };
        (row.source.id.clone(), status)
    });
    let environment = Environment { sources: statuses.collect(), live: live.layers() };
    let http = Http::new();
    let fetch = fetcher(&store, &http, &loaded.sources, &loaded.env);
    let mut layers = layer_states(root, &store, products, &mut loaded.env, &loaded.regions, &environment, fetch)?;
    let mut attention = Vec::new();
    let products = live.products.iter().map(|product| {
        let release = product.release.as_ref().map(|(id, _)| id.clone());
        let layers = match layers.remove(&product.product).unwrap_or(Ok(Vec::new())) {
            Ok(layers) => Some(layers),
            Err(reason) => {
                let about = product.product.clone();
                attention.push(Attention { kind: AttentionKind::Unreachable, about, reason });
                None
            }
        };
        ProductStatus { product: product.product.clone(), release, layers }
    });
    let products: Vec<ProductStatus> = products.collect();
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
    for dir in import::plan(&store, &old_dirs()?)?.dirs.into_iter().filter(|dir| dir.files > 0) {
        let reason =
            format!("{} files, {}; `obc data clean --apply` moves them into the store", dir.files, bytes(dir.bytes));
        attention.push(Attention { kind: AttentionKind::OldCache, about: dir.dir.display().to_string(), reason });
    }
    let check = check.then(|| live.check(&remote)).transpose().map_err(|e| Code::R2Failed.error(e))?;
    if let Some(check) = &check {
        let about = "R2".to_string();
        if !check.drift.is_empty() {
            let reason = format!("{} of live missing or with another size", keys(check.drift.len()));
            attention.push(Attention { kind: AttentionKind::Drift, about: about.clone(), reason });
        }
        if !check.leftovers.is_empty() {
            let size = bytes(check.leftovers.iter().map(|object| object.bytes).sum());
            let reason = format!("{}, {size}, that no live release uses", keys(check.leftovers.len()));
            attention.push(Attention { kind: AttentionKind::Leftovers, about, reason });
        }
    }
    let problems = check.as_ref().is_some_and(|check| !check.drift.is_empty() || !check.leftovers.is_empty());
    let status = Status { from: remote.describe().into(), products, attention, check };
    if json {
        print_json(&status)?;
    } else {
        print_status(&status);
    }
    Ok(if problems { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// The state of each layer of `live`, by product. `Err` with the reason for a product that is
/// blocked, whose step list needs a fetch that fails, or that reads a layer of such a product: the
/// rest of `status` does not need its steps.
fn layer_states(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    env: &mut Env,
    regions: &Regions,
    environment: &Environment,
    mut fetch: impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<BTreeMap<String, Result<Vec<LayerStatus>, String>>, Error> {
    check_layers(products, env)?;
    let (mut listed, mut found, mut refused) = (Vec::new(), BTreeMap::new(), BTreeSet::new());
    for product in products {
        let steps = product_steps(*product, env, regions, store, &mut fetch);
        refused.extend(env.refused.borrow().iter().cloned());
        match steps {
            Ok(Ok(steps)) => {
                found.insert(product.name().to_string(), Ok(Vec::new()));
                listed.push((product.name().to_string(), steps));
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
    let steps: Vec<Step> = listed.into_iter().flat_map(|(_, steps)| steps).collect();
    for layer in state::state(store, root, &steps, environment)? {
        let product = layer.layer.split('/').next().unwrap_or_default().to_string();
        if let Some(Ok(layers)) = found.get_mut(&product) {
            layers.push(LayerStatus { layer: layer.layer, state: layer.state, reason: layer.reason });
        }
    }
    Ok(found)
}

/// A listed product that reads a layer of a product whose layers are unknown, with that reason.
fn reads_unknown(
    listed: &[(String, Vec<Step>)],
    found: &BTreeMap<String, Result<Vec<LayerStatus>, String>>,
) -> Option<(String, String)> {
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
        let Some(layers) = &product.layers else {
            table.push(vec![product.product.clone(), release, "unknown".into()]);
            continue;
        };
        let mut counts: Vec<(State, usize)> = Vec::new();
        for layer in layers {
            match counts.iter_mut().find(|(state, _)| *state == layer.state) {
                Some((_, n)) => *n += 1,
                None => counts.push((layer.state, 1)),
            }
        }
        let counts: Vec<String> = counts.into_iter().map(|(state, n)| format!("{n} {state}")).collect();
        table.push(vec![product.product.clone(), release, counts.join(" · ")]);
    }
    if !table.is_empty() {
        table.insert(0, cells(["PRODUCT", "RELEASE", "LAYERS"]));
        print_table(&table);
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
    let mut leftovers = BTreeMap::<&str, (usize, u64)>::new();
    for object in &check.leftovers {
        let prefix = object.key.split('/').next().unwrap_or_default();
        let (count, size) = leftovers.entry(prefix).or_default();
        (*count, *size) = (*count + 1, *size + object.bytes);
    }
    for (prefix, (count, size)) in leftovers {
        table.push(vec!["  leftovers".into(), format!("{prefix}/"), keys(count), bytes(size)]);
    }
    if table.is_empty() {
        println!("  live and R2 agree");
    }
    print_table(&table);
}

/// `1 key`, `2 keys`.
fn keys(count: usize) -> String {
    format!("{count} key{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::engine::tests::step;
    use crate::engine::{Code as StepCode, Input, Run};
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

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
            Ok(Vec::new())
        }
    }

    impl Product for Fetching {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
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

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
            let code = StepCode { paths: Vec::new(), crates: Vec::new() };
            let run = Run::Command(vec!["true".into()]);
            Ok(vec![step("reading/one", vec![Input::layer("test/one")], code, "out", run)])
        }
    }

    #[test]
    fn a_failed_fetch_makes_its_product_and_the_products_that_read_it_unknown() {
        let scratch = Scratch::new("status-unreachable");
        let store = Store::at(scratch.0.join("store"));
        let regions = Regions::new(Vec::new()).unwrap();
        let environment = Environment { sources: BTreeMap::new(), live: BTreeMap::new() };
        let states = |code: Code| {
            let fetch = |_: &Wanted| Err(code.error("GET https://example.org/index: unreachable"));
            let mut env = Env { name: "live".into(), region: "monaco".into(), ..Env::default() };
            let products: &[&dyn Product] = &[&Fetching, &Listed, &Reading];
            layer_states(&scratch.0, &store, products, &mut env, &regions, &environment, fetch)
        };
        let found = states(Code::FetchFailed).unwrap();
        let reason = "a fetch that the step list needs failed: GET https://example.org/index: unreachable";
        assert_eq!(found["test"], Err(reason.to_string()));
        assert_eq!(found["reading"], Err(reason.to_string()), "it reads a layer of `test`");
        assert_eq!(found["listed"], Ok(Vec::new()));
        assert!(states(Code::Failed).is_err(), "only a failed fetch is unknown layers");
    }
}
