//! The review of an update: what it bakes and drops and why, which sources move, what this machine
//! fetches and bakes, what that costs, whether this machine can do it, and what the target serves
//! after it. Its id names the decisions, so `--expect ID` starts an update only when they are the
//! ones that a person read.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;

use crate::cli::{print_json, print_table, Code, Error};
use crate::engine::plan::{Build, Cause, Fetch, Kept, Plan};
use crate::engine::runs::Limits;
use crate::engine::state::{LayerState, Verdict};
use crate::engine::{Input, Shape, Step};
use crate::product::{RequestKey, Versions};
use crate::release::{Release, Serving};
use crate::settings::Settings;
use crate::store::{sha256_hex, sorted, Store, RESERVE};
use crate::target::Tag;

/// The review of an update.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Review {
    /// The first 12 hex digits of the SHA-256 of the decisions: the tag of `release.json`, the
    /// settings, `layers`, every step that bakes and the source version that each of them reads.
    pub id: String,
    pub target: String,
    /// The release that the target serves; `null` while it serves nothing.
    pub base: Option<String>,
    pub settings: Settings,
    /// Each layer that the update bakes or drops, in dependency order. A layer that only other
    /// layers read is not listed: the layers that read it are.
    pub layers: Vec<LayerChange>,
    /// Each fetch that a baked step reads at another version than the serving release.
    pub sources: Vec<SourceMove>,
    /// The number of outputs that the release keeps by reference.
    pub kept: usize,
    /// What this machine fetches into its store.
    pub fetches: Vec<Fetch>,
    /// The steps that this machine bakes. A step whose output is in the store already is not here.
    pub bakes: Vec<Build>,
    pub cost: ReviewCost,
    pub machine: MachineCheck,
    pub after: AfterUpdate,
    /// The command that starts the update of this review.
    pub argv: Vec<String>,
}

/// A layer that the update changes.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LayerChange {
    pub layer: String,
    pub cause: Cause,
    pub text: String,
    /// The steps that bake.
    pub steps: Vec<String>,
    /// The grid leaves of a tiled layer whose steps bake, as `<i>-<j>`.
    pub leaves: Vec<String>,
    /// The steps of the serving release that no step makes now.
    pub drops: Vec<String>,
    /// The programs that read the layer.
    pub readers: Vec<String>,
    /// The format of the serving release and of the steps, when it changes. Old readers keep what
    /// they have and get no new data until they update.
    pub format: Option<FormatChange>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
pub struct FormatChange {
    pub from: u32,
    pub to: u32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceMove {
    pub source: String,
    pub params: Vec<(String, String)>,
    /// `null` when the serving release does not read the fetch.
    pub from: Option<String>,
    pub to: String,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ReviewCost {
    /// The estimated times of the bakes, added as if one ran after another.
    pub wall_ms: u64,
    /// The highest estimated peak of one bake.
    pub peak_rss_bytes: Option<u64>,
    /// What the fetches and the bakes write into the store; the upload is the client files of it.
    pub store_bytes: u64,
    /// The bakes without an estimate: the store has no receipt of the step, and the serving release
    /// no cost.
    pub unknown: Vec<String>,
}

/// This machine against the cost.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct MachineCheck {
    /// `null` where the system does not tell.
    pub memory_bytes: Option<u64>,
    /// The free bytes of the disk of the store; `null` where the system does not tell.
    pub store_free_bytes: Option<u64>,
    /// Why this machine cannot do the update; empty when it can, or cannot tell.
    pub refusals: Vec<String>,
}

impl MachineCheck {
    /// This machine: its memory, and the free bytes of the disk of `store`.
    pub fn this(store: &Store) -> Result<Self, String> {
        let free = crate::store::free_bytes(store.root())?.map(|(_, free)| free);
        Ok(MachineCheck { memory_bytes: Limits::machine().memory_bytes, store_free_bytes: free, refusals: Vec::new() })
    }
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct AfterUpdate {
    /// The release that becomes the rollback; `null` when the update changes no layer or the target
    /// serves nothing.
    pub rollback: Option<String>,
    /// The rollback of now, which the collection after the switch drops.
    pub collects: Option<Collects>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Collects {
    pub release: String,
    /// The keys of the release that the serving release does not name, and their size: what the
    /// collection deletes at most.
    pub keys: u64,
    pub bytes: u64,
}

/// What [`review`] reads.
pub struct Parts<'a> {
    pub target: &'a str,
    pub base: Option<&'a (Serving, Tag)>,
    pub serving: Option<&'a Release>,
    /// The rollback release of the base.
    pub rollback: Option<&'a Release>,
    pub settings: &'a Settings,
    pub steps: &'a [Step],
    pub only: &'a [String],
    pub plan: &'a Plan,
    pub kept: &'a Kept,
    pub layers: &'a [LayerState],
    pub machine: &'a MachineCheck,
}

/// The review of an update from `parts`.
pub fn review(parts: Parts) -> Review {
    let Parts { target, base, serving, rollback, settings, steps, only, plan, kept, layers, machine } = parts;
    let outputs: BTreeMap<&str, &crate::release::Output> =
        serving.iter().flat_map(|release| &release.outputs).map(|output| (output.step.as_str(), output)).collect();
    let baked: Vec<&Step> = steps.iter().filter(|step| !kept.contains_key(&step.name)).collect();
    let mut changes = Vec::new();
    for state in layers {
        let mine: Vec<&&Step> = baked.iter().filter(|step| step.layer.name == state.layer).collect();
        let drops: Vec<String> = outputs
            .values()
            .filter(|output| output.layer == state.layer && !steps.iter().any(|step| step.name == output.step))
            .map(|output| output.step.clone())
            .collect();
        if mine.is_empty() && drops.is_empty() {
            continue;
        }
        let cause = match (&state.cause, state.state) {
            _ if !mine.is_empty() && only.contains(&state.layer) => Cause::Named,
            (Some(cause), Verdict::Stale) => Cause::Stale { cause: cause.clone() },
            _ if mine.is_empty() => Cause::Off,
            _ => Cause::Named,
        };
        let format = mine.iter().find_map(|step| {
            let served = outputs.get(step.name.as_str()).filter(|served| served.layer == step.layer.name)?;
            (served.format != step.layer.format).then_some(FormatChange { from: served.format, to: step.layer.format })
        });
        let tiled = mine.iter().filter(|step| step.layer.shape == Shape::Tiled);
        let leaves: BTreeSet<&str> =
            tiled.filter_map(|step| step.name.rsplit_once('/')).map(|(_, leaf)| leaf).collect();
        let readers =
            steps.iter().find(|step| step.layer.name == state.layer).map_or(&[][..], |step| step.layer.readers);
        changes.push(LayerChange {
            layer: state.layer.clone(),
            text: text(&cause),
            cause,
            steps: mine.iter().map(|step| step.name.clone()).collect(),
            leaves: leaves.into_iter().map(str::to_string).collect(),
            drops,
            readers: readers.iter().map(|reader| reader.to_string()).collect(),
            format,
        });
    }
    // Each fetch that a baked step reads, with its version.
    let reads: BTreeSet<(RequestKey, &str)> = baked
        .iter()
        .flat_map(|step| &step.inputs)
        .filter_map(|input| match input {
            Input::Snapshot { source, version, params, .. } => {
                Some(((source.clone(), sorted(params)), version.as_str()))
            }
            Input::Step { .. } => None,
        })
        .collect();
    let versions: Versions = serving.map(Release::versions).unwrap_or_default();
    let sources = reads
        .iter()
        .filter(|(key, version)| versions.get(key).map(String::as_str) != Some(*version))
        .map(|((source, params), version)| SourceMove {
            source: source.clone(),
            params: params.clone(),
            from: versions.get(&(source.clone(), params.clone())).cloned(),
            to: version.to_string(),
        })
        .collect();
    let decisions = serde_json::json!({
        "base": base.map(|(_, tag)| &tag.0),
        "settings": settings,
        "layers": &changes,
        "bakes": baked.iter().map(|step| &step.name).collect::<Vec<_>>(),
        "reads": reads,
    });
    let id = sha256_hex(&serde_json::to_vec(&decisions).expect("the decisions are JSON"))[..12].to_string();

    let mut bakes: Vec<Build> = Vec::new();
    for build in plan.builds() {
        if !bakes.iter().any(|known| known.step == build.step) {
            bakes.push(build.clone());
        }
    }
    let fetches = plan.fetches();
    let mut cost =
        ReviewCost { store_bytes: fetches.iter().filter_map(|fetch| fetch.bytes).sum(), ..ReviewCost::default() };
    for build in &bakes {
        match build.estimate {
            Some(estimate) => {
                cost.wall_ms += estimate.wall_ms;
                cost.store_bytes += estimate.bytes_out;
                cost.peak_rss_bytes = cost.peak_rss_bytes.max(estimate.peak_rss_bytes);
            }
            None => cost.unknown.push(build.step.clone()),
        }
    }
    let mut machine = machine.clone();
    machine.refusals = refusals(&machine, &bakes, cost.store_bytes);

    let changes_settings = serving.is_some_and(|serving| serving.settings != *settings);
    let switches = !changes.is_empty() || changes_settings;
    let after = AfterUpdate {
        rollback: base.filter(|_| switches).map(|(base, _)| base.serving.id.clone()),
        collects: rollback.filter(|_| switches).map(|rollback| {
            let named: BTreeSet<String> =
                serving.iter().flat_map(|release| release.files()).map(|file| file.key).collect();
            let only_rollback = rollback.files().into_iter().filter(|file| !named.contains(&file.key));
            let (keys, bytes) = only_rollback.fold((1, 0), |(keys, bytes), file| (keys + 1, bytes + file.size));
            Collects { release: rollback.id(), keys, bytes }
        }),
    };
    Review {
        id,
        target: target.into(),
        base: base.map(|(base, _)| base.serving.id.clone()),
        settings: settings.clone(),
        layers: changes,
        sources,
        kept: kept.len(),
        fetches,
        bakes,
        cost,
        machine,
        after,
        argv: Vec::new(),
    }
}

/// Why `machine` cannot bake `bakes` and write `store_bytes`.
fn refusals(machine: &MachineCheck, bakes: &[Build], store_bytes: u64) -> Vec<String> {
    let mut refusals = Vec::new();
    if let Some(memory) = machine.memory_bytes {
        for build in bakes {
            if let Some(peak) =
                build.estimate.and_then(|estimate| estimate.peak_rss_bytes).filter(|peak| *peak > memory)
            {
                refusals.push(format!(
                    "step `{}` needs about {} of memory, and this machine has {}",
                    build.step,
                    size(peak),
                    size(memory)
                ));
            }
        }
    }
    if let Some(free) = machine.store_free_bytes.filter(|free| *free < store_bytes.saturating_add(RESERVE)) {
        refusals.push(format!(
            "the store gets about {}, and its disk has {} free and keeps {} free",
            size(store_bytes),
            size(free),
            size(RESERVE)
        ));
    }
    refusals
}

/// Refuse an update whose review is not `expect`, that changes a format without
/// `accept_format_change`, or that this machine cannot do.
pub fn check(review: &Review, expect: Option<&str>, accept_format_change: bool) -> Result<(), Error> {
    if let Some(expect) = expect.filter(|expect| *expect != review.id) {
        return Err(Code::Usage
            .error(format!(
                "the review of `{}` is {}, not {expect}: it changed; nothing changed",
                review.target, review.id
            ))
            .fix("Read the review again: run the same command with `--dry-run` and without `--expect`."));
    }
    let formats: Vec<String> = review
        .layers
        .iter()
        .filter_map(|layer| {
            let FormatChange { from, to } = layer.format?;
            Some(format!("{} {from} → {to} (read by {})", layer.layer, layer.readers.join(", ")))
        })
        .collect();
    if !formats.is_empty() && !accept_format_change {
        return Err(Code::Usage
            .error(format!(
                "the update changes the format of {}: old readers keep what they have and get no new data until they \
                 update; nothing changed",
                formats.join("; ")
            ))
            .fix("Start it with `--accept-format-change` when the readers can read the new format."));
    }
    if !review.machine.refusals.is_empty() {
        return Err(Code::Blocked
            .error(format!("this machine cannot do the update: {}; nothing changed", review.machine.refusals.join("; ")))
            .fix("Run the update on a machine with more memory or disk, such as the bake VPS, or release space with `obc data clean`."));
    }
    Ok(())
}

/// Write `review` to standard output.
pub fn print(review: &Review, json: bool) -> Result<(), Error> {
    if json {
        return print_json(review);
    }
    let short = |id: &str| id[..12].to_string();
    println!("review {} of {}", review.id, review.target);
    let mut rows = Vec::new();
    for layer in &review.layers {
        let verb = if layer.steps.is_empty() { "drop" } else { "bake" };
        let mut text = layer.text.clone();
        if !layer.leaves.is_empty() {
            text += &format!("; {} leaves", layer.leaves.len());
        }
        if !layer.readers.is_empty() {
            text += &format!("; read by {}", layer.readers.join(", "));
        }
        rows.push(vec![format!("  {verb}"), layer.layer.clone(), text]);
    }
    for source in &review.sources {
        let params: Vec<String> = source.params.iter().map(|(name, value)| format!("{name}={value}")).collect();
        let from = source.from.as_deref().unwrap_or("new");
        rows.push(vec![
            "  source".into(),
            format!("{} {}", source.source, params.join(" ")).trim().into(),
            format!("{from} → {}", source.to),
        ]);
    }
    rows.push(vec!["  keep".into(), format!("{} outputs by reference", review.kept)]);
    let cost = &review.cost;
    let mut text = format!(
        "{} fetches, {} bakes: about {} min, {} into the store",
        review.fetches.len(),
        review.bakes.len(),
        cost.wall_ms.div_ceil(60_000),
        size(cost.store_bytes)
    );
    if let Some(peak) = cost.peak_rss_bytes {
        text += &format!(", peak {} memory", size(peak));
    }
    if !cost.unknown.is_empty() {
        text += &format!("; no estimate for {}", cost.unknown.join(", "));
    }
    rows.push(vec!["  cost".into(), text]);
    let machine = match review.machine.refusals.as_slice() {
        [] => "this machine can do it".to_string(),
        refusals => format!("this machine cannot do it: {}", refusals.join("; ")),
    };
    rows.push(vec!["  machine".into(), machine]);
    let mut after = match &review.after.rollback {
        Some(rollback) => format!("a new release; rollback {}", short(rollback)),
        None if review.layers.is_empty() && review.base.is_some() => "the serving release".into(),
        None => "a new release".into(),
    };
    if let Some(collects) = &review.after.collects {
        after += &format!(
            "; the collection drops {}: up to {} keys, {}",
            short(&collects.release),
            collects.keys,
            size(collects.bytes)
        );
    }
    rows.push(vec!["  after".into(), after]);
    rows.push(vec!["  start".into(), review.argv.join(" ")]);
    print_table(&rows);
    Ok(())
}

fn text(cause: &Cause) -> String {
    match cause {
        Cause::Stale { cause } => cause.text(),
        Cause::Named => "named by --only".into(),
        Cause::Off => "no step makes it".into(),
    }
}

/// `bytes` in GiB or MiB.
fn size(bytes: u64) -> String {
    match bytes {
        bytes if bytes >= 1 << 30 => format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64),
        bytes => format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64),
    }
}
