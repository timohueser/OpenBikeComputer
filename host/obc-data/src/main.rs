//! `obc data`: read the sources and the regions, fetch sources into the store, and show runs. Read
//! commands change nothing in `data/`. Without a command, a terminal gets the TUI.

mod api;
mod r2_cli;
mod runs_cli;
mod tui;

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use api::{print_json, Code, Error};
use obc_data::fetch::http::Http;
use obc_data::fetch::upstream::{self, Upstream};
use obc_data::fetch::{self, osm, Request};
use obc_data::regions::{Area, Bbox, Region, Regions};
use obc_data::sources::{self, FetchKind, Kind, Registry, Source, State, VersionScheme};
use obc_data::store::{self, FileRecord, Snapshot, Store};

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
    Sources,
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
    /// The regions in data/regions/.
    Region {
        #[command(subcommand)]
        action: Option<RegionAction>,
    },
    /// The runs in the store, newest first; with RUN, its steps.
    Runs(runs_cli::Runs),
    /// Plumbing for scripts: list, read, upload and delete objects in an R2 bucket.
    R2(r2_cli::R2),
}

#[derive(Subcommand)]
enum RegionAction {
    /// Every region with its kind and definition.
    List,
    /// One region, with the regions a union resolves to and its box.
    Show { id: String },
}

fn main() -> ExitCode {
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
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => error.report(json),
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    let json = cli.json;
    let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let Some(command) = cli.command else {
        let root = root()?;
        return if terminal && !json { tui::run(&root) } else { print_sources(&registry(&root)?, json) };
    };
    match command {
        Command::Sources => print_sources(&registry(&root()?)?, json),
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
        Command::Region { action } => {
            let regions = Regions::load(&root()?).map_err(|e| Code::InvalidData.error(e))?;
            match action {
                None | Some(RegionAction::List) => print_regions(&regions, json),
                Some(RegionAction::Show { id }) => print_region(&regions, &id, json),
            }
        }
        Command::Runs(runs) => runs_cli::run(runs, json),
        Command::R2(r2) => r2_cli::run(r2, json),
    }
}

fn root() -> Result<std::path::PathBuf, Error> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    obc_data::find_root(&cwd).ok_or_else(|| Code::Usage.error("no data/sources.toml above the current directory"))
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
    if !obc_data::is_kebab(env) {
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
fn source_rows(registry: &Registry) -> Result<Vec<SourceRow>, Error> {
    let today = obc_data::date::today();
    let mut sorted: Vec<&Source> = registry.sources.iter().collect();
    sorted.sort_by_key(|source| source.kind);
    let (store, http) = (Store::open()?, Http::new());
    let newest: Vec<Upstream> = std::thread::scope(|scope| {
        let checks: Vec<_> = sorted
            .iter()
            .map(|source| scope.spawn(|| upstream::newest(&store, &http, source, upstream::CACHE)))
            .collect();
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

fn print_sources(registry: &Registry, json: bool) -> Result<(), Error> {
    let rows = source_rows(registry)?;
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
