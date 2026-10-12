//! `obc data source prepare ID`: make a version of a prepared source and publish it to a target, as
//! [`crate::prepared`] says. It reviews the work, holds the lock of the source on the target, runs
//! detached, uploads each file once it is finished and writes the manifest last. The same command
//! resumes a run that stopped.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::update_cli::{self, Downloads, MachineCheck};
use super::{print_json, print_table, Code, Error};
use crate::engine::runs::{self, Event, Phase, Run};
use crate::fetch::http::Http;
use crate::lock::Held;
use crate::prepared::{self, Job, Manifest, Opening, Prepared, Scope, PROD};
use crate::settings::Settings;
use crate::sources::{FetchKind, Registry, Source};
use crate::store::{sha256_hex, Store};
use crate::target::Target;

#[derive(Args)]
pub struct PrepareArgs {
    /// The prepared source, as `data/sources.toml` names it.
    id: String,
    /// Only a region of `data/regions/`, or Geofabrik paths joined by `,`. Not for `prod`.
    #[arg(long, conflicts_with = "bbox")]
    region: Option<String>,
    /// Only a box: west,south,east,north in degrees. Not for `prod`.
    #[arg(long = "box", allow_hyphen_values = true)]
    bbox: Option<String>,
    /// The target that gets the version.
    #[arg(long, default_value = PROD)]
    to: String,
    /// Show the review, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Start only when the review has this id, as `--dry-run` showed it.
    #[arg(long, value_name = "ID", conflicts_with = "dry_run")]
    expect: Option<String>,
    /// Do not ask before a prepare for an R2 target. Required without a terminal.
    #[arg(long)]
    yes: bool,
}

/// What a prepare reads besides its target and its run.
pub(super) struct Context<'a> {
    pub root: &'a Path,
    pub store: &'a Store,
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
}

/// The review of a prepare. Its id names the decisions, so `--expect ID` starts a prepare only when
/// they are the ones that a person read.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PrepareReview {
    /// The first 12 hex digits of the SHA-256 of the canonical JSON of the source, the target, the
    /// version, the format, the revision, the scope and the source, version and params of each input.
    pub id: String,
    pub source: String,
    pub target: String,
    /// The version that the run makes: the UTC date when its first run started.
    pub version: String,
    /// The version that a stopped run of this machine started and that this run does not resume:
    /// the target has a newer complete version. `null` when there is none.
    pub abandoned: Option<String>,
    pub format: u32,
    pub revision: u32,
    /// `null`: every place that the source covers.
    pub scope: Option<Scope>,
    pub inputs: Vec<PreparedInput>,
    /// The files that earlier runs of the version uploaded: this run does not upload them again.
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
    /// The size of its files that the store records; `null` when the store has no record.
    pub bytes: Option<u64>,
}

/// Estimates from the newest version that the target lists; `null` without one.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct PreparedCost {
    pub wall_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    /// What goes into the store: the inputs that it lacks, and the files of the version.
    pub store_bytes: u64,
    pub upload_bytes: Option<u64>,
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
    pub target: String,
    pub files: usize,
    pub bytes: u64,
    /// The files that this run uploaded.
    pub uploaded: u64,
    pub manifest_sha256: String,
}

pub fn command(root: &Path, prepared: &[&dyn Prepared], args: PrepareArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, &args.to, &store).map_err(|e| Code::Usage.error(e))?;
    super::api::writable(&args.to, &target, true)?;
    let (http, today) = (Http::new(), crate::date::format(crate::date::today()));
    let context = Context { root, store: &store, http: &http, prepared };
    let asked =
        Asked { id: &args.id, scope: (args.region.as_deref(), args.bbox.as_deref()), expect: args.expect.as_deref() };
    let print = |done: &Done| {
        println!(
            "{}@{} on {}: {} files, {}, manifest {}; this run uploaded {}",
            done.source,
            done.version,
            done.target,
            done.files,
            runs::bytes(done.bytes),
            &done.manifest_sha256[..12],
            done.uploaded
        );
        Ok(())
    };
    let work = |run: &mut Run| prepare(&context, (&args.to, &target), &asked, &today, run);
    if let Some(done) = super::api::detached(&store, work, print) {
        return done;
    }
    let mut review = plan(&context, (&args.to, &target), &asked, &today)?.review;
    review.args = ["source", "prepare", &args.id, "--to", &args.to].map(String::from).into();
    args.region.iter().for_each(|region| review.args.extend(["--region".into(), region.clone()]));
    args.bbox.iter().for_each(|bbox| review.args.extend(["--box".into(), bbox.clone()]));
    review.args.extend(["--expect".into(), review.id.clone()]);
    if args.dry_run {
        return print_review(&review, json);
    }
    check(&review, asked.expect)?;
    let what = format!("a prepare for `{}`", args.to);
    let dry_run = format!("obc data source prepare {} --to {} --dry-run`", args.id, args.to);
    let question = format!("Publish {}@{} to `{}`", review.source, review.version, args.to);
    super::api::approved(&target, (asked.expect.is_some(), args.yes), (&what, &dry_run, &question))?;
    super::api::start(&store, &review.args, &review.id, json)
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
}

/// Find the producer of the source, its version and its scope, and review the work.
fn plan<'a>(
    context: &Context<'a>,
    (name, target): (&str, &Target),
    asked: &Asked,
    today: &str,
) -> Result<Planned<'a>, Error> {
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
    if scope.is_some() && name == PROD {
        return Err(Code::Usage
            .error(format!(
                "a version of a region or a box is not for `{PROD}`, whose versions every target reads; nothing changed"
            ))
            .fix(format!(
                "Prepare the region for another target, such as `--to local`, or all of `{id}` for `{PROD}`."
            )));
    }
    let listed = prepared::versions(target, id)?;
    let newest = listed.iter().map(|listed| listed.version.as_str()).max();
    // A stopped version is resumed only while the target has no complete version as new as it: a
    // run never rewrites a key of a complete version.
    let (resumed, abandoned) = match prepared::started(store, name, id)? {
        Some(started)
            if newest.is_some_and(|newest| newest >= started.as_str())
                || prepared::manifest(target, id, &started)?.is_some() =>
        {
            (None, Some(started))
        }
        started => (started, None),
    };
    let fresh = resumed.is_none();
    let version = resumed.unwrap_or_else(|| today.to_string());
    if fresh && prepared::manifest(target, id, &version)?.is_some() {
        return Err(Code::Usage
            .error(format!("{id}@{version} is complete on `{name}`; nothing changed"))
            .fix("The next version starts on the next day, by UTC."));
    }
    let wanted = producer.inputs(id, scope.as_ref()).map_err(|e| Code::Failed.error(format!("`{id}`: {e}")))?;
    let (mut inputs, mut store_bytes, mut unknown) = (Vec::new(), 0, Vec::new());
    for wanted in wanted {
        let bytes = match &wanted.version {
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
        inputs.push(PreparedInput { source: wanted.source, version: wanted.version, params: wanted.params, bytes });
    }
    let (files, bytes) = prepared::resumed(store, name, id, &version);
    let done = Downloads { files, bytes };
    let mut cost = PreparedCost { store_bytes, unknown, ..PreparedCost::default() };
    if let Some(newest) = listed.last() {
        let manifest = prepared::manifest(target, id, &newest.version)?;
        (cost.wall_ms, cost.peak_rss_bytes) =
            manifest.map_or((None, None), |m| (Some(m.cost.wall_ms), m.cost.peak_rss_bytes));
        cost.store_bytes += newest.bytes;
        cost.upload_bytes = Some(newest.bytes.saturating_sub(done.bytes));
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
    let stale = stale(root, store, (name, target), id, &version)?;
    let decisions = serde_json::json!({
        "source": id,
        "target": name,
        "version": version,
        "format": producer.format(),
        "revision": producer.revision(),
        "scope": scope,
        "inputs": inputs.iter().map(|input| (&input.source, &input.version, &input.params)).collect::<Vec<_>>(),
    });
    let canonical = serde_json::to_vec(&crate::engine::sorted(decisions)).expect("the decisions are JSON");
    let review = PrepareReview {
        id: sha256_hex(&canonical)[..12].to_string(),
        source: id.into(),
        target: name.into(),
        version,
        abandoned,
        format: producer.format(),
        revision: producer.revision(),
        scope: scope.clone(),
        inputs,
        done,
        cost,
        machine,
        stale,
        args: Vec::new(),
    };
    Ok(Planned { review, producer, source, sources, scope, fresh })
}

/// The layers of each target that read a version of `id` older than `version`. Every target reads
/// the versions of `prod`; another target reads only its own besides.
fn stale(
    root: &Path,
    store: &Store,
    (name, target): (&str, &Target),
    id: &str,
    version: &str,
) -> Result<Vec<StaleReaders>, Error> {
    let targets = match name {
        PROD => crate::target::load(root, store).map_err(|e| Code::InvalidData.error(e))?.into_iter().collect(),
        _ => vec![(name.to_string(), target.clone())],
    };
    let mut stale = Vec::new();
    for (name, target) in targets {
        let read = || -> Result<Vec<String>, Error> {
            let Some((serving, _)) = crate::release::serving(&target)? else { return Ok(Vec::new()) };
            let release = update_cli::read(&name, &target, &serving.serving.id)?;
            let old = |output: &&crate::release::Output| {
                let read = output.snapshots.get(id).and_then(|read| prepared::unpin(&read.version));
                read.is_some_and(|(read, _)| read < version)
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
            .fix("Run it on a machine with more memory or disk, or release space with `obc data clean`."));
    }
    Ok(())
}

/// Prepare the source of `asked` for `target` while the run holds the lock of the source there:
/// review again, make and upload each file, then write the manifest and list the version.
pub(super) fn prepare(
    context: &Context,
    (name, target): (&str, &Target),
    asked: &Asked,
    today: &str,
    run: &mut Run,
) -> Result<Done, Error> {
    let id = asked.id;
    let label = format!("source `{id}` on `{name}`");
    let unlock = format!("obc data unlock {name} --source {id}");
    update_cli::locked_as((&label, &unlock), &prepared::folder(target, id), run, |run, held| {
        run.record(&Event::Phase { phase: Phase::Prepare })?;
        let planned = plan(context, (name, target), asked, today)?;
        if !held.beat(Some(&planned.review.id), None, crate::date::now())? {
            return Err(update_cli::lost(&label));
        }
        check(&planned.review, asked.expect)?;
        make(context, (name, target), planned, (run, held))
    })
}

fn make(
    context: &Context,
    (name, target): (&str, &Target),
    planned: Planned,
    (run, held): (&mut Run, &Held),
) -> Result<Done, Error> {
    let Planned { review, producer, source, sources, scope, fresh } = planned;
    let (id, version) = (source.id.as_str(), review.version.as_str());
    let (label, again) = (format!("source `{id}` on `{name}`"), format!("obc data source prepare {id} --to {name}"));
    let lock = (label.as_str(), again.as_str());
    update_cli::enter_as((run, held), lock, Phase::Build)?;
    if let Some(abandoned) = &review.abandoned {
        prepared::forget(context.store, name, id, abandoned)?;
    }
    if fresh {
        // Nothing resumes an older version without a manifest: a stopped run left it.
        for (at, key) in prepared::incomplete(target, id, version)?.iter().enumerate() {
            if at % 100 == 0 && !held.beat(None, None, crate::date::now())? {
                return Err(update_cli::lost(lock.0));
            }
            target.remove(key)?;
        }
    }
    let start = Instant::now();
    let opening = Opening {
        root: context.root,
        store: context.store,
        http: context.http,
        sources: &sources,
        target: (name, target),
    };
    let (files, inputs, uploaded) = {
        let mut job = Job::open(opening, (id, version), scope.clone(), run)?;
        producer.prepare(&mut job).map_err(|e| Code::RunFailed.error(format!("{id}@{version}: {e}")))?;
        (job.files().cloned().collect::<Vec<_>>(), job.reads(), job.uploaded)
    };
    if files.is_empty() {
        return Err(Code::RunFailed.error(format!("{id}@{version}: the producer made no file; nothing is published")));
    }
    update_cli::enter_as((run, held), lock, Phase::Upload)?;
    let manifest = Manifest {
        source: id.into(),
        version: version.into(),
        format: producer.format(),
        revision: producer.revision(),
        inputs,
        files,
        scope,
        licence: source.licence.clone(),
        attribution: source.attribution.clone(),
        machine: crate::machine(),
        time: crate::date::timestamp(crate::date::now()),
        cost: prepared::Cost { wall_ms: start.elapsed().as_millis() as u64, peak_rss_bytes: crate::engine::peak_rss() },
    };
    let listed = prepared::complete(target, &manifest)?;
    prepared::forget(context.store, name, id, version)?;
    Ok(Done {
        source: id.into(),
        version: version.into(),
        target: name.into(),
        files: manifest.files.len(),
        bytes: listed.bytes,
        uploaded,
        manifest_sha256: listed.manifest_sha256,
    })
}

fn print_review(review: &PrepareReview, json: bool) -> Result<(), Error> {
    if json {
        return print_json(review);
    }
    println!("review {} of {}@{} for {}", review.id, review.source, review.version, review.target);
    let mut rows = Vec::new();
    let scope = review.scope.as_ref().map_or("all".to_string(), |scope| format!("region {}", scope.region));
    rows.push(vec!["  scope".into(), scope]);
    rows.push(vec!["  version".into(), format!("format {}, revision {}", review.format, review.revision)]);
    if let Some(abandoned) = &review.abandoned {
        let text = format!("{abandoned} stays incomplete: the target has a newer complete version");
        rows.push(vec!["  abandon".into(), text]);
    }
    for input in &review.inputs {
        let size = input.bytes.map_or("size unknown".into(), runs::bytes);
        let version = input.version.as_deref().unwrap_or("newest");
        rows.push(vec!["  input".into(), format!("{} {}", runs::fetched(&input.source, version, &input.params), size)]);
    }
    if review.done.files > 0 {
        rows.push(vec![
            "  resume".into(),
            format!("{} files, {} on the target", review.done.files, runs::bytes(review.done.bytes)),
        ]);
    }
    let cost = &review.cost;
    let mut text = format!("{} into the store", runs::bytes(cost.store_bytes));
    if let Some(upload) = cost.upload_bytes {
        text += &format!(", {} to the target", runs::bytes(upload));
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
mod tests {
    use super::*;
    use crate::cli::update_cli::tests::region;
    use crate::engine::tests::{fixture, write, Fixture};
    use crate::engine::{Client, Input, Layer, Request, Run as StepRun, Shape, Step};
    use crate::lock::{self, LockFile};
    use crate::product::{Inputs, Pointer, Product, Steps, Unplanned, Wanted};
    use crate::regions::Regions;
    use crate::release::POINTERS;
    use std::fs;

    const SOURCES: &str = r#"
        [[source]]
        id = "head"
        kind = "data"
        licence = "MIT"
        fetch = { kind = "http", url = "https://example.org/head.txt" }
        version = "release"
        redistribute = true

        [[source]]
        id = "sample"
        kind = "data"
        licence = "CC0-1.0"
        attribution = "Sample"
        fetch = { kind = "prepared" }
        version = "date"
        refresh = 1
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

        fn inputs(&self, _: &str, _: Option<&Scope>) -> Result<Vec<Wanted>, String> {
            Ok(vec![head()])
        }

        fn prepare(&self, job: &mut Job) -> Result<(), String> {
            let head = fs::read_to_string(&job.fetch(&head())?["head.txt"]).map_err(|e| e.to_string())?;
            for path in ["a.txt", "b.txt", "c.txt"] {
                if job.done(path) {
                    continue;
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
                job.finish(path)?;
            }
            Ok(())
        }
    }

    static READS: Layer = Layer { name: "reads", shape: Shape::Whole, format: 1, revision: 1, readers: &["tests"] };

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
                run: StepRun::Rust(copy),
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

    /// A store and a repository with `sample`, and the dir targets `prod` and `local`.
    fn setup(name: &str) -> (Fixture, Target, Target) {
        let fixture = fixture(name);
        write(&fixture.root().join("data/sources.toml"), SOURCES);
        let (prod, local) = (fixture.scratch.0.join("prod"), fixture.scratch.0.join("local"));
        let targets = format!(
            "[prod]\nkind = \"dir\"\ndir = \"{}\"\n[local]\nkind = \"dir\"\ndir = \"{}\"\n",
            prod.display(),
            local.display()
        );
        write(&fixture.root().join("data/targets.toml"), &targets);
        (fixture, Target::Dir(prod), Target::Dir(local))
    }

    /// Prepare `sample` of format 1 for the target `name` on the day `today`.
    fn prepare(fixture: &Fixture, name: &str, today: &str, asked: Asked) -> Result<Done, Error> {
        prepare_with(fixture, &Sample(1), (name, today), asked)
    }

    fn prepare_with(
        fixture: &Fixture,
        sample: &Sample,
        (name, today): (&str, &str),
        asked: Asked,
    ) -> Result<Done, Error> {
        let (root, http) = (fixture.root(), Http::new());
        let target = crate::target::find(&root, name, &fixture.store).unwrap();
        let context = Context { root: &root, store: &fixture.store, http: &http, prepared: &[sample] };
        let mut run = Run::create(&fixture.store, ["source", "prepare", "sample"]).unwrap();
        let done = super::prepare(&context, (name, &target), &Asked { id: "sample", ..asked }, today, &mut run);
        crate::cli::api::finish_run(run, done)
    }

    /// Update the target `name` with the producer of format 1.
    fn update(
        fixture: &Fixture,
        target: (&str, &Target),
        asked: &update_cli::Asked,
    ) -> Result<update_cli::Updated, Error> {
        update_with(fixture, &Sample(1), target, asked)
    }

    fn update_with(
        fixture: &Fixture,
        sample: &Sample,
        (name, target): (&str, &Target),
        asked: &update_cli::Asked,
    ) -> Result<update_cli::Updated, Error> {
        let mut run = Run::create(&fixture.store, ["update", name]).unwrap();
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Reader, &Second]);
        let machine = MachineCheck::default();
        let settings = |settings: &mut Settings| if settings.region.is_empty() { region(settings) } else { Ok(()) };
        let code: (&[&dyn Product], &[&dyn Prepared]) = (&products, &[sample]);
        let updated = update_cli::update(
            &root,
            &fixture.store,
            &Http::new(),
            (name, target),
            code,
            settings,
            asked,
            Some(&machine),
            &mut run,
        );
        crate::cli::api::finish_run(run, updated)
    }

    /// The layers that an update of the target `name` bakes, with the text of their cause.
    fn changes(fixture: &Fixture, (name, target): (&str, &Target)) -> Vec<(String, String)> {
        let (products, machine): ([&dyn Product; 2], _) = ([&Reader, &Second], MachineCheck::default());
        let code: (&[&dyn Product], &[&dyn Prepared]) = (&products, &[&Sample(1)]);
        let (root, http) = (fixture.root(), Http::new());
        let prepared = update_cli::prepare(
            &root,
            &fixture.store,
            &http,
            (name, target),
            code,
            |_| Ok(()),
            &[],
            Some(&machine),
            None,
        );
        prepared.unwrap().review.layers.into_iter().map(|layer| (layer.layer, layer.text)).collect()
    }

    /// The version of `sample` that the serving release of `target` reads, and the bytes of its
    /// client file.
    fn served(target: &Target) -> (String, String) {
        let (serving, _) = crate::release::serving(target).unwrap().unwrap();
        let release = crate::release::Release::read(target, &serving.serving.id).unwrap();
        let output = release.outputs.iter().find(|output| output.step == "reader/copy").unwrap();
        let bytes = target.get(&format!("objects/{}", output.files[0].sha256)).unwrap().unwrap();
        (output.snapshots["sample"].version.clone(), String::from_utf8(bytes).unwrap())
    }

    /// The version of `sample` that a step reads for the entry of `day` in `versions.json` of `target`.
    fn pinned(target: &Target, day: &str) -> String {
        let listed = prepared::versions(target, "sample").unwrap();
        let entry = listed.iter().find(|entry| entry.version == day).unwrap();
        format!("{}/{}", entry.version, entry.manifest_sha256)
    }

    fn keys(target: &Target, prefix: &str) -> Vec<String> {
        let listed = target.list().unwrap().into_iter().map(|listed| listed.key);
        listed.filter(|key| key.starts_with(prefix)).collect()
    }

    #[test]
    fn the_manifest_comes_last_and_a_resume_uploads_only_what_the_target_lacks() {
        let (fixture, prod, _) = setup("prepare-resume");
        write(&fixture.root().join("fail-c.txt"), "");
        let failed = prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap_err();
        assert!(failed.message.contains("c.txt failed"), "{}", failed.message);
        let version = "sources/sample/2026-10-01/";
        assert_eq!(keys(&prod, version), [format!("{version}a.txt"), format!("{version}b.txt")], "no manifest");
        assert!(prepared::versions(&prod, "sample").unwrap().is_empty());

        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        let done = prepare(&fixture, "prod", "2026-10-02", Asked::default()).unwrap();
        assert_eq!(
            (done.version.as_str(), done.files, done.uploaded),
            ("2026-10-01", 3, 1),
            "the resume keeps its version"
        );
        assert_eq!(fs::read_to_string(fixture.root().join("made")).unwrap(), "a.txt\nb.txt\nc.txt\n");
        let manifest = prepared::manifest(&prod, "sample", "2026-10-01").unwrap().unwrap();
        let paths: Vec<&str> = manifest.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt", "c.txt"]);
        assert_eq!((manifest.inputs[0].source.as_str(), manifest.licence.as_deref()), ("head", Some("CC0-1.0")));
        let bytes = prod.get(&format!("{version}manifest.json")).unwrap().unwrap();
        let listed = prepared::versions(&prod, "sample").unwrap();
        assert_eq!((listed[0].version.as_str(), &listed[0].manifest_sha256), ("2026-10-01", &sha256_hex(&bytes)));
        assert!(
            !fixture.store.root().join("prepared/prod/sample").read_dir().unwrap().any(|_| true),
            "the work is gone"
        );
        assert_eq!(lock::read(&prepared::folder(&prod, "sample")).unwrap().unwrap().0, LockFile::Free);
        let again = prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap_err();
        assert!(again.message.contains("complete on `prod`"), "{}", again.message);
    }

    #[test]
    fn a_new_version_makes_its_readers_stale_in_a_target_that_reads_an_older_one() {
        let (fixture, prod, local) = setup("prepare-stale");
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap();
        update(&fixture, ("prod", &prod), &Default::default()).unwrap();
        let (root, http) = (fixture.root(), Http::new());
        let context = Context { root: &root, store: &fixture.store, http: &http, prepared: &[&Sample(1)] };
        let review =
            plan(&context, ("prod", &prod), &Asked { id: "sample", ..Asked::default() }, "2026-10-02").unwrap().review;
        let stale: Vec<(&str, &[String])> =
            review.stale.iter().map(|readers| (readers.target.as_str(), &readers.layers[..])).collect();
        assert_eq!(stale, [("local", &[][..]), ("prod", &["reads".to_string()][..])]);
        prepare(&fixture, "prod", "2026-10-02", Asked::default()).unwrap();
        update(&fixture, ("local", &local), &Default::default()).unwrap();

        let show = |day: &str| prepared::show(&pinned(&prod, day));
        let cause = format!("sample {} → {}", show("2026-10-01"), show("2026-10-02"));
        assert_eq!(changes(&fixture, ("prod", &prod)), [("reads".to_string(), cause)]);
        assert_eq!(changes(&fixture, ("local", &local)), [], "it reads the new version already");
    }

    #[test]
    fn a_collection_keeps_the_newest_unused_version_an_incomplete_one_and_older_keys() {
        let (fixture, prod, _) = setup("prepare-collect");
        for day in ["2026-10-01", "2026-10-02"] {
            prepare(&fixture, "prod", day, Asked::default()).unwrap();
        }
        let updated = update(&fixture, ("prod", &prod), &Default::default()).unwrap();
        assert_eq!(updated.collection.unwrap().versions, ["sample@2026-10-01"], "older than the version it reads");
        prepare(&fixture, "prod", "2026-10-03", Asked::default()).unwrap();
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "prod", "2026-10-04", Asked::default()).unwrap_err();
        let Target::Dir(dir) = &prod else { unreachable!() };
        write(&dir.join("reference/v1/x"), "older publisher");

        let collected = crate::collection::collect(&prod, None, crate::date::now(), &|| Ok(()));
        assert_eq!((collected.keys, collected.stopped), (0, None));
        let versions: BTreeSet<String> = keys(&prod, "sources/sample/2026")
            .into_iter()
            .map(|key| key["sources/sample/".len()..].split('/').next().unwrap().to_string())
            .collect();
        assert_eq!(versions.into_iter().collect::<Vec<_>>(), ["2026-10-02", "2026-10-03", "2026-10-04"]);
        let listed = prepared::versions(&prod, "sample").unwrap();
        assert_eq!(
            listed.iter().map(|listed| listed.version.as_str()).collect::<Vec<_>>(),
            ["2026-10-02", "2026-10-03"]
        );
        assert_eq!(keys(&prod, "reference/"), ["reference/v1/x"]);
    }

    #[test]
    fn a_local_bake_reads_a_version_of_prod_and_then_bakes_from_the_store_alone() {
        let (fixture, prod, local) = setup("prepare-offline");
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap();
        update(&fixture, ("local", &local), &Default::default()).unwrap();
        let Target::Dir(dir) = &prod else { unreachable!() };
        fs::rename(dir, dir.with_file_name("prod-gone")).unwrap();
        let only = update_cli::Asked { only: vec!["reads".into()], ..Default::default() };
        let again = update(&fixture, ("local", &local), &only).unwrap();
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
            let first = scope.spawn(|| prepare(&fixture, "prod", "2026-10-01", Asked::default()));
            arrives(&gate.join("waiting"));
            let second = prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap_err();
            write(&gate.join("open"), "");
            assert_eq!(second.code, Code::Blocked);
            assert!(second.message.starts_with("source `sample` on `prod`: run "), "{}", second.message);
            assert!(second.fix.contains("`obc data unlock prod --source sample --from "), "{}", second.fix);
            first.join().unwrap().unwrap();
        });
    }

    #[test]
    fn a_prepare_refuses_another_review_and_a_region_for_prod() {
        let (fixture, prod, local) = setup("prepare-refuse");
        let other = Asked { expect: Some("000000000000"), ..Asked::default() };
        let refused = prepare(&fixture, "prod", "2026-10-01", other).unwrap_err();
        assert!(refused.message.contains(", not 000000000000: it changed"), "{}", refused.message);
        let boxed = || Asked { scope: (None, Some("7,47,8,48")), ..Asked::default() };
        let refused = prepare(&fixture, "prod", "2026-10-01", boxed()).unwrap_err();
        assert!(refused.message.contains("is not for `prod`"), "{}", refused.message);
        assert_eq!(keys(&prod, "sources/sample/2026"), Vec::<String>::new(), "nothing changed");
        prepare(&fixture, "local", "2026-10-01", boxed()).unwrap();
        let manifest = prepared::manifest(&local, "sample", "2026-10-01").unwrap().unwrap();
        assert_eq!(manifest.scope.map(|scope| scope.region), Some("box".into()));
    }

    #[test]
    fn a_bake_refuses_a_file_whose_bytes_are_not_those_of_the_manifest() {
        let (fixture, prod, local) = setup("prepare-tamper");
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap();
        let Target::Dir(dir) = &prod else { unreachable!() };
        let file = dir.join("sources/sample/2026-10-01/a.txt");
        fs::remove_file(&file).unwrap();
        write(&file, "other bytes");
        let refused = update(&fixture, ("local", &local), &Default::default()).unwrap_err();
        assert!(refused.message.contains("2026-10-01/a.txt holds bytes with the SHA-256"), "{}", refused.message);
    }

    #[test]
    fn a_target_reads_its_own_version_of_a_name_and_prod_reads_the_one_of_prod() {
        let (fixture, prod, local) = setup("prepare-identity");
        let boxed = Asked { scope: (None, Some("7,47,8,48")), ..Asked::default() };
        prepare(&fixture, "local", "2026-10-01", boxed).unwrap();
        update(&fixture, ("local", &local), &Default::default()).unwrap();
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap();
        update(&fixture, ("prod", &prod), &Default::default()).unwrap();
        let ((prod_reads, prod_bytes), (local_reads, local_bytes)) = (served(&prod), served(&local));
        assert_eq!((prod_reads, local_reads), (pinned(&prod, "2026-10-01"), pinned(&local, "2026-10-01")));
        assert_ne!(pinned(&prod, "2026-10-01"), pinned(&local, "2026-10-01"), "one name, two versions");
        assert!(prod_bytes.contains("for all") && local_bytes.contains("for box"), "{prod_bytes} {local_bytes}");
        assert_eq!(changes(&fixture, ("local", &local)), [], "the own version of `local` wins");
    }

    #[test]
    fn a_prepare_replaces_a_key_that_a_stopped_run_left_with_other_bytes() {
        let (fixture, prod, _) = setup("prepare-replace");
        let Target::Dir(dir) = &prod else { unreachable!() };
        let (key, right) = (dir.join("sources/sample/2026-10-01/a.txt"), "a.txt of 2026-10-01 for all: head\n");
        write(&key, &right.replace("a.txt", "z.txt"));
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap();
        assert_eq!(fs::read_to_string(&key).unwrap(), right);
        let manifest = prepared::manifest(&prod, "sample", "2026-10-01").unwrap().unwrap();
        assert_eq!(manifest.files[0].sha256, sha256_hex(right.as_bytes()));
    }

    #[test]
    fn a_new_version_deletes_older_incomplete_ones_and_does_not_resume_one_behind_a_complete_version() {
        let (fixture, prod, _) = setup("prepare-abandon");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap_err();
        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        // Another machine starts the next version.
        let (job, aside) =
            (fixture.store.root().join("prepared/prod/sample/2026-10-01"), fixture.scratch.0.join("aside"));
        fs::rename(&job, &aside).unwrap();
        prepare(&fixture, "prod", "2026-10-02", Asked::default()).unwrap();
        assert_eq!(keys(&prod, "sources/sample/2026-10-01"), Vec::<String>::new(), "the stopped version goes");

        fs::rename(&aside, &job).unwrap();
        let (root, http) = (fixture.root(), Http::new());
        let context = Context { root: &root, store: &fixture.store, http: &http, prepared: &[&Sample(1)] };
        let asked = Asked { id: "sample", ..Asked::default() };
        let review = plan(&context, ("prod", &prod), &asked, "2026-10-03").unwrap().review;
        assert_eq!((review.version.as_str(), review.abandoned.as_deref()), ("2026-10-03", Some("2026-10-01")));
        prepare(&fixture, "prod", "2026-10-03", Asked::default()).unwrap();
        assert!(!job.exists(), "the work of the old version goes");
    }

    #[test]
    fn a_target_reads_only_a_version_of_the_format_of_its_producer() {
        let (fixture, prod, local) = setup("prepare-format");
        prepare_with(&fixture, &Sample(1), ("prod", "2026-10-01"), Asked::default()).unwrap();
        prepare_with(&fixture, &Sample(2), ("prod", "2026-10-02"), Asked::default()).unwrap();
        update(&fixture, ("local", &local), &Default::default()).unwrap();
        assert_eq!(served(&local).0, pinned(&prod, "2026-10-01"));
        let refused = update_with(&fixture, &Sample(3), ("prod", &prod), &Default::default()).unwrap_err();
        assert_eq!((refused.code, refused.fix.as_str()), (Code::Blocked, "Run `obc data source prepare sample`."));
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_run_never_rewrites_a_version_that_another_machine_completed() {
        use std::os::unix::fs::MetadataExt;
        let (fixture, prod, _) = setup("prepare-completed");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap_err();
        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        let (job, aside) =
            (fixture.store.root().join("prepared/prod/sample/2026-10-01"), fixture.scratch.0.join("aside"));
        fs::rename(&job, &aside).unwrap();
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap();
        let Target::Dir(dir) = &prod else { unreachable!() };
        let folder = dir.join("sources/sample/2026-10-01");
        let state = || {
            let mut files: Vec<_> = fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()).collect();
            files.sort();
            files
                .into_iter()
                .map(|path| (fs::metadata(&path).unwrap().ino(), fs::read(&path).unwrap()))
                .collect::<Vec<_>>()
        };
        let before = state();

        fs::rename(&aside, &job).unwrap();
        let refused = prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap_err();
        assert!(refused.message.contains("complete on `prod`"), "{}", refused.message);
        assert_eq!(prepare(&fixture, "prod", "2026-10-02", Asked::default()).unwrap().version, "2026-10-02");
        assert_eq!(state(), before, "the keys and the manifest of the complete version stay");
        assert!(!job.exists());
    }

    #[test]
    fn a_resume_makes_again_the_files_whose_keys_a_newer_version_deleted() {
        let (fixture, _, _) = setup("prepare-pruned");
        write(&fixture.root().join("fail-c.txt"), "");
        prepare(&fixture, "prod", "2026-10-01", Asked::default()).unwrap_err();
        let jobs = fixture.store.root().join("prepared/prod/sample");
        let aside = fixture.scratch.0.join("aside");
        fs::rename(jobs.join("2026-10-01"), &aside).unwrap();
        // Another run starts a newer version, deletes the files of 2026-10-01, and stops too.
        prepare(&fixture, "prod", "2026-10-02", Asked::default()).unwrap_err();
        fs::remove_dir_all(jobs.join("2026-10-02")).unwrap();
        fs::rename(&aside, jobs.join("2026-10-01")).unwrap();
        fs::remove_file(fixture.root().join("fail-c.txt")).unwrap();
        let done = prepare(&fixture, "prod", "2026-10-03", Asked::default()).unwrap();
        assert_eq!((done.version.as_str(), done.files, done.uploaded), ("2026-10-01", 3, 3));
    }

    #[test]
    fn a_fetch_skips_a_target_that_it_cannot_read() {
        let (fixture, _, local) = setup("prepare-unread");
        let Target::Dir(dir) = &local else { unreachable!() };
        let targets = format!(
            "[prod]\nkind = \"r2\"\nbucket = \"b\"\npublic = \"https://127.0.0.1:1\"\n[local]\nkind = \"dir\"\ndir = \"{}\"\n",
            dir.display()
        );
        write(&fixture.root().join("data/targets.toml"), &targets);
        prepare(&fixture, "local", "2026-10-01", Asked::default()).unwrap();
        update(&fixture, ("local", &local), &Default::default()).unwrap();
        assert_eq!(served(&local).0, pinned(&local, "2026-10-01"));
    }
}
