//! `obc data`: read the sources and the regions. Read commands change nothing.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;

use obc_data::regions::{Area, Bbox, Region, Regions};
use obc_data::sources::{self, Kind, Registry, Source, State, VersionScheme};

#[derive(Parser)]
#[command(name = "obc data", about = "Data sources, regions and pins")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Every source with licence, R2 copy, live pin, age, policy and state.
    Sources {
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

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("obc data: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = obc_data::find_root(&cwd).ok_or("no data/sources.toml above the current directory")?;
    match cli.command {
        Command::Sources { json } => print_sources(&Registry::load(&root)?, json),
        Command::Region { action, json } => {
            let regions = Regions::load(&root)?;
            match action {
                None | Some(RegionAction::List) => print_regions(&regions, json),
                Some(RegionAction::Show { id }) => print_region(&regions, &id, json),
            }
        }
    }
}

#[derive(Serialize)]
struct SourceRow<'a> {
    #[serde(flatten)]
    source: &'a Source,
    pin: Option<&'a str>,
    age_days: Option<i64>,
    state: State,
    reason: Option<String>,
}

fn print_sources(registry: &Registry, json: bool) -> Result<(), String> {
    let today = obc_data::date::today();
    let rows: Vec<SourceRow> = registry
        .sources
        .iter()
        .map(|source| {
            let pin = registry.pins.get(&source.id).map(String::as_str);
            let present = source.credential.as_ref().is_none_or(|c| c.present());
            let status = sources::status(source, pin, today, present);
            SourceRow { source, pin, age_days: status.age_days, state: status.state, reason: status.reason }
        })
        .collect();
    if json {
        #[derive(Serialize)]
        struct Listing<'a> {
            sources: &'a [SourceRow<'a>],
        }
        return print_json(&Listing { sources: &rows });
    }
    let mut table = vec![cells(["SOURCE", "LICENCE", "R2 COPY", "LIVE PIN", "AGE", "POLICY", "STATE"])];
    let mut tools = false;
    for row in &rows {
        let s = row.source;
        if s.kind == Kind::Tool && !tools {
            tools = true;
            table.push(vec![String::new()]);
            table.push(vec!["tools".into()]);
        }
        let pin = row.pin.map_or("—".into(), |pin| match s.version {
            VersionScheme::Commit | VersionScheme::Digest => pin.chars().take(12).collect(),
            _ => pin.to_string(),
        });
        let licence =
            s.licence.clone().unwrap_or_else(|| if s.kind == Kind::Tool { "—" } else { "not recorded" }.into());
        let state = match &row.reason {
            Some(reason) => format!("{}: {reason}", label(row.state)),
            None => label(row.state).into(),
        };
        table.push(vec![
            s.id.clone(),
            licence,
            if s.r2_copy { "yes" } else { "no" }.into(),
            pin,
            row.age_days.map_or("—".into(), |age| format!("{age} d")),
            s.refresh.to_string(),
            state,
        ]);
    }
    print_table(&table);
    Ok(())
}

fn label(state: State) -> &'static str {
    match state {
        State::Ok => "ok",
        State::Stale => "stale",
        State::Blocked => "blocked",
    }
}

fn definition(region: &Region) -> String {
    match &region.area {
        Area::Geofabrik => "geofabrik".into(),
        Area::Box { bbox } => format!("box {},{} → {},{}", bbox.west, bbox.south, bbox.east, bbox.north),
        Area::Polygon { polygon } => format!("polygon {polygon}"),
        Area::Union { union } => format!("union: {}", union.join(" + ")),
    }
}

fn print_regions(regions: &Regions, json: bool) -> Result<(), String> {
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

fn print_region(regions: &Regions, id: &str, json: bool) -> Result<(), String> {
    let region = regions.get(id).ok_or_else(|| format!("no region `{id}`"))?;
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

fn print_json(value: &impl Serialize) -> Result<(), String> {
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
