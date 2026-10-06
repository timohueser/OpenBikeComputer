//! `obc data`: read the sources and the regions, fetch sources into the store, plan and build the
//! releases of the products, show what is live, and show runs. Read commands change nothing in
//! `data/`. Without a command, a terminal gets the TUI.

mod api;
mod apply_cli;
mod build_cli;
pub mod commit_cli;
mod edit_cli;
mod freshness;
mod r2_cli;
mod regions_cli;
mod runs_cli;
mod status_cli;
mod tui;

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use crate::fetch::http::Http;

use crate::fetch::Request;
use crate::live::{Live, Remote};
use crate::product::Product;
use crate::regions::{Area, Bbox, Region, Regions};
use crate::sources::{self, Kind, Refresh, Registry, Source, State, VersionScheme};
use crate::store::{self, gc, import, FileRecord, Snapshot, Store};
use api::{confirm, print_json, Code, Error};

#[derive(Parser)]
#[command(name = "obc data", about = "Data sources, regions, environments and releases")]
struct Cli {
    /// Write JSON to standard output, also when the command fails.
    #[arg(long, global = true)]
    json: bool,
    /// Without a command: the TUI in a terminal, else what `status` writes.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// What is live: the release of each product, the state of its layers, and what needs attention.
    Status(status_cli::StatusArgs),
    /// Every source with licence, R2 copy, live version, newest upstream version, age, policy and
    /// state.
    Sources {
        /// Check upstream now, not from a check of the last hour.
        #[arg(long)]
        check_now: bool,
    },
    /// Fetch a source version into the store, and print the store path of each file.
    Fetch {
        /// SOURCE or SOURCE@VERSION. Without a version: upstream's newest file.
        target: String,
        /// NAME=VALUE for each `{name}` in the URL of the source, such as `tile=…` or `area=…`.
        params: Vec<String>,
    },
    /// Set how old the live version of a source may get before it is stale: 7, 30, 90, 365 or
    /// manual. A manual source moves only with `--move`.
    Policy { source: String, refresh: Refresh },
    /// The regions in data/regions/. With ENV ID: set the region of data/env/ENV.toml.
    #[command(args_conflicts_with_subcommands = true)]
    Region {
        #[command(subcommand)]
        action: Option<regions_cli::Action>,
        /// The environment whose region to set.
        #[arg(requires = "id")]
        env: Option<String>,
        /// The region id.
        id: Option<String>,
    },
    /// Switch an optional layer of data/env/ENV.toml on or off.
    Layer { env: String, layer: String, switch: edit_cli::Switch },
    /// Restore data/env/ENV.toml to its committed version: the edits that are not applied go.
    Undo { env: String },
    /// What a build of the environment would fetch and build, in groups that are independent.
    Plan(build_cli::PlanArgs),
    /// Resolve the environment's inputs and return a plan for review. Nothing builds or uploads.
    Prepare(build_cli::PlanArgs),
    /// Fetch and build the environment into the store, and write the release of each product
    /// whose every layer is built. Nothing uploads.
    Build(build_cli::BuildArgs),
    /// Build the plan of live, upload what R2 lacks, switch the pointers, and remove from R2 what no
    /// live release uses. Asks once; without a terminal, `--yes` or `--plan` is required.
    Apply(apply_cli::ApplyArgs),
    /// The runs in the store, newest first; with RUN, its steps.
    Runs(runs_cli::Runs),
    /// Clean the local store: delete what no live release or fixture reaches, and move the
    /// cache directories of the older bake tools in. Shows the plan; `--apply` asks, then cleans.
    Clean {
        #[arg(long)]
        apply: bool,
        /// Do not ask. Required without a terminal.
        #[arg(long, requires = "apply")]
        yes: bool,
    },
    /// Plumbing for scripts: list, read, upload and delete objects in an R2 bucket.
    R2(r2_cli::R2),
}

/// Run `obc data` with the products whose steps this binary links.
pub fn main(products: &[&dyn Product]) -> ExitCode {
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
    match run(cli, products) {
        Ok(code) => code,
        Err(error) => error.report(json),
    }
}

/// Report a launcher or worker startup failure with the command's existing error format.
pub fn failed(message: String) -> ExitCode {
    Code::Failed.error(message).report(std::env::args_os().any(|arg| arg == "--json"))
}

fn run(cli: Cli, products: &[&dyn Product]) -> Result<ExitCode, Error> {
    crate::worker::check(&root()?)?;
    let json = cli.json;
    let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let command = match cli.command {
        None if terminal && !json => return tui::run(&root()?, products).map(|()| ExitCode::SUCCESS),
        None => return status_cli::status(&root()?, products, false, json),
        Some(command) => command,
    };
    let done = match command {
        Command::Status(args) => return status_cli::status(&root()?, products, args.check, json),
        Command::Sources { check_now } => print_sources(&root()?, products, check_now, json),
        Command::Fetch { target, params } => {
            let registry = registry(&root()?)?;
            let (id, version) = match target.split_once('@') {
                Some((id, version)) => (id, Some(version.to_string())),
                None => (target.as_str(), None),
            };
            let source = find(&registry, id)?;
            let store = Store::open()?;
            let request = Request { source, version, params: parse_params(&params)? };
            let missing = request
                .version
                .as_ref()
                .map(|version| crate::engine::snapshot_files(&store, id, version, &request.params, &[]))
                .transpose()?
                .flatten()
                .is_none();
            let remote = (request.version.is_some() && source.r2_copy && missing).then(remote).transpose()?;
            let live = remote
                .as_ref()
                .map(|remote| crate::live::Live::read(remote, products, &registry.sources, &store))
                .transpose()?;
            let copies =
                remote.as_ref().zip(live.as_ref()).map(|(remote, live)| crate::input_copy::Restore { remote, live });
            print_snapshot(
                &store,
                &fetched(source, crate::input_copy::fetch(&store, &Http::new(), copies.as_ref(), &request, &[]))?,
                json,
            )
        }
        Command::Policy { source, refresh } => {
            let source = policy(&root()?, &source, refresh)?;
            eprintln!("obc data: the policy of {} is {refresh} in data/sources.toml", source.id);
            if json {
                print_json(&source)?;
            }
            Ok(())
        }
        Command::Region { env: Some(env), id: Some(id), .. } => {
            edit_cli::print(edit_cli::region(&root()?, products, &env, &id)?, json)
        }
        Command::Region { action, .. } => regions_cli::run(&root()?, action, json),
        Command::Layer { env, layer, switch } => {
            edit_cli::print(edit_cli::layer(&root()?, products, &env, &layer, switch)?, json)
        }
        Command::Undo { env } => edit_cli::print(edit_cli::undo(&root()?, &env)?, json),
        Command::Plan(args) => build_cli::plan(&root()?, products, args, json),
        Command::Prepare(args) => build_cli::prepare(&root()?, products, args, json),
        Command::Build(args) => build_cli::build(&root()?, products, args, json),
        Command::Apply(args) => apply_cli::apply(&root()?, products, args, json),
        Command::Runs(runs) => runs_cli::run(runs, json),
        Command::Clean { apply, yes } => clean_command(&root()?, products, apply, yes, json),
        Command::R2(r2) => r2_cli::run(r2, json),
    };
    done.map(|()| ExitCode::SUCCESS)
}

/// What `clean` removes from the store and moves into it, or removed and moved.
#[derive(Debug, Default, Clone, Serialize, JsonSchema)]
struct CleanPlan {
    /// The snapshot records and the objects that nothing reaches, and what stays.
    store: gc::Plan,
    /// The cache directories of the older bake tools.
    import: import::Plan,
}

impl CleanPlan {
    fn is_empty(&self) -> bool {
        let moves = self.import.dirs.iter().any(|dir| dir.files > 0);
        self.store.snapshots.is_empty() && self.store.objects.is_empty() && !moves
    }

    /// The one question before a clean.
    fn question(&self) -> String {
        let removes = match (self.store.objects.len(), self.store.snapshots.len()) {
            (0, 0) => None,
            (0, 1) => Some("1 record".into()),
            (0, records) => Some(format!("{records} records")),
            _ => Some(bytes(self.store.remove_bytes)),
        };
        let moves = self.import.dirs.iter().any(|dir| dir.files > 0).then(|| bytes(self.import.bytes));
        // A process that writes an old cache while it moves can change an object.
        const STOP: &str = "Stop the bakes, the planner and every fetch first.";
        match (removes, moves) {
            (Some(removes), Some(moves)) => {
                format!("Remove {removes} from the local store and move {moves} of old caches into it? {STOP}")
            }
            (Some(removes), None) => format!("Remove {removes} from the local store?"),
            (None, Some(moves)) => format!("Move {moves} of old caches into the local store? {STOP}"),
            (None, None) => "Nothing to clean.".into(),
        }
    }
}

fn remote() -> Result<Remote, Error> {
    Remote::from_env().map_err(|e| Code::Blocked.error(e))
}

/// What is live now.
fn read_live(remote: &Remote, registry: &Registry, products: &[&dyn Product], store: &Store) -> Result<Live, Error> {
    Live::read(remote, products, &registry.sources, store).map_err(|e| Code::R2Failed.error(e))
}

/// The one warning when the LIVE column is unknown.
fn live_unknown(error: &Error) -> String {
    format!("live is unknown (`?`): {}", error.message)
}

/// The roots of a collection: the live releases, and the checkout at `root`.
fn roots(root: &Path, products: &[&dyn Product], store: &Store) -> Result<gc::Roots, Error> {
    let mut roots = gc::Roots::from_repo(root).map_err(|e| Code::InvalidData.error(e))?;
    roots.add_live(&read_live(&remote()?, &registry(root)?, products, store)?);
    Ok(roots)
}

fn old_dirs() -> Result<Vec<std::path::PathBuf>, Error> {
    let home = std::env::var_os("HOME").ok_or_else(|| Code::Usage.error("HOME is not set"))?;
    Ok(import::old_dirs(Path::new(&home)))
}

/// The plan of `clean`.
fn clean_plan(root: &Path, products: &[&dyn Product], store: &Store) -> Result<CleanPlan, Error> {
    let roots = roots(root, products, store)?;
    Ok(CleanPlan { store: gc::plan(store, &roots)?, import: import::plan(store, &old_dirs()?)? })
}

/// Delete what nothing reaches, only when that is still `confirmed`; then move the old cache
/// directories in.
fn clean(root: &Path, products: &[&dyn Product], store: &Store, confirmed: &gc::Plan) -> Result<CleanPlan, Error> {
    let roots = roots(root, products, store)?;
    let removed = gc::apply(store, &roots, confirmed)?.ok_or_else(|| {
        Code::Usage
            .error("a fetch, a build or an import uses the store; nothing was deleted")
            .fix("Run `obc data clean --apply` again when the fetch, the build or the import ends.")
    })?;
    Ok(CleanPlan { store: removed, import: import::apply(store, &old_dirs()?)? })
}

fn clean_command(root: &Path, products: &[&dyn Product], apply: bool, yes: bool, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let plan = clean_plan(root, products, &store)?;
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
    let done = clean(root, products, &store, &plan.store)?;
    if json {
        return print_json(&done);
    }
    let left: Vec<_> = done.import.dirs.iter().flat_map(|dir| &dir.left).collect();
    println!("Removed {} from the store; moved {} into it.", bytes(done.store.remove_bytes), bytes(done.import.bytes));
    if !left.is_empty() {
        println!("THESE STAY:");
        left.iter().for_each(|path| println!("  {}", path.display()));
    }
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
    text +=
        &format!("  {} that nothing reaches, {}\n", gc::objects_text(gc.objects.len() as u64), bytes(gc.remove_bytes));
    text += &format!("KEEP {}, {}\n", gc::objects_text(gc.keep_objects), bytes(gc.keep_bytes));
    let kept =
        gc.kept.iter().map(|kept| vec![format!("  {}", kept.entry), bytes(kept.bytes), kept.because.join(" · ")]);
    text += &table(&kept.collect::<Vec<_>>());
    let moved: Vec<_> = plan.import.dirs.iter().filter(|dir| dir.files > 0).collect();
    if !moved.is_empty() {
        text += &format!("MOVE INTO {}\n", store.root().display());
        let rows = moved
            .iter()
            .map(|dir| vec![format!("  {}", dir.dir.display()), format!("{} files", dir.files), bytes(dir.bytes)]);
        text += &table(&rows.collect::<Vec<_>>());
    }
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

/// A fetch that fails while the credential of its source is not on this machine is blocked.
fn fetched(source: &Source, result: Result<Snapshot, String>) -> Result<Snapshot, Error> {
    let blocked = source.credential.as_ref().is_some_and(|credential| !credential.present());
    result.map_err(|e| if blocked { Code::Blocked } else { Code::FetchFailed }.error(e))
}

/// Set the `refresh` of `id` in `data/sources.toml`.
fn policy(root: &Path, id: &str, refresh: Refresh) -> Result<Source, Error> {
    find(&registry(root)?, id)?;
    let path = root.join("data/sources.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = sources::set_refresh(&text, id, refresh).map_err(|e| Code::Usage.error(e))?;
    let edited = sources::parse_sources(&text)
        .map_err(|e| Code::Usage.error(e).fix(format!("Choose `manual`: `obc data policy {id} manual`.")))?;
    store::write_atomic(&path, text.as_bytes())?;
    Ok(edited.into_iter().find(|s| s.id == id).expect("the edit keeps the source"))
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
struct Sources<'a> {
    /// R2 could not be read: `live` of each source is `null`.
    live_unknown: bool,
    sources: &'a [SourceRow],
}

/// A source of `data/sources.toml` with its live version, its snapshots and its state.
#[derive(Clone, Serialize, JsonSchema)]
struct SourceRow {
    #[serde(flatten)]
    source: Source,
    /// The versions that the live releases read, in order; `null` when R2 could not be read.
    live: Option<Vec<String>>,
    /// The newest upstream version.
    upstream: Option<String>,
    age_days: Option<i64>,
    state: State,
    reason: Option<String>,
    /// The versions in the local store, the one fetched last first.
    snapshots: Vec<Stored>,
    requests: Vec<freshness::RequestStatus>,
    credential_missing: bool,
}

/// A version of a source in the local store.
#[derive(Clone, Serialize, JsonSchema)]
struct Stored {
    version: String,
    /// The size of its files.
    bytes: u64,
}

impl SourceRow {
    /// A version, short enough for a table.
    fn short(&self, version: Option<&str>) -> String {
        version.map_or("—".into(), |version| match self.source.version {
            VersionScheme::Commit | VersionScheme::Digest => version.chars().take(12).collect(),
            _ => version.to_string(),
        })
    }

    /// Source, licence, R2 copy, live version, age, policy and state.
    fn cells(&self) -> Vec<String> {
        let s = &self.source;
        let none = if s.kind == Kind::Tool { "—" } else { "not recorded" };
        vec![
            s.id.clone(),
            s.licence.clone().unwrap_or_else(|| none.into()),
            if s.r2_copy { "yes" } else { "no" }.into(),
            match &self.live {
                None => "?".into(),
                Some(live) if live.is_empty() => "—".into(),
                Some(live) => live.iter().map(|version| self.short(Some(version))).collect::<Vec<_>>().join(", "),
            },
            self.age_days.map_or("—".into(), |age| format!("{age} d")),
            s.refresh.to_string(),
            self.state.to_string(),
        ]
    }
}

/// Source summaries use active requests. The live column also retains held provenance.
fn source_rows(
    registry: &Registry,
    live: Option<&BTreeMap<String, Vec<String>>>,
    inventory: Option<&crate::env::Env>,
    check_now: bool,
) -> Result<Vec<SourceRow>, Error> {
    let mut sorted: Vec<&Source> = registry.sources.iter().collect();
    sorted.sort_by_key(|source| source.kind);
    let (store, http) = (Store::open()?, Http::new());
    sorted
        .into_iter()
        .map(|source| {
            let requests =
                inventory.map(|env| freshness::requests(&store, &http, source, env, check_now)).unwrap_or_default();
            let state = requests.iter().map(|r| r.state).min().unwrap_or_else(|| {
                if source.kind != Kind::Tool && source.licence.is_none() {
                    State::Blocked
                } else {
                    State::Ok
                }
            });
            let reason = requests
                .iter()
                .find(|r| r.state == state)
                .and_then(|r| r.reason.clone())
                .or_else(|| (state == State::Blocked && requests.is_empty()).then(|| "no licence recorded".into()));
            let age_days = requests.iter().filter_map(|r| r.age_days).max();
            let upstream = requests.iter().filter_map(|r| r.observation.result.version()).max().map(str::to_string);
            let mut snapshots = store.snapshots(&source.id)?;
            snapshots.sort_by_cached_key(|s| std::cmp::Reverse(s.files.iter().map(|f| f.retrieved.clone()).max()));
            let snapshots = snapshots
                .into_iter()
                .map(|s| Stored { bytes: s.files.iter().map(|f| f.size).sum(), version: s.version })
                .collect();
            Ok(SourceRow {
                source: source.clone(),
                live: live.map(|live| live.get(&source.id).cloned().unwrap_or_default()),
                upstream,
                age_days,
                state,
                reason,
                snapshots,
                requests,
                credential_missing: source.credential.as_ref().is_some_and(|credential| !credential.present()),
            })
        })
        .collect()
}

type SourceListing = (Vec<SourceRow>, Option<BTreeMap<String, Vec<String>>>);

fn source_listing(root: &Path, products: &[&dyn Product], check_now: bool) -> Result<SourceListing, Error> {
    let (store, mut loaded) = (Store::open()?, build_cli::load(root, "live")?);
    let registry = Registry { sources: loaded.sources.clone() };
    let remote = remote();
    let live = match &remote {
        Ok(remote) => read_live(remote, &registry, products, &store),
        Err(error) => Err(Code::Blocked.error(error.message.clone())),
    };
    let inventory = if let Ok(live) = &live {
        loaded.env.live = live.versions();
        loaded.env.retained = crate::input_copy::retained(live, &store)?;
        let copies = crate::input_copy::Restore { remote: remote.as_ref().unwrap(), live };
        Some(freshness::discover(
            root,
            products,
            &loaded.env,
            &loaded.regions,
            &store,
            &Http::new(),
            &loaded.sources,
            Some(&copies),
            None,
        )?)
    } else {
        eprintln!("obc data: {}", live_unknown(live.as_ref().unwrap_err()));
        None
    };
    let versions = live.as_ref().ok().map(Live::by_source);
    Ok((source_rows(&registry, versions.as_ref(), inventory.as_ref(), check_now)?, versions))
}

fn print_sources(root: &Path, products: &[&dyn Product], check_now: bool, json: bool) -> Result<(), Error> {
    let (rows, live) = source_listing(root, products, check_now)?;
    if json {
        return print_json(&Sources { live_unknown: live.is_none(), sources: &rows });
    }
    let mut table = vec![cells(["SOURCE", "LICENCE", "R2 COPY", "LIVE", "UPSTREAM", "AGE", "POLICY", "STATE"])];
    let mut tools = false;
    for row in &rows {
        let s = &row.source;
        if s.kind == Kind::Tool && !tools {
            tools = true;
            table.push(vec![String::new()]);
            table.push(vec!["tools".into()]);
        }
        let mut cells = row.cells();
        cells.insert(4, row.short(row.upstream.as_deref()));
        if let Some(reason) = &row.reason {
            cells[7] = format!("{}: {reason}", row.state);
        }
        table.push(cells);
    }
    print_table(&table);
    Ok(())
}

fn definition(region: &Region) -> String {
    match &region.area {
        Area::Geofabrik { areas } => format!("geofabrik: {}", areas.join(" + ")),
        Area::Box { bbox } => format!("box {},{} → {},{}", bbox.west, bbox.south, bbox.east, bbox.north),
        Area::Union { union } => format!("union: {}", union.join(" + ")),
    }
}

#[derive(Serialize, JsonSchema)]
struct RegionList<'a> {
    regions: Vec<&'a Region>,
}

fn print_regions(regions: &Regions, json: bool) -> Result<(), Error> {
    if json {
        return print_json(&RegionList { regions: regions.iter().collect() });
    }
    let mut table = vec![cells(["REGION", "NAME", "DEFINITION"])];
    table.extend(regions.iter().map(|r| vec![r.id.clone(), r.name.clone(), definition(r)]));
    print_table(&table);
    Ok(())
}

#[derive(Serialize, JsonSchema)]
struct RegionDetail<'a> {
    #[serde(flatten)]
    region: &'a Region,
    /// The region ids that it resolves to.
    leaves: Vec<&'a str>,
    /// Its box, when every part is a box.
    bounds: Option<Bbox>,
}

fn print_region(regions: &Regions, id: &str, json: bool) -> Result<(), Error> {
    let region = regions.get(id).ok_or_else(|| Code::Usage.error(format!("no region `{id}`")))?;
    let leaves = regions.leaves(id).map_err(|e| Code::InvalidData.error(e))?;
    let detail = RegionDetail { region, leaves, bounds: regions.bounds(id) };
    if json {
        return print_json(&detail);
    }
    let bounds = detail.bounds.map_or("— (from the outline once it is fetched)".into(), |b| {
        format!("{},{} → {},{}", b.west, b.south, b.east, b.north)
    });
    let mut table = vec![
        cells(["region", id]),
        vec!["name".into(), region.name.clone()],
        vec!["definition".into(), definition(region)],
        vec!["box".into(), bounds],
    ];
    if matches!(region.area, Area::Union { .. }) {
        table.push(vec!["resolves to".into(), detail.leaves.join(", ")]);
    }
    print_table(&table);
    Ok(())
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
mod tests {
    use super::*;
    use crate::engine::tests::write;
    use crate::store::tests::Scratch;

    const SOURCES: &str = r#"# The sources.
[[source]]
id = "osm-planet"
kind = "data"
licence = "ODbL-1.0"
fetch = { kind = "http", url = "https://planet.openstreetmap.org/pbf/planet-{yymmdd}.osm.pbf" }
version = "date"
# Seldom worth its download.
refresh = 90
redistribute = true

[[source]]
id = "planetiler"
kind = "tool"
fetch = { kind = "github", url = "https://api.github.com/repos/onthegomap/planetiler" }
version = "release"
refresh = "manual"
redistribute = true
"#;

    #[test]
    fn a_policy_edits_its_source_in_place() {
        let scratch = Scratch::new("cli-policy");
        let root = scratch.0.join("repository");
        write(&root.join("data/sources.toml"), SOURCES);
        let sources = || std::fs::read_to_string(root.join("data/sources.toml")).unwrap();
        let refused = policy(&root, "planetiler", Refresh::Days(30)).unwrap_err();
        assert!(refused.message.contains("needs `version = \"date\"`"), "{}", refused.message);
        assert_eq!(policy(&root, "land", Refresh::Manual).unwrap_err().message, "no source `land`");
        assert_eq!(sources(), SOURCES, "a refused policy changes nothing");

        assert_eq!(policy(&root, "osm-planet", Refresh::Manual).unwrap().refresh, Refresh::Manual);
        assert_eq!(sources(), SOURCES.replacen("refresh = 90", "refresh = \"manual\"", 1));
    }
}
