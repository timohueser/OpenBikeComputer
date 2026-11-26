//! The route bench: the route-quality measures of a region with the cost code of this checkout,
//! and the pairs of routes for a blind comparison of two checkouts.
mod measures;
mod net;
mod pairs;
mod panel;
mod prices;
mod signed;
#[cfg(test)]
mod tests;

use crate::base::Topology;
use crate::Graph;
use net::Net;
use obc_dem::planner::{Recent, Terrain};
use panel::{Panel, Spec};
use planner_router::model::Point;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

const USAGE: &str = "Usage: route_bench run --pbf FILE --dem DIR --bounds W,S,E,N --out DIR [--town W,S,E,N] \
                     [--profiles NAME,…] [--pairs FILE [--pairs-box W,S,E,N]]\n       \
                     route_bench compare A/routes.json B/routes.json --names X,Y --out FILE.html";
const OPTIONS: [&str; 9] = ["pbf", "dem", "bounds", "out", "town", "profiles", "pairs", "pairs-box", "names"];
/// The distance between the points on a signed route.
const STEP_M: f64 = 5_000.0;

/// The cells of one table row of `bench.md` with their column names: one object of `bench.json`.
type Row = Vec<(String, Value)>;

fn round(value: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (value * scale).round() / scale
}

fn row<const N: usize>(cells: [(&str, Value); N]) -> Row {
    cells.into_iter().map(|(name, value)| (name.to_string(), value)).collect()
}

fn objects(rows: &[Row]) -> Value {
    rows.iter().map(|row| Value::Object(row.iter().cloned().collect())).collect()
}

fn text(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_string)
}

fn table(rows: &[Row]) -> String {
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    let names: Vec<String> = rows[0].iter().map(|(name, _)| name.clone()).collect();
    let rule = line(vec!["---".to_string(); names.len()]);
    let body: String = rows.iter().map(|row| line(row.iter().map(|(_, value)| text(value)).collect())).collect();
    format!("{}{rule}{body}\n", line(names))
}

/// `rows` with a row for each column after the first, and a column for each row.
fn transposed(rows: &[Row]) -> Vec<Row> {
    let column = |k: usize| {
        let cells = rows.iter().map(|row| (text(&row[0].1), row[k].1.clone()));
        row([(rows[0][0].0.as_str(), json!(rows[0][k].0))]).into_iter().chain(cells).collect()
    };
    (1..rows[0].len()).map(column).collect()
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}

/// West, south, east and north in degrees.
fn area(text: &str) -> Result<[f64; 4], String> {
    let values: Option<Vec<f64>> = text.split(',').map(|value| value.parse().ok()).collect();
    values.and_then(|values| values.try_into().ok()).ok_or(format!("{text}: not W,S,E,N"))
}

/// The roads of the extract `pbf` inside `bounds`, with the heights of the GLO-30 tiles in `dem`.
fn import(pbf: &Path, dem: &Path, bounds: [f64; 4]) -> Result<Graph, String> {
    let tiles = obc_dem::planner::tiles_in(dem, bounds);
    if tiles.is_empty() {
        return Err(format!("{}: no GLO-30 tile of the bounds", dem.display()));
    }
    let terrain = Terrain::open(&tiles, "", None, bounds)?;
    let mut recent = Recent::default();
    let mut height = |point: Point| terrain.height(point.lat, point.lon, &mut recent);
    crate::osm::import(&[pbf.to_path_buf()], bounds, Some(&mut height))
}

fn signed_rows(signed: &signed::Signed, walking: bool) -> [Vec<Row>; 2] {
    let levels = signed::levels(walking).into_iter().zip(signed.levels).chain([("all", signed.total)]);
    let shares = levels.filter(|(level, share)| share.relations > 0 || *level == "all").map(|(level, share)| {
        let percent = |metres: f64| json!(round(100.0 * metres / share.measured_m.max(1.0), 1));
        row([
            ("level", json!(level)),
            ("relations", json!(share.relations)),
            ("measured km", json!(round(share.measured_m / 1000.0, 1))),
            ("near %", percent(share.near_m)),
            ("same road %", percent(share.same_road_m)),
        ])
    });
    let legs = row([
        ("legs", json!(signed.legs)),
        ("legs with no split", json!(signed.legs_with_no_split)),
        ("splits", json!(signed.splits)),
        ("splits with a signed stretch that is not longer", json!(signed.not_longer)),
    ]);
    [shares.collect(), vec![legs]]
}

fn panel_row(name: &str, panel: &Panel) -> Row {
    let mut cells = row([
        ("panel", json!(name)),
        ("routes", json!(panel.routes)),
        ("km", json!(round(panel.total.length_m / 1000.0, 1))),
        ("length / shortest, p50", json!(round(panel.detour[0], 3))),
        ("length / shortest, p95", json!(round(panel.detour[1], 3))),
        ("length / shortest, max", json!(round(panel.detour[2], 3))),
    ]);
    cells.extend(panel.total.rates().into_iter().map(|(name, rate)| (name.to_string(), json!(round(rate, 2)))));
    cells
}

fn run(options: &HashMap<&str, &str>) -> Result<(), String> {
    let need = |name: &str| options.get(name).copied().ok_or(USAGE);
    let (pbf, dem, out) = (Path::new(need("pbf")?), Path::new(need("dem")?), Path::new(need("out")?));
    let bounds = area(need("bounds")?)?;
    let town = options.get("town").map(|text| area(text)).transpose()?;
    let pairs_box = options.get("pairs-box").map_or(Ok(bounds), |text| area(text))?;
    let names: Vec<&str> = options.get("profiles").map_or(vec!["touring"], |names| names.split(',').collect());
    let profiles = crate::profiles(&names)?;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;

    let clock = Instant::now();
    let graph = import(pbf, dem, bounds)?;
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    crate::check(&graph, &profiles)?;
    let costs = crate::costs(&graph, &topology, &profiles)?;
    eprintln!("Import and costs: {:.0} s", clock.elapsed().as_secs_f64());

    let prices: Vec<Row> = prices::price_list(&profiles)
        .into_iter()
        .map(|price| {
            let factor = |factor: Option<f64>| factor.map_or(json!("-"), |factor| json!(round(factor, 3)));
            let factors = price.factors.into_iter().map(|(column, value)| (column, factor(value)));
            row([("tags", json!(price.tags))]).into_iter().chain(factors).collect()
        })
        .collect();
    let mut text = format!(
        "# Route bench\n\n- OSM: `{}`, bounds `{}`\n- Terrain: `{}`\n\n",
        pbf.display(),
        need("bounds")?,
        dem.display()
    );
    let (mut results, mut routes, mut ends) = (serde_json::Map::new(), serde_json::Map::new(), None);
    for (index, profile) in profiles.iter().enumerate() {
        let clock = Instant::now();
        let net = Net::new(&graph, &topology, &costs, index);
        let [shares, legs] = signed_rows(&signed::signed_share(&net, STEP_M)?, profile.walking);
        let draw = |classes, inside| panel::panel(&net, &panel::sample(&net, &Spec { classes, inside })?);
        let mut panels = vec![panel_row("random", &draw(panel::PANEL, None)?)];
        if town.is_some() {
            panels.push(panel_row("town", &draw(panel::TOWN, town)?));
        }
        if let Some(path) = options.get("pairs") {
            if ends.is_none() {
                ends = Some(pairs::pairs(Path::new(path), &net, pairs_box)?);
            }
            routes.insert(profile.name.clone(), pairs::routes(&net, ends.as_deref().expect("the pairs"))?);
        }
        text += &format!("## `{}`\n\nThe signed routes:\n\n{}{}", profile.name, table(&shares), table(&legs));
        text += &format!("The panels:\n\n{}", table(&transposed(&panels)));
        let result = json!({"signed": objects(&shares), "legs": objects(&legs)[0], "panels": objects(&panels)});
        results.insert(profile.name.clone(), result);
        eprintln!("Profile {}: {:.0} s", profile.name, clock.elapsed().as_secs_f64());
    }
    text += &format!("## Price list\n\nThe cost of 1 m of flat road:\n\n{}", table(&prices));
    let inputs = json!({"pbf": pbf, "dem": dem, "bounds": bounds, "town": town});
    let bench = json!({"inputs": inputs, "profiles": results, "prices": objects(&prices)});
    write(&out.join("bench.json"), &format!("{bench:#}\n"))?;
    write(&out.join("bench.md"), &text)?;
    if let Some(ends) = ends {
        write(&out.join("routes.json"), &json!({"pairs": ends, "routes": routes}).to_string())?;
    }
    Ok(())
}

fn compare(options: &HashMap<&str, &str>, runs: &[&str]) -> Result<(), String> {
    let (Some(names), Some(out), [a, b]) = (options.get("names"), options.get("out"), runs) else {
        return Err(USAGE.into());
    };
    let Some((x, y)) = names.split_once(',') else { return Err(USAGE.into()) };
    let read = |path: &str| -> Result<Value, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}"))
    };
    let (data, missing) = pairs::compare([&read(a)?, &read(b)?], [x, y])?;
    eprintln!("{missing} pairs have no route in one of the runs");
    write(Path::new(out), &pairs::page(&data))
}

fn command(args: &[String]) -> Result<(), String> {
    let (mut options, mut rest) = (HashMap::new(), Vec::new());
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word.strip_prefix("--") {
            None => rest.push(word),
            Some(name) if OPTIONS.contains(&name) => {
                options.insert(name, words.next().ok_or(USAGE)?);
            }
            Some(_) => return Err(USAGE.into()),
        }
    }
    match rest.split_first() {
        Some((&"run", [])) => run(&options),
        Some((&"compare", runs)) => compare(&options, runs),
        _ => Err(USAGE.into()),
    }
}

/// The command `route_bench`.
pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = command(&args) {
        eprintln!("route_bench: {error}");
        std::process::exit(1);
    }
}
