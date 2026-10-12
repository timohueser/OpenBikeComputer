//! `obc data`: what each target serves, how its layers stand against the steps of now, and the
//! command to run next.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use super::{cells, freshness, listing, print_json, print_table, Code, Error};
use crate::engine::runs::Limits;
use crate::engine::state::{Environment, LayerState, Verdict};
use crate::fetch::http::Http;
use crate::lock::{self, Holder};
use crate::prepared::Prepared;
use crate::product::{BlockedLayer, Inputs, Note, Product};
use crate::regions::{geofabrik, Area, Bbox, Regions};
use crate::release::{self, Serving, SERVING};
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
#[serde(deny_unknown_fields)]
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
    /// The machine that made the serving release, and when; `null` while it serves nothing.
    pub made: Option<Made>,
    /// Where `obc data serve` serves a dir target, and how each app opens it; `null` for an R2
    /// target.
    pub serve: Option<super::serve_cli::Listening>,
    /// The run that holds the lock of the target; `null` while it is free.
    pub lock: Option<Lock>,
    /// Why `lock.json` of the target cannot be read; `null` when it can.
    pub lock_error: Option<String>,
    /// The region of the serving release.
    pub region: Option<String>,
    /// The area of the serving release, as boxes that a map draws; `null` while it serves nothing.
    pub area: Option<AreaBoxes>,
    /// Each source version that the serving release read.
    pub sources: Vec<SourceVersion>,
    /// Each layer of the steps and of the serving release; `null` while the target serves nothing
    /// or its steps cannot be listed.
    pub layers: Option<Vec<LayerState>>,
    /// The steps that cannot be planned.
    pub blocked: Vec<BlockedLayer>,
    /// What layers bake without, such as a prepared source that no target lists yet.
    pub notes: Vec<Note>,
    /// The pointer keys whose bytes are not those of the serving release.
    pub drift: Vec<String>,
    /// Why the target or its steps could not be read; `null` when they could.
    pub error: Option<String>,
    /// What to do about `error`.
    pub fix: Option<String>,
    /// The command to run next, or the run to wait for while it holds the lock; `null` when the
    /// target serves its settings as the steps make them now. For an R2 target, the command is the
    /// review: its update needs the id that the owner approved.
    pub next: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Made {
    pub machine: String,
    /// `YYYY-MM-DDTHH:MM:SSZ`.
    pub time: String,
}

/// An area as boxes.
#[derive(Serialize, JsonSchema)]
pub struct AreaBoxes {
    pub name: String,
    /// A box for each part: the box of a box region, and the bounds of each Geofabrik area in the
    /// cached index.
    pub boxes: Vec<Bbox>,
    /// Why a part has no box; `null` when each part has one.
    pub missing: Option<String>,
}

/// The areas of the cached Geofabrik index, read when a region first needs them.
type Index<'a> = &'a dyn Fn() -> Result<Option<&'a BTreeMap<String, geofabrik::Area>>, String>;

/// The area of region `id` of `regions`, with the bounds of its Geofabrik areas from `index`.
fn area(regions: &Regions, id: &str, index: Index) -> Option<AreaBoxes> {
    let name = regions.get(id)?.name.clone();
    let (mut boxes, mut missing) = (Vec::new(), None);
    for part in regions.leaves(id).ok()?.into_iter().filter_map(|leaf| regions.get(leaf)) {
        match &part.area {
            Area::Box { bbox } => boxes.push(*bbox),
            Area::Geofabrik { areas } => match index() {
                Ok(Some(index)) => {
                    for path in areas {
                        match index.get(path) {
                            Some(area) => boxes.push(area.bounds),
                            None => missing = Some(format!("the Geofabrik index has no area `{path}`")),
                        }
                    }
                }
                Ok(None) => missing = Some("the store has no Geofabrik index: `obc data fetch geofabrik-index`".into()),
                Err(error) => missing = Some(format!("the Geofabrik index cannot be read: {error}")),
            },
            Area::Union { .. } => {}
        }
    }
    Some(AreaBoxes { name, boxes, missing })
}

/// A source version that a release read.
#[derive(Serialize, JsonSchema)]
pub struct SourceVersion {
    pub source: String,
    pub version: String,
    /// When a machine first retrieved a file of it, `YYYY-MM-DDTHH:MM:SSZ`; `null` without a file.
    pub retrieved: Option<String>,
}

/// The run that holds the lock of a target, as its `lock.json` says.
#[derive(Serialize, JsonSchema)]
pub struct Lock {
    #[serde(flatten)]
    pub holder: Holder,
    /// Whether the last heartbeat is as old as `unlock` needs, by the clock of this machine: the
    /// run likely ended without freeing the lock.
    pub stale: bool,
}

impl Lock {
    pub fn of(holder: Holder) -> Self {
        Lock { stale: lock::stale_at(&holder) <= crate::date::now(), holder }
    }
}

pub fn overview(
    root: &Path,
    (products, producers): (&[&dyn Product], &[&dyn Prepared]),
    json: bool,
) -> Result<(), Error> {
    let (store, http) = (Store::open()?, Http::new());
    let targets = crate::target::load(root, &store).map_err(|e| Code::InvalidData.error(e))?;
    let sources = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?.sources;
    // A target names its Geofabrik areas without their bounds; the cached index has them. It is
    // read only for a region with a Geofabrik area.
    let cached = std::cell::OnceCell::new();
    let index = || match cached.get_or_init(|| super::cached_index(&store).map_err(|e| e.message)) {
        Ok(found) => Ok(found.as_ref().map(|(_, areas)| areas)),
        Err(error) => Err(error.clone()),
    };
    let targets = targets
        .iter()
        .map(|(name, target)| state(root, &store, &http, (products, producers), (&sources, &index), name, target));
    let limits = Limits::machine();
    let free = crate::store::free_bytes(store.root())?.map(|(_, free)| free);
    let name = crate::machine();
    let machine = Machine {
        colour: crate::machines::colour(root, &name),
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
                if let Some(Made { machine, time }) = &target.made {
                    rows.push(vec!["  made".into(), format!("on {machine} at {time}")]);
                }
                if let Some(rollback) = &serving.rollback {
                    rows.push(vec!["  rollback".into(), role(rollback)]);
                }
            }
        }
        if let Some(Lock { holder, .. }) = &target.lock {
            let (done, total) =
                holder.progress.iter().fold((0, 0), |(done, total), layer| (done + layer.done, total + layer.total));
            let held = format!(
                "run {} of `{}`, {}, {done} of {total} steps, heartbeat {}",
                holder.run, holder.machine, holder.phase, holder.heartbeat
            );
            rows.push(vec!["  lock".into(), held]);
        }
        if let Some(error) = &target.lock_error {
            rows.push(vec!["  lock".into(), format!("unreadable: {error}")]);
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
        for note in &target.notes {
            rows.push(vec![format!("  note {}", note.layer), note.text.clone()]);
        }
        for key in &target.drift {
            rows.push(vec!["  drift".into(), key.clone()]);
        }
        if let Some(error) = &target.error {
            rows.push(vec!["  error".into(), error.clone()]);
        }
        if let Some(fix) = &target.fix {
            rows.push(vec!["  fix".into(), fix.clone()]);
        }
        let next = target.next.clone().unwrap_or_else(|| "nothing to do".into());
        rows.push(vec!["  next".into(), next]);
        print_table(&rows);
    }
    Ok(())
}

fn state(
    root: &Path,
    store: &Store,
    http: &Http,
    (products, producers): (&[&dyn Product], &[&dyn Prepared]),
    (sources, index): (&[Source], Index),
    name: &str,
    target: &Target,
) -> TargetState {
    let (kind, at) = match target {
        Target::Dir(dir) => ("dir", dir.display().to_string()),
        Target::R2(remote) => ("r2", remote.url("").trim_end_matches('/').to_string()),
    };
    let mut state = TargetState {
        target: name.into(),
        kind: kind.into(),
        at,
        release: None,
        made: None,
        serve: matches!(target, Target::Dir(_)).then(|| super::serve_cli::listening(root, name, true)),
        lock: None,
        lock_error: None,
        region: None,
        area: None,
        sources: Vec::new(),
        layers: None,
        blocked: Vec::new(),
        notes: Vec::new(),
        drift: Vec::new(),
        error: None,
        fix: None,
        next: None,
    };
    // An R2 target serves riders: its update starts from a review that the owner approves.
    let update = match kind {
        "dir" => format!("obc data update {name}"),
        _ => format!("obc data update {name} --dry-run"),
    };
    match lock::holder(target) {
        Ok(holder) => state.lock = holder.map(Lock::of),
        Err(error) => state.lock_error = Some(error),
    }
    let judged = (|| -> Result<(), Error> {
        let Some(bytes) = target.get(SERVING)? else {
            state.next = Some(format!("{update} --region ID"));
            return Ok(());
        };
        let serving: Serving = serde_json::from_slice(&bytes).map_err(|e| format!("{SERVING}: {e}"))?;
        let release = super::update_cli::read(name, target, &serving.serving.id)?;
        (state.release, state.region) = (Some(serving), Some(release.settings.region.clone()));
        state.made = Some(Made { machine: release.machine.clone(), time: release.time.clone() });
        let mut versions: BTreeMap<(&str, &str), Option<&str>> = BTreeMap::new();
        for fetched in &release.sources {
            let first = versions.entry((&fetched.source, &fetched.version)).or_default();
            let retrieved = fetched.files.iter().map(|file| file.retrieved.as_str());
            *first = first.iter().copied().chain(retrieved).min();
        }
        state.sources = versions
            .into_iter()
            .map(|((source, version), retrieved)| SourceVersion {
                source: source.into(),
                version: version.into(),
                retrieved: retrieved.map(str::to_string),
            })
            .collect();
        state.drift = release::drift(target, &release)?;
        let regions = release.settings.regions()?;
        state.area = area(&regions, &release.settings.region, index);
        let mut sources = sources.to_vec();
        release.settings.policies(&mut sources)?;
        let mut inputs =
            Inputs { settings: release.settings.clone(), serving: release.versions(), ..Inputs::default() };
        let read = inputs.serving.keys().cloned().collect();
        crate::prepared::resolve(root, store, (name, target), (&sources, &read, producers), &mut inputs)?;
        let list = |inputs: &mut Inputs| {
            let fetch = listing::fetcher(root, store, http, &sources, None, false);
            listing::steps(root, products, inputs, &regions, store, true, fetch)
        };
        let mut listed = list(&mut inputs)?;
        // The steps can read a prepared source that the serving release does not, such as one that
        // got its first version.
        let asked: BTreeSet<_> = inputs.requests.borrow().difference(&read).cloned().collect();
        let known = inputs.resolved.len() + inputs.due.len();
        crate::prepared::resolve(root, store, (name, target), (&sources, &asked, producers), &mut inputs)?;
        if inputs.resolved.len() + inputs.due.len() > known {
            listed = list(&mut inputs)?;
        }
        state.blocked = listed.blocked;
        state.notes = listed.notes;
        let newer = freshness::newer(store, http, &sources, &inputs.requests.borrow(), &inputs.serving);
        let outputs = release.outputs.into_iter().map(|output| (output.step.clone(), output)).collect();
        let layers = crate::engine::state::state(store, &listed.steps, &Environment { release: outputs, newer })?;
        let stale = layers.iter().any(|layer| layer.state == Verdict::Stale);
        state.next = (stale || !state.drift.is_empty()).then(|| update.clone());
        state.layers = Some(layers);
        Ok(())
    })();
    if let Err(error) = judged {
        (state.error, state.fix) = (Some(error.message), Some(error.fix));
    }
    if let Some(Lock { holder, .. }) = &state.lock {
        state.next = Some(format!("wait for run {} of `{}` to end", holder.run, holder.machine));
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::Region;

    #[test]
    fn the_area_of_a_box_and_of_two_geofabrik_areas_is_their_boxes() {
        let bbox = |edges| Bbox::new(edges).unwrap();
        let region =
            |id: &str, area| Region { id: id.into(), name: id.into(), area, countries: Vec::new(), time_zone: None };
        let paths = vec!["europe/a".to_string(), "europe/b".to_string()];
        let box_ = bbox([7.65, 47.85, 8.15, 48.15]);
        let regions = Regions::new(vec![
            region("box", Area::Box { bbox: box_ }),
            region("areas", Area::Geofabrik { areas: paths.clone() }),
        ])
        .unwrap();
        let bounds = [bbox([5.0, 45.0, 6.0, 46.0]), bbox([6.0, 45.0, 7.0, 46.0])];
        let index: BTreeMap<String, geofabrik::Area> = paths
            .iter()
            .zip(bounds)
            .map(|(id, bounds)| {
                let (name, parent, countries, polygons) = (id.clone(), None, Vec::new(), Vec::new());
                (id.clone(), geofabrik::Area { id: id.clone(), name, parent, countries, polygons, bounds })
            })
            .collect();
        let boxes = |id| area(&regions, id, &|| Ok(Some(&index))).unwrap().boxes;
        assert_eq!(boxes("areas"), bounds);
        let unread = area(&regions, "box", &|| panic!("a box region reads no index")).unwrap();
        assert_eq!(unread.boxes, [box_]);
        let missing = area(&regions, "areas", &|| Ok(None)).unwrap();
        assert!(missing.boxes.is_empty() && missing.missing.unwrap().contains("no Geofabrik index"));
    }
}
