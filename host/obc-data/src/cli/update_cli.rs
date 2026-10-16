//! `obc data update TARGET`: make a target serve its settings. It reviews what is stale, bakes it,
//! keeps the rest of the serving release by reference, publishes the new release and switches to
//! it.

mod review;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::{freshness, listing, Code, Error};
use crate::collection::{self, Collected};
use crate::engine::changes::{self, Update};
use crate::engine::plan::{outputs, Fetch, Kept};
use crate::engine::runs::{self, Context, Event, Limits, Phase, Run};
use crate::engine::state::Newer;
use crate::engine::{select_layer, Input, InputKind, Step};
use crate::fetch::http::Http;
use crate::lock::{self, Held};
use crate::prepared::Prepared as Producer;
use crate::product::{Inputs, Product, RequestKey, Wanted};
use crate::ready::{self, Next, Wait};
use crate::regions::{Area, Bbox, Region, Regions};
use crate::release::{self, Fetched, FetchedFile, File, Output, Release, Serving};
use crate::settings::Settings;
use crate::sources::{FetchKind, Refresh, Registry, Source};
use crate::store::{sorted, Store};
use crate::target::{Tag, Target};
pub use review::{disk_refusal, expect, Downloads, MachineCheck, Review};

#[derive(Args)]
pub struct UpdateArgs {
    /// The target, as `data/targets.toml` names it.
    target: String,
    /// A region of `data/regions/`, or Geofabrik paths joined by `,`.
    #[arg(long, conflicts_with = "bbox")]
    region: Option<String>,
    /// A box: west,south,east,north in degrees.
    #[arg(long = "box", allow_hyphen_values = true)]
    bbox: Option<String>,
    /// Switch an optional layer on.
    #[arg(long)]
    add: Vec<String>,
    /// Switch an optional layer off.
    #[arg(long)]
    remove: Vec<String>,
    /// The refresh policy of a source for this target: SOURCE=DAYS or SOURCE=manual.
    #[arg(long, value_name = "SOURCE=DAYS|manual")]
    refresh: Vec<String>,
    /// Bake these layers again, also when they are not stale, with the stale layers that they read.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Show the review, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Start only when the review has this id, as `--dry-run` or `--plan` showed it.
    #[arg(long, value_name = "ID", conflicts_with = "dry_run")]
    expect: Option<String>,
    /// Plan the update in a run: fetch what the plan reads, write its review, and bake nothing.
    #[arg(long, conflicts_with_all = ["dry_run", "expect"])]
    plan: bool,
    /// Start also when the format of a layer changes: its readers need new data.
    #[arg(long)]
    accept_format_change: bool,
    /// Do not ask before an update of an R2 target. Required without a terminal.
    #[arg(long)]
    yes: bool,
}

impl UpdateArgs {
    /// The arguments after `obc data` of the update with these settings.
    fn settings(&self) -> Vec<String> {
        let mut args: Vec<String> = ["update", &self.target].map(String::from).into();
        let mut flag = |name: &str, value: &str| args.extend([format!("--{name}"), value.to_string()]);
        self.region.iter().for_each(|region| flag("region", region));
        self.bbox.iter().for_each(|bbox| flag("box", bbox));
        self.add.iter().for_each(|layer| flag("add", layer));
        self.remove.iter().for_each(|layer| flag("remove", layer));
        self.refresh.iter().for_each(|policy| flag("refresh", policy));
        if !self.only.is_empty() {
            flag("only", &self.only.join(","));
        }
        args
    }

    /// The arguments after `obc data` that start the update of `review`.
    fn args(&self, review: &Review) -> Vec<String> {
        let mut args = self.settings();
        args.extend(["--expect".into(), review.id.clone()]);
        if review.layers.iter().any(|layer| layer.format.is_some()) {
            args.push("--accept-format-change".into());
        }
        args
    }
}

/// What the person asks of an update besides its settings.
#[derive(Debug, Default)]
pub(super) struct Asked {
    pub only: Vec<String>,
    pub expect: Option<String>,
    pub accept_format_change: bool,
}

/// What an update did.
#[derive(Debug)]
pub struct Updated {
    pub target: String,
    /// The id of the review that the update did.
    pub review: String,
    /// The release that the target serves now.
    pub serving: String,
    /// The release that it served before; `null` when it served nothing or the update changed
    /// nothing.
    pub rollback: Option<String>,
    /// The steps that the run baked or found in the store, in dependency order.
    pub baked: Vec<Baked>,
    /// The number of outputs that the release keeps by reference.
    pub kept: usize,
    /// What the collection after the switch did; `null` without a switch.
    pub collection: Option<Collected>,
}

#[derive(Debug)]
pub struct Baked {
    pub step: String,
    pub key: String,
    pub reused: bool,
    /// It kept its output of the serving release.
    pub unchanged: bool,
}

pub fn command(
    root: &Path,
    (products, producers): (&[&dyn Product], &[&dyn Producer]),
    args: UpdateArgs,
    json: bool,
) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, &args.target, &store).map_err(|e| Code::Usage.error(e))?;
    super::api::writable(&args.target, &target, true)?;
    // A step reads a kept output from the target when this store lacks it.
    let store = store.reading(&target);
    let http = Http::new();
    let edit = |settings: &mut Settings| edit(root, products, &args, settings);
    let target = (args.target.as_str(), &target);
    let asked =
        Asked { only: args.only.clone(), expect: args.expect.clone(), accept_format_change: args.accept_format_change };
    let code = (products, producers);
    if args.plan {
        let work = |run: &mut Run| plan(root, &store, &http, target, code, edit, &args, run);
        if let Some(done) = super::api::detached(&store, work, |review| review::print(review, false)) {
            return done;
        }
        let mut start = args.settings();
        start.push("--plan".into());
        return super::api::start(&store, &start, None, json);
    }
    let work = |run: &mut Run| update(root, &store, &http, target, code, edit, &asked, None, run);
    if let Some(done) = super::api::detached(&store, work, print) {
        return done;
    }
    let mut prepared = prepare(root, &store, &http, target, code, edit, &args.only, None, None)?;
    prepared.review.args = args.args(&prepared.review);
    prepared.review.optional = optional(products);
    if args.dry_run {
        return review::print(&prepared.review, json);
    }
    review::check(&prepared.review, asked.expect.as_deref(), asked.accept_format_change)?;
    let what = format!("an update of `{}`", args.target);
    let dry_run = format!("obc data update {} --dry-run` with the same flags", args.target);
    let question = format!("Publish review {} to `{}`, which serves riders", prepared.review.id, args.target);
    super::api::approved(target.1, (asked.expect.is_some(), args.yes), (&what, &dry_run, &question))?;
    super::api::start(&store, &prepared.review.args, Some(&prepared.review.id), json)
}

/// The optional layers of `products`, sorted.
fn optional(products: &[&dyn Product]) -> Vec<String> {
    let mut optional: Vec<String> =
        products.iter().flat_map(|product| product.optional()).map(|layer| layer.to_string()).collect();
    optional.sort();
    optional
}

/// Plan the update of `target` in `run`: fetch what its listing reads, and write the review to
/// `runs/<id>/review.json`, where `runs RUN` gives it. It takes no lock: it writes nothing to the
/// target, and a download locks its own file in the store. It bakes nothing.
#[allow(clippy::too_many_arguments)]
fn plan(
    root: &Path,
    store: &Store,
    http: &Http,
    target: (&str, &Target),
    code: (&[&dyn Product], &[&dyn Producer]),
    edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    args: &UpdateArgs,
    run: &mut Run,
) -> Result<Review, Error> {
    let mut prepared = prepare(root, store, http, target, code, edit, &args.only, None, Some(run))?;
    prepared.review.args = args.args(&prepared.review);
    prepared.review.optional = optional(code.0);
    let file = store.run(run.id()).join(runs::REVIEW);
    let bytes = serde_json::to_vec(&prepared.review).expect("a review serializes");
    crate::store::write_atomic(&file, &bytes).map_err(|e| Code::Failed.error(e))?;
    Ok(prepared.review)
}

/// What `obc data update --json` writes: the run that it started.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Started {
    pub run: String,
    /// The id of the review that the run does; `null` for `--plan`, whose run makes the review.
    pub review: Option<String>,
}

/// Write what an update did.
fn print(updated: &Updated) -> Result<(), Error> {
    for baked in &updated.baked {
        let verb = match (baked.unchanged, baked.reused) {
            (true, _) => "unchanged",
            (false, true) => "reused",
            (false, false) => "baked",
        };
        println!("{verb:<9} {}  {}", &baked.key[..12], baked.step);
    }
    match &updated.rollback {
        Some(rollback) => println!(
            "{} serves release {}; rollback {}; {} outputs kept",
            updated.target,
            &updated.serving[..12],
            &rollback[..12],
            updated.kept
        ),
        None => println!("{} serves release {}", updated.target, &updated.serving[..12]),
    }
    println!("review {}", updated.review);
    if let Some(collected) = &updated.collection {
        println!("{}", collected.text());
    }
    Ok(())
}

/// What an update reads and decides before it changes anything.
pub(super) struct Prepared {
    pub review: Review,
    base: Option<(Serving, Tag)>,
    serving: Option<Release>,
    settings: Settings,
    sources: Vec<Source>,
    steps: Vec<Step>,
    update: Update,
    /// The fetches whose files this machine downloads from their copies on the target, each with
    /// the names of the files that the store lacks; none for all of them.
    copies: Vec<(Fetched, Vec<String>)>,
}

/// Update `target` with the settings of its serving release after `edit`, when the review passes
/// [`review::check`].
#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    root: &Path,
    store: &Store,
    http: &Http,
    target: (&str, &Target),
    code: (&[&dyn Product], &[&dyn Producer]),
    edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    asked: &Asked,
    machine: Option<&MachineCheck>,
    run: &mut Run,
) -> Result<Updated, Error> {
    locked(target, run, |run, held| {
        let prepared = prepare(root, store, http, target, code, edit, &asked.only, machine, Some(run))?;
        if !held.beat(Some(&prepared.review.id), None, crate::date::now())? {
            return Err(lost(&format!("target `{}`", target.0)));
        }
        review::check(&prepared.review, asked.expect.as_deref(), asked.accept_format_change)?;
        apply(root, store, http, target, code.0, prepared, (run, held))
    })
}

/// Do `work` while the run holds the lock of `target`: a thread writes the heartbeat, and the lock
/// is free again at the end. When another run takes the lock, the run stops. SIGINT and SIGTERM
/// stop the run, so it frees the lock.
pub(super) fn locked<T>(
    (name, target): (&str, &Target),
    run: &mut Run,
    work: impl FnOnce(&mut Run, &Held) -> Result<T, Error>,
) -> Result<T, Error> {
    locked_as((&format!("target `{name}`"), &format!("obc data unlock {name}")), target, run, work)
}

/// [`locked`] for the lock of `target`, which messages name `label` and `unlock` frees.
pub(super) fn locked_as<T>(
    (label, unlock): (&str, &str),
    target: &Target,
    run: &mut Run,
    work: impl FnOnce(&mut Run, &Held) -> Result<T, Error>,
) -> Result<T, Error> {
    crate::engine::runs::stop_on_signals();
    let held = Held::take(target, run.id(), crate::date::now()).map_err(|refused| match refused {
        lock::Refused::Held(holder) => {
            Code::Blocked.error(format!("{label}: {}", lock::held_by(&holder))).fix(format!(
                "Wait for that run to end. When `{0}` stopped it, free the lock after {1}, 10 minutes after its \
                 last heartbeat: `{unlock} --from {0}`.",
                holder.machine,
                crate::date::timestamp(
                    lock::stale_since(target, &holder).map_or(lock::stale_at(&holder), |(at, _)| at)
                )
            ))
        }
        lock::Refused::Failed(e) => Code::Failed
            .error(format!("{label}: {e}"))
            .fix(format!("Read {} of the target: another build of `obc data` can have written it, and a run can hold it. Wait for that run, or use that build.", lock::LOCK)),
    })?;
    let (stop, (done, wait), file) = (run.stopper(), mpsc::channel::<()>(), run.status_file());
    let result = std::thread::scope(|scope| {
        let held = &held;
        scope.spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = wait.recv_timeout(Duration::from_secs(lock::BEAT)) {
                held.progress(runs::progress(&file));
                // A write that fails is tried again at the next beat; a lock that another run took
                // stops this run.
                if let Ok(false) = held.beat(None, None, crate::date::now()) {
                    stop.store(true, Ordering::Relaxed);
                    return;
                }
            }
        });
        let result = work(run, held);
        drop(done);
        result
    });
    let (stays, fix) = match held.release() {
        Ok(()) => return result,
        // The lock is not this run's to free.
        Err(lock::Refused::Held(holder)) => (
            format!("another run took the lock of {label} before this run freed it: {}", lock::held_by(&holder)),
            "Run `obc data` to see the run that holds it.".to_string(),
        ),
        Err(lock::Refused::Failed(e)) => (
            format!("the lock of {label} stays taken: {e}"),
            format!(
                "Its heartbeat stopped. After {} minutes, free it: `{unlock} --from {}`.",
                lock::STALE / 60,
                crate::machine()
            ),
        ),
    };
    Err(match result {
        Ok(_) => Code::Failed.error(stays).fix(fix),
        Err(mut error) => {
            (error.message, error.fix) = (format!("{}; {stays}", error.message), format!("{} {fix}", error.fix));
            error
        }
    })
}

/// Enter `phase`, after a check that nothing stopped the run and that it still holds the lock.
pub(super) fn enter(work: (&mut Run, &Held), name: &str, phase: Phase) -> Result<(), Error> {
    enter_as(work, (&format!("target `{name}`"), &format!("obc data update {name}")), phase)
}

/// [`enter`] for the lock that messages name `label`, of a run that `again` does again.
pub(super) fn enter_as(
    (run, held): (&mut Run, &Held),
    (label, again): (&str, &str),
    phase: Phase,
) -> Result<(), Error> {
    if run.stopping() {
        return Err(Code::Failed
            .error(format!("the run stopped before the phase {phase}; nothing more changed"))
            .fix(format!("Run `{again}` again: it keeps what this run did.")));
    }
    run.record(&Event::Phase { phase })?;
    match held.beat(None, Some(phase), crate::date::now())? {
        true => Ok(()),
        false => Err(lost(label)),
    }
}

/// The error of a run whose lock, which messages name `label`, another run took.
pub(super) fn lost(label: &str) -> Error {
    Code::Failed
        .error(format!("another run took the lock of {label}; this run stopped before it changed more"))
        .fix("Run `obc data` to see the run that holds it.")
}

/// Read the serving release of `target`, apply `edit` to its settings, list the steps and review
/// them. Only the fetches that a step list needs to be listed run, into the store; `run` records
/// them. The review checks `machine`, or else this machine after the listing.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare(
    root: &Path,
    store: &Store,
    http: &Http,
    (name, target): (&str, &Target),
    (products, producers): (&[&dyn Product], &[&dyn Producer]),
    edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    only: &[String],
    machine: Option<&MachineCheck>,
    mut run: Option<&mut Run>,
) -> Result<Prepared, Error> {
    let base = release::serving(target)?;
    let serving = base.as_ref().map(|(serving, _)| read(name, target, &serving.serving.id)).transpose()?;
    let rollback = base.as_ref().and_then(|(serving, _)| serving.rollback.as_ref());
    let rollback = rollback.map(|rollback| read(name, target, &rollback.id)).transpose()?;
    let mut settings = serving.as_ref().map(|release| release.settings.clone()).unwrap_or_default();
    edit(&mut settings)?;
    if settings.region.is_empty() {
        return Err(Code::Usage
            .error(format!("target `{name}` has no region"))
            .fix(format!("Name one: `obc data update {name} --region ID` or `--box W,S,E,N`.")));
    }
    let regions = settings.regions().map_err(|e| Code::InvalidData.error(e))?;
    let mut sources = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?.sources;
    settings.policies(&mut sources).map_err(|e| Code::Usage.error(e))?;
    let manual = sources.iter().filter(|source| source.refresh == Refresh::Manual).map(|s| s.id.clone()).collect();
    let serving_versions = serving.as_ref().map(Release::versions).unwrap_or_default();
    let mut inputs = Inputs { settings: settings.clone(), serving: serving_versions, manual, ..Inputs::default() };

    if let Some(run) = run.as_deref_mut() {
        run.record(&Event::Phase { phase: Phase::Prepare })?;
    }
    let requests = freshness::discover(root, products, &inputs, &regions, store, http, &sources)?;
    for (key, newer) in freshness::newer(store, http, &sources, &requests, &inputs.serving) {
        match newer {
            Some(version) => {
                inputs.resolved.insert(key, version);
            }
            None => {
                inputs.due.insert(key);
            }
        }
    }
    crate::prepared::resolve(root, store, (name, target), (&sources, &requests, producers), &mut inputs)?;
    let listed = {
        let mut fetch = listing::fetcher(root, store, http, &sources, run, true);
        // The listing asks for what the store lacks.
        let refetch = |wanted: &Wanted| {
            if let Some(version) = &wanted.version {
                let (source, params) = (wanted.source.clone(), wanted.params.clone());
                let wanted = Fetch { source, version: version.clone(), params, files: Vec::new(), bytes: None };
                refetchable(store, &wanted, serving.as_ref(), &sources)?;
            }
            fetch(wanted)
        };
        listing::steps(root, products, &mut inputs, &regions, store, false, refetch)?
    };
    crate::prepared::named(&listed.steps, &sources).map_err(|e| {
        Code::Failed.error(e).fix("The code of the product is wrong, not the data: name the files that the step reads.")
    })?;
    if !listed.blocked.is_empty() {
        let reasons: Vec<String> =
            listed.blocked.iter().map(|b| format!("{} ({}): {}", b.layer, b.step, b.reason)).collect();
        return Err(Code::Blocked.error(format!("layers cannot be planned: {}", reasons.join("; "))));
    }
    let release: BTreeMap<&str, &Output> =
        serving.iter().flat_map(|release| &release.outputs).map(|output| (output.step.as_str(), output)).collect();
    let mut update = changes::update(store, &listed.steps, &release, &Newer::new(), only)?;
    let read = serving.iter().flat_map(|release| &release.sources);
    same_bytes(store, &listed.steps, &update.kept, read.clone())?;
    // A fetch whose files the target copies comes from the target.
    let fetches = update.plan.fetches();
    let copies: Vec<(Fetched, Vec<String>)> = read
        .clone()
        .filter(|fetched| fetched.copy)
        .filter_map(|fetched| {
            Some((fetched.clone(), fetches.iter().find(|fetch| covers(fetched, fetch))?.files.clone()))
        })
        .collect();
    for group in &mut update.plan.groups {
        group.fetches.retain(|fetch| !copies.iter().any(|(copy, _)| covers(copy, fetch)));
    }
    for fetch in update.plan.fetches() {
        refetchable(store, &fetch, serving.as_ref(), &sources)?;
    }
    let downloads = downloads(store, &listed.steps, &update, &copies);
    let machine = match machine {
        Some(machine) => machine.clone(),
        None => MachineCheck::this(store)?,
    };
    let uploads = new_copies(store, &listed.steps, &update, serving.as_ref(), &sources)?;
    let mut review = review::review(review::Parts {
        target: name,
        base: base.as_ref(),
        serving: serving.as_ref(),
        rollback: rollback.as_ref(),
        settings: &settings,
        steps: &listed.steps,
        only,
        plan: &update.plan,
        kept: &update.kept,
        layers: &update.layers,
        unheld: &update.unheld,
        downloads,
        copies: uploads,
        machine: &machine,
    });
    review.notes = listed.notes;
    Ok(Prepared { review, base, serving, settings, sources, steps: listed.steps, update, copies })
}

/// Whether the copy `fetched` on a target has the files that `fetch` gets.
fn covers(fetched: &Fetched, fetch: &Fetch) -> bool {
    (&fetched.source, &fetched.version, &fetched.params) == (&fetch.source, &fetch.version, &sorted(&fetch.params))
        && fetch.files.iter().all(|name| fetched.files.iter().any(|file| &file.name == name))
}

/// Refuse a step that bakes and reads other bytes of a fetch in the store than the serving release
/// read: the release of another machine and the one of this machine would not agree.
fn same_bytes<'a>(
    store: &Store,
    steps: &[Step],
    kept: &Kept,
    read: impl Iterator<Item = &'a Fetched>,
) -> Result<(), Error> {
    let read: Vec<&Fetched> = read.collect();
    for input in steps.iter().filter(|step| !kept.contains_key(&step.name)).flat_map(|step| &step.inputs) {
        let Input::Snapshot { source, version, params, .. } = input else { continue };
        let fetch = |fetched: &&&Fetched| {
            (&fetched.source, &fetched.version, &fetched.params) == (source, version, &sorted(params))
        };
        let (Some(fetched), Some(record)) = (read.iter().find(fetch), store.snapshot(source, version)?) else {
            continue;
        };
        for file in &fetched.files {
            let Some(held) = record.files.iter().find(|held| held.name == file.name) else { continue };
            if held.sha256 != file.sha256 {
                return Err(Code::VerifyFailed
                    .error(format!(
                        "the store holds other bytes of {} of {source}@{version} than the serving release read; nothing changed",
                        file.name
                    ))
                    .fix(format!(
                        "Upstream gave this machine other bytes for this version than the machine that made the release: \
                         run the update on that machine, or move `{source}` to another version."
                    )));
            }
        }
    }
    Ok(())
}

/// Refuse `fetch`, which the store lacks, when the serving release read its version, the target does
/// not copy it, and a capture cannot give it: this machine cannot get that version again. One
/// version of such a source names all its requests, such as the tiles of a national terrain model,
/// so the params do not count.
fn refetchable(store: &Store, fetch: &Fetch, serving: Option<&Release>, sources: &[Source]) -> Result<(), Error> {
    let Some(source) = sources.iter().find(|source| source.id == fetch.source) else { return Ok(()) };
    let newest_only = source.fetch.kind == FetchKind::Capture;
    let read = |serving: &&Release| {
        let version = |read: &&Fetched| (&read.source, &read.version) == (&fetch.source, &fetch.version);
        serving.sources.iter().filter(version).any(|read| !read.copy)
    };
    let Some(serving) = serving.filter(|serving| newest_only && read(serving)) else { return Ok(()) };
    if crate::fetch::capture::can_fetch(store, source, &fetch.version)? {
        return Ok(());
    }
    let (id, version) = (&source.id, &fetch.version);
    let policy = match source.refresh {
        Refresh::Days(days) => days.to_string(),
        Refresh::Manual => "manual".into(),
    };
    Err(Code::Blocked
        .error(format!(
            "this machine cannot fetch {id}@{version}, which the serving release read: upstream gives only its \
             newest version, and the target holds no copy; nothing changed"
        ))
        .fix(format!(
            "Run this update on `{}`, whose store has {id}@{version}. Or move `{id}` to a new version with \
             `--refresh {id}=1`, then set its policy back with `--refresh {id}={policy}`.",
            serving.machine
        )))
}

/// What this machine downloads from the target: the files of the copies that the store lacks, and
/// the files of kept steps that a step that bakes reads.
fn downloads(store: &Store, steps: &[Step], update: &Update, copies: &[(Fetched, Vec<String>)]) -> review::Downloads {
    let mut objects: BTreeMap<&str, u64> = BTreeMap::new();
    for file in copies.iter().flat_map(|(copy, names)| copy.named(names)) {
        objects.insert(&file.sha256, file.size);
    }
    for step in update.plan.builds().filter_map(|build| steps.iter().find(|step| step.name == build.step)) {
        for input in &step.inputs {
            let Input::Step { name, files } = input else { continue };
            let Some(output) = update.kept.get(name) else { continue };
            objects
                .extend(select_layer(&output.files, files).0.into_iter().map(|file| (file.sha256.as_str(), file.size)));
        }
    }
    objects.retain(|sha256, _| !store.object(sha256).is_file());
    review::Downloads { files: objects.len() as u64, bytes: objects.values().sum() }
}

/// Bake, publish and switch as `prepared` says, then collect.
fn apply(
    root: &Path,
    store: &Store,
    http: &Http,
    (name, target): (&str, &Target),
    products: &[&dyn Product],
    prepared: Prepared,
    (run, held): (&mut Run, &Held),
) -> Result<Updated, Error> {
    let Prepared { review, base, serving, settings, sources, steps, mut update, copies } = prepared;
    if !copies.is_empty() {
        enter((run, held), name, Phase::Download)?;
        for (copy, names) in &copies {
            copy.restore(target, store, names)?;
        }
    }
    enter((run, held), name, Phase::Build)?;
    let served =
        serving.iter().flat_map(|release| &release.outputs).map(|output| (output.step.as_str(), output)).collect();
    let context = Context {
        store,
        root,
        sources: &sources,
        http,
        kept: &update.kept,
        served: &served,
        limits: Limits::machine(),
    };
    // The fetches first: upstream too must give the bytes that the serving release read.
    run.fetch(&context, &update.plan).map_err(|e| Code::RunFailed.error(e))?;
    same_bytes(store, &steps, &update.kept, serving.iter().flat_map(|release| &release.sources))?;
    for group in &mut update.plan.groups {
        group.fetches.clear();
    }
    let built = run.build(&context, &steps, &update.plan).map_err(|e| Code::RunFailed.error(e))?;
    // An unchanged step is kept by reference.
    update.kept.extend(built.iter().filter_map(|built| Some((built.receipt.step.clone(), built.kept.clone()?))));
    let baked = built.into_iter().map(|built| Baked {
        unchanged: built.kept.is_some(),
        step: built.receipt.step,
        key: built.receipt.key,
        reused: built.reused,
    });
    let mut outputs = outputs(store, &steps, &update.kept)?
        .ok_or_else(|| Code::Failed.error("the store lacks the output of a step after the run"))?;
    outputs.sort_by(|a, b| a.step.cmp(&b.step));
    let fetched = reads(store, &outputs, serving.as_ref(), &sources)?;
    let next = compose(products, settings, outputs, fetched)?;
    let mut updated = Updated {
        target: name.into(),
        review: review.id,
        serving: String::new(),
        rollback: None,
        baked: baked.collect(),
        kept: update.kept.len(),
        collection: None,
    };
    if let Some(serving) = serving.as_ref().filter(|serving| serving.serves_as(&next)) {
        let (current, _) = base.as_ref().expect("a serving release has a base");
        // The serving release has every change: a switch to it writes a pointer that drifts.
        if !release::drift(target, serving)?.is_empty() {
            enter((run, held), name, Phase::Switch)?;
            if !release::switch(target, base.as_ref(), &current.serving.id)? {
                return Err(changed(name));
            }
        }
        updated.serving = current.serving.id.clone();
        return Ok(updated);
    }

    enter((run, held), name, Phase::Verify)?;
    for product in products {
        let failed =
            |e: String| Code::VerifyFailed.error(format!("product `{}`: {e}; nothing changed", product.name()));
        product.verify(root, serving.as_ref(), &next, store).map_err(failed)?;
    }
    enter((run, held), name, Phase::Upload)?;
    let id = release::publish(target, store, &next)?;
    let wait = matches!(target, Target::R2(_)).then_some(Wait::RUN);
    switch_to((name, target), base.as_ref(), &id, http, wait, (run, held))?;
    // The switch is done: a lock that is lost now stops only the collection.
    let since = base.as_ref().map(|(base, _)| base.serving.since.as_str());
    let holds = || match held.beat(None, None, crate::date::now())? {
        true => Ok(()),
        false => Err(lost(name).message),
    };
    let collect = enter((run, held), name, Phase::Collect).and_then(|()| Ok(target.now()?));
    updated.collection = Some(match collect {
        Ok(now) => collection::collect(target, since, now, &holds),
        Err(error) => Collected { stopped: Some(error.message), ..Collected::default() },
    });
    updated.rollback = base.map(|(base, _)| base.serving.id);
    updated.serving = id;
    Ok(updated)
}

/// The error of a switch whose `release.json` another writer changed.
fn changed(name: &str) -> Error {
    Code::Failed
        .error(format!("release.json of `{name}` changed during the update; nothing switched"))
        .fix(format!("Run `obc data update {name}` again: it keeps what this run baked."))
}

/// Switch `target` from `base` to the published release `id`. With `wait`, as for an R2 target:
/// `planner/next.json` names the release first, and the switch waits until the follower of the
/// target serves its planner release. A wait that ends without that deletes `planner/next.json`,
/// and the target stays as it was.
fn switch_to(
    (name, target): (&str, &Target),
    base: Option<&(Serving, Tag)>,
    id: &str,
    http: &Http,
    wait: Option<Wait>,
    (run, held): (&mut Run, &Held),
) -> Result<(), Error> {
    let next = match wait {
        Some(wait) => Some(router_ready((name, target), id, http, wait, (run, held))?),
        None => None,
    };
    enter((run, held), name, Phase::Switch)?;
    if !release::switch(target, base, id)? {
        return Err(changed(name));
    }
    run.record(&Event::Switched { release: id.into() })?;
    run.sync()?;
    drop(next);
    Ok(())
}

/// Name the release `id` in `planner/next.json` of `target`, and ask the follower each
/// `wait.every` until it serves the planner release of `id`. The status and the lock of the run
/// carry each new answer. A stop of the run, a lost lock or `wait.give_up` ends the wait without a
/// switch.
fn router_ready<'a>(
    (name, target): (&str, &'a Target),
    id: &str,
    http: &Http,
    wait: Wait,
    (run, held): (&mut Run, &'a Held),
) -> Result<Next<'a>, Error> {
    enter((run, held), name, Phase::RouterReady)?;
    let url = ready::url(target, id).map_err(|e| Code::Failed.error(format!("the new release: {e}")))?;
    let next = Next::name(target, held, id)?;
    let (since, lost) = ((wait.now)(), run.stopper());
    let mut last = None;
    while let Some(reason) = ready::ask(http, &url) {
        if last.as_ref() != Some(&reason) {
            run.record(&Event::Waiting { reason: reason.clone() })?;
            held.waiting(Some(reason.clone()));
        }
        let ended = if run.stopping() {
            Some("The run stopped while it waited.".to_string())
        } else if lost.load(Ordering::Relaxed) {
            Some(format!("Another run took the lock of `{name}`."))
        } else if (wait.now)().saturating_sub(since) >= wait.give_up {
            let hours = wait.give_up.as_secs() / 3600;
            Some(format!("The follower did not serve the planner release in {hours} h: deploy services that read it."))
        } else {
            None
        };
        if let Some(ended) = ended {
            let fix = format!("{ended} Run `obc data update {name}` again: it keeps what this run baked.");
            return Err(Code::Failed.error(format!("{name} unchanged: {reason}")).fix(fix));
        }
        last = Some(reason);
        (wait.sleep)(wait.every);
    }
    held.waiting(None);
    Ok(next)
}

/// The release of `outputs`: the pointer and the named files of each product.
fn compose(
    products: &[&dyn Product],
    settings: Settings,
    outputs: Vec<Output>,
    sources: Vec<Fetched>,
) -> Result<Release, Error> {
    let (mut pointers, mut named) = (Vec::new(), Vec::new());
    for product in products {
        let bug = |message: String| {
            let fix = format!("The code of product `{}` is wrong, not the data: correct it.", product.name());
            Code::Failed.error(message).fix(fix)
        };
        let pointer = product.pointer();
        let output = outputs.iter().find(|output| output.step == pointer.step);
        let file = output.and_then(|output| output.files.iter().find(|file| file.path == pointer.path));
        let file = file.ok_or_else(|| bug(format!("no step makes the file {} of {}", pointer.path, pointer.step)))?;
        pointers.push(File { key: pointer.key.into(), size: file.size, sha256: file.sha256.clone() });
        let files = product.named(&outputs).map_err(bug)?;
        let folder = crate::engine::digest(files.iter().map(|(path, file)| (path.as_str(), file.sha256.as_str())));
        named.extend(files.into_iter().map(|(path, file)| File {
            key: format!("releases/{folder}/{path}"),
            size: file.size,
            sha256: file.sha256,
        }));
    }
    pointers.sort();
    named.sort();
    let release = Release {
        settings,
        outputs,
        pointers,
        named,
        sources,
        machine: crate::machine(),
        time: crate::date::timestamp(crate::date::now()),
    };
    release.check().map_err(|e| Code::Failed.error(format!("the new release: {e}")))?;
    Ok(release)
}

/// The release `id` of the target `name`. A manifest that does not parse is `invalid_data`; a failed
/// read stays what it is.
pub(super) fn read(name: &str, target: &Target, id: &str) -> Result<Release, Error> {
    let key = release::manifest_key(id);
    let bytes = target.get(&key)?.ok_or_else(|| Code::Failed.error(format!("target `{name}` has no {key}")))?;
    Release::parse(id, &bytes).map_err(|e| {
        let fix = match target {
            Target::Dir(dir) => format!(
                "An older `obc data` can have written it: move the folder aside with `mv {0} {0}.old`, then run \
                 `obc data update {name} --region ID` again.",
                dir.display()
            ),
            Target::R2(_) => format!(
                "An older `obc data` can have written it. Ask the owner: `obc data rollback {name}` serves the \
                 rollback, when it reads."
            ),
        };
        Code::InvalidData.error(format!("target `{name}`: {e}")).fix(fix)
    })
}

/// The size of the copies under `sources/` that the update adds to the target: the files of a copied
/// source that a step that bakes reads, and that the serving release does not name; and each such
/// fetch that the store lacks yet, as `SOURCE@VERSION`.
fn new_copies(
    store: &Store,
    steps: &[Step],
    update: &Update,
    serving: Option<&Release>,
    sources: &[Source],
) -> Result<(u64, BTreeSet<String>), Error> {
    let named: BTreeSet<String> = serving.iter().flat_map(|release| release.files()).map(|file| file.key).collect();
    let (mut copies, mut unknown) = (BTreeMap::new(), BTreeSet::new());
    for input in steps.iter().filter(|step| !update.kept.contains_key(&step.name)).flat_map(|step| &step.inputs) {
        let Input::Snapshot { source, version, params, files } = input else { continue };
        if !sources.iter().any(|declared| declared.id == *source && declared.copied()) {
            continue;
        }
        let Some(objects) = crate::engine::snapshot_files(store, source, version, params, files)? else {
            let read = serving.iter().flat_map(|release| &release.sources);
            if !read.into_iter().any(|read| read.copy && (&read.source, &read.version) == (source, version)) {
                unknown.insert(format!("{source}@{version}"));
            }
            continue;
        };
        for (name, object) in objects {
            let size = std::fs::metadata(&object).map_err(|e| format!("{}: {e}", object.display()))?.len();
            copies.insert(release::copy_key(source, version, &name), size);
        }
    }
    Ok((copies.into_iter().filter(|(key, _)| !named.contains(key)).map(|(_, size)| size).sum(), unknown))
}

/// The files of each fetch that `outputs` read: from the snapshot record in the store, else from
/// the serving release, whose kept outputs read them. A target copies the files of a source that
/// `sources` lets it give to others.
fn reads(
    store: &Store,
    outputs: &[Output],
    serving: Option<&Release>,
    sources: &[Source],
) -> Result<Vec<Fetched>, Error> {
    // The names of the files of each fetch and version.
    let mut names: BTreeMap<(RequestKey, String), BTreeSet<String>> = BTreeMap::new();
    for output in outputs {
        for input in output.inputs.iter().filter(|input| input.kind == InputKind::Snapshot) {
            let read = output.snapshots.get(&input.name).ok_or_else(|| {
                Code::Failed.error(format!("step `{}` reads {} and has no version of it", output.step, input.name))
            })?;
            let fetch = ((input.name.clone(), read.params.clone()), read.version.clone());
            names.entry(fetch).or_default().extend(input.files.iter().cloned());
        }
    }
    let mut fetched = Vec::new();
    for (((source, params), version), names) in names {
        let pick = |files: Vec<FetchedFile>| {
            let mut files: Vec<FetchedFile> = files.into_iter().filter(|file| names.contains(&file.name)).collect();
            files.sort_by(|a, b| a.name.cmp(&b.name));
            (files.len() == names.len()).then_some(files)
        };
        let recorded = store.snapshot(&source, &version)?.and_then(|snapshot| {
            let files = snapshot.files.into_iter();
            pick(
                files
                    .map(|file| FetchedFile {
                        name: file.name,
                        url: file.url,
                        size: file.size,
                        sha256: file.sha256,
                        retrieved: file.retrieved,
                    })
                    .collect(),
            )
        });
        let served = serving.iter().flat_map(|release| &release.sources);
        let earlier = served.into_iter().find(|f| (&f.source, &f.version, &f.params) == (&source, &version, &params));
        let earlier = earlier.and_then(|fetched| pick(fetched.files.clone()));
        // A file that the serving release read keeps its entry there, so the manifest stays the same.
        let files = match (recorded, earlier) {
            (Some(recorded), Some(earlier)) => recorded
                .into_iter()
                .map(|file| earlier.iter().find(|served| served.same(&file)).cloned().unwrap_or(file))
                .collect(),
            (recorded, earlier) => recorded.or(earlier).ok_or_else(|| {
                Code::Failed
                    .error(format!("neither the store nor the serving release has the files of {source}@{version}"))
            })?,
        };
        let copied = sources.iter().find(|declared| declared.id == source && declared.copied());
        let (licence, attribution) =
            copied.map_or((None, None), |source| (source.licence.clone(), source.attribution.clone()));
        fetched.push(Fetched { source, version, params, files, copy: copied.is_some(), licence, attribution });
    }
    Ok(fetched)
}

/// Give `settings` the region of `--region` and `--box`.
pub(super) fn select(
    root: &Path,
    (region, bbox): (Option<&str>, Option<&str>),
    settings: &mut Settings,
) -> Result<(), Error> {
    let usage = |message: String| Code::Usage.error(message);
    if let Some(region) = region {
        let presets = Regions::load(root).map_err(|e| Code::InvalidData.error(e))?;
        match presets.get(region) {
            Some(_) => settings.select(&presets, region).map_err(usage)?,
            None => {
                let areas: Vec<String> = region.split(',').map(str::to_string).collect();
                let path = |area: &String| area.split('/').all(crate::is_kebab);
                if let Some(area) = areas.iter().find(|area| !path(area)) {
                    return Err(usage(format!("`{area}` is no region of data/regions/ and no Geofabrik path")));
                }
                let id = if areas.len() == 1 { areas[0].clone() } else { "areas".into() };
                // Geofabrik's index names no country for some areas, such as `europe/dach`: an area
                // takes the countries of the region of data/regions/ that reads only it.
                let preset = |area: &String| {
                    presets.iter().find(|region| region.area == Area::Geofabrik { areas: vec![area.clone()] })
                };
                let mut countries: Vec<String> = Vec::new();
                for code in areas.iter().filter_map(preset).flat_map(|region| &region.countries) {
                    if !countries.contains(code) {
                        countries.push(code.clone());
                    }
                }
                let time_zone = match &areas[..] {
                    [area] => preset(area).and_then(|region| region.time_zone.clone()),
                    _ => None,
                };
                let area = Area::Geofabrik { areas: areas.clone() };
                defined(settings, Region { id, name: areas.join(" + "), area, countries, time_zone })?;
            }
        }
    }
    if let Some(text) = bbox {
        let numbers: Vec<f64> = text
            .split(',')
            .map(|n| n.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| usage(format!("`--box {text}` is not four numbers")))?;
        let corners: [f64; 4] = numbers.try_into().map_err(|_| usage(format!("`--box {text}` is not four numbers")))?;
        let bbox = Bbox::new(corners).map_err(usage)?;
        let name = format!("Box {text}");
        defined(
            settings,
            Region { id: "box".into(), name, area: Area::Box { bbox }, countries: Vec::new(), time_zone: None },
        )?;
    }
    Ok(())
}

/// Apply the flags of `args` to `settings`.
fn edit(root: &Path, products: &[&dyn Product], args: &UpdateArgs, settings: &mut Settings) -> Result<(), Error> {
    let usage = |message: String| Code::Usage.error(message);
    select(root, (args.region.as_deref(), args.bbox.as_deref()), settings)?;
    for layer in &args.add {
        if !products.iter().any(|product| product.optional().contains(&layer.as_str())) {
            return Err(usage(format!("no product has the optional layer `{layer}`")));
        }
        if !settings.layers.contains(layer) {
            settings.layers.push(layer.clone());
        }
    }
    settings.layers.retain(|layer| !args.remove.contains(layer));
    settings.layers.sort();
    let sources = crate::sources::all();
    for policy in &args.refresh {
        let (id, refresh) =
            policy.split_once('=').ok_or_else(|| usage(format!("`--refresh {policy}` is not SOURCE=DAYS|manual")))?;
        if !sources.iter().any(|source| source.id == id) {
            return Err(usage(format!("no source `{id}`")));
        }
        let refresh: Refresh = refresh.parse().map_err(|e: String| usage(format!("`--refresh {policy}`: {e}")))?;
        settings.refresh.insert(id.into(), refresh);
    }
    Ok(())
}

/// Select `region`, a definition that no file of `data/regions/` holds.
fn defined(settings: &mut Settings, region: Region) -> Result<(), Error> {
    let id = region.id.clone();
    let regions = Regions::new(vec![region]).map_err(|e| Code::Usage.error(e))?;
    settings.select(&regions, &id).map_err(|e| Code::Usage.error(e))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline, snapshot, write, Fixture};
    use crate::product::{Pointer, Steps, Unplanned};
    use crate::release::POINTERS;

    /// The test pipeline: `test/upper` reads the version of `head` that the update chose.
    struct Pipeline;

    impl Product for Pipeline {
        fn name(&self) -> &'static str {
            "test"
        }

        fn optional(&self) -> &'static [&'static str] {
            &["extra"]
        }

        fn steps(&self, root: &Path, inputs: &Inputs, _: &Regions, store: &Store) -> Result<Steps, Unplanned> {
            let head = crate::product::version(inputs, store, "head", &[]).map_err(Unplanned::Failed)?;
            let head = head.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            // The file `listed` makes the step list read `head` per tile, as the device reads a
            // national terrain model. Its step reads `head` without params.
            let files = crate::engine::snapshot_files(store, "head", &head, &[], &[]).map_err(Unplanned::Failed)?;
            if root.join("listed").exists() && files.is_none() {
                let params = vec![("tile".into(), "0001-0002".into())];
                let wanted = Wanted { source: "head".into(), version: Some(head), params };
                return Err(Unplanned::NeedsFetch(vec![wanted]));
            }
            let mut steps = pipeline();
            steps[2].inputs = vec![snapshot("head", &head, &["head.txt"])];
            crate::engine::tests::revise(root, &mut steps);
            // The file `upper-internal` gives `test/upper` no client file.
            if root.join("upper-internal").exists() {
                steps[2].client = crate::engine::Client::None;
            }
            // The file `format-2` gives the layer `test` a new format.
            static FORMAT_2: crate::engine::Layer = crate::engine::Layer { format: 2, ..crate::engine::tests::TEST };
            if root.join("format-2").exists() {
                steps.iter_mut().filter(|step| step.layer.name == "test").for_each(|step| step.layer = &FORMAT_2);
            }
            Ok(steps.into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: POINTERS[0], step: "test/join", path: "joined.txt" }
        }
    }

    /// A product whose pointer is the count of the pipeline. The file `earth` gives it a step that
    /// reads `natural-earth`, a source whose versions have no order.
    struct Counter;

    impl Product for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }

        fn steps(&self, root: &Path, inputs: &Inputs, _: &Regions, store: &Store) -> Result<Steps, Unplanned> {
            if root.join("leaves").exists() {
                return leaves(root, inputs, store);
            }
            if root.join("gate").exists() {
                static GATE: crate::engine::Layer = crate::engine::Layer { name: "gate", ..crate::engine::tests::TEST };
                let mut step =
                    crate::engine::tests::step("counter/gate", Vec::new(), "gate.txt", crate::engine::Run::Rust(gate));
                (step.layer, step.options) =
                    (&GATE, serde_json::json!({ "gate": root.join("gate").display().to_string() }));
                return Ok(vec![step].into());
            }
            if !root.join("earth").exists() {
                return Ok(Vec::new().into());
            }
            let version = crate::product::version(inputs, store, "natural-earth", &[]).map_err(Unplanned::Failed)?;
            let version = version.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            static EARTH: crate::engine::Layer = crate::engine::Layer { name: "earth", ..crate::engine::tests::TEST };
            let read = vec![snapshot("natural-earth", &version, &["earth.zip"])];
            let mut step =
                crate::engine::tests::step("counter/earth", read, "earth.zip", crate::engine::Run::Rust(earth));
            step.layer = &EARTH;
            Ok(vec![step].into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: POINTERS[1], step: "test/count", path: "count/lines.txt" }
        }
    }

    pub(in crate::cli) fn update(
        fixture: &Fixture,
        target: &Target,
        edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    ) -> Result<Updated, Error> {
        update_with(fixture, target, &Http::new(), edit)
    }

    fn update_with(
        fixture: &Fixture,
        target: &Target,
        http: &Http,
        edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    ) -> Result<Updated, Error> {
        asked(fixture, target, http, (&Asked::default(), &MachineCheck::default()), edit)
    }

    /// An update with `asked` on `machine`.
    fn asked(
        fixture: &Fixture,
        target: &Target,
        http: &Http,
        (asked, machine): (&Asked, &MachineCheck),
        edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    ) -> Result<Updated, Error> {
        let mut run = Run::create(&fixture.store, ["update", "local"]).unwrap();
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let updated = super::update(
            &root,
            &fixture.store,
            http,
            ("local", target),
            (&products, &[]),
            edit,
            asked,
            Some(machine),
            &mut run,
        );
        crate::cli::api::finish_run(run, updated)
    }

    /// A step that writes `waiting` in the folder `gate` of its options, and waits for `open` there.
    fn gate(request: &crate::engine::Request) -> Result<(), String> {
        let gate = Path::new(request.options["gate"].as_str().unwrap());
        write(&gate.join("waiting"), "");
        arrives(&gate.join("open"));
        std::fs::write(request.output.join("gate.txt"), "open").map_err(|e| e.to_string())
    }

    /// Wait until `path` exists, for 30 seconds at most.
    fn arrives(path: &Path) {
        let start = std::time::Instant::now();
        while !path.exists() {
            assert!(start.elapsed().as_secs() < 30, "{} did not come", path.display());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Writes the file `open` of a gate when dropped, so a test that fails does not wait.
    struct Opens(std::path::PathBuf);

    impl Drop for Opens {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, "");
        }
    }

    /// The internal step `counter/split` writes line `n` of `head` as `split/0001-000<n>`, and the
    /// tiled layer `leaves` copies each of the two leaves. The file `split-2` gives the split a new
    /// revision, and the file `one-leaf` drops the second leaf.
    fn leaves(root: &Path, inputs: &Inputs, store: &Store) -> Result<Steps, Unplanned> {
        use crate::engine::{tests::step, Client, Layer, Run, Shape};
        let head = crate::product::version(inputs, store, "head", &[]).map_err(Unplanned::Failed)?;
        let head = head.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
        static SPLIT: Layer = Layer { name: "split", readers: &[], ..crate::engine::tests::TEST };
        static SPLIT_2: Layer = Layer { revision: 2, ..SPLIT };
        static LEAVES: Layer = Layer { name: "leaves", shape: Shape::Tiled, ..crate::engine::tests::TEST };
        let mut split = step("counter/split", vec![snapshot("head", &head, &["head.txt"])], "split", Run::Rust(split));
        (split.layer, split.client) = (if root.join("split-2").exists() { &SPLIT_2 } else { &SPLIT }, Client::None);
        let leaf = |leaf: &str| {
            let read = Input::Step { name: "counter/split".into(), files: vec![format!("split/{leaf}")] };
            Step { layer: &LEAVES, ..step(&format!("counter/leaf/{leaf}"), vec![read], "leaf.txt", Run::Rust(copy)) }
        };
        let second = (!root.join("one-leaf").exists()).then(|| leaf("0001-0002"));
        Ok([split, leaf("0001-0001")].into_iter().chain(second).collect::<Vec<_>>().into())
    }

    fn split(request: &crate::engine::Request) -> Result<(), String> {
        let head = std::fs::read_to_string(&request.snapshots["head"]["head.txt"]).map_err(|e| e.to_string())?;
        std::fs::create_dir(request.output.join("split")).map_err(|e| e.to_string())?;
        for n in 1..=2 {
            let line = head.lines().nth(n - 1).unwrap_or_default();
            std::fs::write(request.output.join(format!("split/0001-000{n}")), line).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn copy(request: &crate::engine::Request) -> Result<(), String> {
        let read = request.layers["counter/split"].values().next().ok_or("no leaf")?;
        std::fs::copy(read, request.output.join("leaf.txt")).map(drop).map_err(|e| e.to_string())
    }

    fn earth(request: &crate::engine::Request) -> Result<(), String> {
        let copied = std::fs::copy(&request.snapshots["natural-earth"]["earth.zip"], request.output.join("earth.zip"));
        copied.map(|_| ()).map_err(|e| e.to_string())
    }

    fn asked_update(fixture: &Fixture, target: &Target, asked: &Asked) -> Result<Updated, Error> {
        self::asked(fixture, target, &Http::new(), (asked, &MachineCheck::default()), |_| Ok(()))
    }

    /// The review of an update without flags, as `--dry-run` gives it.
    fn review(fixture: &Fixture, target: &Target) -> Review {
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let machine = MachineCheck::default();
        let prepared = prepare(
            &root,
            &fixture.store,
            &Http::new(),
            ("local", target),
            (&products, &[]),
            |_| Ok(()),
            &[],
            Some(&machine),
            None,
        );
        prepared.unwrap().review
    }

    pub(in crate::cli) fn region(settings: &mut Settings) -> Result<(), Error> {
        let area = Area::Box { bbox: Bbox::new([7.0, 47.0, 8.0, 48.0]).unwrap() };
        defined(settings, Region { id: "box".into(), name: "Box".into(), area, countries: Vec::new(), time_zone: None })
    }

    fn steps(updated: &Updated) -> Vec<&str> {
        updated.baked.iter().map(|baked| baked.step.as_str()).collect()
    }

    #[test]
    fn an_update_bakes_what_is_stale_keeps_the_rest_by_reference_and_remembers_its_settings() {
        let fixture = fixture("update-local");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let refused = update(&fixture, &target, |_| Ok(())).unwrap_err();
        assert_eq!((refused.code, refused.message.as_str()), (Code::Usage, "target `local` has no region"));

        let first = update(&fixture, &target, |settings| {
            settings.layers.push("extra".into());
            region(settings)
        })
        .unwrap();
        assert_eq!((steps(&first), first.rollback.as_ref()), (vec!["test/upper", "test/join", "test/count"], None));
        let release = Release::read(&target, &first.serving).unwrap();
        assert_eq!(
            (release.settings.region.as_str(), release.settings.layers.as_slice()),
            ("box", &["extra".to_string()][..])
        );
        assert!(release::drift(&target, &release).unwrap().is_empty());

        let same = update(&fixture, &target, |_| Ok(())).unwrap();
        assert_eq!((&same.serving, steps(&same), same.kept), (&first.serving, Vec::new(), 3), "nothing is stale");

        write(&fixture.root().join("revised"), "test/join");
        let revised = update(&fixture, &target, |_| Ok(())).unwrap();
        // `test/join` writes the same bytes, so `test/count` keeps its output.
        assert_eq!((steps(&revised), revised.kept), (vec!["test/join", "test/count"], 2));
        assert_eq!(revised.rollback.as_ref(), Some(&first.serving));
        assert_eq!(Release::read(&target, &revised.serving).unwrap().settings, release.settings, "remembered");

        // A newer version of a source in the store moves the steps that read it.
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        let moved = update(&fixture, &target, |_| Ok(())).unwrap();
        assert_eq!(steps(&moved), ["test/upper", "test/join", "test/count"]);
        let serving = release::serving(&target).unwrap().unwrap().0;
        assert_eq!(
            (serving.serving.id, serving.rollback.map(|role| role.id)),
            (moved.serving.clone(), Some(revised.serving))
        );

        // An older `obc data` wrote a manifest without `sources`.
        let key = release::manifest_key(&moved.serving);
        let mut older: serde_json::Value = serde_json::from_slice(&target.get(&key).unwrap().unwrap()).unwrap();
        older.as_object_mut().unwrap().remove("sources");
        let bytes = serde_json::to_vec(&older).unwrap();
        let id = crate::store::sha256_hex(&bytes);
        let any = crate::target::Condition::Any;
        target.put(&release::manifest_key(&id), &bytes, &any).unwrap();
        let named =
            release::Serving { serving: release::Role { id, since: "2026-10-12T00:00:00Z".into() }, rollback: None };
        target.put(release::SERVING, &serde_json::to_vec(&named).unwrap(), &any).unwrap();
        let refused = update(&fixture, &target, |_| Ok(())).unwrap_err();
        assert_eq!(refused.code, Code::InvalidData, "{}", refused.message);
        assert!(refused.fix.contains("local.old"), "{}", refused.fix);
    }

    #[test]
    fn a_plan_run_writes_the_review_that_its_view_gives_and_bakes_nothing() {
        let fixture = fixture("update-plan");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let args = UpdateArgs {
            target: "local".into(),
            region: None,
            bbox: None,
            add: Vec::new(),
            remove: Vec::new(),
            refresh: Vec::new(),
            only: Vec::new(),
            dry_run: false,
            expect: None,
            plan: true,
            accept_format_change: false,
            yes: false,
        };
        let mut run = Run::create(&fixture.store, ["update", "local", "--plan"]).unwrap();
        let id = run.id().to_string();
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let planned =
            plan(&root, &fixture.store, &Http::new(), ("local", &target), (&products, &[]), region, &args, &mut run);
        let review = crate::cli::api::finish_run(run, planned).unwrap();
        assert_eq!(review.args, ["update", "local", "--expect", &review.id]);
        let view = runs::view(&fixture.store, &id).unwrap();
        assert_eq!(view.review.unwrap()["id"], review.id.as_str());
        assert!(release::serving(&target).unwrap().is_none(), "a plan bakes nothing");
    }

    #[test]
    fn a_geofabrik_area_takes_the_countries_of_the_region_that_reads_only_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut settings = Settings::default();
        select(&root, (Some("europe/dach"), None), &mut settings).unwrap();
        let regions = settings.regions().unwrap();
        let region = regions.get("europe/dach").unwrap();
        assert_eq!(
            (region.countries.as_slice(), region.time_zone.as_deref()),
            (&["DE", "AT", "CH", "LI"].map(String::from)[..], Some("Europe/Berlin"))
        );
    }

    #[test]
    fn a_due_source_moves_to_the_newer_version_that_upstream_names() {
        use crate::fetch::tests::{serve, whole};
        // Upstream gives `head.txt` with the day 2026-10-05.
        let (actual, _) = serve(|_, _| whole(b"newer head\n"));
        let actual = actual.replacen("file.bin", "head.txt", 1);
        let fixture = fixture("update-due");
        let url = actual.replacen("http://", "https://", 1);
        write(
            &fixture.root().join("data/sources.toml"),
            &format!(
                "[[source]]\nid = \"head\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Head\"\n\
                 fetch = {{ kind = \"http\", url = \"{url}\" }}\nversion = \"date\"\nrefresh = 1\nredistribute = true\n"
            ),
        );
        fixture.fetched_version("head", "2026-01-01", "head.txt", b"head\n");
        let (target, http) = (Target::Dir(fixture.scratch.0.join("local")), Http::loopback(&actual));
        let first = update_with(&fixture, &target, &http, region).unwrap();
        let read = |id: &str| Release::read(&target, id).unwrap().versions()[&("head".to_string(), Vec::new())].clone();
        assert_eq!(read(&first.serving), "2026-01-01");

        let moved = update_with(&fixture, &target, &http, |_| Ok(())).unwrap();
        assert_eq!(steps(&moved), ["test/upper", "test/join", "test/count"]);
        assert_eq!(read(&moved.serving), "2026-10-05", "the policy is due, and upstream names a newer day");
    }

    #[test]
    fn a_clean_keeps_what_each_target_serves_and_its_rollback_and_reads_every_target() {
        let fixture = fixture("update-clean");
        fixture.with_sources(&[]);
        let dir = fixture.scratch.0.join("local");
        write(
            &fixture.root().join("data/targets.toml"),
            &format!("[local]\nkind = \"dir\"\ndir = \"{}\"\n", dir.display()),
        );
        let target = Target::Dir(dir.clone());
        update(&fixture, &target, region).unwrap();
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        update(&fixture, &target, |_| Ok(())).unwrap();
        let roots = crate::cli::roots(&fixture.root(), &fixture.store).unwrap();
        for version in ["1", "2"] {
            assert_eq!(roots.served[&("head".to_string(), version.to_string())], ["local"], "head {version}");
        }

        std::fs::write(dir.join(release::SERVING), b"not json").unwrap();
        let refused = crate::cli::roots(&fixture.root(), &fixture.store).unwrap_err();
        assert!(refused.message.starts_with("target `local`"), "a clean deletes nothing: {}", refused.message);
    }

    /// Declare the sources `ids`, which a target copies.
    fn copied(fixture: &Fixture, ids: &[&str]) {
        let sources = ids.iter().map(|id| {
            let mut source = crate::fetch::tests::source(&format!("https://example.org/{id}.txt"), "date");
            source.id = id.to_string();
            source
        });
        fixture.with_sources(&sources.collect::<Vec<_>>());
    }

    /// An update without flags on another machine: its store has no objects, records or receipts,
    /// and takes what it lacks from `target`.
    fn elsewhere(fixture: &Fixture, target: &Target) -> (Store, Result<Updated, Error>) {
        for dir in ["objects", "snapshots", "requests", "layers"] {
            let _ = std::fs::remove_dir_all(fixture.store.root().join(dir));
        }
        let store = Store::at(fixture.store.root()).reading(target);
        let mut run = Run::create(&store, ["update", "local"]).unwrap();
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let (asked, machine) = (Asked::default(), MachineCheck::default());
        let updated = super::update(
            &root,
            &store,
            &Http::new(),
            ("local", target),
            (&products, &[]),
            |_| Ok(()),
            &asked,
            Some(&machine),
            &mut run,
        );
        (store, crate::cli::api::finish_run(run, updated))
    }

    /// Releases A, B and C of `target`, from `head` 1, 2 and 3: C serves, and B is its rollback.
    /// The target copies the files of `head`.
    fn three_releases(fixture: &Fixture, target: &Target) -> (String, String, Updated) {
        copied(fixture, &["head"]);
        let a = update(fixture, target, region).unwrap().serving;
        fixture.fetched_version("head", "2", "head.txt", b"2\n");
        let b = update(fixture, target, |_| Ok(())).unwrap().serving;
        fixture.fetched_version("head", "3", "head.txt", b"3\n");
        (a, b, update(fixture, target, |_| Ok(())).unwrap())
    }

    fn keys(target: &Target) -> std::collections::BTreeSet<String> {
        target.list().unwrap().into_iter().map(|listed| listed.key).collect()
    }

    /// A collection after the grace of a release that stopped serving now.
    fn late(target: &Target) -> Collected {
        let now = crate::date::now();
        collection::collect(target, Some(&crate::date::timestamp(now)), now + collection::GRACE + 60, &|| Ok(()))
    }

    #[test]
    fn a_collection_after_the_grace_deletes_dropped_releases_and_orphans_and_keeps_older_keys() {
        use std::collections::BTreeSet;
        let fixture = fixture("update-collect");
        let dir = fixture.scratch.0.join("local");
        let target = Target::Dir(dir.clone());
        let (a, b, c) = three_releases(&fixture, &target);
        let collected = c.collection.unwrap();
        assert_eq!((collected.deferred, collected.keys), (vec![a.clone()], 0), "a stopped serving just now");

        let files = |id: &str| -> BTreeSet<String> {
            Release::read(&target, id).unwrap().files().into_iter().map(|file| file.key).collect()
        };
        let mut only_a: BTreeSet<String> = &(&files(&a) - &files(&b)) - &files(&c.serving);
        assert!(only_a.contains("sources/head/1/head.txt"), "the copy of a source file: {only_a:?}");
        assert!(!files(&a).iter().any(|key| key.starts_with("sources/tail/")), "tail is not declared");
        only_a.insert(release::manifest_key(&a));
        // `sources/x/y` is a key that no release names, such as a prepared source.
        let older =
            ["reference/v1/x", "cell-catalog/cells/x", "planner/objects/x", "reference/v1/.x.part", "sources/x/y"];
        let orphans = ["objects/0000", "objects/.1.2.part", ".release.json.1.tmp", "cell-catalog/.catalog.json.1.tmp"];
        for key in older.iter().chain(&orphans) {
            write(&dir.join(key), key);
        }
        let before = keys(&target);
        assert!(before.contains("sources/head/1/head.txt"), "the publish copies it");

        // A collection that stopped after one file resumes from the manifest.
        let first = only_a.iter().find(|key| key.starts_with("objects/")).unwrap().clone();
        target.remove(&first).unwrap();
        let late = late(&target);
        assert_eq!((late.releases, late.stopped), (vec![a], None));
        assert_eq!(late.keys as usize, only_a.len() - 1 + orphans.len());
        let gone: BTreeSet<String> = only_a.into_iter().chain(orphans.map(String::from)).collect();
        assert_eq!(keys(&target), &before - &gone);
    }

    #[test]
    fn a_collection_deletes_nothing_while_a_pointer_drifts_a_manifest_cannot_be_read_or_the_lock_is_lost() {
        let fixture = fixture("update-collect-guards");
        let dir = fixture.scratch.0.join("local");
        let target = Target::Dir(dir.clone());
        three_releases(&fixture, &target);
        let before = keys(&target);
        let pointer = dir.join(POINTERS[0]);
        let saved = std::fs::read(&pointer).unwrap();
        std::fs::write(&pointer, b"another publisher").unwrap();
        let drifted = late(&target);
        assert_eq!(drifted.stopped.as_deref(), Some("cell-catalog/catalog.json is not of the serving release"));
        assert_eq!((drifted.keys, keys(&target)), (0, before.clone()));

        std::fs::write(&pointer, saved).unwrap();
        let unreadable = release::manifest_key(&"f".repeat(64));
        write(&dir.join(&unreadable), "not a manifest");
        let unknown = late(&target);
        assert!(unknown.stopped.unwrap().starts_with("the files of a release are not known"));
        assert_eq!((unknown.keys, keys(&target).len()), (0, before.len() + 1));

        target.remove(&unreadable).unwrap();
        let now = crate::date::now();
        let lost = || Err("another run took the lock".to_string());
        let lost =
            collection::collect(&target, Some(&crate::date::timestamp(now)), now + collection::GRACE + 60, &lost);
        assert_eq!(lost.stopped.as_deref(), Some("another run took the lock"));
        assert_eq!((lost.keys, keys(&target)), (0, before));
    }

    #[test]
    fn an_update_starts_only_when_its_review_is_the_one_that_was_read() {
        let fixture = fixture("update-review");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        let read = review(&fixture, &target);
        assert!(read.layers.is_empty(), "nothing is stale");

        // A newer version of `head` comes after the review was read.
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        let expect = |id: &str| Asked { expect: Some(id.into()), ..Asked::default() };
        let refused = asked_update(&fixture, &target, &expect(&read.id)).unwrap_err();
        assert_eq!(refused.code, Code::Usage, "{}", refused.message);
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, first.serving, "nothing changed");

        let review = review(&fixture, &target);
        let moved = &review.sources[0];
        assert_eq!((moved.source.as_str(), moved.from.as_deref(), moved.to.as_str()), ("head", Some("1"), "2"));
        assert_eq!((review.layers.len(), review.layers[0].text.as_str()), (1, "head 1 → 2"));
        let updated = asked_update(&fixture, &target, &expect(&review.id)).unwrap();
        assert_eq!((&updated.review, updated.rollback.as_ref()), (&review.id, Some(&first.serving)));
    }

    #[test]
    fn the_upload_of_a_review_counts_the_copies_that_the_serving_release_does_not_name() {
        let fixture = fixture("update-upload");
        copied(&fixture, &["head"]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        update(&fixture, &target, region).unwrap();
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        let review = review(&fixture, &target);
        let bakes: u64 = review.bakes.iter().filter_map(|bake| bake.estimate).map(|bake| bake.client_bytes).sum();
        assert_eq!(review.cost.upload_bytes, bakes + 6, "with the copy of head 2");
    }

    #[test]
    fn a_format_change_starts_only_with_its_acknowledgement() {
        let fixture = fixture("update-format");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        write(&fixture.root().join("format-2"), "");
        let format = review(&fixture, &target).layers[0].format.clone().unwrap();
        let effects: Vec<_> =
            format.readers.iter().map(|reader| (reader.reader.as_str(), reader.effect.as_str())).collect();
        assert_eq!((format.from, format.to, &effects[..]), (1, 2, &[("tests", "The tests read it.")][..]));
        let refused = asked_update(&fixture, &target, &Asked::default()).unwrap_err();
        assert_eq!(refused.code, Code::Usage);
        assert!(refused.message.contains("test 1 → 2 (read by tests)"), "{}", refused.message);
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, first.serving, "nothing changed");
        let accepted = Asked { accept_format_change: true, ..Asked::default() };
        assert_eq!(asked_update(&fixture, &target, &accepted).unwrap().rollback, Some(first.serving));
    }

    #[test]
    fn a_machine_with_less_memory_or_disk_than_the_estimates_refuses_the_update() {
        let fixture = fixture("update-machine");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let checked = |edit: fn(&mut Settings) -> Result<(), Error>, machine: &MachineCheck| {
            let prepared = prepare(
                &root,
                &fixture.store,
                &Http::new(),
                ("local", &target),
                (&products, &[]),
                edit,
                &[],
                Some(machine),
                None,
            );
            prepared.unwrap().review
        };
        let roomy = MachineCheck { memory_bytes: None, store_free_bytes: Some(u64::MAX / 2), refusals: Vec::new() };
        let unknown = checked(region, &roomy).cost.unknown;
        assert_eq!(unknown.estimate, ["test/upper", "test/join", "test/count"]);
        let first = update(&fixture, &target, region).unwrap();

        // `test/join` runs Python: its receipt has a peak.
        write(&fixture.root().join("revised"), "test/join");
        let estimated = checked(|_| Ok(()), &roomy);
        let peak =
            estimated.bakes.iter().find(|bake| bake.step == "test/join").unwrap().estimate.unwrap().peak_rss_bytes;
        let (bytes, peak) = (estimated.cost.store_bytes, peak.unwrap());
        let disk = |free: u64| MachineCheck { store_free_bytes: Some(crate::store::RESERVE + free), ..roomy.clone() };
        assert!(checked(|_| Ok(()), &disk(bytes)).machine.refusals.is_empty(), "room for the estimate");
        let memory = MachineCheck { memory_bytes: Some(peak - 1), ..roomy.clone() };
        assert!(checked(|_| Ok(()), &memory).machine.refusals[0].starts_with("step `test/join` needs about"));

        let layers = fixture.store.layers().unwrap().len();
        let refused = asked(&fixture, &target, &Http::new(), (&Asked::default(), &disk(bytes - 1)), |_| Ok(()));
        let refused = refused.unwrap_err();
        assert_eq!(refused.code, Code::Blocked);
        assert!(
            refused.message.starts_with("this machine cannot do the update: the store gets"),
            "{}",
            refused.message
        );
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, first.serving);
        assert_eq!(fixture.store.layers().unwrap().len(), layers, "nothing baked");
    }

    #[test]
    fn a_source_whose_versions_have_no_order_moves_only_after_a_refresh() {
        use crate::fetch::tests::{serve, whole};
        use crate::store::sha256_hex;
        let (actual, _) = serve(|_, _| whole(b"earth 2\n"));
        let actual = actual.replacen("file.bin", "earth.zip", 1);
        let fixture = fixture("update-refresh");
        let url = actual.replacen("http://", "https://", 1);
        write(
            &fixture.root().join("data/sources.toml"),
            &format!(
                "[[source]]\nid = \"natural-earth\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Earth\"\n\
                 fetch = {{ kind = \"http\", url = \"{url}\" }}\nversion = \"digest\"\nredistribute = true\n"
            ),
        );
        write(&fixture.root().join("earth"), "");
        let dir = fixture.scratch.0.join("local");
        write(
            &fixture.root().join("data/targets.toml"),
            &format!("[local]\nkind = \"dir\"\ndir = \"{}\"\n", dir.display()),
        );
        let (one, two) = (sha256_hex(b"earth 1\n"), sha256_hex(b"earth 2\n"));
        fixture.fetched_version("natural-earth", &one, "earth.zip", b"earth 1\n");
        let (target, http) = (Target::Dir(dir), Http::loopback(&actual));
        let first = update_with(&fixture, &target, &http, region).unwrap();

        let registry = crate::sources::Registry::load(&fixture.root()).unwrap();
        let request =
            crate::fetch::Request { refresh: false, source: &registry.sources[0], version: None, params: vec![] };
        assert_eq!(crate::fetch::fetch(&fixture.root(), &fixture.store, &http, &request).unwrap().version, two);
        let same = update_with(&fixture, &target, &http, |_| Ok(())).unwrap();
        assert_eq!((&same.serving, steps(&same)), (&first.serving, Vec::new()), "a newer file in the store");

        let refreshed = crate::cli::source_cli::refresh(&fixture.root(), &fixture.store, &http, "natural-earth");
        let refreshed = refreshed.unwrap();
        assert_eq!((refreshed.from, refreshed.version), (vec![one.clone()], two.clone()), "from what `local` serves");
        let moved = &review(&fixture, &target).sources[0];
        assert_eq!((moved.source.as_str(), moved.from.as_ref(), &moved.to), ("natural-earth", Some(&one), &two));
        assert_eq!(steps(&update_with(&fixture, &target, &http, |_| Ok(())).unwrap()), ["counter/earth"]);
    }

    #[test]
    fn the_review_names_the_leaves_that_no_step_makes_now() {
        let fixture = fixture("update-dropped-leaf");
        copied(&fixture, &["head", "tail"]);
        write(&fixture.root().join("leaves"), "");
        let target = Target::Dir(fixture.scratch.0.join("local"));
        update(&fixture, &target, region).unwrap();
        write(&fixture.root().join("one-leaf"), "");
        let review = review(&fixture, &target);
        let layer = review.layers.iter().find(|layer| layer.layer == "leaves").unwrap();
        assert_eq!(
            (&layer.drops[..], &layer.dropped_leaves[..]),
            (&["counter/leaf/0001-0002".into()][..], &["0001-0002".into()][..])
        );
    }

    #[test]
    fn a_refresh_bakes_only_the_leaves_whose_input_changed_also_on_another_machine() {
        let fixture = fixture("update-leaves");
        copied(&fixture, &["head", "tail"]);
        write(&fixture.root().join("leaves"), "");
        let target = Target::Dir(fixture.scratch.0.join("local"));
        update(&fixture, &target, region).unwrap();
        let unchanged = |updated: &Updated| -> BTreeMap<String, bool> {
            let leaves = updated.baked.iter().filter(|baked| baked.step.starts_with("counter/leaf/"));
            leaves.map(|baked| (baked.step.clone(), baked.unchanged)).collect()
        };
        let leaves = |store: &Store| {
            let id = runs::list(store).unwrap().remove(0).id;
            let log = std::fs::read_to_string(store.root().join("runs").join(&id).join("log")).unwrap();
            let status = runs::status(store, &id).unwrap();
            let layer = status.layers.into_iter().find(|layer| layer.layer == "leaves").unwrap();
            (layer.leaves_unchanged, log.lines().find(|line| line.contains(" leaves baked")).unwrap()[9..].to_string())
        };

        // Only the second line of `head` changes.
        fixture.fetched_version("head", "2", "head.txt", b"head\nb\n");
        let refreshed = update(&fixture, &target, |_| Ok(())).unwrap();
        let expected = |first: bool| {
            BTreeMap::from([("counter/leaf/0001-0001".into(), true), ("counter/leaf/0001-0002".into(), first)])
        };
        assert_eq!(unchanged(&refreshed), expected(false));
        assert_eq!(
            leaves(&fixture.store),
            (vec!["0001-0001".into()], "leaves: 1 of 2 leaves baked, 1 unchanged".into())
        );

        // Another machine has no layer of the store: a new revision of the split gives the same
        // leaves, which keep the outputs of the serving release.
        write(&fixture.root().join("split-2"), "");
        let (store, elsewhere) = elsewhere(&fixture, &target);
        let elsewhere = elsewhere.unwrap();
        assert_eq!(unchanged(&elsewhere), expected(true));
        assert_eq!(leaves(&store).1, "leaves: 0 of 2 leaves baked, 2 unchanged");
        let outputs = |id: &str| -> Vec<Output> {
            let release = Release::read(&target, id).unwrap();
            release.outputs.into_iter().filter(|output| output.layer == "leaves").collect()
        };
        assert_eq!(outputs(&elsewhere.serving), outputs(&refreshed.serving), "kept by reference");
    }

    #[test]
    fn an_unchanged_step_takes_the_versions_and_the_client_of_its_step_now() {
        let fixture = fixture("update-settles");
        fixture.with_sources(&[]);
        write(&fixture.root().join("upper-internal"), "");
        let target = Target::Dir(fixture.scratch.0.join("local"));
        update(&fixture, &target, region).unwrap();

        // `head` 2 has the bytes of 1, `test/upper` gets a client file, and the store has no layer.
        fixture.fetched_version("head", "2", "head.txt", b"head\n");
        std::fs::remove_file(fixture.root().join("upper-internal")).unwrap();
        std::fs::remove_dir_all(fixture.store.root().join("layers")).unwrap();
        let moved = update(&fixture, &target, |_| Ok(())).unwrap();
        assert!(!moved.baked.is_empty() && moved.baked.iter().all(|baked| baked.unchanged), "{:?}", moved.baked);
        let release = Release::read(&target, &moved.serving).unwrap();
        let upper = release.outputs.iter().find(|output| output.step == "test/upper").unwrap();
        assert_eq!((upper.snapshots["head"].version.as_str(), &upper.client), ("2", &crate::engine::Client::All));
        let file = &upper.files[0];
        assert!(target.missing(&[(format!("objects/{}", file.sha256), file.size)]).unwrap().is_empty());
        let settled = review(&fixture, &target);
        assert!(settled.layers.is_empty() && settled.bakes.is_empty(), "{:?}", settled.layers);
    }

    #[test]
    fn a_machine_without_the_files_of_the_release_downloads_only_what_its_bakes_read() {
        let fixture = fixture("update-elsewhere");
        copied(&fixture, &["head", "tail"]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        let file = |release: &Release, step: &str| {
            release.outputs.iter().find(|output| output.step == step).unwrap().files[0].sha256.clone()
        };
        let upper = file(&Release::read(&target, &first.serving).unwrap(), "test/upper");

        write(&fixture.root().join("revised"), "test/join");
        let (store, updated) = elsewhere(&fixture, &target);
        let updated = updated.unwrap();
        assert_eq!(steps(&updated), ["test/join", "test/count"]);
        let release = Release::read(&target, &updated.serving).unwrap();
        let tail = &store.snapshot("tail", "1").unwrap().unwrap().files[0].sha256;
        let objects: BTreeSet<String> = [file(&release, "test/join")].into();
        let downloads: BTreeSet<String> = [upper, tail.clone()].into();
        let folders = std::fs::read_dir(store.root().join("objects")).unwrap();
        let held: BTreeSet<String> = folders
            .flat_map(|folder| std::fs::read_dir(folder.unwrap().path()).unwrap())
            .map(|object| object.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let read = "test/join reads upper.txt and tail@1, test/count keeps its output, and nothing reads head@1";
        assert_eq!(held, &objects | &downloads, "{read}");
    }

    #[test]
    fn a_bake_that_reads_a_file_that_no_client_reads_bakes_its_step_again_on_a_machine_without_it() {
        let fixture = fixture("update-unheld");
        copied(&fixture, &["head", "tail"]);
        write(&fixture.root().join("upper-internal"), "");
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        write(&fixture.root().join("revised"), "test/join");
        let (store, updated) = elsewhere(&fixture, &target);
        let updated = updated.unwrap();
        assert_eq!(steps(&updated), ["test/upper", "test/join", "test/count"]);
        assert!(store.snapshot("head", "1").unwrap().is_some(), "from the copy on the target");
        assert_eq!(updated.rollback, Some(first.serving));
    }

    #[test]
    fn a_machine_that_cannot_fetch_a_capture_that_the_release_read_refuses_while_it_lists() {
        let fixture = fixture("update-capture");
        let target = captured(&fixture);
        let refused = elsewhere(&fixture, &target).1.unwrap_err();
        assert_eq!(refused.code, Code::Blocked, "{}", refused.message);
        assert!(refused.message.starts_with("this machine cannot fetch head@1"), "{}", refused.message);
        assert!(refused.fix.starts_with(&format!("Run this update on `{}`", crate::machine())), "{}", refused.fix);
    }

    #[test]
    fn a_capture_version_that_the_store_started_takes_a_new_tile() {
        let fixture = fixture("update-capture-tile");
        let target = captured(&fixture);
        // The store started head@1, and lacks the file of the tile that the listing asks for.
        let record = fixture.store.snapshot("head", "1").unwrap().unwrap();
        std::fs::remove_file(fixture.store.object(&record.files[0].sha256)).unwrap();
        let error = update(&fixture, &target, |_| Ok(())).unwrap_err();
        assert!(error.message.contains("has no fetcher yet"), "the capture runs: {}", error.message);
    }

    /// A target that serves a release that read `head`, a manual capture that the target does not
    /// copy, with a step list that asks for `head` per tile.
    fn captured(fixture: &Fixture) -> Target {
        let mut head = crate::fetch::tests::source("https://example.org/head.txt", "date");
        (head.id, head.fetch.kind, head.redistribute) = ("head".into(), FetchKind::Capture, false);
        fixture.with_sources(&[head]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        update(fixture, &target, region).unwrap();
        write(&fixture.root().join("listed"), "");
        target
    }

    #[test]
    fn a_file_that_a_machine_retrieved_again_keeps_its_entry_and_switches_nothing() {
        let fixture = fixture("update-retrieved");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        let mut record = fixture.store.snapshot("head", "1").unwrap().unwrap();
        record.files[0].retrieved = crate::date::timestamp(0);
        fixture.store.put_snapshot(&record).unwrap();
        assert_eq!(update(&fixture, &target, |_| Ok(())).unwrap().serving, first.serving, "nothing switched");

        write(&fixture.root().join("revised"), "test/upper");
        let revised = update(&fixture, &target, |_| Ok(())).unwrap();
        let head = |id: &str| Release::read(&target, id).unwrap().sources.into_iter().find(|f| f.source == "head");
        assert_eq!(head(&revised.serving).unwrap().files, head(&first.serving).unwrap().files);
    }

    #[test]
    fn a_store_with_other_bytes_of_a_fetch_than_the_release_read_bakes_nothing() {
        let fixture = fixture("update-bytes");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        // This store holds other bytes of head@1 than the store of the machine that made the release.
        let mut record = fixture.store.snapshot("head", "1").unwrap().unwrap();
        record.files[0].sha256 = crate::store::sha256_hex(b"other head\n");
        fixture.store.put_snapshot(&record).unwrap();
        write(&fixture.root().join("revised"), "test/upper");
        let layers = fixture.store.layers().unwrap().len();
        let refused = update(&fixture, &target, |_| Ok(())).unwrap_err();
        assert_eq!(refused.code, Code::VerifyFailed);
        assert!(refused.message.contains("head.txt of head@1"), "{}", refused.message);
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, first.serving);
        assert_eq!(fixture.store.layers().unwrap().len(), layers, "nothing baked");
    }

    #[test]
    fn of_two_updates_at_the_same_time_one_holds_the_lock_and_the_other_stops() {
        let fixture = fixture("update-lock");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        update(&fixture, &target, region).unwrap();
        let gate = fixture.root().join("gate");
        std::fs::create_dir_all(&gate).unwrap();
        std::thread::scope(|scope| {
            let _opens = Opens(gate.join("open"));
            let first = scope.spawn(|| update(&fixture, &target, |_| Ok(())));
            arrives(&gate.join("waiting"));
            let second = update(&fixture, &target, |_| Ok(())).unwrap_err();
            assert_eq!(second.code, Code::Blocked);
            assert!(second.message.contains("holds the lock (phase build"), "{}", second.message);
            write(&gate.join("open"), "");
            assert_eq!(steps(&first.join().unwrap().unwrap()), ["counter/gate"]);
        });
        assert_eq!(lock::read(&target).unwrap().unwrap().0, lock::LockFile::Free, "the run frees it");
    }

    #[test]
    fn a_run_whose_lock_another_took_does_not_switch() {
        let fixture = fixture("update-takeover");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let served = update(&fixture, &target, region).unwrap().serving;
        let gate = fixture.root().join("gate");
        std::fs::create_dir_all(&gate).unwrap();
        std::thread::scope(|scope| {
            let _opens = Opens(gate.join("open"));
            let first = scope.spawn(|| update(&fixture, &target, |_| Ok(())));
            arrives(&gate.join("waiting"));
            let Some((lock::LockFile::Held(holder), tag)) = lock::read(&target).unwrap() else { panic!("free") };
            assert!(lock::stale_at(&holder) > crate::date::now(), "a young heartbeat");
            lock::free(&target, tag).unwrap();
            let taken = lock::Held::take(&target, "another", crate::date::now()).unwrap();
            write(&gate.join("open"), "");
            let refused = first.join().unwrap().unwrap_err();
            assert!(refused.message.starts_with("another run took the lock"), "{}", refused.message);
            assert!(!refused.fix.contains("unlock"), "the lock is not stale: {}", refused.fix);
            assert_eq!(lock::holder(&target).unwrap().unwrap().run, "another", "the old run does not free it");
            taken.release().unwrap();
        });
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, served, "release.json is unchanged");
    }

    std::thread_local! {
        /// The clock of a wait for the follower: each sleep moves it on.
        static CLOCK: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
    }

    /// What `planner/next.json` and `release.json` named at each answer of a follower.
    pub(in crate::cli) type Notes = std::sync::Arc<std::sync::Mutex<Vec<(Option<String>, String)>>>;

    /// A follower of `dir` at the origin that it gives, whose ready answers are `answers` in turn,
    /// then the last one again.
    pub(in crate::cli) fn follower(
        runtime: &tokio::runtime::Runtime,
        dir: &Path,
        answers: &[(u16, &'static str)],
    ) -> (String, Notes) {
        let (asked, answers, dir) = (Notes::default(), answers.to_vec(), dir.to_path_buf());
        let notes = asked.clone();
        let ready = move || {
            let named = |key: &str| std::fs::read(dir.join(key)).ok();
            let next = named(release::NEXT).map(|bytes| serde_json::from_slice::<release::Role>(&bytes).unwrap().id);
            let serving = serde_json::from_slice::<Serving>(&named(release::SERVING).unwrap()).unwrap().serving.id;
            let mut notes = notes.lock().unwrap();
            notes.push((next, serving));
            let (status, body) = answers[(notes.len() - 1).min(answers.len() - 1)];
            std::future::ready((axum::http::StatusCode::from_u16(status).unwrap(), body))
        };
        let router = axum::Router::new().route("/planner-api/releases/{id}/ready", axum::routing::get(ready));
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        runtime.spawn(async move { axum::serve(listener, router).await.unwrap() });
        (origin, asked)
    }

    /// Switch `target` to the published release `id` as for an R2 target, on the clock of the
    /// test, with a wait that gives up after `give_up`.
    fn switch_after_ready(fixture: &Fixture, target: &Target, id: &str, give_up: Duration) -> Result<(), Error> {
        let base = release::serving(target).unwrap();
        let wait = Wait { give_up, now: || CLOCK.get(), sleep: |every| CLOCK.set(CLOCK.get() + every), ..Wait::RUN };
        let mut run = Run::create(&fixture.store, ["update", "local"]).unwrap();
        let switched = locked(("local", target), &mut run, |run, held| {
            switch_to(("local", target), base.as_ref(), id, &Http::new(), Some(wait), (run, held))
        });
        crate::cli::api::finish_run(run, switched)
    }

    #[test]
    fn an_r2_switch_names_the_release_in_next_json_and_waits_until_the_follower_serves_it() {
        let fixture = fixture("update-router-ready");
        let dir = fixture.scratch.0.join("local");
        let target = Target::Dir(dir.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let unknown = r#"{"state":"unknown","reason":"no release of the target has the planner release"}"#;
        // A 200 that is no answer of a follower, such as a page of the host, does not switch.
        let page = "<html>OpenBikeComputer</html>";
        let answers = [(503, r#"{"state":"starting"}"#), (404, unknown), (200, page), (200, r#"{"state":"ready"}"#)];
        let (api, asked) = follower(&runtime, &dir, &answers);
        let (a, _) = crate::serve::tests::release_at(&target, "device a", b"routing a", 1, &api);
        assert!(release::switch(&target, None, &a).unwrap());
        let (b, _) = crate::serve::tests::release_at(&target, "device b", b"routing b", 1, &api);

        switch_after_ready(&fixture, &target, &b, ready::GIVE_UP).unwrap();
        let waited = vec![(Some(b.clone()), a.clone()); 4];
        assert_eq!(*asked.lock().unwrap(), waited, "next.json names b, and release.json serves a until it is ready");
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, b);
        assert!(!dir.join(release::NEXT).exists(), "the switch deletes next.json");
    }

    #[test]
    fn a_follower_that_never_serves_the_release_leaves_the_target_as_it_was() {
        let fixture = fixture("update-router-give-up");
        let dir = fixture.scratch.0.join("local");
        let target = Target::Dir(dir.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let reason = "not ready: needs a routing service that reads test format 2; it reads 1";
        let failed: &'static str = format!(r#"{{"state":"failed","reason":"{reason}"}}"#).leak();
        let (api, asked) = follower(&runtime, &dir, &[(503, failed)]);
        let (a, _) = crate::serve::tests::release_at(&target, "device a", b"routing a", 1, &api);
        assert!(release::switch(&target, None, &a).unwrap());
        let (b, _) = crate::serve::tests::release_at(&target, "device b", b"routing b", 2, &api);
        let keys = [release::SERVING, POINTERS[0], POINTERS[1]];
        let before = keys.map(|key| std::fs::read(dir.join(key)).unwrap());

        let refused = switch_after_ready(&fixture, &target, &b, Duration::from_secs(60)).unwrap_err();
        assert_eq!(refused.message, format!("local unchanged: {reason}"));
        assert_eq!(asked.lock().unwrap().len(), 7, "an answer each 10 s, from 0 to 60 s");
        assert_eq!(keys.map(|key| std::fs::read(dir.join(key)).unwrap()), before, "release.json and the pointers");
        assert!(!dir.join(release::NEXT).exists(), "the run deletes next.json");
    }
}
