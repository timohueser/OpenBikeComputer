//! `obc data status`: what is live, the state of its layers, and what needs attention. `--check`
//! also lists the prefixes that live owns on R2.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::build_cli::{fetcher, load, steps, Loaded};
use super::{bytes, cells, old_dirs, print_json, print_table, read_live, registry, source_rows, Code, Error};
use crate::engine::state::{self, Environment, LayerState};
use crate::fetch::http::Http;
use crate::live::{Check, Remote};
use crate::product::{Product, Wanted};
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
    /// Each layer of the environment `live`, in dependency order; `None` when its steps cannot
    /// be listed, and `attention` says why.
    pub layers: Option<Vec<LayerStatus>>,
}

#[derive(Serialize, JsonSchema)]
pub struct LayerStatus {
    pub layer: String,
    pub state: State,
    pub reason: Option<String>,
}

/// Something that needs a person.
#[derive(Serialize, JsonSchema)]
pub struct Attention {
    pub kind: AttentionKind,
    /// The source, the directory, or where live was read.
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
    /// A fetch that the step list needs failed, so the layer states are unknown.
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
    let (store, registry, loaded) = (Store::open()?, registry(root)?, load(root, "live")?);
    let (remote, live) = read_live(&registry, products, &store)?;
    if check && matches!(remote, Remote::Public(_)) {
        let error = Code::Blocked.error("`--check` lists R2, and a listing needs the OBC_R2_* variables");
        return Err(error);
    }
    let rows = source_rows(&registry, false)?;
    let statuses = rows.iter().map(|row| {
        let status = sources::Status { state: row.state, reason: row.reason.clone(), age_days: row.age_days };
        (row.source.id.clone(), status)
    });
    let environment = Environment { sources: statuses.collect(), live: live.layers() };
    let http = Http::new();
    let layers = layer_states(root, &store, products, &loaded, &environment, fetcher(&store, &http, &loaded.sources))?;
    let products = live.products.iter().map(|product| {
        let prefix = format!("{}/", product.product);
        let layers = layers.as_ref().ok().map(|layers| {
            let layers = layers.iter().filter(|layer| layer.layer.starts_with(&prefix));
            layers.map(|l| LayerStatus { layer: l.layer.clone(), state: l.state, reason: l.reason.clone() }).collect()
        });
        let release = product.release.as_ref().map(|(id, _)| id.clone());
        ProductStatus { product: product.product.clone(), release, layers }
    });

    let mut attention = Vec::new();
    if let Err(reason) = &layers {
        let about = "upstream".to_string();
        attention.push(Attention { kind: AttentionKind::Unreachable, about, reason: reason.clone() });
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
    for dir in import::waiting(&store, &old_dirs()?)? {
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
    let status = Status { from: remote.describe().into(), products: products.collect(), attention, check };
    if json {
        print_json(&status)?;
    } else {
        print_status(&status);
    }
    Ok(if problems { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// The state of each layer of `live`. `Err` with the reason when a fetch that a step list needs
/// fails: the rest of `status` does not need the steps.
fn layer_states(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    loaded: &Loaded,
    environment: &Environment,
    fetch: impl FnMut(&Wanted) -> Result<(), Error>,
) -> Result<Result<Vec<LayerState>, String>, Error> {
    match steps(products, &loaded.env, &loaded.regions, store, fetch) {
        Ok(steps) => Ok(Ok(state::state(store, root, &steps, environment)?)),
        Err(e) if matches!(e.code, Code::FetchFailed | Code::Blocked) => {
            Ok(Err(format!("upstream not reachable: {}", e.message)))
        }
        Err(e) => Err(e),
    }
}

fn print_status(status: &Status) {
    println!("LIVE from {}", status.from);
    let mut table = Vec::new();
    for product in &status.products {
        let release = product.release.as_ref().map_or("nothing live".into(), |id| format!("release {}", &id[..8]));
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
    use crate::engine::Step;
    use crate::env::Env;
    use crate::product::Unplanned;
    use crate::regions::Regions;
    use crate::store::tests::Scratch;

    /// A product whose step list always needs a fetch.
    struct Fetching;

    impl Product for Fetching {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
            let wanted = Wanted { source: "index".into(), version: None, params: Vec::new() };
            Err(Unplanned::NeedsFetch(vec![wanted]))
        }
    }

    #[test]
    fn a_failed_fetch_makes_the_layer_states_unknown_and_nothing_else_fails() {
        let scratch = Scratch::new("status-unreachable");
        let store = Store::at(scratch.0.join("store"));
        let env = Env { name: "live".into(), region: "monaco".into(), layers: Vec::new(), pins: BTreeMap::new() };
        let loaded = Loaded { env, sources: Vec::new(), regions: Regions::new(Vec::new()).unwrap() };
        let environment = Environment { sources: BTreeMap::new(), live: BTreeMap::new() };
        let states = |code: Code| {
            let fetch = |_: &Wanted| Err(code.error("GET https://example.org/index: unreachable"));
            layer_states(&scratch.0, &store, &[&Fetching], &loaded, &environment, fetch)
        };
        let reason = states(Code::FetchFailed).unwrap().unwrap_err();
        assert_eq!(reason, "upstream not reachable: GET https://example.org/index: unreachable");
        assert!(states(Code::Failed).is_err(), "only a failed fetch is unknown layers");
    }
}
