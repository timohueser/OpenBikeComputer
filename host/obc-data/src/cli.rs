//! `obc data`: update a target, show what each target serves, read the sources and the regions,
//! fetch sources into the store, and show runs. Without a command, it shows every target.

mod api;
mod archive_cli;
mod copy_cli;
mod deploy_cli;
mod freshness;
mod holding;
mod listing;
mod open_cli;
mod overview;
mod prepare_cli;
mod r2_cli;
mod rollback_cli;
mod router_cli;
mod runs_cli;
mod serve_cli;
mod setup_cli;
mod source_cli;
mod unlock_cli;
mod update_cli;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use crate::fetch::http::Http;
use crate::fetch::Request;
use crate::prepared::Prepared;
use crate::product::Product;
use crate::regions::{geofabrik, Area, Bbox, Region, Regions};
use crate::release::{Release, SERVING};
use crate::sources::{Kind, Refresh, Registry, Source, State};
use crate::store::{gc, hash_file, FileRecord, Snapshot, Store};
use api::{confirm, print_json};
pub use api::{Code, Error};

/// `obc data ui`: serve the page of this machine. It takes the repository root. The binary that
/// links the page gives it; `obc_data` keeps no page, because the page is GPL.
pub type Ui = fn(&Path) -> Result<(), Error>;

#[derive(Parser)]
#[command(name = "obc data", about = "Data sources, regions, targets and their releases")]
struct Cli {
    /// Write JSON to standard output, also when the command fails.
    #[arg(long, global = true)]
    json: bool,
    /// Without a command: what each target serves, and the command to run next.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Bake what is stale for a target, publish the new release and switch the target to it. The
    /// flags change the settings of the target, which its release keeps.
    Update(update_cli::UpdateArgs),
    /// Switch a target back to its rollback release. The release that served becomes the rollback,
    /// so a second rollback undoes the first.
    Rollback(rollback_cli::RollbackArgs),
    /// Whether the follower of an R2 target serves the planner releases of its serving release and
    /// of its rollback. Changes nothing.
    Router { target: String },
    /// Free the lock of a target that a stopped run of MACHINE holds: its heartbeat is older than
    /// 10 minutes. Asks first.
    Unlock {
        #[arg(required_unless_present = "source", conflicts_with = "source")]
        target: Option<String>,
        #[arg(long, value_name = "MACHINE")]
        from: String,
        /// The lock of this prepared source in the archive, not the lock of a target.
        #[arg(long, value_name = "ID")]
        source: Option<String>,
        /// Do not ask. Required without a terminal.
        #[arg(long)]
        yes: bool,
    },
    /// Every source with its licence, how this machine holds it, and its state for each target;
    /// with `--layer` or a region, the sources of each layer or of a bake of the region.
    Sources {
        /// Check upstream now, not from a check of the last hour.
        #[arg(long)]
        check_now: bool,
        /// For each layer, or LAYER only: the sources that it reads, and how this machine holds them.
        #[arg(long, value_name = "LAYER", num_args = 0..=1, default_missing_value = "")]
        #[arg(conflicts_with_all = ["check_now", "region", "bbox"])]
        layer: Option<String>,
        /// The sources that a bake of this region reads, and what it still downloads or prepares: a
        /// region of `data/regions/`, or Geofabrik paths joined by `,`.
        #[arg(long, value_name = "ID|PATH", conflicts_with_all = ["check_now", "bbox"])]
        region: Option<String>,
        /// The sources that a bake of this box needs: west, south, east, north.
        #[arg(long = "box", value_name = "W,S,E,N", conflicts_with = "check_now")]
        bbox: Option<String>,
    },
    /// Act on one source of `data/sources.toml`.
    Source {
        #[command(subcommand)]
        command: source_cli::SourceCommand,
    },
    /// Fetch a source version into the store, and print the store path of each file.
    Fetch {
        /// SOURCE or SOURCE@VERSION. Without a version: upstream's newest file.
        target: String,
        /// NAME=VALUE for each `{name}` in the URL of the source, such as `tile=…` or `area=…`.
        params: Vec<String>,
    },
    /// The datasets of the archive in OBC_DATA_ARCHIVE: their state, and their get and verify.
    Archive {
        #[command(subcommand)]
        command: archive_cli::ArchiveCommand,
    },
    /// The regions of `data/regions/` and the Geofabrik areas of the cached index whose id or name
    /// holds QUERY. Nothing is fetched.
    Areas {
        #[arg(default_value = "")]
        query: String,
    },
    /// Serve a dir target to the apps of this machine and of its network, and follow the release
    /// that it serves. It runs until Ctrl-C, or with `--detach` as a run.
    Serve(serve_cli::ServeArgs),
    /// The runs in the store, newest first; with RUN, its steps.
    Runs(runs_cli::Runs),
    /// Clean the local store: delete what no release of a target reaches, and empty `partial/`.
    /// Shows the plan; `--apply` asks, then cleans.
    Clean {
        #[arg(long)]
        apply: bool,
        /// Do not ask. Required without a terminal.
        #[arg(long, requires = "apply")]
        yes: bool,
    },
    /// The follower and the tiles Worker of prod: which commit each runs; with an action, deploy
    /// the commit of this checkout.
    Deploy {
        #[command(subcommand)]
        command: Option<deploy_cli::DeployCommand>,
    },
    /// Plumbing for scripts: list, read, upload and delete objects in an R2 bucket.
    R2(r2_cli::R2),
    /// What this machine can do: its tools and credentials (only whether each is set), and the
    /// store.
    Setup,
    /// Serve the page of this machine on http://127.0.0.1:8700/ until it is stopped.
    Ui,
    /// Open the page of MACHINE through an SSH tunnel at http://localhost:8701/.
    Open { machine: String },
}

/// Run `obc data` with the products and the producers of prepared sources that this binary links,
/// and its page.
pub fn main(products: &[&dyn Product], prepared: &[&dyn Prepared], ui: Option<Ui>) -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // The arguments did not parse, so `--json` is only known as a word among them.
        Err(e) if e.use_stderr() && std::env::args_os().any(|arg| arg == "--json") => {
            let text = e.render().to_string();
            let first = text.split("\n\n").next().unwrap_or_default().trim_start_matches("error: ");
            let message: Vec<&str> = first.lines().map(str::trim).collect();
            return Code::Usage.error(message.join(" ")).report(true);
        }
        Err(e) => e.exit(),
    };
    let json = cli.json;
    // Not for the page: each command that it runs loads the file, so a credential set on the page
    // serves the next command.
    if !matches!(cli.command, Some(Command::Ui)) {
        crate::credentials::load_home();
    }
    match run(cli, products, prepared, ui) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => error.report(json),
    }
}

/// Report a startup failure with the command's existing error format.
pub fn failed(message: String) -> ExitCode {
    Code::Failed.error(message).report(std::env::args_os().any(|arg| arg == "--json"))
}

fn run(cli: Cli, products: &[&dyn Product], prepared: &[&dyn Prepared], ui: Option<Ui>) -> Result<(), Error> {
    let json = cli.json;
    let Some(command) = cli.command else {
        return overview::overview(&root()?, (products, prepared), json);
    };
    match command {
        Command::Update(args) => update_cli::command(&root()?, (products, prepared), args, json),
        Command::Rollback(args) => rollback_cli::command(&root()?, args, json),
        Command::Router { target } => router_cli::command(&root()?, &target, json),
        Command::Unlock { target, from, source, yes } => {
            unlock_cli::command(&root()?, (target.as_deref(), source.as_deref()), &from, yes, json)
        }
        Command::Sources { check_now, layer, region, bbox } => {
            let root = root()?;
            let view = match (layer, region, bbox) {
                (Some(layer), _, _) => View::Layer(Some(layer).filter(|layer| !layer.is_empty())),
                (_, None, None) => View::All { check_now },
                (_, region, bbox) => View::Region(region, bbox),
            };
            sources_command(&root, (products, prepared), view, json)
        }
        Command::Source { command } => source_cli::run(&root()?, prepared, command, json),
        Command::Fetch { target, params } => {
            let registry = registry(&root()?)?;
            let (id, version) = match target.split_once('@') {
                Some((id, version)) => (id, Some(version.to_string())),
                None => (target.as_str(), None),
            };
            let source = find(&registry, id)?;
            let store = Store::open()?;
            let request = Request { refresh: false, source, version, params: parse_params(&params)? };
            let snapshot = crate::fetch::fetch(&root()?, &store, &Http::new(), &request);
            print_snapshot(&store, &fetched(source, snapshot)?, json)
        }
        Command::Archive { command } => archive_cli::run(&root()?, command, json),
        Command::Areas { query } => print_areas(&root()?, &query, json),
        Command::Serve(args) => serve_cli::command(&root()?, args, json),
        Command::Runs(runs) => runs_cli::run(runs, json),
        Command::Clean { apply, yes } => clean_command(&root()?, apply, yes, json),
        Command::Deploy { command } => deploy_cli::command(&root()?, command, json),
        Command::R2(r2) => r2_cli::run(r2, json),
        Command::Setup => setup_cli::setup(&root()?, json),
        Command::Ui => match ui {
            Some(ui) => ui(&root()?),
            None => Err(Code::Usage.error("this binary has no page").fix("Run `obc data ui`.")),
        },
        Command::Open { machine } => open_cli::open(&root()?, &machine),
    }
}

/// What `clean` removes from the store, or removed.
#[derive(Debug, Default, Clone, Serialize, JsonSchema)]
struct CleanPlan {
    /// The snapshot records and the objects that nothing reaches, and what stays.
    store: gc::Plan,
}

impl CleanPlan {
    fn is_empty(&self) -> bool {
        let gc = &self.store;
        gc.snapshots.is_empty() && gc.objects.is_empty() && gc.partial_bytes == 0 && gc.unpacked.is_empty()
    }

    /// The one question before a clean.
    fn question(&self) -> String {
        let gc = &self.store;
        match (gc.objects.len() + gc.unpacked.len(), gc.snapshots.len(), gc.partial_bytes) {
            (0, 0, 0) => "Nothing to clean.".into(),
            (0, 1, 0) => "Remove 1 record from the local store?".into(),
            (0, records, 0) => format!("Remove {records} records from the local store?"),
            _ => format!("Remove {} from the local store?", bytes(gc.remove_bytes + gc.partial_bytes)),
        }
    }
}

/// The serving release and the rollback of a target, or why the target cannot be read.
type Releases = Result<Vec<Release>, Error>;

/// The [`Releases`] of each target, by target.
fn releases(root: &Path, store: &Store) -> Result<Vec<(String, Releases)>, Error> {
    let targets = crate::target::load(root, store).map_err(|e| Code::InvalidData.error(e))?;
    let read = |name: &str, target: &crate::target::Target| -> Result<Vec<Release>, Error> {
        let Some(bytes) = target.get(SERVING)? else { return Ok(Vec::new()) };
        let serving: crate::release::Serving = serde_json::from_slice(&bytes)
            .map_err(|e| Code::InvalidData.error(format!("target `{name}`: {SERVING}: {e}")))?;
        let ids = std::iter::once(&serving.serving).chain(&serving.rollback).map(|role| role.id.as_str());
        ids.map(|id| update_cli::read(name, target, id)).collect()
    };
    Ok(targets
        .into_iter()
        .map(|(name, target)| {
            let releases = read(&name, &target);
            (name, releases)
        })
        .collect())
}

/// The roots of a collection: the releases of the targets, and the checkout at `root`.
fn roots(root: &Path, store: &Store) -> Result<gc::Roots, Error> {
    let mut roots = gc::Roots::from_repo(root).map_err(|e| Code::InvalidData.error(e))?;
    for (name, releases) in releases(root, store)? {
        let releases = releases.map_err(|e| match e.code {
            Code::InvalidData => e,
            _ => {
                let fix = "A clean keeps what each target serves: run it again when every target can be read.";
                Code::Failed.error(format!("target `{name}`: {}", e.message)).fix(fix)
            }
        })?;
        releases.iter().for_each(|release| roots.add_release(&name, release));
    }
    roots.refreshed = store.refreshes()?.into_iter().map(|refreshed| (refreshed.source, refreshed.to)).collect();
    roots.archived = crate::fetch::archive::Archive::from_env().and_then(|archive| archive.sha256s());
    Ok(roots)
}

fn clean_command(root: &Path, apply: bool, yes: bool, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let plan = CleanPlan { store: gc::plan(&store, &roots(root, &store)?)? };
    if json && !apply {
        return print_json(&plan);
    }
    let text = clean_text(&store, &plan);
    // With `--json`, the output is what the clean did.
    if json {
        eprint!("{text}");
    } else {
        print!("{text}");
    }
    if !apply {
        if !plan.is_empty() {
            println!("`--apply` asks once, then cleans.");
        }
        return Ok(());
    }
    if plan.is_empty() {
        return if json { print_json(&plan) } else { Ok(()) };
    }
    confirm(&plan.question(), yes)?;
    let removed = gc::apply(&store, &roots(root, &store)?, &plan.store)?.ok_or_else(|| {
        Code::Usage
            .error("a run or fetch uses the store; nothing was deleted")
            .fix("Run `obc data clean --apply` again when the run or fetch ends.")
    })?;
    let done = CleanPlan { store: removed };
    if json {
        return print_json(&done);
    }
    println!("Removed {} from the store.", bytes(done.store.remove_bytes + done.store.partial_bytes));
    Ok(())
}

fn clean_text(store: &Store, plan: &CleanPlan) -> String {
    if plan.is_empty() {
        return "Nothing to clean.\n".into();
    }
    let (gc, mut text) = (&plan.store, String::new());
    text += &format!("REMOVE FROM {}\n", store.root().display());
    gc.snapshots.iter().for_each(|snapshot| text += &format!("  snapshot {snapshot}\n"));
    gc.objects.iter().for_each(|(sha256, size)| text += &format!("  object {sha256}  {}\n", bytes(*size)));
    let objects = gc.objects.iter().map(|(_, size)| size).sum();
    text += &format!("  {} that nothing reaches, {}\n", gc::objects_text(gc.objects.len() as u64), bytes(objects));
    for (sha256, size) in &gc.unpacked {
        text += &format!("  unpacked/{sha256}: its archive file is no longer listed, {}\n", bytes(*size));
    }
    if gc.partial_bytes > 0 {
        text += &format!("  partial/: unfinished downloads and stopped steps, {}\n", bytes(gc.partial_bytes));
    }
    let copies = match gc.keep_copies {
        0 => String::new(),
        1 => " and 1 unpacked copy".into(),
        copies => format!(" and {copies} unpacked copies"),
    };
    text += &format!("KEEP {}{copies}, {}\n", gc::objects_text(gc.keep_objects), bytes(gc.keep_bytes));
    let kept =
        gc.kept.iter().map(|kept| vec![format!("  {}", kept.entry), bytes(kept.bytes), kept.because.join(" · ")]);
    text += &table(&kept.collect::<Vec<_>>());
    text
}

fn bytes(bytes: u64) -> String {
    match bytes {
        0..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

fn root() -> Result<std::path::PathBuf, Error> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    crate::find_root(&cwd).ok_or_else(|| Code::Usage.error("no data/sources.toml above the current directory"))
}

fn registry(root: &Path) -> Result<Registry, Error> {
    Registry::load(root).map_err(|e| Code::InvalidData.error(e))
}

fn find<'a>(registry: &'a Registry, id: &str) -> Result<&'a Source, Error> {
    registry.sources.iter().find(|s| s.id == id).ok_or_else(|| Code::Usage.error(format!("no source `{id}`")))
}

fn parse_params(params: &[String]) -> Result<Vec<(String, String)>, Error> {
    params
        .iter()
        .map(|param| match param.split_once('=') {
            Some((name, value)) if !name.is_empty() && !value.is_empty() => Ok((name.into(), value.into())),
            _ => Err(Code::Usage.error(format!("`{param}` is not NAME=VALUE"))),
        })
        .collect()
}

/// A fetch that fails while the credential of its source is not on this machine is blocked. An
/// archive file with other bytes than its manifest is data to correct, not a download to retry.
fn fetched(source: &Source, result: Result<Snapshot, String>) -> Result<Snapshot, Error> {
    let blocked = source.credential.as_ref().is_some_and(|credential| !credential.present());
    result.map_err(|e| {
        if crate::fetch::archive::mismatch(&e) {
            let fix = "Replace the archive file, or correct its manifest, then run again.".into();
            return Error { fix, ..Code::InvalidData.error(e) };
        }
        if crate::prepared::absent_from_archive(&e) {
            let fix = "Set OBC_DATA_ARCHIVE to the roots of the archive and mount their drives, or run on the \
                       machine that has them.";
            return Error { fix: fix.into(), ..Code::Blocked.error(e) };
        }
        if blocked { Code::Blocked } else { Code::FetchFailed }.error(e)
    })
}

/// The requested files of a snapshot.
#[derive(Serialize, JsonSchema)]
struct Fetched<'a> {
    source: &'a str,
    version: &'a str,
    files: Vec<FetchedFile<'a>>,
}

#[derive(Serialize, JsonSchema)]
struct FetchedFile<'a> {
    #[serde(flatten)]
    file: &'a FileRecord,
    /// The object in the store.
    path: &'a Path,
}

fn print_snapshot(store: &Store, snapshot: &Snapshot, json: bool) -> Result<(), Error> {
    let paths: Vec<_> = snapshot.files.iter().map(|file| store.object(&file.sha256)).collect();
    if json {
        let files = snapshot.files.iter().zip(&paths).map(|(file, path)| FetchedFile { file, path }).collect();
        return print_json(&Fetched { source: &snapshot.source, version: &snapshot.version, files });
    }
    paths.iter().for_each(|path| println!("{}", path.display()));
    Ok(())
}

#[derive(Serialize, JsonSchema)]
struct Sources {
    sources: Vec<SourceRow>,
}

/// A source of `data/sources.toml`, what each target reads of it, and its versions in the store.
#[derive(Serialize, JsonSchema)]
struct SourceRow {
    #[serde(flatten)]
    source: Source,
    /// By target: its policy, and the state of each request that its serving release reads.
    targets: BTreeMap<String, SourceTarget>,
    /// The versions in the local store, the one fetched last first.
    snapshots: Vec<Stored>,
    /// For a source fetched with params: the newest record in the local store of each set of
    /// params, sorted by params.
    requests: Vec<StoredRequest>,
    credential_missing: bool,
    /// Whether `source refresh` moves the source.
    refreshable: bool,
    /// For a prepared source: its home in the archive, its complete versions there, and the run
    /// that holds its lock. `null` for another source.
    prepared: Option<PreparedIn>,
    /// How this machine holds the copy that a bake reads.
    held: holding::Held,
}

#[derive(Serialize, JsonSchema)]
struct PreparedIn {
    /// The archive root of the home; `null` when no root of `OBC_DATA_ARCHIVE` has one.
    root: Option<PathBuf>,
    versions: Vec<crate::prepared::Version>,
    lock: Option<overview::Lock>,
    /// Why the versions or the lock of the source cannot be read; `null` when they can.
    error: Option<String>,
}

/// The [`PreparedIn`] of the source `id` in the archive roots `roots`.
fn prepared_in(store: &Store, roots: &[PathBuf], id: &str) -> PreparedIn {
    let Some(root) = crate::prepared::home(roots, id) else {
        return PreparedIn { root: None, versions: Vec::new(), lock: None, error: None };
    };
    let folder = crate::prepared::folder(root, id);
    let read = || -> Result<_, String> {
        let lock = crate::lock::holder(&folder)?.map(|holder| overview::Lock::of(store, holder));
        Ok((crate::prepared::versions(&folder, id)?, lock))
    };
    let (versions, lock, error) = match read() {
        Ok((versions, lock)) => (versions, lock, None),
        Err(e) => (Vec::new(), None, Some(e)),
    };
    PreparedIn { root: Some(root.clone()), versions, lock, error }
}

#[derive(Serialize, JsonSchema)]
struct SourceTarget {
    refresh: Refresh,
    /// The least state of its requests; `unused` when the serving release reads none.
    state: State,
    requests: Vec<freshness::RequestStatus>,
}

/// A version of a source in the local store.
#[derive(Serialize, JsonSchema)]
struct Stored {
    version: String,
    /// The size of its files.
    bytes: u64,
}

/// A fetch of a source with params, as the local store records it.
#[derive(Serialize, JsonSchema)]
struct StoredRequest {
    params: Vec<(String, String)>,
    version: String,
    /// The size of its files.
    bytes: u64,
    /// When its newest file was retrieved, `YYYY-MM-DDTHH:MM:SSZ`; `null` for a fetch without
    /// files.
    fetched: Option<String>,
}

/// The newest record of each set of params of `source` whose version is in `snapshots`.
fn stored_requests(store: &Store, source: &str, snapshots: &[Snapshot]) -> Result<Vec<StoredRequest>, String> {
    let mut newest = BTreeMap::<Vec<(String, String)>, StoredRequest>::new();
    for record in store.requests_of(source)? {
        let Some(snapshot) = snapshots.iter().find(|snapshot| snapshot.version == record.version) else {
            continue;
        };
        let files: Vec<&FileRecord> = snapshot.files.iter().filter(|file| record.files.contains(&file.name)).collect();
        let request = StoredRequest {
            bytes: files.iter().map(|file| file.size).sum(),
            fetched: files.iter().map(|file| file.retrieved.clone()).max(),
            version: record.version,
            params: record.params,
        };
        let key = crate::store::sorted(&request.params);
        let newer = |kept: &StoredRequest| (&request.fetched, &request.version) > (&kept.fetched, &kept.version);
        if newest.get(&key).is_none_or(newer) {
            newest.insert(key, request);
        }
    }
    Ok(newest.into_values().collect())
}

/// The views of `obc data sources`.
enum View {
    All {
        check_now: bool,
    },
    /// `--layer`, with LAYER or for each.
    Layer(Option<String>),
    /// `--region` or `--box`.
    Region(Option<String>, Option<String>),
}

/// What the store holds of the source `id`.
fn in_store(store: &Store, id: &str) -> Result<holding::InStore, Error> {
    let mut snapshots = store.snapshots(id)?;
    let requests = stored_requests(store, id, &snapshots)?;
    let retrieved = snapshots.iter().flat_map(|s| s.files.iter().map(|f| f.retrieved.clone())).max();
    snapshots.sort_by_cached_key(|s| std::cmp::Reverse(s.files.iter().map(|f| f.retrieved.clone()).max()));
    let snapshots = snapshots
        .into_iter()
        .map(|s| Stored { bytes: s.files.iter().map(|f| f.size).sum(), version: s.version })
        .collect();
    Ok(holding::InStore { snapshots, requests, retrieved })
}

fn sources_command(
    root: &Path,
    (products, prepared): (&[&dyn Product], &[&dyn Prepared]),
    view: View,
    json: bool,
) -> Result<(), Error> {
    let (store, http) = (Store::open()?, Http::new());
    let registry = registry(root)?;
    let mut stored = BTreeMap::new();
    for source in &registry.sources {
        stored.insert(source.id.clone(), in_store(&store, &source.id)?);
    }
    // Only a region needs the index; the other views name areas by their paths without it.
    let index = match view {
        View::Region(..) => cached_index(&store)?,
        _ => cached_index(&store).ok().flatten(),
    };
    let index = index.map(|(_, index)| index);
    let mut holdings = holding::Holdings::read((root, http.roots()), &registry, prepared, stored, index)?;
    match view {
        View::Layer(layer) => {
            let layers = holding::layers(&holdings, &registry.sources, layer.as_deref())?;
            if json {
                return print_json(&layers);
            }
            holding::print_layers(&layers);
            Ok(())
        }
        View::Region(region, bbox) => {
            let region = holding::region(
                (&holdings, &store, http.roots()),
                (products, prepared),
                &registry.sources,
                (root, region.as_deref(), bbox.as_deref()),
            )?;
            if json {
                return print_json(&region);
            }
            holding::print_region(&region);
            Ok(())
        }
        View::All { check_now } => print_sources(root, (&store, &http), &mut holdings, registry, check_now, json),
    }
}

fn print_sources(
    root: &Path,
    (store, http): (&Store, &Http),
    holdings: &mut holding::Holdings,
    registry: Registry,
    check_now: bool,
    json: bool,
) -> Result<(), Error> {
    let mut sorted = registry.sources;
    sorted.sort_by_key(|source| source.kind);
    let served: Vec<(String, Release)> = releases(root, store)?
        .into_iter()
        .filter_map(|(name, releases)| match releases {
            Ok(mut releases) if !releases.is_empty() => Some((name, releases.swap_remove(0))),
            Ok(_) => None,
            Err(error) => {
                eprintln!("obc data: target `{name}` cannot be read: {}", error.message);
                None
            }
        })
        .collect();
    let all = crate::target::load(root, store).map_err(|e| Code::InvalidData.error(e))?;
    let mut rows = Vec::new();
    for source in sorted {
        let mut targets = BTreeMap::new();
        // A target without a release reads nothing yet, by the policies of `data/sources.toml`.
        for name in all.keys() {
            let release = served.iter().find(|(served, _)| served == name).map(|(_, release)| release);
            let settings = release.map(|release| release.settings.clone()).unwrap_or_default();
            let mut policy = vec![source.clone()];
            settings.policies(&mut policy).map_err(|e| Code::InvalidData.error(e))?;
            let versions = release.map(Release::versions).unwrap_or_default();
            let keys = versions.keys().filter(|(id, _)| *id == source.id).cloned().collect();
            let requests = freshness::requests(store, http, &policy[0], &keys, &versions, check_now);
            let state = requests.iter().map(|request| request.state).min().unwrap_or(State::Unused);
            targets.insert(name.clone(), SourceTarget { refresh: policy[0].refresh, state, requests });
        }
        let held = holdings.held(&source);
        let holding::InStore { snapshots, requests, .. } = holdings.stored.remove(&source.id).unwrap_or_default();
        let credential_missing = source.credential.as_ref().is_some_and(|credential| !credential.present());
        let prepared = match source.fetch.kind {
            crate::sources::FetchKind::Prepared => Some(prepared_in(store, http.roots(), &source.id)),
            _ => None,
        };
        let refreshable = source_cli::moves(&source);
        rows.push(SourceRow { source, targets, snapshots, requests, credential_missing, refreshable, prepared, held });
    }
    if json {
        return print_json(&Sources { sources: rows });
    }
    let names: Vec<&str> = all.keys().map(String::as_str).collect();
    let head = cells(["SOURCE", "LICENCE", "RULE", "COVERS", "STATUS", "WHERE", "SIZE", "DATE"]);
    let mut table = vec![[head, names.iter().map(|n| n.to_uppercase()).collect()].concat()];
    let mut tools = false;
    for row in &rows {
        let s = &row.source;
        if s.kind == Kind::Tool && !tools {
            tools = true;
            table.push(vec![String::new()]);
            table.push(vec!["tools".into()]);
        }
        let none = if s.kind == Kind::Tool { "—" } else { "not recorded" };
        let licence = s.licence.clone().unwrap_or_else(|| none.into());
        let states = names.iter().map(|name| row.targets[*name].state.to_string());
        table.push([vec![s.id.clone(), licence], holding::held_cells(&row.held), states.collect()].concat());
    }
    print_table(&table);
    Ok(())
}

/// What `areas` writes.
#[derive(Serialize, JsonSchema)]
struct Areas {
    /// The regions of `data/regions/` whose id or name holds the query.
    regions: Vec<Region>,
    /// The version of the cached Geofabrik index; `null` while the store has none.
    index: Option<String>,
    /// Its areas whose path or name holds the query.
    areas: Vec<GeofabrikArea>,
}

#[derive(Serialize, JsonSchema)]
struct GeofabrikArea {
    id: String,
    name: String,
    parent: Option<String>,
    countries: Vec<String>,
    bounds: Bbox,
}

fn print_areas(root: &Path, query: &str, json: bool) -> Result<(), Error> {
    let query = query.trim().to_lowercase();
    let holds = |id: &str, name: &str| id.contains(&query) || name.to_lowercase().contains(&query);
    let presets = Regions::load(root).map_err(|e| Code::InvalidData.error(e))?;
    let regions: Vec<Region> = presets.iter().filter(|region| holds(&region.id, &region.name)).cloned().collect();
    let (index, areas) = match cached_index(&Store::open()?)? {
        Some((version, index)) => {
            let areas = index.into_values().filter(|area| holds(&area.id, &area.name));
            let areas = areas.map(|area| GeofabrikArea {
                id: area.id,
                name: area.name,
                parent: area.parent,
                countries: area.countries,
                bounds: area.bounds,
            });
            (Some(version), areas.collect())
        }
        None => (None, Vec::new()),
    };
    let areas = Areas { regions, index, areas };
    if json {
        return print_json(&areas);
    }
    let mut rows = vec![cells(["REGION", "NAME", "DEFINITION"])];
    rows.extend(areas.regions.iter().map(|region| vec![region.id.clone(), region.name.clone(), definition(region)]));
    print_table(&rows);
    match &areas.index {
        None => println!("The store has no Geofabrik index: `obc data fetch geofabrik-index` gets it."),
        Some(_) => {
            let mut rows = vec![cells(["AREA", "NAME", "COUNTRIES"])];
            rows.extend(
                areas.areas.iter().map(|area| vec![area.id.clone(), area.name.clone(), area.countries.join(", ")]),
            );
            print_table(&rows);
        }
    }
    Ok(())
}

const INDEX_URL: &str = "https://download.geofabrik.de/index-v1.json";

/// The version of a Geofabrik index, and its areas by id.
type Index = (String, BTreeMap<String, geofabrik::Area>);

/// The newest Geofabrik index in the store, checked against its record.
fn cached_index(store: &Store) -> Result<Option<Index>, Error> {
    let mut records = store.snapshots("geofabrik-index")?;
    records.sort_by(|a, b| b.version.cmp(&a.version));
    let Some((version, file)) = records.into_iter().find_map(|snapshot| {
        snapshot.files.into_iter().find(|file| file.url == INDEX_URL).map(|file| (snapshot.version, file))
    }) else {
        return Ok(None);
    };
    let path = store.object(&file.sha256);
    if hash_file(&path)? != (file.sha256.clone(), file.size) {
        return Err(Code::InvalidData.error("the cached Geofabrik index does not match its snapshot"));
    }
    let body = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(Some((version, geofabrik::parse(&body).map_err(|e| Code::InvalidData.error(e))?)))
}

fn definition(region: &Region) -> String {
    match &region.area {
        Area::Geofabrik { areas } => format!("geofabrik: {}", areas.join(" + ")),
        Area::Box { bbox } => format!("box {},{} → {},{}", bbox.west, bbox.south, bbox.east, bbox.north),
        Area::Union { union } => format!("union: {}", union.join(" + ")),
    }
}

fn cells<const N: usize>(row: [&str; N]) -> Vec<String> {
    row.iter().map(|c| c.to_string()).collect()
}

fn print_table(rows: &[Vec<String>]) {
    print!("{}", table(rows));
}

/// The width of each column: its widest cell. A row of one cell is a heading and has no columns.
fn widths(rows: &[Vec<String>]) -> Vec<usize> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    (0..columns)
        .map(|i| {
            rows.iter().filter(|r| r.len() > 1).filter_map(|r| r.get(i)).map(|c| c.chars().count()).max().unwrap_or(0)
        })
        .collect()
}

fn table(rows: &[Vec<String>]) -> String {
    let widths = widths(rows);
    rows.iter().map(|row| row_text(row, &widths) + "\n").collect()
}

/// Each cell but the last padded to its column, two spaces apart.
fn row_text(row: &[String], widths: &[usize]) -> String {
    let cells: Vec<String> = row
        .iter()
        .enumerate()
        .map(|(i, cell)| if i + 1 == row.len() { cell.clone() } else { format!("{cell:<w$}", w = widths[i]) })
        .collect();
    cells.join("  ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    /// The functions of the Rust steps of the tests of the commands.
    pub(crate) const FUNCTIONS: [&crate::engine::Function; 5] = [
        &update_cli::tests::WAIT_FOR_GATE,
        &update_cli::tests::COPY_EARTH,
        &update_cli::tests::SPLIT_LINES,
        &update_cli::tests::COPY_LEAF,
        &prepare_cli::tests::COPY,
    ];

    #[test]
    fn a_source_lists_the_newest_fetch_of_each_area_with_its_version_size_and_day() {
        let dir = Scratch::new("stored-requests");
        let store = Store::at(&dir.0);
        let fetch = |version: &str, area: &str, size, retrieved: &str| {
            let name = format!("{area}.poly");
            let file = FileRecord {
                name: name.clone(),
                url: format!("https://download.geofabrik.de/{name}"),
                size,
                sha256: "aa".into(),
                retrieved: retrieved.into(),
                archive: None,
            };
            let mut snapshot = store.snapshot("poly", version).unwrap().unwrap_or(Snapshot {
                source: "poly".into(),
                version: version.into(),
                files: Vec::new(),
            });
            snapshot.files.push(file);
            store.put_snapshot(&snapshot).unwrap();
            let params = vec![("area".to_string(), area.to_string())];
            store
                .put_requested("poly", &crate::store::Requested { version: version.into(), params, files: vec![name] })
                .unwrap();
        };
        fetch("2026-10-01", "europe/germany/hessen", 300, "2026-10-01T08:00:00Z");
        fetch("2026-10-01", "europe/germany/bayern", 500, "2026-10-01T09:00:00Z");
        fetch("2026-10-15", "europe/germany/bayern", 700, "2026-10-15T09:00:00Z");
        let requests = stored_requests(&store, "poly", &store.snapshots("poly").unwrap()).unwrap();
        let listed: Vec<_> = requests
            .iter()
            .map(|r| (r.params[0].1.as_str(), r.version.as_str(), r.bytes, r.fetched.as_deref().unwrap()))
            .collect();
        assert_eq!(
            listed,
            [
                ("europe/germany/bayern", "2026-10-15", 700, "2026-10-15T09:00:00Z"),
                ("europe/germany/hessen", "2026-10-01", 300, "2026-10-01T08:00:00Z"),
            ]
        );
    }

    #[test]
    fn a_prepared_source_names_its_home_versions_and_lock_or_why_they_cannot_be_read() {
        let dir = Scratch::new("prepared-in");
        let (other, roots) = (dir.0.join("other"), vec![dir.0.join("empty"), dir.0.clone()]);
        let folder = dir.0.join("prepared/wiki");
        std::fs::create_dir_all(&folder).unwrap();
        let version = r#"[{"version": "2026-09-01", "manifest_sha256": "aa", "format": 1, "bytes": 5, "time": "2026-09-02T00:00:00Z"}]"#;
        std::fs::write(folder.join("versions.json"), version).unwrap();
        let lock = r#"{"state": "held", "machine": "bake", "run": "r1", "review": null, "phase": "build", "heartbeat": "2026-09-03T00:00:00Z"}"#;
        std::fs::write(folder.join("lock.json"), lock).unwrap();
        let store = Store::at(dir.0.join("store"));
        let found = prepared_in(&store, &roots, "wiki");
        let lock = found.lock.as_ref().unwrap();
        assert_eq!((found.versions[0].version.as_str(), lock.holder.machine.as_str()), ("2026-09-01", "bake"));
        assert_eq!(found.root.as_deref(), Some(dir.0.as_path()), "the first root that has the home");
        assert!(lock.stale);
        assert_eq!(prepared_in(&store, &[other], "wiki").root, None);

        std::fs::write(folder.join("versions.json"), "[").unwrap();
        let found = prepared_in(&store, &roots, "wiki");
        assert!(found.error.as_ref().unwrap().contains("versions.json"), "{:?}", found.error);
    }
}
