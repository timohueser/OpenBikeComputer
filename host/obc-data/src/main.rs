//! `obc data`: read the sources and the regions, and fetch sources into the store. Read commands
//! change nothing in `data/`.

use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;

use obc_data::fetch::http::Http;
use obc_data::fetch::upstream::{self, Upstream};
use obc_data::fetch::{self, Request};
use obc_data::regions::{Area, Bbox, Region, Regions};
use obc_data::sources::{self, Kind, Registry, Source, State, VersionScheme};
use obc_data::store::{self, FileRecord, Snapshot, Store};

#[derive(Parser)]
#[command(name = "obc data", about = "Data sources, regions and pins")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Every source with licence, R2 copy, live pin, newest upstream version, age, policy and state.
    Sources {
        #[arg(long)]
        json: bool,
    },
    /// Fetch a source version into the store, and print the store path of each file.
    Fetch {
        /// SOURCE or SOURCE@VERSION. Without a version: the live pin, or else upstream's newest file.
        target: String,
        /// NAME=VALUE for each `{name}` in the URL of the source, such as `tile=…` or `area=…`.
        params: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Fetch the newest upstream version of a source and pin it in an environment.
    Refresh {
        source: String,
        /// NAME=VALUE for each `{name}` in the URL of the source.
        params: Vec<String>,
        #[arg(long, default_value = "live")]
        env: String,
        #[arg(long)]
        json: bool,
    },
    /// The regions in data/regions/.
    Region {
        #[command(subcommand)]
        action: Option<RegionAction>,
        #[arg(long, global = true)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RegionAction {
    /// Every region with its kind and definition.
    List,
    /// One region, with the regions a union resolves to and its box.
    Show { id: String },
}

/// Why a command failed, and its exit status: 1 for a problem in the files, 2 for a usage error.
struct Failure {
    status: u8,
    message: String,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { status: 1, message }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

fn main() -> ExitCode {
    // clap exits with status 2 on a usage error.
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("obc data: {}", failure.message);
            ExitCode::from(failure.status)
        }
    }
}

fn run(cli: Cli) -> Result<(), Failure> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = obc_data::find_root(&cwd).ok_or("no data/sources.toml above the current directory")?;
    match cli.command {
        Command::Sources { json } => print_sources(&Registry::load(&root)?, json),
        Command::Fetch { target, params, json } => {
            let registry = Registry::load(&root)?;
            let (id, version) = match target.split_once('@') {
                Some((id, version)) => (id, Some(version.to_string())),
                None => (target.as_str(), None),
            };
            let source = find(&registry, id)?;
            let version = version.or_else(|| registry.pins.get(id).cloned());
            let store = Store::open()?;
            let request = Request { source, version, params: parse_params(&params)? };
            print_snapshot(&store, &fetch::fetch(&store, &Http::new(), &request)?, json)
        }
        Command::Refresh { source, params, env, json } => refresh(&root, &source, &params, &env, json),
        Command::Region { action, json } => {
            let regions = Regions::load(&root)?;
            match action {
                None | Some(RegionAction::List) => print_regions(&regions, json),
                Some(RegionAction::Show { id }) => print_region(&regions, &id, json),
            }
        }
    }
}

fn usage(message: &str) -> Failure {
    Failure { status: 2, message: message.into() }
}

fn find<'a>(registry: &'a Registry, id: &str) -> Result<&'a Source, Failure> {
    registry.sources.iter().find(|s| s.id == id).ok_or_else(|| usage(&format!("no source `{id}`")))
}

fn parse_params(params: &[String]) -> Result<Vec<(String, String)>, Failure> {
    params
        .iter()
        .map(|param| match param.split_once('=') {
            Some((name, value)) if !name.is_empty() && !value.is_empty() => Ok((name.into(), value.into())),
            _ => Err(usage(&format!("`{param}` is not NAME=VALUE"))),
        })
        .collect()
}

fn refresh(root: &Path, id: &str, params: &[String], env: &str, json: bool) -> Result<(), Failure> {
    let registry = Registry::load(root)?;
    let source = find(&registry, id)?;
    if !obc_data::is_kebab(env) {
        return Err(usage(&format!("`{env}` is not an environment name")));
    }
    let path = root.join("data/env").join(format!("{env}.toml"));
    let text = std::fs::read_to_string(&path).map_err(|e| usage(&format!("{}: {e}", path.display())))?;
    let (store, http) = (Store::open()?, Http::new());
    let version = upstream::newest(&store, &http, source, 0).version().map(str::to_string);
    let named = source.fetch.url.as_deref().is_some_and(|url| url.contains("{version}"));
    if version.is_none() && (named || matches!(source.version, VersionScheme::Release | VersionScheme::Commit)) {
        return Err(format!("the newest version of `{id}` is not known: fetch {id}@VERSION and pin it by hand").into());
    }
    let snapshot = fetch::fetch(&store, &http, &Request { source, version, params: parse_params(params)? })?;
    let text = sources::set_pin(&text, id, &snapshot.version);
    sources::parse_pins(&text, &registry.sources).map_err(|e| format!("{}: {e}", path.display()))?;
    store::write_atomic(&path, text.as_bytes())?;
    eprintln!("obc data: pinned {id} = \"{}\" in data/env/{env}.toml", snapshot.version);
    print_snapshot(&store, &snapshot, json)
}

fn print_snapshot(store: &Store, snapshot: &Snapshot, json: bool) -> Result<(), Failure> {
    let paths: Vec<_> = snapshot.files.iter().map(|file| store.object(&file.sha256)).collect();
    if json {
        #[derive(Serialize)]
        struct File<'a> {
            #[serde(flatten)]
            file: &'a FileRecord,
            path: &'a Path,
        }
        #[derive(Serialize)]
        struct Fetched<'a> {
            source: &'a str,
            version: &'a str,
            files: Vec<File<'a>>,
        }
        let files = snapshot.files.iter().zip(&paths).map(|(file, path)| File { file, path }).collect();
        return print_json(&Fetched { source: &snapshot.source, version: &snapshot.version, files });
    }
    paths.iter().for_each(|path| println!("{}", path.display()));
    Ok(())
}

#[derive(Serialize)]
struct SourceRow<'a> {
    #[serde(flatten)]
    source: &'a Source,
    pin: Option<&'a str>,
    upstream: Option<&'a str>,
    age_days: Option<i64>,
    state: State,
    reason: Option<String>,
}

fn print_sources(registry: &Registry, json: bool) -> Result<(), Failure> {
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
    let rows: Vec<SourceRow> = sorted
        .into_iter()
        .zip(&newest)
        .map(|(source, upstream)| {
            let pin = registry.pins.get(&source.id).map(String::as_str);
            let present = source.credential.as_ref().is_none_or(|c| c.present());
            let status = sources::status(source, pin, upstream, today, present);
            SourceRow {
                source,
                pin,
                upstream: upstream.version(),
                age_days: status.age_days,
                state: status.state,
                reason: status.reason,
            }
        })
        .collect();
    if json {
        #[derive(Serialize)]
        struct Listing<'a> {
            sources: &'a [SourceRow<'a>],
        }
        return print_json(&Listing { sources: &rows });
    }
    let mut table = vec![cells(["SOURCE", "LICENCE", "R2 COPY", "LIVE PIN", "UPSTREAM", "AGE", "POLICY", "STATE"])];
    let mut tools = false;
    for row in &rows {
        let s = row.source;
        if s.kind == Kind::Tool && !tools {
            tools = true;
            table.push(vec![String::new()]);
            table.push(vec!["tools".into()]);
        }
        let short = |version: Option<&str>| {
            version.map_or("—".into(), |version| match s.version {
                VersionScheme::Commit | VersionScheme::Digest => version.chars().take(12).collect(),
                _ => version.to_string(),
            })
        };
        let licence =
            s.licence.clone().unwrap_or_else(|| if s.kind == Kind::Tool { "—" } else { "not recorded" }.into());
        let state = match &row.reason {
            Some(reason) => format!("{}: {reason}", row.state),
            None => row.state.to_string(),
        };
        table.push(vec![
            s.id.clone(),
            licence,
            if s.r2_copy { "yes" } else { "no" }.into(),
            short(row.pin),
            short(row.upstream),
            row.age_days.map_or("—".into(), |age| format!("{age} d")),
            s.refresh.to_string(),
            state,
        ]);
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

fn print_regions(regions: &Regions, json: bool) -> Result<(), Failure> {
    if json {
        #[derive(Serialize)]
        struct Listing<'a> {
            regions: Vec<&'a Region>,
        }
        return print_json(&Listing { regions: regions.iter().collect() });
    }
    let mut table = vec![cells(["REGION", "NAME", "DEFINITION"])];
    table.extend(regions.iter().map(|r| vec![r.id.clone(), r.name.clone(), definition(r)]));
    print_table(&table);
    Ok(())
}

#[derive(Serialize)]
struct RegionDetail<'a> {
    #[serde(flatten)]
    region: &'a Region,
    leaves: Vec<&'a str>,
    bounds: Option<Bbox>,
}

fn print_region(regions: &Regions, id: &str, json: bool) -> Result<(), Failure> {
    let region = regions.get(id).ok_or_else(|| Failure { status: 2, message: format!("no region `{id}`") })?;
    let detail = RegionDetail { region, leaves: regions.leaves(id)?, bounds: regions.bounds(id) };
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

fn print_json(value: &impl Serialize) -> Result<(), Failure> {
    println!("{}", serde_json::to_string_pretty(value).map_err(|e| e.to_string())?);
    Ok(())
}

fn print_table(rows: &[Vec<String>]) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|i| {
            rows.iter().filter(|r| r.len() > 1).filter_map(|r| r.get(i)).map(|c| c.chars().count()).max().unwrap_or(0)
        })
        .collect();
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| if i + 1 == row.len() { cell.clone() } else { format!("{cell:<w$}", w = widths[i]) })
            .collect();
        println!("{}", line.join("  "));
    }
}
