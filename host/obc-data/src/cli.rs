//! `obc data`: read the sources and the regions, fetch sources into the store, plan and build the
//! releases of the products, and show runs. Read commands change nothing in `data/`. Without a
//! command, a terminal gets the TUI.

mod api;
mod build_cli;
mod r2_cli;
mod runs_cli;
mod tui;

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use crate::fetch::http::Http;
use crate::fetch::upstream::{self, Upstream};
use crate::fetch::{self, osm, Request};
use crate::product::Product;
use crate::regions::{Area, Bbox, Region, Regions};
use crate::sources::{self, FetchKind, Kind, Refresh, Registry, Source, State, VersionScheme};
use crate::store::{self, gc, import, FileRecord, Snapshot, Store};
use api::{print_json, Code, Error};

#[derive(Parser)]
#[command(name = "obc data", about = "Data sources, regions and pins")]
struct Cli {
    /// Write JSON to standard output, also when the command fails.
    #[arg(long, global = true)]
    json: bool,
    /// Without a command: the TUI in a terminal, else what `sources` writes.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Every source with licence, R2 copy, live pin, newest upstream version, age, policy and state.
    Sources {
        /// Check upstream now, not from a check of the last hour.
        #[arg(long)]
        check_now: bool,
    },
    /// Fetch a source version into the store, and print the store path of each file.
    Fetch {
        /// SOURCE or SOURCE@VERSION. Without a version: the live pin, or else upstream's newest file.
        target: String,
        /// NAME=VALUE for each `{name}` in the URL of the source, such as `tile=…` or `area=…`.
        params: Vec<String>,
    },
    /// Fetch the newest upstream version of a source and pin it in an environment.
    Refresh {
        source: String,
        /// NAME=VALUE for each `{name}` in the URL of the source.
        params: Vec<String>,
        #[arg(long, default_value = "live")]
        env: String,
    },
    /// Set how old the pin of a source may get before it is stale: 7, 30, 90, 365 or manual.
    Policy { source: String, refresh: Refresh },
    /// The regions in data/regions/.
    Region {
        #[command(subcommand)]
        action: Option<RegionAction>,
    },
    /// What a build of the environment would fetch and build, in groups that are independent.
    Plan(build_cli::PlanArgs),
    /// Fetch and build the environment into the store, and write the release of each product
    /// whose every layer is built. Nothing uploads.
    Build(build_cli::BuildArgs),
    /// The runs in the store, newest first; with RUN, its steps.
    Runs(runs_cli::Runs),
    /// The local store.
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
    /// Delete what nothing uses.
    Gc {
        #[command(subcommand)]
        what: GcWhat,
    },
    /// Plumbing for scripts: list, read, upload and delete objects in an R2 bucket.
    R2(r2_cli::R2),
}

#[derive(Subcommand)]
enum StoreAction {
    /// Move the cache directories of the older bake tools into the store. Shows the plan; `--apply` moves.
    Import {
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand)]
enum GcWhat {
    /// The objects and snapshot records that no environment, pin or fixture reaches. Shows the plan; `--apply` deletes.
    Store {
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand)]
enum RegionAction {
    /// Every region with its kind and definition.
    List,
    /// One region, with the regions a union resolves to and its box.
    Show { id: String },
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
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => error.report(json),
    }
}

fn run(cli: Cli, products: &[&dyn Product]) -> Result<(), Error> {
    let json = cli.json;
    let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let Some(command) = cli.command else {
        let root = root()?;
        return if terminal && !json { tui::run(&root) } else { print_sources(&registry(&root)?, false, json) };
    };
    match command {
        Command::Sources { check_now } => print_sources(&registry(&root()?)?, check_now, json),
        Command::Fetch { target, params } => {
            let registry = registry(&root()?)?;
            let (id, version) = match target.split_once('@') {
                Some((id, version)) => (id, Some(version.to_string())),
                None => (target.as_str(), None),
            };
            let source = find(&registry, id)?;
            let version = version.or_else(|| registry.pins.get(id).cloned());
            let store = Store::open()?;
            let params = osm::with_base(source, &registry.pins, parse_params(&params)?).map_err(not_the_base)?;
            let request = Request { source, version, params };
            print_snapshot(&store, &fetched(source, fetch::fetch(&store, &Http::new(), &request))?, json)
        }
        Command::Refresh { source, params, env } => refresh(&root()?, &source, &params, &env, json),
        Command::Policy { source, refresh } => {
            let source = policy(&root()?, &source, refresh)?;
            eprintln!("obc data: the policy of {} is {refresh} in data/sources.toml", source.id);
            if json {
                print_json(&source)?;
            }
            Ok(())
        }
        Command::Region { action } => {
            let regions = Regions::load(&root()?).map_err(|e| Code::InvalidData.error(e))?;
            match action {
                None | Some(RegionAction::List) => print_regions(&regions, json),
                Some(RegionAction::Show { id }) => print_region(&regions, &id, json),
            }
        }
        Command::Plan(args) => build_cli::plan(&root()?, products, args, json),
        Command::Build(args) => build_cli::build(&root()?, products, args, json),
        Command::Runs(runs) => runs_cli::run(runs, json),
        Command::Store { action: StoreAction::Import { apply } } => store_import(apply, json),
        Command::Gc { what: GcWhat::Store { apply } } => gc_store(&root()?, apply, json),
        Command::R2(r2) => r2_cli::run(r2, json),
    }
}

fn store_import(apply: bool, json: bool) -> Result<(), Error> {
    let home = std::env::var_os("HOME").ok_or_else(|| Code::Usage.error("HOME is not set"))?;
    let (store, dirs) = (Store::open()?, import::old_dirs(Path::new(&home)));
    let plan = if apply { import::apply(&store, &dirs)? } else { import::plan(&store, &dirs)? };
    if json {
        return print_json(&plan);
    }
    let home = std::fs::canonicalize(&home).unwrap_or_else(|_| home.into());
    let short =
        |dir: &Path| dir.strip_prefix(&home).map_or(dir.display().to_string(), |dir| format!("~/{}", dir.display()));
    println!("{} INTO {}", if apply { "MOVED" } else { "MOVE" }, store.root().display());
    let mut table = Vec::new();
    for dir in &plan.dirs {
        let size = if dir.present { format!("{} files", dir.files) } else { "not present".into() };
        table.push(vec![format!("  {}", short(&dir.dir)), size, bytes(dir.bytes)]);
    }
    print_table(&table);
    println!("{} in; duplicates are kept once; the store grows by {}.", bytes(plan.bytes), bytes(plan.new_bytes));
    let left: Vec<_> = plan.dirs.iter().flat_map(|dir| &dir.left).collect();
    if !left.is_empty() {
        println!("{}", if apply { "THESE STAY:" } else { "THESE STAY AFTER --apply:" });
        left.iter().for_each(|path| println!("  {}", short(path)));
    }
    if !apply {
        println!(
            "`--apply` moves the files and deletes the directories that are then empty. Stop the bakes, the planner"
        );
        println!("and every fetch first. The older bake tools then fetch and build again.");
    }
    Ok(())
}

/// The plan of `gc store`.
fn collect(root: &Path, store: &Store) -> Result<gc::Plan, Error> {
    let roots = gc::Roots::from_repo(root).map_err(|e| Code::InvalidData.error(e))?;
    Ok(gc::plan(store, &roots)?)
}

/// `gc store --apply`: what it deleted. With `confirmed`, only when that is still the plan.
fn clean(root: &Path, store: &Store, confirmed: Option<&gc::Plan>) -> Result<gc::Plan, Error> {
    let roots = gc::Roots::from_repo(root).map_err(|e| Code::InvalidData.error(e))?;
    gc::apply(store, &roots, confirmed)?.ok_or_else(|| {
        Code::Usage
            .error("a fetch, a build or an import uses the store; nothing was deleted")
            .fix("Run `obc data gc store --apply` again when the fetch, the build or the import ends.")
    })
}

fn gc_store(root: &Path, apply: bool, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let plan = if apply { clean(root, &store, None)? } else { collect(root, &store)? };
    if json {
        return print_json(&plan);
    }
    println!(
        "Roots: the pins of {}/data/env/*.toml, its fixtures and planner recipes, the import records, and the newest record of each source and request.",
        root.display()
    );
    println!("{} {}", if apply { "REMOVED FROM" } else { "REMOVE FROM" }, store.root().display());
    plan.snapshots.iter().for_each(|snapshot| println!("  snapshot {snapshot}"));
    plan.objects.iter().for_each(|(sha256, size)| println!("  object {sha256}  {}", bytes(*size)));
    println!("  {} objects that nothing reaches, {}", plan.objects.len(), bytes(plan.remove_bytes));
    println!("KEEP {} objects, {}", plan.keep_objects, bytes(plan.keep_bytes));
    let kept =
        plan.kept.iter().map(|kept| vec![format!("  {}", kept.entry), bytes(kept.bytes), kept.because.join(" · ")]);
    print_table(&kept.collect::<Vec<_>>());
    if !apply {
        println!("`--apply` deletes them. It refuses to start while a fetch, a build or an import runs.");
    }
    Ok(())
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

/// `osm::with_base` refuses a `from=` that is not the pin of the base source.
fn not_the_base(message: String) -> Error {
    Code::Usage.error(message).fix("Leave out `from=`: the fetch takes the pin of the base source.")
}

/// A fetch that fails while the credential of its source is not on this machine is blocked.
fn fetched(source: &Source, result: Result<Snapshot, String>) -> Result<Snapshot, Error> {
    let blocked = source.credential.as_ref().is_some_and(|credential| !credential.present());
    result.map_err(|e| if blocked { Code::Blocked } else { Code::FetchFailed }.error(e))
}

fn refresh(root: &Path, id: &str, params: &[String], env: &str, json: bool) -> Result<(), Error> {
    let registry = registry(root)?;
    let source = find(&registry, id)?;
    if !crate::is_kebab(env) {
        return Err(Code::Usage.error(format!("`{env}` is not an environment name")));
    }
    let path = root.join("data/env").join(format!("{env}.toml"));
    let text = std::fs::read_to_string(&path).map_err(|e| Code::Usage.error(format!("{}: {e}", path.display())))?;
    let invalid = |e| Code::InvalidData.error(format!("{}: {e}", path.display()));
    let pins = sources::parse_pins(&text, &registry.sources).map_err(invalid)?;
    let params = osm::with_base(source, &pins, parse_params(params)?).map_err(not_the_base)?;
    let (store, http) = (Store::open()?, Http::new());
    let version = upstream::newest(&store, &http, source, 0).version().map(str::to_string);
    // The `geofabrik` fetcher finds the newest day of a URL with `{yymmdd}` itself.
    let named = source.fetch.url.as_deref().is_some_and(|url| {
        url.contains("{version}") || (source.fetch.kind == FetchKind::Http && url.contains("{yymmdd}"))
    });
    if version.is_none() && (named || matches!(source.version, VersionScheme::Release | VersionScheme::Commit)) {
        let message = format!("the newest version of `{id}` is not known: fetch {id}@VERSION and pin it by hand");
        return Err(Code::FetchFailed.error(message));
    }
    // The diffs of a source that starts at this pin end at its own pin, so they cannot start after it.
    if let Some(version) = &version {
        let starts_here = registry.sources.iter().filter(|s| s.fetch.from.as_deref() == Some(id));
        let mut pinned = starts_here.filter_map(|s| Some((&s.id, pins.get(&s.id)?)));
        if let Some((diffs, pin)) = pinned.find(|(_, pin)| version > *pin) {
            let message = format!("`{id}` {version} is after the `{diffs}` pin {pin}");
            return Err(Code::Usage.error(message).fix(format!("Refresh `{diffs}` first.")));
        }
    }
    let snapshot = fetched(source, fetch::fetch(&store, &http, &Request { source, version, params }))?;
    let text = sources::set_pin(&text, id, &snapshot.version);
    sources::parse_pins(&text, &registry.sources).map_err(invalid)?;
    store::write_atomic(&path, text.as_bytes())?;
    eprintln!("obc data: pinned {id} = \"{}\" in data/env/{env}.toml", snapshot.version);
    print_snapshot(&store, &snapshot, json)
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
    sources: &'a [SourceRow],
}

/// A source of `data/sources.toml` with its live pin, its snapshots and its state.
#[derive(Clone, Serialize, JsonSchema)]
struct SourceRow {
    #[serde(flatten)]
    source: Source,
    pin: Option<String>,
    /// The newest upstream version.
    upstream: Option<String>,
    age_days: Option<i64>,
    state: State,
    reason: Option<String>,
    /// The versions in the local store, the one fetched last first.
    snapshots: Vec<Stored>,
}

/// A version of a source in the local store.
#[derive(Clone, Serialize, JsonSchema)]
struct Stored {
    version: String,
    /// The size of its files.
    bytes: u64,
}

impl SourceRow {
    /// The pin or another version, short enough for a table.
    fn short(&self, version: Option<&str>) -> String {
        version.map_or("—".into(), |version| match self.source.version {
            VersionScheme::Commit | VersionScheme::Digest => version.chars().take(12).collect(),
            _ => version.to_string(),
        })
    }

    /// Source, licence, R2 copy, live pin, age, policy and state.
    fn cells(&self) -> Vec<String> {
        let s = &self.source;
        let none = if s.kind == Kind::Tool { "—" } else { "not recorded" };
        vec![
            s.id.clone(),
            s.licence.clone().unwrap_or_else(|| none.into()),
            if s.r2_copy { "yes" } else { "no" }.into(),
            self.short(self.pin.as_deref()),
            self.age_days.map_or("—".into(), |age| format!("{age} d")),
            s.refresh.to_string(),
            self.state.to_string(),
        ]
    }
}

/// Every source in kind order, with its state from the newest upstream version.
fn source_rows(registry: &Registry, check_now: bool) -> Result<Vec<SourceRow>, Error> {
    let max_age = if check_now { 0 } else { upstream::CACHE };
    let today = crate::date::today();
    let mut sorted: Vec<&Source> = registry.sources.iter().collect();
    sorted.sort_by_key(|source| source.kind);
    let (store, http) = (Store::open()?, Http::new());
    let newest: Vec<Upstream> = std::thread::scope(|scope| {
        let checks: Vec<_> =
            sorted.iter().map(|source| scope.spawn(|| upstream::newest(&store, &http, source, max_age))).collect();
        checks.into_iter().map(|check| check.join().unwrap_or(Upstream::Failed("panicked".into()))).collect()
    });
    sorted
        .into_iter()
        .zip(&newest)
        .map(|(source, upstream)| {
            let pin = registry.pins.get(&source.id).map(String::as_str);
            let base = source.fetch.from.as_ref().and_then(|from| registry.pins.get(from)).map(String::as_str);
            let present = source.credential.as_ref().is_none_or(|c| c.present());
            let status = sources::status(source, pin, base, upstream, today, present);
            let mut snapshots = store.snapshots(&source.id)?;
            // `retrieved` is `YYYY-MM-DDTHH:MM:SSZ`, so it sorts as text.
            snapshots.sort_by_cached_key(|s| std::cmp::Reverse(s.files.iter().map(|f| f.retrieved.clone()).max()));
            let snapshots = snapshots
                .into_iter()
                .map(|s| Stored { bytes: s.files.iter().map(|f| f.size).sum(), version: s.version })
                .collect();
            Ok(SourceRow {
                source: source.clone(),
                pin: pin.map(str::to_string),
                upstream: upstream.version().map(str::to_string),
                age_days: status.age_days,
                state: status.state,
                reason: status.reason,
                snapshots,
            })
        })
        .collect()
}

fn print_sources(registry: &Registry, check_now: bool, json: bool) -> Result<(), Error> {
    let rows = source_rows(registry, check_now)?;
    if json {
        return print_json(&Sources { sources: &rows });
    }
    let mut table = vec![cells(["SOURCE", "LICENCE", "R2 COPY", "LIVE PIN", "UPSTREAM", "AGE", "POLICY", "STATE"])];
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
        Area::Geofabrik => "geofabrik".into(),
        Area::Box { bbox } => format!("box {},{} → {},{}", bbox.west, bbox.south, bbox.east, bbox.north),
        Area::Polygon { polygon } => format!("polygon {polygon}"),
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
        write(&root.join("data/env/live.toml"), "[pins]\n");
        let sources = || std::fs::read_to_string(root.join("data/sources.toml")).unwrap();
        let refused = policy(&root, "planetiler", Refresh::Days(30)).unwrap_err();
        assert!(refused.message.contains("needs `version = \"date\"`"), "{}", refused.message);
        assert_eq!(policy(&root, "land", Refresh::Manual).unwrap_err().message, "no source `land`");
        assert_eq!(sources(), SOURCES, "a refused policy changes nothing");

        assert_eq!(policy(&root, "osm-planet", Refresh::Manual).unwrap().refresh, Refresh::Manual);
        assert_eq!(sources(), SOURCES.replacen("refresh = 90", "refresh = \"manual\"", 1));
    }
}
