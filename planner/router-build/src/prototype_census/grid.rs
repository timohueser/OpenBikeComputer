//! PROTOTYPE. The grid: what each candidate cost change does to the census and to a panel, and
//! the limits of one candidate: the production query and the bytes of a package.
use super::census::{self, Census, Net};
use super::panel::{self, Measures, Pair, Spec};
use super::report;
use super::variant::{self, Variant};
use crate::base::Topology;
use crate::Graph;
use planner_router::data::{RoutingData, Selection};
use planner_router::package::{digest, Manifest, Package, Source};
use planner_router::search::{PROTOTYPE_NANOS, PROTOTYPE_PUSHED, PROTOTYPE_SETTLED, PROTOTYPE_STALE};
use planner_router::shape::{self, Near};
use planner_router::{Control, Error, Router};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;
use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Instant;

/// V1+V2T-100.
pub const BASE: Variant = Variant {
    path_as_cycleway: true,
    turn: 100,
    through: true,
    speed_class: false,
    full_bonus: false,
    paved_unknown: false,
    signal: 0,
    walk_bonus: false,
    walk_turn: 0,
    cluster: 0,
    keys: variant::KEYS_ALL,
    road_signal: false,
    entry: false,
};
const LIMITS: &str = "V1+V2T-100";

fn variants() -> Vec<(&'static str, Variant)> {
    let v0 = Variant::default();
    let v1 = Variant { path_as_cycleway: true, ..v0 };
    let turn = |turn: u32| Variant { turn, ..v0 };
    let speed = Variant { speed_class: true, ..BASE };
    let bonus = Variant { full_bonus: true, ..speed };
    vec![
        ("V0", v0),
        ("V1", v1),
        ("V2-60", turn(60)),
        ("V2-100", turn(100)),
        ("V2-150", turn(150)),
        ("V2T-100", Variant { path_as_cycleway: false, ..BASE }),
        ("V3", Variant { speed_class: true, ..v0 }),
        ("V4", Variant { full_bonus: true, ..v0 }),
        ("V5", Variant { paved_unknown: true, ..v0 }),
        ("V1+V2-100", Variant { turn: 100, ..v1 }),
        (LIMITS, BASE),
        ("V1+V2T-100+V3", speed),
        ("V1+V2T-100+V3+V4", bonus),
        ("V1+V2T-100+V3+V4+V5", Variant { paved_unknown: true, ..bonus }),
    ]
}

pub struct Row {
    pub name: &'static str,
    pub census: Census,
    /// The measures of the routes of each panel.
    pub panels: Vec<Vec<Measures>>,
    /// The labels that the plain search settled for the first panel, for each query.
    pub settled: f64,
    pub seconds: [f64; 3],
}

/// The pairs of a panel, and the length of the shortest route of each.
pub struct Panel {
    pub pairs: Vec<Pair>,
    pub shortest: Vec<f64>,
}

pub fn quantile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

pub fn percent(part: f64, whole: f64) -> String {
    format!("{:.1} %", 100.0 * part / whole.max(1e-9))
}

/// The costs of the profile `name` under `variant`, as the bake computes them.
pub fn bake(graph: &Graph, topology: &Topology, name: &str, variant: Variant) -> Result<crate::Costs, String> {
    variant::set(variant);
    crate::costs(graph, topology, &crate::profiles(&[name])?)
}

/// The panels of `specs` for the profile `name`, and the shortest route of each pair, on the
/// roads and turns that the profile may take today.
pub fn panels(graph: &Graph, topology: &Topology, name: &str, specs: &[Spec]) -> Result<Vec<Panel>, String> {
    let clock = Instant::now();
    let arcs = (topology.first.values(), topology.head.values());
    let profile = &crate::profiles(&[name])?[0];
    let today = bake(graph, topology, name, Variant::default())?;
    let (totals, turns) = (today.roads[0].totals.values(), today.roads[0].turns.values());
    let net = Net::new(graph, profile, arcs, (totals, turns));
    let lengths: Vec<u32> = (0..totals.len())
        .map(|road| if totals[road] == u32::MAX { u32::MAX } else { graph.roads.length_m.values()[road] })
        .collect();
    let legal: Vec<u32> = turns.iter().map(|&turn| if turn == u32::MAX { u32::MAX } else { 0 }).collect();
    let direct = Net::new(graph, profile, arcs, (&lengths, &legal));
    let mut panels = Vec::new();
    for spec in specs {
        let pairs = panel::sample(&net, today.snap[0].values(), spec)?;
        let shortest = panel::routes(&direct, &pairs)?
            .iter()
            .map(|roads| roads.iter().map(|&road| direct.length(road)).sum())
            .collect();
        panels.push(Panel { pairs, shortest });
    }
    eprintln!("Panels: {:.1} s", clock.elapsed().as_secs_f64());
    Ok(panels)
}

/// The census and the panels of the profile `name` under each variant. Writes the split list of
/// each variant as `<lists>/splits_<prefix><variant>.csv`.
pub fn measure(
    graph: &Graph,
    topology: &Topology,
    vias: &[i64],
    step_m: f64,
    name: &str,
    variants: &[(&'static str, Variant)],
    panels: &[Panel],
    lists: (&Path, &str),
) -> Result<Vec<Row>, String> {
    measure_with(graph, topology, Some((vias, step_m)), name, variants, panels, lists)
}

/// `measure`, with an empty census and no split list where `census` is `None`.
pub fn measure_with(
    graph: &Graph,
    topology: &Topology,
    census: Option<(&[i64], f64)>,
    name: &str,
    variants: &[(&'static str, Variant)],
    panels: &[Panel],
    (lists, prefix): (&Path, &str),
) -> Result<Vec<Row>, String> {
    std::fs::create_dir_all(lists).map_err(|e| e.to_string())?;
    let arcs = (topology.first.values(), topology.head.values());
    let profile = &crate::profiles(&[name])?[0];
    let mut rows = Vec::new();
    for &(variant_name, variant) in variants {
        let mut clock = Instant::now();
        let mut lap = || std::mem::replace(&mut clock, Instant::now()).elapsed().as_secs_f64();
        let costs = bake(graph, topology, name, variant)?;
        let mut net = Net::new(graph, profile, arcs, (costs.roads[0].totals.values(), costs.roads[0].turns.values()));
        let bake_s = lap();
        let census = match census {
            Some((vias, step_m)) => census::run(&mut net, vias, (step_m, step_m))?,
            None => Census::default(),
        };
        let census_s = lap();
        let (mut measures, mut settled) = (Vec::new(), 0.0);
        for (i, panel) in panels.iter().enumerate() {
            let before = PROTOTYPE_SETTLED.load(Relaxed);
            let routes = panel::routes(&net, &panel.pairs)?;
            if i == 0 {
                settled = (PROTOTYPE_SETTLED.load(Relaxed) - before) as f64 / panel.pairs.len() as f64;
            }
            measures.push(routes.iter().map(|roads| panel::measure(&net, roads)).collect());
        }
        if census.queries > 0 {
            let file = lists.join(format!("splits_{prefix}{}.csv", variant_name.replace('+', "_")));
            std::fs::write(&file, report::splits_csv(&census)).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        let seconds = [bake_s, census_s, lap()];
        eprintln!(
            "Variant {variant_name}: bake {:.1} s, census {:.1} s, panels {:.1} s",
            seconds[0], seconds[1], seconds[2]
        );
        rows.push(Row { name: variant_name, census, panels: measures, settled, seconds });
    }
    variant::set(Variant::default());
    Ok(rows)
}

pub fn run(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let started = Instant::now();
    let panels = panels(graph, &topology, "touring", &[Spec { seed: 3836, classes: panel::DISTANCES, inside: None }])?;
    let rows = measure(graph, &topology, vias, step_m, "touring", &variants(), &panels, (&out.join("grid"), ""))?;
    let grid_s = started.elapsed().as_secs_f64();

    let mut text = String::from("# Grid of candidate cost changes, profile `touring` (prototype for issue 3836)\n\n");
    text.push_str(
        "The candidates are stand-ins that measure the reach of an idea. They are not proposals, and nothing is \
         tuned. Each row bakes the costs of `touring` again with the real cost code and one or more switches \
         (`prototype_census/variant.rs`).\n\n\
         - V1: `highway=path|footway` with `bicycle=designated` has class 0 for the class preference and for the \
         rule of a missing surface tag.\n\
         - V2-T: a turn cost of T × (1 − cos of the change of heading) at a junction of three or more road ends; \
         the headings are taken over 15 m. V2T: no turn cost onto a road with the same `name`, the same `ref`, or \
         one same `route=bicycle` relation; a U-turn onto the same way keeps its cost.\n\
         - V3: the class preference of a main road is 1.5 with a tagged limit of 30 km/h or less, 2.0 up to 50, \
         else 3.0.\n\
         - V4: a road of class 1, 3 or 4 with a cycle route rank in its direction has no class preference.\n\
         - V5: a road of class 0, 1 or 2 with no `surface` tag and no `tracktype` has the surface factor of a paved \
         road.\n",
    );
    census_table(&mut text, "## A. The census", &rows, "grid/splits_<variant>.csv");
    panel_table(&mut text, "## B. The panel", PANEL_NOTE, &rows, 0, &panels[0], true);
    writeln!(text, "\nThe 14 bakes, censuses and panels took {grid_s:.0} s.").unwrap();
    let clock = Instant::now();
    match limits(graph, bounds, &panels[0].pairs, &rows) {
        Ok(section) => text.push_str(&section),
        Err(error) => writeln!(text, "\n## C. Limits\n\nNot measured: {error}").unwrap(),
    }
    writeln!(text, "\nSection C took {:.0} s.", clock.elapsed().as_secs_f64()).unwrap();
    let path = out.join("grid.md");
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}

/// Table A. The level columns are the levels of the first row that have a relation.
pub fn census_table(text: &mut String, title: &str, rows: &[Row], lists: &str) {
    let first = &rows[0].census;
    let levels: Vec<usize> = (1..6).rev().filter(|&level| first.levels[level].relations > 0).collect();
    let names: Vec<&str> = levels.iter().map(|&level| first.level_names[level]).collect();
    write!(
        text,
        "\n{title}\n\n\
         Points about every 5 km on each relation with at least 5 km of main line, as in `census.md`; the legs are \
         the same for each variant. `within 30 m` is the share of the measured signed length that lies within 30 m of \
         the router's line, for all relations and by level. Splits count once each; `not longer` counts the splits \
         whose signed stretch is not longer than the router's stretch.\n\n\
         | variant | within 30 m | gain | {} | on the same directed road | legs with no split | splits | \
         not longer | split share of signed length | cost router / signed |\n|---|---|---|{}---|---|---|---|---|---|\n",
        names.join(" | "),
        "---|".repeat(names.len()),
    )
    .unwrap();
    let share = |census: &Census| census.total.near_m / census.total.measured_m;
    for row in rows {
        let (census, total) = (&row.census, &row.census.total);
        let by_level: Vec<String> =
            levels.iter().map(|&level| percent(census.levels[level].near_m, census.levels[level].measured_m)).collect();
        let not_longer = census
            .splits
            .iter()
            .filter(|split| split.router.length_m > 0.0 && split.signed.length_m <= split.router.length_m);
        writeln!(
            text,
            "| {} | **{}** | {:+.1} | {} | {} | {} | {} | {} | {} | {} |",
            row.name,
            percent(total.near_m, total.measured_m),
            100.0 * (share(census) - share(first)),
            by_level.join(" | "),
            percent(total.same_road_m, total.measured_m),
            percent(total.identical_legs as f64, total.legs as f64),
            census.splits.len(),
            not_longer.count(),
            percent(total.split_m, total.measured_m),
            percent(total.router_cost, total.signed_cost),
        )
        .unwrap();
    }
    let total = &first.total;
    writeln!(
        text,
        "\n{} relations, {:.0} km of measured signed length, {} legs. The split lists are in `{lists}`. \
         Shortcuts: one box; GLO-30 terrain only; relations overlap, so a stretch in three relations counts three \
         times in the shares; the line direction only; plain bidirectional Dijkstra between junctions. Check: {} \
         legs in all rows where the router's stretch costs more than the signed one (must be 0).",
        total.relations,
        total.measured_m / 1000.0,
        total.legs,
        rows.iter().map(|row| row.census.optimality_violations).sum::<usize>(),
    )
    .unwrap();
}

pub const PANEL_NOTE: &str =
    "300 pairs of junctions where a road with the snap bit of `touring` arrives, the same for \
     each variant: 100 each at 5, 15 and 40 km straight-line distance (within 10 %), SplitMix64 with a fixed seed.";

/// What a route of a panel is, and the columns of table B.
pub const PANEL_LEGEND: &str =
    "Each route is the cheapest road sequence from any road that arrives at the first junction to any \
         road that arrives at the second, by plain bidirectional Dijkstra: no snap to a road position, no \
         landmarks. The rates pool all routes, so the long routes weigh most.\n\n\
         - `/ shortest`: route length against the shortest route on the roads and turns that the profile may take.\n\
         - `/ V0`: the sum of the lengths against V0, and the 95th percentile of the ratio of each route.\n\
         - Turn: a change of heading over 45° at a junction of three or more road ends.\n\
         - Leave and rejoin: the next road shares no `name`, `ref` or route relation with the road before, and a \
         road within 2 km of route length shares one again. A roundabout or a link with no name counts.\n\
         - Signals: signal nodes on the route. Signalled junctions: the same without a signal node that a road \
         shorter than 40 m joins to the signal node before.\n\
         - Fast main road: class 2, a limit over 50 km/h or no tagged limit, and no cycle track by \
         `traffic::protection`. Slow main road: class 2 with a tagged limit of 50 km/h or less.\n\
         - `class 1 at 80`: quiet roads (class 1: residential, unclassified, tertiary, service) with a tagged \
         limit of 80 km/h or more; `class 1 over 50`: with a tagged limit over 50 km/h. `no limit`: the share of \
         the route length with no tagged limit, all classes.\n\
         - `differs`: the routes with more than 10 % of their length farther than 30 m from the route of the \
         first row.\n\
         - `settled`: the labels that the plain search settles for each query.";

/// Table B for panel `index` of each row. `full` adds the table of the surfaces and the seconds.
pub fn panel_table(text: &mut String, title: &str, note: &str, rows: &[Row], index: usize, panel: &Panel, full: bool) {
    panel_rows(text, title, &format!("{note} {PANEL_LEGEND}"), rows, index, panel, full);
}

/// `panel_table` with `note` alone above the rows.
pub fn panel_rows(text: &mut String, title: &str, note: &str, rows: &[Row], index: usize, panel: &Panel, full: bool) {
    write!(
        text,
        "\n{title}\n\n{note}\n\n\
         | variant | / shortest p50 | p95 | max | / V0 sum | / V0 p95 | turns per km | leave and rejoin per 100 km | \
         signals per 100 km | signalled junctions per 100 km | fast main km per 100 km | slow main km per 100 km | \
         class 1 at 80 km per 100 km | class 1 over 50 km per 100 km | no limit | differs | settled |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    )
    .unwrap();
    let sum = |routes: &[Measures], pick: fn(&Measures) -> f64| routes.iter().map(pick).sum::<f64>();
    let base = &rows[0].panels[index];
    for row in rows {
        let routes = &row.panels[index];
        let km = sum(routes, |m| m.length_m) / 1000.0;
        let detour: Vec<f64> = routes.iter().zip(&panel.shortest).map(|(m, shortest)| m.length_m / shortest).collect();
        let against: Vec<f64> = routes.iter().zip(base).map(|(m, base)| m.length_m / base.length_m).collect();
        let differs =
            routes.iter().zip(base).filter(|(m, base)| shape::far(&m.line, &Near::new(&base.line)) > 0.1 * m.length_m);
        writeln!(
            text,
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.2} | {:.1} | {:.1} | {:.1} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} | {} |",
            row.name,
            quantile(&detour, 0.5),
            quantile(&detour, 0.95),
            quantile(&detour, 1.0),
            km / (sum(base, |m| m.length_m) / 1000.0),
            quantile(&against, 0.95),
            sum(routes, |m| f64::from(m.turns)) / km,
            100.0 * sum(routes, |m| f64::from(m.events)) / km,
            100.0 * sum(routes, |m| f64::from(m.signals)) / km,
            100.0 * sum(routes, |m| f64::from(m.signalled)) / km,
            100.0 * sum(routes, |m| m.fast_main_m) / 1000.0 / km,
            100.0 * sum(routes, |m| m.slow_main_m) / 1000.0 / km,
            100.0 * sum(routes, |m| m.fast_quiet_m) / 1000.0 / km,
            100.0 * sum(routes, |m| m.quiet_over_50_m) / 1000.0 / km,
            percent(sum(routes, |m| m.no_limit_m) / 1000.0, km),
            percent(differs.count() as f64, routes.len() as f64),
            if index == 0 { format!("{:.0}", row.settled) } else { "-".into() },
        )
        .unwrap();
    }
    if !full {
        return;
    }
    text.push_str(
        "\n| variant | route km | on a signed cycle route | unknown surface | paved | compacted | gravel | dirt | rough | \
         ascent m per km | bake s | census s | panels s |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for row in rows {
        let routes = &row.panels[index];
        let metres = sum(routes, |m| m.length_m);
        let surface: Vec<String> =
            (0..6).map(|s| percent(routes.iter().map(|m| m.surface_m[s]).sum::<f64>(), metres)).collect();
        writeln!(
            text,
            "| {} | {:.0} | {} | {} | {:.1} | {:.1} | {:.1} | {:.1} |",
            row.name,
            metres / 1000.0,
            percent(sum(routes, |m| m.signed_m), metres),
            surface.join(" | "),
            sum(routes, |m| m.ascent_m) / (metres / 1000.0),
            row.seconds[0],
            row.seconds[1],
            row.seconds[2],
        )
        .unwrap();
    }
    text.push_str(
        "\n`on a signed cycle route`: the way has a cycle route rank in the direction of travel. The surface classes \
         are those of `osm::surface_class`. The seconds are wall time on 4 threads: the bake of one profile (way \
         bases, road pass, snap bits), the census, and the routes of all panels with their measures.\n",
    );
}

#[derive(Clone)]
struct Memory(Arc<HashMap<String, Vec<u8>>>);

impl Source for Memory {
    fn read(&self, key: &str) -> planner_router::Result<Vec<u8>> {
        self.0.get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
    }
}

type Objects = HashMap<String, Vec<u8>>;

/// The package of `profiles` under `variant`, written into memory.
fn package(
    graph: &Graph,
    bounds: [f64; 4],
    profiles: &[&str],
    variant: Variant,
) -> Result<(Manifest, Objects), String> {
    variant::set(variant);
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let costs = crate::costs(graph, &topology, &crate::profiles(profiles)?)?;
    let mut objects = HashMap::new();
    let place = ("prototype".to_string(), bounds);
    let manifest = crate::package(graph, topology, costs, place, vec![], String::new(), |bytes| {
        let key = digest(bytes);
        objects.insert(key.clone(), bytes.to_vec());
        Ok(key)
    })?;
    variant::set(Variant::default());
    Ok((manifest, objects))
}

/// The times and the counters of the production router for the queries of a panel.
pub struct Race {
    pub name: &'static str,
    /// The fastest time of each query, in ms.
    pub millis: Vec<f64>,
    /// For each query: the ms inside `search::Workspace`, the labels that it settled, the labels
    /// that it set, and the stale queue entries that it dropped.
    pub counters: [Vec<f64>; 4],
    pub metres: f64,
    pub failed: usize,
}

/// Runs `Router::route` of `touring` for `pairs` on the package of each variant: one pass that
/// fills the caches, then three passes where the routers take turns. Each query keeps its fastest
/// time; the counters are those of the first measured pass.
pub fn race(
    graph: &Graph,
    pairs: &[Pair],
    packages: Vec<(&'static str, Manifest, Objects)>,
) -> Result<Vec<Race>, String> {
    let points = graph.points.values();
    let mut routers = Vec::new();
    let mut races = Vec::new();
    for (name, manifest, objects) in packages {
        let json = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
        let package = Package::open(Memory(Arc::new(objects)), &json).map_err(|e| e.to_string())?;
        let selection = Selection::whole(package);
        let budget = selection.default_budget();
        routers.push(Router::new(selection, budget));
        let millis = vec![f64::MAX; pairs.len()];
        races.push(Race { name, millis, counters: Default::default(), metres: 0.0, failed: 0 });
    }
    let counters = [&PROTOTYPE_NANOS, &PROTOTYPE_SETTLED, &PROTOTYPE_PUSHED, &PROTOTYPE_STALE];
    for pass in 0..4 {
        for (router, race) in routers.iter_mut().zip(&mut races) {
            for (query, pair) in pairs.iter().enumerate() {
                let ends =
                    [pair.from, pair.to].map(|junction| [points[junction as usize].lon, points[junction as usize].lat]);
                let before = counters.map(|counter| counter.load(Relaxed));
                let clock = Instant::now();
                let route = router.route(&shape::request("touring", &ends, vec![]), &Control::default());
                if pass == 0 {
                    continue;
                }
                race.millis[query] = race.millis[query].min(1000.0 * clock.elapsed().as_secs_f64());
                if pass == 1 {
                    for (i, counter) in counters.iter().enumerate() {
                        let count = (counter.load(Relaxed) - before[i]) as f64;
                        race.counters[i].push(if i == 0 { count / 1e6 } else { count });
                    }
                    match route {
                        Ok(route) => race.metres += route.totals().distance_m as f64,
                        Err(_) => race.failed += 1,
                    }
                }
            }
        }
    }
    Ok(races)
}

pub fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

/// Section C: the package of the 10 profiles and the production router, today and with `LIMITS`.
fn limits(graph: &Graph, bounds: [f64; 4], pairs: &[Pair], rows: &[Row]) -> Result<String, String> {
    let mut text = format!(
        "\n## C. Limits: V0 against {LIMITS}\n\n\
         Each side bakes the 10 preset profiles and writes the package into memory with `crate::package` (the pages \
         of a written package, each zlib page once by its SHA-256). With {LIMITS}, each cycling profile with \
         weights has the turn cost and the class of V1; `road` keeps its own turn cost and the hiking profiles have \
         none. The overlay index, the route catalog and the blocks are not in these bytes.\n"
    );
    let (mut queries, mut tables, mut notes) = (String::new(), String::new(), String::new());
    let mut packages = Vec::new();
    for (name, variant) in [("V0", Variant::default()), (LIMITS, BASE)] {
        let (manifest, objects) = package(graph, bounds, &["all"], variant)?;
        let json = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
        let bytes = |keys: &BTreeSet<&String>| keys.iter().map(|key| objects[*key].len()).sum::<usize>();
        let (mut turn_pages, mut cost_pages) = (BTreeSet::new(), BTreeSet::new());
        let mut same = BTreeMap::<&Vec<String>, Vec<&str>>::new();
        let mut each = Vec::new();
        for (profile, metric) in &manifest.metrics {
            let turns = &metric.weights.turns;
            turn_pages.extend(&turns.values.blocks);
            cost_pages.extend(&metric.weights.road_costs.values.blocks);
            same.entry(&turns.values.blocks).or_default().push(profile.as_str());
            each.push(format!(
                "{profile} {:?} {:.2}",
                turns.width,
                bytes(&turns.values.blocks.iter().collect()) as f64 / 1e6
            ));
        }
        let groups: Vec<String> = same.values().map(|group| group.join(" = ")).collect();
        let size = objects.values().map(Vec::len).sum::<usize>() + json.len();
        writeln!(
            tables,
            "| {name} | {:.2} | {:.2} | {:.2} | {:.2} | {} |",
            size as f64 / 1e6,
            bytes(&turn_pages) as f64 / 1e6,
            bytes(&cost_pages) as f64 / 1e6,
            json.len() as f64 / 1e6,
            same.len(),
        )
        .unwrap();
        writeln!(
            notes,
            "\n{name}: the same turn table: {}. Width and MB of each: {}.",
            groups.join("; "),
            each.join(", ")
        )
        .unwrap();
        packages.push((name, manifest, objects));
    }
    for race in race(graph, pairs, packages)? {
        let plain = rows.iter().find(|row| row.name == race.name).expect("a row of the grid");
        writeln!(
            queries,
            "| {} | {:.1} | {:.1} | {:.1} | {:.0} | {:.0} | {:.0} | {} | {:.0} | {:.0} |",
            race.name,
            mean(&race.millis),
            quantile(&race.millis, 0.5),
            quantile(&race.millis, 0.95),
            mean(&race.counters[1]),
            quantile(&race.counters[1], 0.95),
            plain.settled,
            race.failed,
            race.metres / 1000.0,
            plain.panels[0].iter().map(|m| m.length_m).sum::<f64>() / 1000.0,
        )
        .unwrap();
    }
    write!(
        text,
        "\n### 1. The production query on the panel\n\n\
         `Router::route` with `Selection::whole` on the package in memory: snap, landmarks, search and answer, one \
         thread. The first pass over the 300 pairs fills the caches; then the two routers take turns for three \
         passes, and each query keeps its fastest time. `settled` is the count of labels that `search::Workspace` \
         takes from its queues for one request, from a counter that the prototype adds. `plain settled` is the same \
         count of the search without landmarks of section B. The route km agree with section B, so the production \
         router gives the routes of the plain search.\n\n\
         | variant | mean ms | p50 ms | p95 ms | settled mean | settled p95 | plain settled mean | failed | route km | \
         route km of section B |\n|---|---|---|---|---|---|---|---|---|---|\n{queries}\n\
         ### 2. The bytes of the package\n\n\
         | variant | package MB | turn tables MB | road cost tables MB | manifest MB | different turn tables of 10 |\n\
         |---|---|---|---|---|---|\n{tables}{notes}\n\
         Not measured: the query time and the bytes of the other variants; the overlay index, the route \
         catalog and the blocks of the `planner/routing` layer; the pack files of a package on disk; a profile \
         other than `touring` in sections A and B; a second region.\n"
    )
    .unwrap();
    Ok(text)
}

/// The packages of `touring` alone for `variants`, for a race.
pub fn touring_packages(
    graph: &Graph,
    bounds: [f64; 4],
    variants: &[(&'static str, Variant)],
) -> Result<Vec<(&'static str, Manifest, Objects)>, String> {
    let packages = variants.iter().map(|&(name, variant)| {
        let (manifest, objects) = package(graph, bounds, &["touring"], variant)?;
        Ok((name, manifest, objects))
    });
    packages.collect()
}
