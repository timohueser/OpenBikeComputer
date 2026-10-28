//! `obc data source prepare ID`: make a version of a prepared source in its home in the archive, as
//! [`crate::prepared`] says. It reviews the work, holds the lock of the source, runs detached, moves
//! each file into the home once it is finished and writes the manifest last. The same command
//! resumes a run that stopped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::update_cli::{self, Downloads, MachineCheck};
use super::{print_json, print_table, Code, Error};
use crate::engine::runs::{self, Event, Phase, Run};
use crate::fetch::http::Http;
use crate::lock::Held;
use crate::prepared::{self, Job, Manifest, Opening, Prepared, Scope};
use crate::settings::Settings;
use crate::sources::{FetchKind, Registry, Source};
use crate::store::{sha256_hex, Store};

#[derive(Args)]
pub struct PrepareArgs {
    /// The prepared source, as `data/sources.toml` names it.
    id: String,
    /// Only a region of `data/regions/`, or Geofabrik paths joined by `,`.
    #[arg(long, conflicts_with = "bbox")]
    region: Option<String>,
    /// Only a box: west,south,east,north in degrees.
    #[arg(long = "box", allow_hyphen_values = true)]
    bbox: Option<String>,
    /// Wiki article languages, joined by commas; all supported languages by default.
    #[arg(long, value_delimiter = ',', num_args = 1)]
    languages: Vec<String>,
    /// Show the review, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Start only when the review has this id, as `--dry-run` showed it.
    #[arg(long, value_name = "ID", conflicts_with = "dry_run")]
    expect: Option<String>,
}

/// What a prepare reads besides its run.
pub(super) struct Context<'a> {
    pub root: &'a Path,
    pub store: &'a Store,
    /// Its archive holds the home of each prepared source.
    pub http: &'a Http,
    pub prepared: &'a [&'a dyn Prepared],
}

/// What the person asks of a prepare.
#[derive(Default)]
pub(super) struct Asked<'a> {
    pub id: &'a str,
    /// `--region` and `--box`.
    pub scope: (Option<&'a str>, Option<&'a str>),
    pub expect: Option<&'a str>,
    pub languages: &'a [String],
}

/// The review of a prepare. Its id names the decisions, so `--expect ID` starts a prepare only when
/// they are the ones that a person read.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PrepareReview {
    /// The first 12 hex digits of the SHA-256 of the canonical JSON of the source, the root, the
    /// version, the format, the revision, the scope, the languages, `publish_text` and each planned
    /// input, including archive paths, URLs, sizes and object identities.
    pub id: String,
    pub source: String,
    /// The archive root of the home of the source.
    pub root: PathBuf,
    /// The version that the run makes: the UTC date when its first run started, with `.2`, `.3`
    /// and so on after the first version of that day.
    pub version: String,
    /// The version that a stopped run of this machine started and that this run does not resume:
    /// the home has a newer complete version. `null` when there is none.
    pub abandoned: Option<String>,
    /// Whether the manifest of the version is in the home and names the files of the journal: the
    /// run only lists the version, and makes and writes nothing else.
    pub listing: bool,
    pub format: u32,
    pub revision: u32,
    /// `null`: every place that the source covers.
    pub scope: Option<Scope>,
    pub languages: Vec<String>,
    pub inputs: Vec<PreparedInput>,
    /// The files of the version, at most; `null` when the producer cannot tell before the run.
    pub files: Option<u64>,
    /// Whether the version publishes its article text: `publish_text` of the source; `null` for a
    /// source without text.
    pub publish_text: Option<bool>,
    /// The files that earlier runs of the version moved into the home: this run does not make them
    /// again.
    pub done: Downloads,
    pub cost: PreparedCost,
    pub machine: MachineCheck,
    /// Each target whose serving release reads an older version, with those layers: they are stale
    /// once the version is complete.
    pub stale: Vec<StaleReaders>,
    /// The arguments after `obc data` that start the prepare of this review.
    pub args: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PreparedInput {
    pub source: String,
    /// `null` for the newest version upstream.
    pub version: Option<String>,
    pub params: Vec<(String, String)>,
    /// Input bytes from local files, upstream metadata or the store; `null` when unknown.
    pub bytes: Option<u64>,
    /// Archive bytes absent on this machine; null for other source inputs.
    pub download_bytes: Option<u64>,
}

/// Estimates from the newest version in the home; `null` without one.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct PreparedCost {
    pub wall_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    /// What goes into the store: the inputs that it lacks.
    pub store_bytes: u64,
    /// What goes into the archive: the files of the version that earlier runs did not move.
    pub archive_bytes: Option<u64>,
    /// The inputs whose size the store does not know, as `SOURCE@VERSION`.
    pub unknown: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StaleReaders {
    pub target: String,
    pub layers: Vec<String>,
    /// Why the target cannot be read; `null` when it can.
    pub error: Option<String>,
}

/// What a prepare did.
#[derive(Debug)]
pub struct Done {
    pub source: String,
    pub version: String,
    /// The archive root of the home.
    pub root: PathBuf,
    pub files: usize,
    pub bytes: u64,
    /// The files that this run moved into the home.
    pub uploaded: u64,
    pub manifest_sha256: String,
}

pub fn command(root: &Path, prepared: &[&dyn Prepared], args: PrepareArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let (http, today) = (Http::new(), crate::date::format(crate::date::today()));
    let context = Context { root, store: &store, http: &http, prepared };
    let asked = Asked {
        id: &args.id,
        scope: (args.region.as_deref(), args.bbox.as_deref()),
        expect: args.expect.as_deref(),
        languages: &args.languages,
    };
    let print = |done: &Done| {
        println!(
            "{}@{} in {}: {} files, {}, manifest {}; this run moved {}",
            done.source,
            done.version,
            done.root.display(),
            done.files,
            runs::bytes(done.bytes),
            &done.manifest_sha256[..12],
            done.uploaded
        );
        Ok(())
    };
    let work = |run: &mut Run| prepare(&context, &asked, &today, run);
    if let Some(done) = super::api::detached(&store, work, print) {
        return done;
    }
    let mut review = plan(&context, &home(&http, &args.id)?, &asked, &today)?.review;
    review.args = ["source", "prepare", &args.id].map(String::from).into();
    args.region.iter().for_each(|region| review.args.extend(["--region".into(), region.clone()]));
    args.bbox.iter().for_each(|bbox| review.args.extend(["--box".into(), bbox.clone()]));
    if !review.languages.is_empty() {
        review.args.extend(["--languages".into(), review.languages.join(",")]);
    }
    review.args.extend(["--expect".into(), review.id.clone()]);
    if args.dry_run {
        return print_review(&review, json);
    }
    check(&review, asked.expect)?;
    super::api::start(&store, &review.args, Some(&review.id), json)
}

/// The archive root of the home of `id` in the archive of `http`, as [`prepared::place`] chooses it.
pub(super) fn home(http: &Http, id: &str) -> Result<PathBuf, Error> {
    prepared::warn_shadowed(http.roots(), id);
    prepared::place(http.roots(), id).map_err(|e| {
        Code::Blocked
            .error(format!("`{id}`: {e}; nothing changed"))
            .fix("Set OBC_DATA_ARCHIVE to the roots of the archive, separated by `:`, and mount their drives.")
    })
}

/// What a prepare decides before it changes anything.
struct Planned<'a> {
    review: PrepareReview,
    producer: &'a dyn Prepared,
    source: Source,
    sources: Vec<Source>,
    scope: Option<Scope>,
    /// Whether the run starts a new version, and resumes none.
    fresh: bool,
    made: prepared::Plan,
}

/// Find the producer of the source, its version and its scope, and review the work in the home in
/// the archive root `home`.
fn plan<'a>(context: &Context<'a>, home: &Path, asked: &Asked, today: &str) -> Result<Planned<'a>, Error> {
    let Context { root, store, .. } = *context;
    let id = asked.id;
    let sources = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?.sources;
    let source = sources.iter().find(|source| source.id == id && source.fetch.kind == FetchKind::Prepared);
    let source = source.cloned().ok_or_else(|| {
        Code::Usage.error(format!("data/sources.toml has no prepared source `{id}`")).fix(
            "A prepared source has `fetch.kind = \"prepared\"`. Only a source whose licence lets us give the bytes \
             to others can be one.",
        )
    })?;
    let producer = prepared::producer(context.prepared, id).ok_or_else(|| {
        Code::Usage
            .error(format!("this `obc data` has no producer of `{id}`"))
            .fix("Run `obc data` of the repository: it links every producer.")
    })?;
    let mut settings = Settings::default();
    update_cli::select(root, asked.scope, &mut settings)?;
    let scope = (!settings.region.is_empty())
        .then(|| Scope { region: settings.region.clone(), definitions: settings.definitions.clone() });
    let languages = producer.languages(asked.languages).map_err(|e| Code::Usage.error(e))?;
    let folder = prepared::folder(home, id);
    let listed = prepared::versions(&folder, id)?;
    if listed.is_empty() {
        uncopied(root, store, id)?;
    }
    let newest = listed.iter().filter_map(|listed| prepared::order(&listed.version)).max();
    // A run never writes a file of a complete version. When the manifest of a stopped version is
    // the one of its journal, that run crashed before it listed the version: this run lists it.
    let started = prepared::started(store, id)?;
    let journaled = started.as_ref().map_or(Vec::new(), |started| prepared::journaled(store, id, started));
    let kept = started.as_ref().map(|started| prepared::manifest(&folder, id, started)).transpose()?.flatten();
    let (resumed, abandoned) = match started {
        Some(started) if kept.as_ref().is_some_and(|kept| kept.files == journaled) => (Some(started), None),
        Some(started) if kept.is_some() || newest.is_some_and(|newest| Some(newest) >= prepared::order(&started)) => {
            (None, Some(started))
        }
        started => (started, None),
    };
    let listing = kept.is_some() && abandoned.is_none();
    let fresh = resumed.is_none();
    let version = match resumed {
        Some(resumed) => resumed,
        None => prepared::next(&folder, id, today, &listed)?,
    };
    let plan_path = prepared::job_dir(store, id, &version).join("state/plan.json");
    let made: prepared::Plan = if !fresh && plan_path.is_file() {
        let selection = plan_path.parent().unwrap().parent().unwrap().join("selection.json");
        let expected = prepared::selection(home, scope.as_ref(), &languages);
        if std::fs::read(selection).map_err(|e| e.to_string())? != expected {
            return Err(Code::Usage.error(
                "a run for another archive root, region or language selection stopped; resume it with its original selection",
            ));
        }
        serde_json::from_slice(&std::fs::read(plan_path).map_err(|e| e.to_string())?)
            .map_err(|e| Code::InvalidData.error(e.to_string()))?
    } else {
        producer
            .plan(
                &prepared::Planning { store, http: context.http, sources: &sources },
                &source,
                scope.as_ref(),
                &languages,
            )
            .map_err(|e| Code::Failed.error(format!("`{id}`: {e}")))?
    };
    let (mut inputs, mut store_bytes, mut unknown) = (Vec::new(), 0, Vec::new());
    for input in &made.inputs {
        let (wanted, known) = (&input.wanted, input.bytes);
        let download_bytes = input.archive.as_ref().map(|archive| {
            if std::fs::metadata(&archive.path).is_ok_and(|m| Some(m.len()) == known) {
                0
            } else {
                known.unwrap_or(0)
            }
        });
        store_bytes += download_bytes.unwrap_or(0);
        let bytes = match &wanted.version {
            // A file on this machine that the producer reads in place, such as a dump.
            _ if known.is_some() => known,
            Some(version) => {
                let (source, params) = (wanted.source.clone(), wanted.params.clone());
                let fetch = crate::engine::plan::Fetch {
                    source,
                    version: version.clone(),
                    params,
                    files: Vec::new(),
                    bytes: None,
                };
                let bytes = crate::engine::plan::fetch_bytes(store, &fetch)?;
                if crate::engine::snapshot_files(store, &wanted.source, version, &wanted.params, &[])?.is_none() {
                    store_bytes += bytes.unwrap_or(0);
                }
                bytes
            }
            None => None,
        };
        if bytes.is_none() {
            let version = wanted.version.as_deref().unwrap_or("newest");
            unknown.push(format!("{}@{version}", wanted.source));
        }
        inputs.push(PreparedInput {
            source: wanted.source.clone(),
            version: wanted.version.clone(),
            params: wanted.params.clone(),
            bytes,
            download_bytes,
        });
    }
    let journaled = if fresh { Vec::new() } else { journaled };
    let done = Downloads { files: journaled.len() as u64, bytes: journaled.iter().map(|file| file.size).sum() };
    let store_bytes = store_bytes + made.work_bytes;
    let mut cost = PreparedCost { store_bytes, unknown, ..PreparedCost::default() };
    if let Some(newest) = listed.last() {
        let manifest = prepared::manifest(&folder, id, &newest.version)?;
        if let Some(manifest) = manifest.filter(|m| m.scope == scope && m.languages == languages) {
            cost.wall_ms = Some(manifest.cost.wall_ms);
            cost.peak_rss_bytes = manifest.cost.peak_rss_bytes;
            cost.archive_bytes = Some(newest.bytes.saturating_sub(done.bytes));
        }
    }

    let mut machine = MachineCheck::this(store)?;
    if let Some((peak, memory)) = cost.peak_rss_bytes.zip(machine.memory_bytes).filter(|(peak, memory)| peak > memory) {
        machine.refusals.push(format!(
            "the last version needed {} of memory, and this machine has {}",
            runs::bytes(peak),
            runs::bytes(memory)
        ));
    }
    machine.refusals.extend(update_cli::disk_refusal(&machine, cost.store_bytes));
    if let Err(reason) = crate::store::check_free(home, cost.archive_bytes.unwrap_or(0)) {
        machine.refusals.push(reason);
    }
    let archive_bytes = inputs.iter().filter_map(|input| input.download_bytes).sum();
    let directories: BTreeSet<_> =
        made.inputs.iter().filter_map(|input| input.archive.as_ref()?.path.parent()).collect();
    for directory in directories {
        if let Err(reason) = crate::store::check_free(directory, archive_bytes) {
            machine.refusals.push(reason);
        }
    }
    for read in std::iter::once(id).chain(inputs.iter().map(|input| input.source.as_str())) {
        let credential = sources.iter().find(|source| source.id == read).and_then(|s| s.credential.as_ref());
        if let Some(credential) = credential.filter(|credential| !credential.present()) {
            machine.refusals.push(format!("`{read}` needs {}, which this machine lacks", credential.describe()));
        }
    }
    let stale = stale(root, store, id, &version)?;
    let decisions = serde_json::json!({
        "source": id,
        "root": home,
        "version": version,
        "format": producer.format(),
        "revision": producer.revision(),
        "scope": scope,
        "languages": languages,
        "publish_text": source.publish_text,
        "inputs": made.inputs,
    });
    let canonical = serde_json::to_vec(&crate::engine::sorted(decisions)).expect("the decisions are JSON");
    let review = PrepareReview {
        id: sha256_hex(&canonical)[..12].to_string(),
        source: id.into(),
        root: home.into(),
        version,
        abandoned,
        listing,
        format: producer.format(),
        revision: producer.revision(),
        scope: scope.clone(),
        languages,
        inputs,
        files: made.files,
        publish_text: source.publish_text,
        done,
        cost,
        machine,
        stale,
        args: Vec::new(),
    };
    Ok(Planned { review, producer, source, sources, scope, fresh, made })
}

/// The layers of each target of `data/targets.toml` that read a version of `id` older than
/// `version`: each target reads the versions of the archive.
fn stale(root: &Path, store: &Store, id: &str, version: &str) -> Result<Vec<StaleReaders>, Error> {
    let targets = crate::target::load(root, store).map_err(|e| Code::InvalidData.error(e))?;
    let mut stale = Vec::new();
    for (name, target) in targets {
        let read = || -> Result<Vec<String>, Error> {
            let Some((serving, _)) = crate::release::serving(&target)? else { return Ok(Vec::new()) };
            let release = update_cli::read(&name, &target, &serving.serving.id)?;
            let old = |output: &&crate::release::Output| {
                let read = output.snapshots.get(id).and_then(|read| prepared::unpin(&read.version));
                read.is_some_and(|(read, _)| prepared::order(read) < prepared::order(version))
            };
            let layers: BTreeSet<String> =
                release.outputs.iter().filter(old).map(|output| output.layer.clone()).collect();
            Ok(layers.into_iter().collect())
        };
        stale.push(match read() {
            Ok(layers) => StaleReaders { target: name, layers, error: None },
            Err(error) => StaleReaders { target: name, layers: Vec::new(), error: Some(error.message) },
        });
    }
    Ok(stale)
}

/// Refuse a prepare whose review is not `expect`, or that this machine cannot do.
fn check(review: &PrepareReview, expect: Option<&str>) -> Result<(), Error> {
    update_cli::expect(&review.source, &review.id, expect)?;
    if !review.machine.refusals.is_empty() {
        return Err(Code::Blocked
            .error(format!(
                "this machine cannot do the prepare: {}; nothing changed",
                review.machine.refusals.join("; ")
            ))
            .fix("Run it on a machine with the memory, the disk and the credentials that it needs, or release space with `obc data clean`."));
    }
    Ok(())
}

/// Refuse the first version of `id` while a target of `data/targets.toml` lists versions in its
/// legacy folder `sources/<id>/`: they go into the archive first, so no version name is taken twice.
/// It goes with `obc data source copy`.
fn uncopied(root: &Path, store: &Store, id: &str) -> Result<(), Error> {
    for (name, target) in crate::target::load(root, store).map_err(|e| Code::InvalidData.error(e))? {
        let listed = prepared::versions(&target.within(&format!("sources/{id}")), id).map_err(|e| {
            Code::Blocked
                .error(format!("`{name}` cannot be read ({e}); nothing changed"))
                .fix(format!("Run again when `{name}` answers: it can hold versions of `{id}` to copy first."))
        })?;
        if !listed.is_empty() {
            return Err(Code::Blocked
                .error(format!(
                    "`{name}` lists {} versions of `{id}` in sources/{id}/, and the archive none; nothing changed",
                    listed.len()
                ))
                .fix(format!("Copy them first: `obc data source copy {id} --from {name}`.")));
        }
    }
    Ok(())
}

/// The label of the lock of the source `id`, and the command that frees it.
pub(super) fn lock_names(id: &str) -> (String, String) {
    (format!("source `{id}` in the archive"), format!("obc data unlock --source {id}"))
}

/// Do `work` while `run` holds the lock of the source `id` in the archive root `home`. When the run
/// made the home and fails, the home goes again while it holds only the lock: a refusal never fixes
/// the home on a root.
pub(super) fn locked<T>(
    store: &Store,
    (home, id): (&Path, &str),
    run: &mut Run,
    work: impl FnOnce(&mut Run, &Held) -> Result<T, Error>,
) -> Result<T, Error> {
    let new = !prepared::path(home, id).exists();
    let (label, unlock) = lock_names(id);
    let done = update_cli::locked_as(store, (&label, &unlock), &prepared::folder(home, id), run, work);
    if done.is_err() && new {
        prepared::unmake(home, id);
    }
    done
}

/// Prepare the source of `asked` while the run holds the lock of the source: review again, make each
/// file and move it into the home, then write the manifest and list the version.
pub(super) fn prepare(context: &Context, asked: &Asked, today: &str, run: &mut Run) -> Result<Done, Error> {
    let id = asked.id;
    let home = home(context.http, id)?;
    locked(context.store, (&home, id), run, |run, held| {
        run.record(&Event::Phase { phase: Phase::Prepare })?;
        let planned = plan(context, &home, asked, today)?;
        if !held.beat(Some(&planned.review.id), None, crate::date::now())? {
            return Err(update_cli::lost(&lock_names(id).0));
        }
        check(&planned.review, asked.expect)?;
        make(context, &home, planned, (run, held))
    })
}

fn make(context: &Context, home: &Path, planned: Planned, (run, held): (&mut Run, &Held)) -> Result<Done, Error> {
    let Planned { review, producer, source, sources, scope, fresh, made } = planned;
    let (id, version) = (source.id.as_str(), review.version.as_str());
    let folder = prepared::folder(home, id);
    let (label, again) = (lock_names(id).0, format!("obc data source prepare {id}"));
    let lock = (label.as_str(), again.as_str());
    let done = |listed: prepared::Version, uploaded| Done {
        source: id.into(),
        version: version.into(),
        root: home.into(),
        files: review.done.files as usize,
        bytes: listed.bytes,
        uploaded,
        manifest_sha256: listed.manifest_sha256,
    };
    if review.listing {
        update_cli::enter_as((run, held), lock, Phase::Upload)?;
        let journaled = prepared::journaled(context.store, id, version);
        if let Some(missing) = prepared::absent(&folder, version, &journaled)?.first() {
            return Err(Code::Failed
                .error(format!("{id}/{missing} is not in the archive; the version is not listed"))
                .fix(format!(
                    "Delete {}: the next run makes a new version.",
                    prepared::job_dir(context.store, id, version).display()
                )));
        }
        let listed = prepared::list(&folder, id, version)?;
        prepared::forget(context.store, id, version)?;
        return Ok(done(listed, 0));
    }
    update_cli::enter_as((run, held), lock, Phase::Build)?;
    if let Some(abandoned) = &review.abandoned {
        prepared::forget(context.store, id, abandoned)?;
    }
    if fresh {
        // Nothing resumes an older version without a manifest: a stopped run left it.
        for (at, key) in prepared::incomplete(&folder, id, version)?.iter().enumerate() {
            if at % 100 == 0 && !held.beat(None, None, crate::date::now())? {
                return Err(update_cli::lost(lock.0));
            }
            folder.remove(key)?;
        }
    }
    let start = Instant::now();
    let opening = Opening { root: context.root, store: context.store, http: context.http, sources: &sources, home };
    let (files, reused, extends, inputs, uploaded, peak_rss_bytes) = {
        let mut job = Job::open(opening, (id, version), scope.clone(), &review.languages, run)?;
        crate::store::write_atomic(&job.state.join("plan.json"), &serde_json::to_vec(&made).expect("plan serializes"))?;
        job.plan = made;
        producer.prepare(&mut job).map_err(|e| Code::RunFailed.error(format!("{id}@{version}: {e}")))?;
        (
            job.files().cloned().collect::<Vec<_>>(),
            job.reused().clone(),
            job.extends().to_vec(),
            job.reads(),
            job.uploaded,
            job.peak_rss_bytes(),
        )
    };
    if files.is_empty() {
        return Err(Code::RunFailed.error(format!("{id}@{version}: the producer made no file; nothing is listed")));
    }
    update_cli::enter_as((run, held), lock, Phase::Upload)?;
    let manifest = Manifest {
        source: id.into(),
        version: version.into(),
        format: producer.format(),
        revision: producer.revision(),
        inputs,
        files,
        reused,
        scope,
        extends,
        languages: review.languages.clone(),
        licence: source.licence.clone(),
        attribution: source.attribution.clone(),
        machine: crate::machine(),
        time: crate::date::timestamp(crate::date::now()),
        cost: prepared::Cost { wall_ms: start.elapsed().as_millis() as u64, peak_rss_bytes },
    };
    let listed = prepared::complete(&folder, &manifest)?;
    prepared::forget(context.store, id, version)?;
    Ok(Done { files: manifest.files.len(), ..done(listed, uploaded) })
}

fn print_review(review: &PrepareReview, json: bool) -> Result<(), Error> {
    if json {
        return print_json(review);
    }
    println!("review {} of {}@{} in {}", review.id, review.source, review.version, review.root.display());
    let mut rows = Vec::new();
    let scope = review.scope.as_ref().map_or("all".to_string(), |scope| format!("region {}", scope.region));
    rows.push(vec!["  scope".into(), scope]);
    if !review.languages.is_empty() {
        rows.push(vec!["  languages".into(), review.languages.join(", ")]);
    }
    rows.push(vec!["  version".into(), format!("format {}, revision {}", review.format, review.revision)]);
    if let Some(files) = review.files {
        rows.push(vec!["  files".into(), format!("up to {files}")]);
    }
    if let Some(published) = review.publish_text {
        rows.push(vec!["  text".into(), if published { "text published" } else { "text kept local" }.into()]);
    }
    if let Some(abandoned) = &review.abandoned {
        let text = format!("{abandoned} stays incomplete: the home has a newer complete version");
        rows.push(vec!["  abandon".into(), text]);
    }
    if review.listing {
        rows.push(vec!["  list".into(), "the manifest is in the home: the run only lists the version".into()]);
    }
    for input in &review.inputs {
        let mut size = input.bytes.map_or("size unknown".into(), runs::bytes);
        if let Some(download) = input.download_bytes {
            size += &if download == 0 { " cached".into() } else { format!(" to download ({})", runs::bytes(download)) };
        }
        let version = input.version.as_deref().unwrap_or("newest");
        rows.push(vec!["  input".into(), format!("{} {}", runs::fetched(&input.source, version, &input.params), size)]);
    }
    if review.done.files > 0 {
        rows.push(vec![
            "  resume".into(),
            format!("{} files, {} in the home", review.done.files, runs::bytes(review.done.bytes)),
        ]);
    }
    let cost = &review.cost;
    let mut text = format!("{} into the store", runs::bytes(cost.store_bytes));
    match cost.archive_bytes {
        Some(archive) => text += &format!(", {} into the archive", runs::bytes(archive)),
        None => text += ", archive bytes unknown",
    }
    if let Some(wall_ms) = cost.wall_ms {
        text += &format!(", about {}", runs::duration(wall_ms));
    }
    if let Some(peak) = cost.peak_rss_bytes {
        text += &format!(", peak {} memory", runs::bytes(peak));
    }
    rows.push(vec!["  cost".into(), text]);
    let mut machine = match review.machine.refusals.as_slice() {
        [] => "this machine can do it".to_string(),
        refusals => format!("this machine cannot do it: {}", refusals.join("; ")),
    };
    if !cost.unknown.is_empty() {
        machine += &format!("; input sizes unknown: {}", cost.unknown.join(", "));
    }
    rows.push(vec!["  machine".into(), machine]);
    for readers in &review.stale {
        let text = match (&readers.error, readers.layers.as_slice()) {
            (Some(error), _) => format!("cannot be read: {error}"),
            (None, []) => "no layer reads an older version".into(),
            (None, layers) => format!("stale once complete: {}", layers.join(", ")),
        };
        rows.push(vec![format!("  {}", readers.target), text]);
    }
    rows.push(vec!["  start".into(), format!("obc data {}", review.args.join(" "))]);
    print_table(&rows);
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::cli::update_cli::tests::region;
    use crate::engine::tests::{fixture, write, Fixture};
    use crate::engine::{Client, Input, Layer, Request, Run as StepRun, Shape, Step};
    use crate::lock::{self, LockFile};
    use crate::product::{Inputs, Pointer, Product, Steps, Unplanned, Wanted};
    use crate::regions::Regions;
    use crate::release::POINTERS;
    use crate::target::Target;
    use std::fs;

    const SOURCES: &str = r#"
        [[source]]
        id = "head"
        kind = "data"
        licence = "MIT"
        fetch = { kind = "http", url = "https://example.org/head.txt" }
        version = "release"
        keep = "store"
        redistribute = true

        [[source]]
        id = "sample"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Sample"
        fetch = { kind = "prepared" }
        version = "date"
        refresh = 1
        keep = "prepared"
        redistribute = true
    "#;

    fn head() -> Wanted {
        Wanted { source: "head".into(), version: Some("1".into()), params: Vec::new() }
    }

    /// The test producer of the format of its number: three files of `head@1` and of the region.
    /// The file `fail-<path>` of the repository fails the run at that file, and the folder `gate`
    /// holds the first file until `gate/open` comes. `made` gets each file that it makes.
    struct Sample(u32);

    impl Prepared for Sample {
        fn name(&self) -> &'static str {
            "sample"
        }

        fn format(&self) -> u32 {
            self.0
        }

        fn revision(&self) -> u32 {
            1
        }

        fn plan(
            &self,
            _: &prepared::Planning,
            _: &Source,
            _: Option<&Scope>,
            _: &[String],
        ) -> Result<prepared::Plan, String> {
            Ok(prepared::Plan { inputs: vec![(head(), None).into()], files: Some(3), work_bytes: 0 })
        }

        fn prepare(&self, job: &mut Job) -> Result<(), String> {
            let head = fs::read_to_string(&job.fetch(&head())?.1["head.txt"]).map_err(|e| e.to_string())?;
            for path in ["a.txt", "b.txt", "c.txt"] {
                if job.done(path) {
                    continue;
                }
                // With `reuse-<path>`, the version keeps the file of the version before.
                if job.root.join(format!("reuse-{path}")).exists() {
                    if let Some(previous) = job.extend(self.0)? {
                        job.reuse(&previous, path)?;
                        continue;
                    }
                }
                let gate = job.root.join("gate");
                if gate.is_dir() {
                    write(&gate.join("waiting"), "");
                    arrives(&gate.join("open"));
                }
                if job.root.join(format!("fail-{path}")).exists() {
                    return Err(format!("{path} failed"));
                }
                let mut made = fs::OpenOptions::new().create(true).append(true).open(job.root.join("made")).unwrap();
                std::io::Write::write_all(&mut made, format!("{path}\n").as_bytes()).unwrap();
                let region = job.scope.as_ref().map_or("all", |scope| scope.region.as_str());
                write(&job.work.join(path), &format!("{path} of {} for {region}: {head}", job.version));
                job.finish(&[path.to_string()])?;
            }
            Ok(())
        }
    }

    static READS: Layer =
        Layer { name: "reads", shape: Shape::Whole, format: 1, revision: 1, readers: &[&crate::engine::tests::TESTS] };

    /// A product that reads `a.txt` of the version of `sample` that its target reads.
    struct Reader;

    impl Product for Reader {
        fn name(&self) -> &'static str {
            "reader"
        }

        fn steps(&self, _: &Path, inputs: &Inputs, _: &Regions, store: &Store) -> Result<Steps, Unplanned> {
            let version = crate::product::version(inputs, store, "sample", &[]).map_err(Unplanned::Failed)?;
            let version = version.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            let read =
                Input::Snapshot { source: "sample".into(), version, params: Vec::new(), files: vec!["a.txt".into()] };
            let step = Step {
                layer: &READS,
                name: "reader/copy".into(),
                inputs: vec![read],
                options: serde_json::json!({}),
                outputs: vec!["copy.txt".into()],
                run: StepRun::Rust(&COPY),
                client: Client::All,
            };
            Ok(vec![step].into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: POINTERS[0], step: "reader/copy", path: "copy.txt" }
        }
    }

    /// The other pointer of a release.
    struct Second;

    impl Product for Second {
        fn name(&self) -> &'static str {
            "second"
        }

        fn steps(&self, _: &Path, _: &Inputs, _: &Regions, _: &Store) -> Result<Steps, Unplanned> {
            Ok(Vec::new().into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: POINTERS[1], step: "reader/copy", path: "copy.txt" }
        }
    }

    pub(in crate::cli) static COPY: crate::engine::Function =
        crate::engine::Function { name: "test/sample-copy", run: copy };

    fn copy(request: &Request) -> Result<(), String> {
        fs::copy(&request.snapshots["sample"]["a.txt"], request.output.join("copy.txt"))
            .map(drop)
            .map_err(|e| e.to_string())
    }

    /// Wait until `path` exists, for 30 seconds at most.
    fn arrives(path: &Path) {
        let start = Instant::now();
        while !path.exists() {
            assert!(start.elapsed().as_secs() < 30, "{} did not come", path.display());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// A store and a repository with `sample`, the dir targets `prod` and `local`, and the archive
    /// root `archive` of [`http`]: the fixture, the root and the folder of `prod`.
    pub(in crate::cli) fn setup(name: &str) -> (Fixture, PathBuf, PathBuf) {
        let fixture = fixture(name);
        write(&fixture.root().join("data/sources.toml"), SOURCES);
        let (prod, local) = (fixture.scratch.0.join("prod"), fixture.scratch.0.join("local"));
        let targets = format!(
            "[prod]\nkind = \"dir\"\ndir = \"{}\"\n[local]\nkind = \"dir\"\ndir = \"{}\"\n",
            prod.display(),
            local.display()
        );
        write(&fixture.root().join("data/targets.toml"), &targets);
        let archive = fixture.scratch.0.join("archive");
        fs::create_dir_all(&archive).unwrap();
        (fixture, archive, prod)
    }

    /// A client whose archive is the root of [`setup`].
    pub(in crate::cli) fn http(fixture: &Fixture) -> Http {
        Http::new().with_archive(&fixture.scratch.0.join("archive"))
    }

    fn target(fixture: &Fixture, name: &str) -> Target {
        crate::target::find(&fixture.root(), name, &fixture.store).unwrap()
    }

    /// Prepare `sample` of format 1 on the day `today`.
    pub(in crate::cli) fn prepare(fixture: &Fixture, today: &str, asked: Asked) -> Result<Done, Error> {
        prepare_with(fixture, (&Sample(1), &http(fixture)), today, asked)
    }

    fn prepare_with(
        fixture: &Fixture,
        (sample, http): (&Sample, &Http),
        today: &str,
        asked: Asked,
    ) -> Result<Done, Error> {
        let root = fixture.root();
        let context = Context { root: &root, store: &fixture.store, http, prepared: &[sample] };
        let mut run = Run::create(&fixture.store, ["source", "prepare", "sample"]).unwrap();
        let done = super::prepare(&context, &Asked { id: "sample", ..asked }, today, &mut run);
        crate::cli::api::finish_run(run, done)
    }

    /// Update the target `name` with the producer of format 1.
    fn update(fixture: &Fixture, name: &str, asked: &update_cli::Asked) -> Result<update_cli::Updated, Error> {
        update_with(fixture, (&Sample(1), &http(fixture)), name, asked)
    }

    fn update_with(
        fixture: &Fixture,
        (sample, http): (&Sample, &Http),
        name: &str,
        asked: &update_cli::Asked,
    ) -> Result<update_cli::Updated, Error> {
        let mut run = Run::create(&fixture.store, ["update", name]).unwrap();
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Reader, &Second]);
        let machine = MachineCheck::default();
        let settings = |settings: &mut Settings| if settings.region.is_empty() { region(settings) } else { Ok(()) };
        let code: (&[&dyn Product], &[&dyn Prepared]) = (&products, &[sample]);
        let target = target(fixture, name);
        let updated = update_cli::update(
            &root,
            &fixture.store,
            http,
            (name, &target),
            code,
            settings,
            asked,
            Some(&machine),
            &mut run,
        );
        crate::cli::api::finish_run(run, updated)
    }

    /// The layers that an update of the target `name` bakes, with the text of their cause.
    fn changes(fixture: &Fixture, name: &str) -> Vec<(String, String)> {
        let (products, machine): ([&dyn Product; 2], _) = ([&Reader, &Second], MachineCheck::default());
        let code: (&[&dyn Product], &[&dyn Prepared]) = (&products, &[&Sample(1)]);
        let (root, http, target) = (fixture.root(), http(fixture), target(fixture, name));
        let prepared = update_cli::prepare(
            &root,
            &fixture.store,
            &http,
            (name, &target),
            code,
            |_| Ok(()),
            &[],
            (Some(&machine), Vec::new),
            None,
        );
        prepared.unwrap().review.layers.into_iter().map(|layer| (layer.layer, layer.text)).collect()
    }

    /// The version of `sample` that the serving release of the target `name` reads, and the bytes of
    /// its client file.
    fn served(fixture: &Fixture, name: &str) -> (String, String) {
        let target = target(fixture, name);
        let (serving, _) = crate::release::serving(&target).unwrap().unwrap();
        let release = crate::release::Release::read(&target, &serving.serving.id).unwrap();
        let output = release.outputs.iter().find(|output| output.step == "reader/copy").unwrap();
        let bytes = target.get(&format!("objects/{}", output.files[0].sha256)).unwrap().unwrap();
        (output.snapshots["sample"].version.clone(), String::from_utf8(bytes).unwrap())
    }

    /// The version of `sample` that a step reads for the entry of `day` in the home in `root`.
    fn pinned(root: &Path, day: &str) -> String {
        let listed = prepared::versions(&prepared::folder(root, "sample"), "sample").unwrap();
        let entry = listed.iter().find(|entry| entry.version == day).unwrap();
        format!("{}/{}", entry.version, entry.manifest_sha256)
    }

    /// The keys of the home of `sample` in `root` that start with `prefix`.
    fn keys(root: &Path, prefix: &str) -> Vec<String> {
        let listed = prepared::folder(root, "sample").list().unwrap().into_iter().map(|listed| listed.key);
        listed.filter(|key| key.starts_with(prefix)).collect()
    }

    fn versions(root: &Path) -> Vec<String> {
        let listed = prepared::versions(&prepared::folder(root, "sample"), "sample").unwrap();
        listed.into_iter().map(|listed| listed.version).collect()
    }

    #[test]
    fn the_manifest_comes_last_and_a_resume_moves_only_what_the_home_lacks() {
        let (fixture, archive, _) = setup("prepare-resume");
        write(&fixture.root().join("fail-c.txt"), "");
        let failed = prepare(&fixture, "2026-10-01", Asked::default()).unwrap_err();
        assert!(failed.message.contains("c.txt failed"), "{}", failed.message);
        assert_eq!(keys(&archive, "2026-10-01/"), ["2026-10-01/a.txt", "2026-10-01/b.txt"], "no manifest");
        assert!(versions(&archive).is_empty());

        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        let done = prepare(&fixture, "2026-10-02", Asked::default()).unwrap();
        assert_eq!(
            (done.version.as_str(), done.files, done.uploaded, done.root.as_path()),
            ("2026-10-01", 3, 1, archive.as_path()),
            "the resume keeps its version"
        );
        assert_eq!(fs::read_to_string(fixture.root().join("made")).unwrap(), "a.txt\nb.txt\nc.txt\n");
        let folder = prepared::folder(&archive, "sample");
        let manifest = prepared::manifest(&folder, "sample", "2026-10-01").unwrap().unwrap();
        let paths: Vec<&str> = manifest.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt", "c.txt"]);
        assert_eq!((manifest.inputs[0].source.as_str(), manifest.licence.as_deref()), ("head", Some("CC0-1.0")));
        let bytes = folder.get("2026-10-01/manifest.json").unwrap().unwrap();
        let listed = prepared::versions(&folder, "sample").unwrap();
        assert_eq!((listed[0].version.as_str(), &listed[0].manifest_sha256), ("2026-10-01", &sha256_hex(&bytes)));
        assert!(!fixture.store.root().join("prepared/sample").read_dir().unwrap().any(|_| true), "the work is gone");
        assert_eq!(lock::read(&folder).unwrap().unwrap().0, LockFile::Free);
    }

    #[test]
    fn a_later_version_of_the_same_day_takes_the_next_ordinal_and_extends_the_one_before() {
        let (fixture, archive, _) = setup("prepare-same-day");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        write(&fixture.root().join("reuse-a.txt"), "");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap_err();
        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        let resumed = prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        assert_eq!((resumed.version.as_str(), resumed.uploaded), ("2026-10-01.2", 1), "the stopped run resumes");
        let second =
            prepared::manifest(&prepared::folder(&archive, "sample"), "sample", "2026-10-01.2").unwrap().unwrap();
        assert_eq!(second.reused, [("a.txt".to_string(), "2026-10-01".to_string())].into());

        assert_eq!(prepare(&fixture, "2026-10-01", Asked::default()).unwrap().version, "2026-10-01.3");
        assert_eq!(versions(&archive), ["2026-10-01", "2026-10-01.2", "2026-10-01.3"]);
    }

    #[test]
    fn a_new_version_makes_its_readers_stale_in_each_target_that_reads_an_older_one() {
        let (fixture, archive, _) = setup("prepare-stale");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        update(&fixture, "prod", &Default::default()).unwrap();
        let (root, http) = (fixture.root(), http(&fixture));
        let context = Context { root: &root, store: &fixture.store, http: &http, prepared: &[&Sample(1)] };
        let review =
            plan(&context, &archive, &Asked { id: "sample", ..Asked::default() }, "2026-10-02").unwrap().review;
        let stale: Vec<(&str, &[String])> =
            review.stale.iter().map(|readers| (readers.target.as_str(), &readers.layers[..])).collect();
        assert_eq!(stale, [("local", &[][..]), ("prod", &["reads".to_string()][..])]);
        prepare(&fixture, "2026-10-02", Asked::default()).unwrap();
        update(&fixture, "local", &Default::default()).unwrap();

        let show = |day: &str| prepared::show(&pinned(&archive, day));
        let cause = format!("sample {} → {}", show("2026-10-01"), show("2026-10-02"));
        assert_eq!(changes(&fixture, "prod"), [("reads".to_string(), cause)]);
        assert_eq!(changes(&fixture, "local"), [], "it reads the new version already");
    }

    #[test]
    fn a_version_reads_a_file_that_it_reuses_from_the_version_before_and_no_collection_deletes_a_version() {
        let (fixture, archive, _) = setup("prepare-reuse");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        write(&fixture.root().join("reuse-a.txt"), "");
        prepare(&fixture, "2026-10-02", Asked::default()).unwrap();
        let manifest =
            prepared::manifest(&prepared::folder(&archive, "sample"), "sample", "2026-10-02").unwrap().unwrap();
        assert_eq!(manifest.reused, [("a.txt".to_string(), "2026-10-01".to_string())].into());
        assert!(keys(&archive, "2026-10-02/a.txt").is_empty(), "a reused file is not moved again");
        update(&fixture, "prod", &Default::default()).unwrap();
        assert_eq!(served(&fixture, "prod").0, pinned(&archive, "2026-10-02"));
        prepare(&fixture, "2026-10-03", Asked::default()).unwrap();
        update(&fixture, "prod", &Default::default()).unwrap();
        assert_eq!(versions(&archive), ["2026-10-01", "2026-10-02", "2026-10-03"], "the archive keeps each one");
    }

    #[test]
    fn a_bake_reads_a_version_of_the_archive_and_then_bakes_from_the_store_without_it() {
        let (fixture, archive, _) = setup("prepare-offline");
        let refused = update(&fixture, "local", &Default::default()).unwrap_err();
        assert_eq!(refused.code, Code::Blocked);
        assert!(refused.message.contains("the archive lists no version of `sample`"), "{}", refused.message);
        assert!(refused.fix.contains("OBC_DATA_ARCHIVE"), "{}", refused.fix);
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        update(&fixture, "local", &Default::default()).unwrap();
        fs::rename(&archive, archive.with_file_name("unmounted")).unwrap();
        let only = update_cli::Asked { only: vec!["reads".into()], ..Default::default() };
        let again = update(&fixture, "local", &only).unwrap();
        assert_eq!(
            again.baked.iter().map(|baked| (baked.step.as_str(), baked.reused)).collect::<Vec<_>>(),
            [("reader/copy", false)]
        );
    }

    #[test]
    fn of_two_prepares_at_once_one_holds_the_lock_of_the_source() {
        let (fixture, _, _) = setup("prepare-lock");
        let gate = fixture.root().join("gate");
        fs::create_dir_all(&gate).unwrap();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| prepare(&fixture, "2026-10-01", Asked::default()));
            arrives(&gate.join("waiting"));
            let second = prepare(&fixture, "2026-10-01", Asked::default()).unwrap_err();
            write(&gate.join("open"), "");
            assert_eq!(second.code, Code::Blocked);
            assert!(second.message.starts_with("source `sample` in the archive: run "), "{}", second.message);
            assert!(second.fix.contains("`obc data unlock --source sample --from "), "{}", second.fix);
            first.join().unwrap().unwrap();
        });
    }

    #[test]
    fn a_prepare_refuses_another_review_and_an_archive_that_it_cannot_use() {
        let (fixture, archive, _) = setup("prepare-refuse");
        let without = Http::with_backoff(std::time::Duration::ZERO);
        let refused = prepare_with(&fixture, (&Sample(1), &without), "2026-10-01", Asked::default()).unwrap_err();
        assert!(refused.message.contains("OBC_DATA_ARCHIVE names no root"), "{}", refused.message);
        let unmounted = Http::new().with_roots(vec![archive.clone(), fixture.scratch.0.join("unmounted")]);
        let refused = prepare_with(&fixture, (&Sample(1), &unmounted), "2026-10-01", Asked::default()).unwrap_err();
        assert!(refused.message.contains("unmounted does not exist"), "{}", refused.message);
        assert!(!prepared::path(&archive, "sample").exists(), "nothing changed");
        let other = Asked { expect: Some("000000000000"), ..Asked::default() };
        let refused = prepare(&fixture, "2026-10-01", other).unwrap_err();
        assert!(refused.message.contains(", not 000000000000: it changed"), "{}", refused.message);
        assert!(versions(&archive).is_empty());

        let boxed = Asked { scope: (None, Some("7,47,8,48")), ..Asked::default() };
        prepare(&fixture, "2026-10-01", boxed).unwrap();
        let manifest =
            prepared::manifest(&prepared::folder(&archive, "sample"), "sample", "2026-10-01").unwrap().unwrap();
        assert_eq!(manifest.scope.map(|scope| scope.region), Some("box".into()));
    }

    #[test]
    fn a_regional_wiki_review_and_resume_pin_the_languages() {
        struct Wiki;
        impl Prepared for Wiki {
            fn name(&self) -> &'static str {
                "wiki"
            }
            fn format(&self) -> u32 {
                1
            }
            fn revision(&self) -> u32 {
                1
            }
            fn languages(&self, requested: &[String]) -> Result<Vec<String>, String> {
                Ok(requested.to_vec())
            }
            fn plan(
                &self,
                context: &prepared::Planning,
                source: &Source,
                scope: Option<&Scope>,
                languages: &[String],
            ) -> Result<prepared::Plan, String> {
                Sample(1).plan(context, source, scope, languages)
            }
            fn prepare(&self, job: &mut Job) -> Result<(), String> {
                Sample(1).prepare(job)
            }
        }
        let (fixture, archive, _) = setup("wiki-selection");
        let root = fixture.root();
        let registry = root.join("data/sources.toml");
        fs::write(&registry, fs::read_to_string(&registry).unwrap().replace("id = \"sample\"", "id = \"wiki\""))
            .unwrap();
        let http = http(&fixture);
        let context = Context { root: &root, store: &fixture.store, http: &http, prepared: &[&Wiki] };
        let languages = ["en".into(), "de".into()];
        let asked = Asked { id: "wiki", scope: (None, Some("7,47,8,48")), languages: &languages, ..Default::default() };
        let review = plan(&context, &archive, &asked, "2026-10-01").unwrap().review;
        assert_eq!(review.languages, languages);
        assert_eq!(review.scope.as_ref().unwrap().region, "box");
        let english = Asked { languages: &languages[..1], ..asked };
        let other = plan(&context, &archive, &english, "2026-10-01").unwrap().review;
        assert_ne!(review.id, other.id);
        assert!(check(&other, Some(&review.id)).is_err());
        write(&root.join("fail-c.txt"), "");
        let mut run = Run::create(&fixture.store, ["source", "prepare", "wiki"]).unwrap();
        assert!(super::prepare(&context, &asked, "2026-10-01", &mut run).is_err());
        fs::remove_file(root.join("fail-c.txt")).unwrap();
        let error = super::prepare(&context, &english, "2026-10-01", &mut run).unwrap_err();
        assert!(error.message.contains("region or language selection"), "{}", error.message);
        super::prepare(&context, &asked, "2026-10-01", &mut run).unwrap();
        let manifest = prepared::manifest(&prepared::folder(&archive, "wiki"), "wiki", "2026-10-01").unwrap().unwrap();
        assert_eq!(manifest.languages, languages);
        assert_eq!(manifest.scope, review.scope);
        let changed = plan(&context, &archive, &english, "2026-10-02").unwrap().review;
        assert_eq!(changed.cost.wall_ms, None, "another selection has no measured cost");
    }

    #[test]
    fn a_bake_refuses_a_file_whose_bytes_are_not_those_of_the_manifest() {
        let (fixture, archive, _) = setup("prepare-tamper");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        let file = prepared::path(&archive, "sample").join("2026-10-01/a.txt");
        fs::remove_file(&file).unwrap();
        write(&file, "other bytes");
        let refused = update(&fixture, "local", &Default::default()).unwrap_err();
        assert!(refused.message.contains("2026-10-01/a.txt holds bytes with the SHA-256"), "{}", refused.message);
    }

    #[test]
    fn a_test_root_named_first_holds_the_versions_that_the_targets_read() {
        let (fixture, archive, _) = setup("prepare-test-root");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        let test = fixture.scratch.0.join("test-root");
        fs::create_dir_all(prepared::path(&test, "sample")).unwrap();
        let tested = Http::new().with_roots(vec![test.clone(), archive.clone()]);
        let boxed = Asked { scope: (None, Some("7,47,8,48")), ..Asked::default() };
        prepare_with(&fixture, (&Sample(1), &tested), "2026-10-01", boxed).unwrap();
        update_with(&fixture, (&Sample(1), &tested), "local", &Default::default()).unwrap();
        update(&fixture, "prod", &Default::default()).unwrap();
        let ((prod_reads, prod_bytes), (local_reads, local_bytes)) =
            (served(&fixture, "prod"), served(&fixture, "local"));
        assert_eq!((prod_reads, local_reads), (pinned(&archive, "2026-10-01"), pinned(&test, "2026-10-01")));
        assert_ne!(pinned(&archive, "2026-10-01"), pinned(&test, "2026-10-01"), "one name, two versions");
        assert!(prod_bytes.contains("for all") && local_bytes.contains("for box"), "{prod_bytes} {local_bytes}");
        let read = changes(&fixture, "local");
        let next = prepared::show(&pinned(&archive, "2026-10-01"));
        assert!(read.len() == 1 && read[0].1.ends_with(&next), "without the test root, the home: {read:?}");
    }

    #[test]
    fn a_stopped_run_resumes_only_into_its_own_root_and_a_listed_version_never_goes() {
        let (fixture, archive, _) = setup("prepare-root");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap_err();
        let test = fixture.scratch.0.join("test-root");
        fs::create_dir_all(prepared::path(&test, "sample")).unwrap();
        let tested = Http::new().with_roots(vec![test.clone(), archive.clone()]);
        let refused = prepare_with(&fixture, (&Sample(1), &tested), "2026-10-01", Asked::default()).unwrap_err();
        assert!(refused.message.contains("another archive root"), "{}", refused.message);
        assert_eq!(keys(&test, "2026"), Vec::<String>::new(), "the test root gets no file");

        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        fs::remove_file(prepared::path(&archive, "sample").join("2026-10-01/manifest.json")).unwrap();
        let refused = prepare(&fixture, "2026-10-02", Asked::default()).unwrap_err();
        assert!(refused.message.contains("is in versions.json and has no manifest"), "{}", refused.message);
        assert_eq!(keys(&archive, "2026-10-01/").len(), 3, "its files stay");
    }

    #[test]
    fn a_prepare_replaces_a_file_that_a_stopped_run_left_with_other_bytes() {
        let (fixture, archive, _) = setup("prepare-replace");
        let (key, right) =
            (prepared::path(&archive, "sample").join("2026-10-01/a.txt"), "a.txt of 2026-10-01 for all: head\n");
        write(&key, &right.replace("a.txt", "z.txt"));
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        assert_eq!(fs::read_to_string(&key).unwrap(), right);
        let manifest =
            prepared::manifest(&prepared::folder(&archive, "sample"), "sample", "2026-10-01").unwrap().unwrap();
        assert_eq!(manifest.files[0].sha256, sha256_hex(right.as_bytes()));
    }

    #[test]
    fn a_new_version_deletes_older_incomplete_ones_and_does_not_resume_one_behind_a_complete_version() {
        let (fixture, archive, _) = setup("prepare-abandon");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap_err();
        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        // Another checkout starts the next version.
        let (job, aside) = (fixture.store.root().join("prepared/sample/2026-10-01"), fixture.scratch.0.join("aside"));
        fs::rename(&job, &aside).unwrap();
        prepare(&fixture, "2026-10-02", Asked::default()).unwrap();
        assert_eq!(keys(&archive, "2026-10-01"), Vec::<String>::new(), "the stopped version goes");

        fs::rename(&aside, &job).unwrap();
        let (root, http) = (fixture.root(), http(&fixture));
        let context = Context { root: &root, store: &fixture.store, http: &http, prepared: &[&Sample(1)] };
        let asked = Asked { id: "sample", ..Asked::default() };
        let review = plan(&context, &archive, &asked, "2026-10-03").unwrap().review;
        assert_eq!((review.version.as_str(), review.abandoned.as_deref()), ("2026-10-03", Some("2026-10-01")));
        prepare(&fixture, "2026-10-03", Asked::default()).unwrap();
        assert!(!job.exists(), "the work of the old version goes");
    }

    #[test]
    fn a_target_reads_only_a_version_of_the_format_of_its_producer() {
        let (fixture, archive, _) = setup("prepare-format");
        let http = http(&fixture);
        prepare_with(&fixture, (&Sample(1), &http), "2026-10-01", Asked::default()).unwrap();
        prepare_with(&fixture, (&Sample(2), &http), "2026-10-02", Asked::default()).unwrap();
        update(&fixture, "local", &Default::default()).unwrap();
        assert_eq!(served(&fixture, "local").0, pinned(&archive, "2026-10-01"));
        let refused = update_with(&fixture, (&Sample(3), &http), "prod", &Default::default()).unwrap_err();
        assert_eq!(refused.code, Code::Blocked);
        assert!(refused.fix.ends_with("or run `obc data source prepare sample`."), "{}", refused.fix);
    }

    /// The inode and the bytes of each file of `folder`, sorted by name.
    #[cfg(unix)]
    fn inodes(folder: &Path) -> Vec<(u64, Vec<u8>)> {
        use std::os::unix::fs::MetadataExt;
        let mut files: Vec<_> = fs::read_dir(folder).unwrap().map(|entry| entry.unwrap().path()).collect();
        files.sort();
        files.into_iter().map(|path| (fs::metadata(&path).unwrap().ino(), fs::read(&path).unwrap())).collect()
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_run_never_rewrites_a_version_that_another_run_completed() {
        let (fixture, archive, _) = setup("prepare-completed");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap_err();
        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        let (job, aside) = (fixture.store.root().join("prepared/sample/2026-10-01"), fixture.scratch.0.join("aside"));
        fs::rename(&job, &aside).unwrap();
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        let folder = prepared::path(&archive, "sample").join("2026-10-01");
        let state = || inodes(&folder);
        let before = state();

        fs::rename(&aside, &job).unwrap();
        assert_eq!(prepare(&fixture, "2026-10-01", Asked::default()).unwrap().version, "2026-10-01.2");
        assert_eq!(state(), before, "the files and the manifest of the complete version stay");
        assert!(!job.exists());
    }

    #[test]
    fn a_resume_makes_again_the_files_that_a_newer_version_deleted() {
        let (fixture, _, _) = setup("prepare-pruned");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap_err();
        let jobs = fixture.store.root().join("prepared/sample");
        let aside = fixture.scratch.0.join("aside");
        fs::rename(jobs.join("2026-10-01"), &aside).unwrap();
        // Another run starts a newer version, deletes the files of 2026-10-01, and stops too.
        prepare(&fixture, "2026-10-02", Asked::default()).unwrap_err();
        fs::remove_dir_all(jobs.join("2026-10-02")).unwrap();
        fs::rename(&aside, jobs.join("2026-10-01")).unwrap();
        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        let done = prepare(&fixture, "2026-10-03", Asked::default()).unwrap();
        assert_eq!((done.version.as_str(), done.files, done.uploaded), ("2026-10-01", 3, 3));
    }

    #[cfg(unix)]
    #[test]
    fn a_run_that_stopped_after_its_manifest_is_listed_and_nothing_else_is_written() {
        let (fixture, archive, _) = setup("prepare-listing");
        let home = prepared::path(&archive, "sample");
        let (gate, versions_file) = (fixture.root().join("gate"), home.join("versions.json"));
        fs::create_dir_all(&gate).unwrap();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| prepare(&fixture, "2026-10-01", Asked::default()));
            arrives(&gate.join("waiting"));
            // A folder in its place: the run stops after the manifest, before `versions.json`.
            fs::create_dir_all(&versions_file).unwrap();
            write(&gate.join("open"), "");
            first.join().unwrap().unwrap_err();
        });
        fs::remove_dir(&versions_file).unwrap();
        let folder = home.join("2026-10-01");
        let before = inodes(&folder);
        assert!(folder.join("manifest.json").exists());

        let done = prepare(&fixture, "2026-10-02", Asked::default()).unwrap();
        assert_eq!((done.version.as_str(), done.uploaded), ("2026-10-01", 0));
        assert_eq!(inodes(&folder), before, "the files and the manifest stay");
        assert_eq!(fs::read_to_string(fixture.root().join("made")).unwrap(), "a.txt\nb.txt\nc.txt\n", "no producer");
        let listed = prepared::versions(&prepared::folder(&archive, "sample"), "sample").unwrap();
        assert_eq!(listed[0].manifest_sha256, sha256_hex(&fs::read(folder.join("manifest.json")).unwrap()));
    }
}
