//! `obc data`: what each target serves, how its layers stand against the steps of now, and the
//! command to run next.

use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use super::{cells, freshness, listing, print_json, print_table, Code, Error};
use crate::engine::runs::Limits;
use crate::engine::state::{Environment, LayerState, Verdict};
use crate::fetch::http::Http;
use crate::product::{BlockedLayer, Inputs, Product};
use crate::release::{self, Release, Serving, SERVING};
use crate::sources::{Registry, Source};
use crate::store::Store;
use crate::target::Target;

/// What `obc data` writes.
#[derive(Serialize, JsonSchema)]
pub struct Overview {
    pub machine: Machine,
    /// One per target of `data/targets.toml`, by name.
    pub targets: Vec<TargetState>,
}

/// The machine that runs the command.
#[derive(Serialize, JsonSchema)]
pub struct Machine {
    /// `OBC_DATA_MACHINE`, or else the short host name.
    pub name: String,
    /// The header colour of its page: `data/machines.toml`, or else the charcoal of a laptop.
    pub colour: String,
    /// `linux` or `macos`.
    pub os: String,
    pub cores: usize,
    /// `null` where the system does not tell.
    pub memory_bytes: Option<u64>,
    /// The free bytes of the disk of the store; `null` where the system does not tell.
    pub store_free_bytes: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
pub struct TargetState {
    pub target: String,
    /// `dir` or `r2`, as `data/targets.toml` says.
    pub kind: String,
    /// The folder of a dir target, or the public URL of an R2 bucket.
    pub at: String,
    /// What `release.json` of the target says; `null` while it serves nothing.
    pub release: Option<Serving>,
    /// The region of the serving release.
    pub region: Option<String>,
    /// Each layer of the steps and of the serving release; `null` while the target serves nothing
    /// or its steps cannot be listed.
    pub layers: Option<Vec<LayerState>>,
    /// The steps that cannot be planned.
    pub blocked: Vec<BlockedLayer>,
    /// The pointer keys whose bytes are not those of the serving release.
    pub drift: Vec<String>,
    /// Why the target or its steps could not be read; `null` when they could.
    pub error: Option<String>,
    /// The command to run next; `null` when the target serves its settings as the steps make them
    /// now, or when no command of this `obc data` writes the target: an R2 bucket.
    pub next: Option<String>,
}

pub fn overview(root: &Path, products: &[&dyn Product], json: bool) -> Result<(), Error> {
    let (store, http) = (Store::open()?, Http::new());
    let targets = crate::target::load(root, &store).map_err(|e| Code::InvalidData.error(e))?;
    let sources = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?.sources;
    let targets = targets.iter().map(|(name, target)| state(root, &store, &http, products, &sources, name, target));
    let limits = Limits::machine();
    let free = crate::store::free_bytes(store.root())?.map(|(_, free)| free);
    let name = crate::machine();
    let machine = Machine {
        colour: crate::machines::colour(root, &name).map_err(|e| Code::InvalidData.error(e))?,
        name,
        os: std::env::consts::OS.into(),
        cores: limits.jobs,
        memory_bytes: limits.memory_bytes,
        store_free_bytes: free,
    };
    let overview = Overview { machine, targets: targets.collect() };
    if json {
        return print_json(&overview);
    }
    for target in &overview.targets {
        println!("{}  {}", target.target, target.at);
        let mut rows = Vec::new();
        match &target.release {
            None => rows.push(cells(["  serves", "nothing"])),
            Some(serving) => {
                let role = |role: &release::Role| format!("{} since {}", &role.id[..12], role.since);
                rows.push(vec!["  serves".into(), role(&serving.serving)]);
                if let Some(rollback) = &serving.rollback {
                    rows.push(vec!["  rollback".into(), role(rollback)]);
                }
            }
        }
        if let Some(region) = &target.region {
            rows.push(vec!["  region".into(), region.clone()]);
        }
        for layer in target.layers.iter().flatten().filter(|layer| layer.state == Verdict::Stale) {
            rows.push(vec![format!("  stale {}", layer.layer), layer.text.clone().unwrap_or_default()]);
        }
        for blocked in &target.blocked {
            rows.push(vec![format!("  blocked {}", blocked.layer), blocked.reason.clone()]);
        }
        for key in &target.drift {
            rows.push(vec!["  drift".into(), key.clone()]);
        }
        if let Some(error) = &target.error {
            rows.push(vec!["  error".into(), error.clone()]);
        }
        let next = match (&target.next, target.kind == "r2") {
            (Some(next), _) => next.clone(),
            (None, true) => "a later `obc data` writes an R2 bucket".into(),
            (None, false) => "nothing to do".into(),
        };
        rows.push(vec!["  next".into(), next]);
        print_table(&rows);
    }
    Ok(())
}

fn state(
    root: &Path,
    store: &Store,
    http: &Http,
    products: &[&dyn Product],
    sources: &[Source],
    name: &str,
    target: &Target,
) -> TargetState {
    let (kind, at) = match target {
        Target::Dir(dir) => ("dir", dir.display().to_string()),
        Target::R2 { public } => ("r2", public.clone()),
    };
    let mut state = TargetState {
        target: name.into(),
        kind: kind.into(),
        at,
        release: None,
        region: None,
        layers: None,
        blocked: Vec::new(),
        drift: Vec::new(),
        error: None,
        next: None,
    };
    let writes = kind == "dir";
    let judged = (|| -> Result<(), Error> {
        let Some(bytes) = target.get(SERVING)? else {
            state.next = writes.then(|| format!("obc data update {name} --region ID"));
            return Ok(());
        };
        let serving: Serving = serde_json::from_slice(&bytes).map_err(|e| format!("{SERVING}: {e}"))?;
        let release = Release::read(target, &serving.serving.id)?;
        (state.release, state.region) = (Some(serving), Some(release.settings.region.clone()));
        state.drift = release::drift(target, &release)?;
        let regions = release.settings.regions()?;
        let mut sources = sources.to_vec();
        release.settings.policies(&mut sources)?;
        let mut inputs =
            Inputs { settings: release.settings.clone(), serving: release.versions(), ..Inputs::default() };
        let fetch = listing::fetcher(root, store, http, &sources, None, false);
        let listed = listing::steps(root, products, &mut inputs, &regions, store, true, fetch)?;
        state.blocked = listed.blocked;
        let newer = freshness::newer(store, http, &sources, &inputs.requests.borrow(), &inputs.serving);
        let outputs = release.outputs.into_iter().map(|output| (output.step.clone(), output)).collect();
        let layers = crate::engine::state::state(store, &listed.steps, &Environment { release: outputs, newer })?;
        let stale = layers.iter().any(|layer| layer.state == Verdict::Stale);
        state.next = (writes && (stale || !state.drift.is_empty())).then(|| format!("obc data update {name}"));
        state.layers = Some(layers);
        Ok(())
    })();
    if let Err(error) = judged {
        state.error = Some(error.message);
    }
    state
}
