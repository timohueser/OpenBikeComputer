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
use crate::engine::state::{self, Field, LayerState, Verdict};
use crate::engine::{leaf, partition, Input, Reader, Run, Shape, Step};
use crate::product::{Note, RequestKey, Versions};
use crate::release::{Release, Serving};
use crate::settings::Settings;
use crate::store::{sha256_hex, sorted, Store, RESERVE};
use crate::target::Tag;

/// The review of an update.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Review {
    /// The first 12 hex digits of the SHA-256 of the canonical JSON of the decisions: the target,
    /// the tag of `release.json`, the settings, `layers`, every step that bakes and the source version
    /// that each of them reads. Two machines that read the same target give the same id when their
    /// stores resolve the same versions and files.
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
    /// What this machine downloads from the target: copies of source files, and files of kept steps
    /// that a step that bakes reads.
    pub downloads: Downloads,
    pub cost: ReviewCost,
    pub machine: MachineCheck,
    pub after: AfterUpdate,
    /// The arguments after `obc data` that start the update of this review.
    pub args: Vec<String>,
    /// The optional layers that `--add` and `--remove` name, sorted; `settings.layers` holds those
    /// that are on.
    pub optional: Vec<String>,
    /// What layers bake without, such as a prepared source that no target lists yet.
    pub notes: Vec<Note>,
}

/// A layer that the update changes.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LayerChange {
    pub layer: String,
    pub cause: Cause,
    pub text: String,
    /// The steps that bake.
    pub steps: Vec<String>,
    /// The grid leaves whose steps bake, as `<i>-<j>`: the last part of the name of each step of a
    /// tiled layer that has that form.
    pub leaves: Vec<String>,
    /// The partitions of a partitioned layer whose steps bake.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub partitions: Vec<String>,
    /// The steps of the serving release that no step makes now.
    pub drops: Vec<String>,
    /// The grid leaves of `drops` of a tiled layer, as `leaves` names them.
    pub dropped_leaves: Vec<String>,
    /// The programs that read the layer.
    pub readers: Vec<String>,
    /// The format of the serving release and of the steps, when it changes. Old readers keep what
    /// they have and get no new data until they update.
    pub format: Option<FormatChange>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FormatChange {
    pub from: u32,
    pub to: u32,
    /// What the new format means for each reader of the layer.
    pub readers: Vec<FormatReader>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FormatReader {
    pub reader: String,
    pub effect: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceMove {
    pub source: String,
    pub params: Vec<(String, String)>,
    /// `null` when the serving release does not read the fetch.
    pub from: Option<String>,
    pub to: String,
}

#[derive(Debug, Default, Clone, Copy, Serialize, JsonSchema)]
pub struct Downloads {
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ReviewCost {
    /// The estimated times of the bakes, added as if one ran after another.
    pub wall_ms: u64,
    /// The highest estimated peak of one bake that runs in its own process.
    pub peak_rss_bytes: Option<u64>,
    /// The highest peak of the `obc` process in the estimates of the bakes that run in it: an upper
    /// bound, not the peak of a step. It refuses nothing.
    pub process_peak_rss_bytes: Option<u64>,
    /// What the fetches, the downloads and the bakes write into the store.
    pub store_bytes: u64,
    /// What goes to the target: the files of the bakes that a client reads, and the copies under
    /// `sources/` that the serving release does not name.
    pub upload_bytes: u64,
    /// What the figures lack.
    pub unknown: Unknown,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Unknown {
    /// The bakes without an estimate.
    pub estimate: Vec<String>,
    /// The bakes whose estimate has no peak.
    pub peak: Vec<String>,
    /// The fetches whose size is not known, as `SOURCE@VERSION`.
    pub size: Vec<String>,
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
    /// The tools that the planner steps need and this machine lacks, as `obc data setup` names them.
    #[serde(skip)]
    pub planner_missing: Vec<String>,
    /// The Python groups that the bakes need and this machine lacks.
    #[serde(skip)]
    pub python_missing: Vec<String>,
}

impl MachineCheck {
    /// This machine: its memory, and the free bytes of the disk of `store`.
    pub fn this(store: &Store) -> Result<Self, String> {
        let free = crate::store::free_bytes(store.root())?.map(|(_, free)| free);
        Ok(MachineCheck {
            memory_bytes: Limits::machine().memory_bytes,
            store_free_bytes: free,
            refusals: Vec::new(),
            planner_missing: crate::cli::setup_cli::planner_missing(),
            python_missing: Vec::new(),
        })
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
    /// The steps that bake only because the target does not hold files of them.
    pub unheld: &'a BTreeSet<String>,
    pub downloads: Downloads,
    /// The size of the copies under `sources/` that the update adds, and the fetches of copies
    /// that the store lacks yet.
    pub copies: (u64, BTreeSet<String>),
    pub machine: &'a MachineCheck,
}

/// The review of an update from `parts`.
pub fn review(parts: Parts) -> Review {
    let Parts {
        target,
        base,
        serving,
        rollback,
        settings,
        steps,
        only,
        plan,
        kept,
        layers,
        unheld,
        downloads,
        copies,
        machine,
    } = parts;
    let outputs = serving.iter().flat_map(|release| &release.outputs);
    let baked: Vec<&Step> = steps.iter().filter(|step| !kept.contains_key(&step.name)).collect();
    let layer_of: BTreeMap<&str, &str> = baked.iter().map(|step| (step.name.as_str(), step.layer.name)).collect();
    let mut changes = Vec::new();
    for state in layers {
        let mine: Vec<&&Step> = baked.iter().filter(|step| step.layer.name == state.layer).collect();
        let drops: Vec<String> = outputs
            .clone()
            .filter(|output| output.layer == state.layer && !steps.iter().any(|step| step.name == output.step))
            .map(|output| output.step.clone())
            .collect();
        if mine.is_empty() && drops.is_empty() {
            continue;
        }
        // A layer that bakes because a layer that it reads bakes follows that layer.
        let read = mine.iter().flat_map(|step| step.reads());
        let follows = read.filter_map(|name| layer_of.get(name).copied());
        let cause = match (&state.cause, state.state) {
            _ if !mine.is_empty() && only.contains(&state.layer) => Cause::Named,
            (Some(cause), Verdict::Stale) => Cause::Stale { cause: cause.clone() },
            _ if mine.is_empty() => Cause::Off,
            // Without `--only`, a layer that is not stale bakes only for files that the target does
            // not hold.
            _ if only.is_empty() => Cause::Unheld,
            _ => match follows.into_iter().find(|layer| *layer != state.layer) {
                Some(layer) => Cause::Stale { cause: state::Cause::Input { name: layer.into() } },
                None => Cause::Named,
            },
        };
        let readers =
            steps.iter().find(|step| step.layer.name == state.layer).map_or(&[][..], |step| step.layer.readers);
        let format = match &state.cause {
            Some(state::Cause::Version { field: Field::Format, from: Some(from), to }) if !mine.is_empty() => {
                let effect =
                    |reader: &&Reader| FormatReader { reader: reader.name.into(), effect: reader.on_new_format.into() };
                Some(FormatChange { from: *from, to: *to, readers: readers.iter().map(effect).collect() })
            }
            _ => None,
        };
        let tiled = mine.iter().filter(|step| step.layer.shape == Shape::Tiled);
        let leaves: BTreeSet<&str> = tiled.filter_map(|step| leaf(&step.name)).collect();
        let partitions: BTreeSet<&str> = mine.iter().filter_map(|step| partition(step)).collect();
        changes.push(LayerChange {
            layer: state.layer.clone(),
            text: text(&cause),
            cause,
            steps: mine.iter().map(|step| step.name.clone()).collect(),
            leaves: leaves.into_iter().map(str::to_string).collect(),
            partitions: partitions.into_iter().map(str::to_string).collect(),
            dropped_leaves: match state.shape {
                Some(state::LayerShape::Tiled) => {
                    drops.iter().filter_map(|step| leaf(step)).map(str::to_string).collect()
                }
                _ => Vec::new(),
            },
            drops,
            readers: readers.iter().map(|reader| reader.name.to_string()).collect(),
            format,
        });
    }
    // A layer that only other layers read is listed when it bakes only for files that the target
    // does not hold.
    for step in steps.iter().filter(|step| unheld.contains(&step.name)) {
        let leaf = leaf(&step.name).filter(|_| step.layer.shape == Shape::Tiled).map(str::to_string);
        let part = partition(step).map(str::to_string);
        match changes.iter_mut().find(|change| change.layer == step.layer.name) {
            Some(change) if !change.steps.contains(&step.name) => {
                change.steps.push(step.name.clone());
                change.leaves.extend(leaf.filter(|leaf| !change.leaves.contains(leaf)));
                change.leaves.sort();
                change.partitions.extend(part.filter(|part| !change.partitions.contains(part)));
                change.partitions.sort();
            }
            Some(_) => {}
            None => changes.push(LayerChange {
                layer: step.layer.name.into(),
                cause: Cause::Unheld,
                text: text(&Cause::Unheld),
                steps: vec![step.name.clone()],
                leaves: leaf.into_iter().collect(),
                partitions: part.into_iter().collect(),
                drops: Vec::new(),
                dropped_leaves: Vec::new(),
                readers: Vec::new(),
                format: None,
            }),
        }
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
        "target": target,
        "base": base.map(|(_, tag)| &tag.0),
        "settings": settings,
        "layers": &changes,
        "bakes": baked.iter().map(|step| &step.name).collect::<Vec<_>>(),
        "reads": reads,
    });
    let canonical = serde_json::to_vec(&crate::engine::sorted(decisions)).expect("the decisions are JSON");
    let id = sha256_hex(&canonical)[..12].to_string();

    let mut seen = BTreeSet::new();
    let bakes: Vec<Build> = plan.builds().filter(|build| seen.insert(&build.step)).cloned().collect();
    let fetches = plan.fetches();
    let (upload_bytes, unknown_copies) = copies;
    let mut cost = ReviewCost { upload_bytes, ..ReviewCost::default() };
    for fetch in &fetches {
        match fetch.bytes {
            Some(bytes) => cost.store_bytes += bytes,
            None => cost.unknown.size.push(format!("{}@{}", fetch.source, fetch.version)),
        }
    }
    // The size of a copy is known only after its fetch.
    for fetch in unknown_copies.into_iter().filter(|fetch| !cost.unknown.size.contains(fetch)).collect::<Vec<_>>() {
        cost.unknown.size.push(fetch);
    }
    cost.store_bytes += downloads.bytes;
    let in_process: BTreeSet<&str> =
        steps.iter().filter(|step| matches!(step.run, Run::Rust(_))).map(|step| step.name.as_str()).collect();
    for build in &bakes {
        let Some(estimate) = build.estimate else {
            cost.unknown.estimate.push(build.step.clone());
            continue;
        };
        cost.wall_ms += estimate.wall_ms;
        cost.store_bytes += estimate.bytes_out;
        cost.upload_bytes += estimate.client_bytes;
        let peak = match in_process.contains(build.step.as_str()) {
            true => &mut cost.process_peak_rss_bytes,
            false => &mut cost.peak_rss_bytes,
        };
        match estimate.peak_rss_bytes {
            Some(bytes) => *peak = (*peak).max(Some(bytes)),
            None => cost.unknown.peak.push(build.step.clone()),
        }
    }
    let own: Vec<&Build> = bakes.iter().filter(|build| !in_process.contains(build.step.as_str())).collect();
    let mut machine = machine.clone();
    let planner = bakes.iter().any(|build| build.step.starts_with("planner/"));
    machine.refusals = refusals(&machine, (&own, planner), cost.store_bytes);

    let changes_settings = serving.is_some_and(|serving| serving.settings != *settings);
    let switches = !changes.is_empty() || changes_settings;
    let after = AfterUpdate {
        rollback: base.filter(|_| switches).map(|(base, _)| base.serving.id.clone()),
        collects: rollback.filter(|_| switches).map(|rollback| {
            let named: BTreeSet<String> =
                serving.iter().flat_map(|release| release.files()).map(|file| file.key).collect();
            let only_rollback = rollback.files().into_iter().filter(|file| !named.contains(&file.key));
            // The manifest is a key too; its size is not counted.
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
        downloads,
        cost,
        machine,
        after,
        args: Vec::new(),
        optional: Vec::new(),
        notes: Vec::new(),
    }
}

/// Why `machine` cannot bake `own`, the bakes that run in their own process, and write
/// `store_bytes`; with `planner`, also bake planner steps. Only a child process has a peak of its
/// own (`wait4`), so only such a bake can need more memory than the machine has.
fn refusals(machine: &MachineCheck, (own, planner): (&[&Build], bool), store_bytes: u64) -> Vec<String> {
    let mut refusals = Vec::new();
    if planner && !machine.planner_missing.is_empty() {
        refusals.push(format!(
            "the planner steps need {}; `obc data setup` shows how to install them",
            machine.planner_missing.join(", ")
        ));
    }
    if !machine.python_missing.is_empty() {
        refusals.push(format!(
            "the Python groups {} are not installed; `obc doctor --install` installs them",
            machine.python_missing.join(", ")
        ));
    }
    if let Some(memory) = machine.memory_bytes {
        for build in own {
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
    refusals.extend(disk_refusal(machine, store_bytes));
    refusals
}

/// Why the disk of the store of `machine` cannot take `store_bytes` more.
pub fn disk_refusal(machine: &MachineCheck, store_bytes: u64) -> Option<String> {
    let free = machine.store_free_bytes.filter(|free| *free < store_bytes.saturating_add(RESERVE))?;
    Some(format!(
        "the store gets about {}, and its disk has {} free and keeps {} free",
        size(store_bytes),
        size(free),
        size(RESERVE)
    ))
}

/// Refuse a run of `what` whose review `id` is not `expect`.
pub fn expect(what: &str, id: &str, expect: Option<&str>) -> Result<(), Error> {
    match expect.filter(|expect| *expect != id) {
        Some(expect) => Err(Code::Usage
            .error(format!("the review of `{what}` is {id}, not {expect}: it changed; nothing changed"))
            .fix("Read the review again: run the same command with `--dry-run` and without `--expect`.")),
        None => Ok(()),
    }
}

/// Refuse an update whose review is not `expect`, that changes a format without
/// `accept_format_change`, or that this machine cannot do.
pub fn check(review: &Review, expect: Option<&str>, accept_format_change: bool) -> Result<(), Error> {
    self::expect(&review.target, &review.id, expect)?;
    let formats: Vec<String> = review
        .layers
        .iter()
        .filter_map(|layer| {
            let FormatChange { from, to, .. } = layer.format.as_ref()?;
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
        // A planned step can end unchanged and not bake, so the counts are upper bounds.
        if !layer.leaves.is_empty() {
            text += &format!("; at most {} leaves", layer.leaves.len());
        }
        if !layer.partitions.is_empty() {
            text += &format!("; at most the partitions {}", layer.partitions.join(", "));
        }
        if !layer.readers.is_empty() {
            text += &format!("; read by {}", layer.readers.join(", "));
        }
        rows.push(vec![format!("  {verb}"), layer.layer.clone(), text]);
    }
    for source in &review.sources {
        let params: Vec<String> = source.params.iter().map(|(name, value)| format!("{name}={value}")).collect();
        let from = source.from.as_deref().map_or("new".into(), crate::prepared::show);
        rows.push(vec![
            "  source".into(),
            format!("{} {}", source.source, params.join(" ")).trim().into(),
            format!("{from} → {}", crate::prepared::show(&source.to)),
        ]);
    }
    for note in &review.notes {
        rows.push(vec!["  note".into(), note.layer.clone(), note.text.clone()]);
    }
    rows.push(vec!["  keep".into(), format!("{} outputs by reference", review.kept)]);
    let cost = &review.cost;
    let mut text = format!(
        "{} fetches, {} downloads, at most {} bakes: about {} min, {} into the store, {} to the target",
        review.fetches.len(),
        review.downloads.files,
        review.bakes.len(),
        cost.wall_ms.div_ceil(60_000),
        size(cost.store_bytes),
        size(cost.upload_bytes)
    );
    if let Some(peak) = cost.peak_rss_bytes {
        text += &format!(", peak {} memory", size(peak));
    }
    if let Some(peak) = cost.process_peak_rss_bytes {
        text += &format!(", process peak {}", size(peak));
    }
    let Unknown { estimate, peak, size: sizes } = &cost.unknown;
    let unknown: Vec<String> = [
        (estimate.len(), "bakes without an estimate"),
        (peak.len(), "bakes without a RAM estimate"),
        (sizes.len(), "fetches of unknown size"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{count} {what}"))
    .collect();
    if !unknown.is_empty() {
        text += &format!("; {}", unknown.join(", "));
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
    rows.push(vec!["  start".into(), format!("obc data {}", review.args.join(" "))]);
    print_table(&rows);
    Ok(())
}

fn text(cause: &Cause) -> String {
    match cause {
        Cause::Stale { cause } => cause.text(),
        Cause::Named => "named by --only".into(),
        Cause::Off => "no step makes it".into(),
        Cause::Unheld => "a bake reads files of it that only a store holds".into(),
    }
}

/// `bytes` in GiB or MiB.
fn size(bytes: u64) -> String {
    match bytes {
        bytes if bytes >= 1 << 30 => format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64),
        bytes => format!("{:.1} MiB", bytes as f64 / (1u64 << 20) as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_without_a_tool_or_python_group_cannot_bake_the_steps_that_need_it() {
        let machine =
            MachineCheck { planner_missing: vec!["Java 21+".into(), "Maven".into()], ..MachineCheck::default() };
        let refused = refusals(&machine, (&[], true), 0);
        assert_eq!(refused, ["the planner steps need Java 21+, Maven; `obc data setup` shows how to install them"]);
        assert!(refusals(&machine, (&[], false), 0).is_empty(), "an update without planner steps");
        let machine = MachineCheck { python_missing: vec!["planner-climate".into()], ..MachineCheck::default() };
        assert_eq!(
            refusals(&machine, (&[], false), 0),
            ["the Python groups planner-climate are not installed; `obc doctor --install` installs them"]
        );
    }
}
