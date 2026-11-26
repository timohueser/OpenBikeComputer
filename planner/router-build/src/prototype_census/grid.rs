//! PROTOTYPE. The grid: what each candidate cost change does to the census and to the panel, and
//! the limits of one candidate: the production query and the bytes of a package.
use super::census::{self, Census, Net, LEVELS};
use super::panel::{self, Measures, Pair};
use super::report;
use super::variant::{self, Variant};
use crate::base::Topology;
use crate::Graph;
use planner_router::data::{RoutingData, Selection};
use planner_router::package::{digest, Package, Source};
use planner_router::search::PROTOTYPE_SETTLED;
use planner_router::shape::{self, Near};
use planner_router::{Control, Error, Router};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;
use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Instant;

const LIMITS: &str = "V1+V2T-100";

fn variants() -> Vec<(&'static str, Variant)> {
    let v0 = Variant::default();
    let v1 = Variant { path_as_cycleway: true, ..v0 };
    let turn = |turn: u32| Variant { turn, ..v0 };
    let through = Variant { turn: 100, through: true, ..v0 };
    let both = Variant { path_as_cycleway: true, ..through };
    let speed = Variant { speed_class: true, ..both };
    let bonus = Variant { full_bonus: true, ..speed };
    vec![
        ("V0", v0),
        ("V1", v1),
        ("V2-60", turn(60)),
        ("V2-100", turn(100)),
        ("V2-150", turn(150)),
        ("V2T-100", through),
        ("V3", Variant { speed_class: true, ..v0 }),
        ("V4", Variant { full_bonus: true, ..v0 }),
        ("V5", Variant { paved_unknown: true, ..v0 }),
        ("V1+V2-100", Variant { turn: 100, ..v1 }),
        (LIMITS, both),
        ("V1+V2T-100+V3", speed),
        ("V1+V2T-100+V3+V4", bonus),
        ("V1+V2T-100+V3+V4+V5", Variant { paved_unknown: true, ..bonus }),
    ]
}

struct Row {
    name: &'static str,
    census: Census,
    routes: Vec<Measures>,
    /// The labels that the plain search settled for the panel, for each query.
    settled: f64,
    seconds: [f64; 3],
}

fn quantile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn percent(part: f64, whole: f64) -> String {
    format!("{:.1} %", 100.0 * part / whole.max(1e-9))
}

/// The costs of `touring` under `variant`, as the bake computes them.
fn bake(graph: &Graph, topology: &Topology, variant: Variant) -> Result<crate::Costs, String> {
    variant::set(variant);
    crate::costs(graph, topology, &crate::profiles(&["touring"])?)
}

pub fn run(graph: &Graph, vias: &[i64], step_m: f64, out: &Path) -> Result<(), String> {
    let lists = out.join("grid");
    std::fs::create_dir_all(&lists).map_err(|e| e.to_string())?;
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let arcs = (topology.first.values(), topology.head.values());
    let profile = &crate::profiles(&["touring"])?[0];
    let started = Instant::now();

    // The pairs of the panel and the shortest route of each, with the roads and turns of today.
    let today = bake(graph, &topology, Variant::default())?;
    let (totals, turns) = (today.roads[0].totals.values(), today.roads[0].turns.values());
    let pairs = panel::sample(&Net::new(graph, profile, arcs, (totals, turns)), today.snap[0].values())?;
    let lengths: Vec<u32> = (0..totals.len())
        .map(|road| if totals[road] == u32::MAX { u32::MAX } else { graph.roads.length_m.values()[road] })
        .collect();
    let legal: Vec<u32> = turns.iter().map(|&turn| if turn == u32::MAX { u32::MAX } else { 0 }).collect();
    let direct = Net::new(graph, profile, arcs, (&lengths, &legal));
    let shortest: Vec<f64> = panel::routes(&direct, &pairs)?
        .iter()
        .map(|roads| roads.iter().map(|&road| direct.length(road)).sum())
        .collect();
    drop(today);
    eprintln!("Panel: {} pairs, {:.1} s", pairs.len(), started.elapsed().as_secs_f64());

    let mut rows = Vec::new();
    for (name, variant) in variants() {
        let mut clock = Instant::now();
        let mut lap = || std::mem::replace(&mut clock, Instant::now()).elapsed().as_secs_f64();
        let costs = bake(graph, &topology, variant)?;
        let mut net = Net::new(graph, profile, arcs, (costs.roads[0].totals.values(), costs.roads[0].turns.values()));
        let bake_s = lap();
        let census = census::run(&mut net, vias, (step_m, step_m))?;
        let census_s = lap();
        let settled = PROTOTYPE_SETTLED.load(Relaxed);
        let routes: Vec<Measures> =
            panel::routes(&net, &pairs)?.iter().map(|roads| panel::measure(&net, roads)).collect();
        let settled = (PROTOTYPE_SETTLED.load(Relaxed) - settled) as f64 / pairs.len() as f64;
        let file = lists.join(format!("splits_{}.csv", name.replace('+', "_")));
        std::fs::write(&file, report::splits_csv(&census)).map_err(|e| format!("{}: {e}", file.display()))?;
        let seconds = [bake_s, census_s, lap()];
        eprintln!("Variant {name}: bake {:.1} s, census {:.1} s, panel {:.1} s", seconds[0], seconds[1], seconds[2]);
        rows.push(Row { name, census, routes, settled, seconds });
    }
    variant::set(Variant::default());
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
    census_table(&mut text, &rows);
    panel_table(&mut text, &rows, &pairs, &shortest);
    writeln!(text, "\nThe 14 bakes, censuses and panels took {grid_s:.0} s.").unwrap();
    let clock = Instant::now();
    match limits(graph, &pairs, &rows) {
        Ok(section) => text.push_str(&section),
        Err(error) => writeln!(text, "\n## C. Limits\n\nNot measured: {error}").unwrap(),
    }
    writeln!(text, "\nSection C took {:.0} s.", clock.elapsed().as_secs_f64()).unwrap();
    let path = out.join("grid.md");
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}

fn census_table(text: &mut String, rows: &[Row]) {
    text.push_str(
        "\n## A. The census\n\n\
         Points about every 5 km on each relation with at least 5 km of main line, as in `census.md`; the legs are \
         the same for each variant. `within 30 m` is the share of the measured signed length that lies within 30 m of \
         the router's line. No relation has the level ncn. Splits count once each; `not longer` counts the splits \
         whose signed stretch is not longer than the router's stretch.\n\n\
         | variant | within 30 m | gain | icn | rcn | lcn | on the same directed road | legs with no split | splits | \
         not longer | split share of signed length | cost router / signed |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let share = |census: &Census| census.total.near_m / census.total.measured_m;
    for row in rows {
        let (census, total) = (&row.census, &row.census.total);
        let level = |name: &str| {
            let tally = &census.levels[LEVELS.iter().position(|level| *level == name).expect("a level")];
            percent(tally.near_m, tally.measured_m)
        };
        let not_longer = census
            .splits
            .iter()
            .filter(|split| split.router.length_m > 0.0 && split.signed.length_m <= split.router.length_m);
        writeln!(
            text,
            "| {} | **{}** | {:+.1} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            row.name,
            percent(total.near_m, total.measured_m),
            100.0 * (share(census) - share(&rows[0].census)),
            level("icn"),
            level("rcn"),
            level("lcn"),
            percent(total.same_road_m, total.measured_m),
            percent(total.identical_legs as f64, total.legs as f64),
            census.splits.len(),
            not_longer.count(),
            percent(total.split_m, total.measured_m),
            percent(total.router_cost, total.signed_cost),
        )
        .unwrap();
    }
    let total = &rows[0].census.total;
    writeln!(
        text,
        "\n{} relations, {:.0} km of measured signed length, {} legs. The split lists are in `grid/splits_<variant>.csv`. \
         Shortcuts: one box; GLO-30 terrain only; relations overlap, so a stretch in three relations counts three \
         times in the shares; the line direction only; plain bidirectional Dijkstra between junctions.",
        total.relations,
        total.measured_m / 1000.0,
        total.legs
    )
    .unwrap();
}

fn panel_table(text: &mut String, rows: &[Row], pairs: &[Pair], shortest: &[f64]) {
    write!(
        text,
        "\n## B. The panel\n\n\
         {} pairs of junctions where a road with the snap bit of `touring` arrives, the same for each variant: {} \
         each at {:?} km straight-line distance (within 10 %), SplitMix64 with a fixed seed. Each route is the \
         cheapest road sequence from any road that arrives at the first junction to any road that arrives at the \
         second, by plain bidirectional Dijkstra: no snap to a road position, no landmarks. The rates pool all \
         routes, so the 40 km routes weigh most.\n\n\
         - `/ shortest`: route length against the shortest route on the roads and turns that `touring` may take.\n\
         - `/ V0`: the sum of the lengths against V0, and the 95th percentile of the ratio of each route.\n\
         - Turn: a change of heading over 45° at a junction of three or more road ends.\n\
         - Leave and rejoin: the next road shares no `name`, `ref` or cycle route relation with the road before, \
         and a road within 2 km of route length shares one again. A roundabout or a link with no name counts.\n\
         - Fast main road: class 2, a limit over 50 km/h or no tagged limit, and no cycle track by \
         `traffic::protection`. Slow main road: class 2 with a tagged limit of 50 km/h or less.\n\
         - `differs`: the routes with more than 10 % of their length farther than 30 m from the route of V0.\n\
         - `settled`: the labels that the plain search settles for each query.\n\n\
         | variant | / shortest p50 | p95 | max | / V0 sum | / V0 p95 | turns per km | leave and rejoin per 100 km | \
         signals per 100 km | fast main km per 100 km | slow main km per 100 km | differs | settled |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
        pairs.len(),
        panel::PAIRS_EACH,
        panel::DISTANCES_KM,
    )
    .unwrap();
    let sum = |routes: &[Measures], pick: fn(&Measures) -> f64| routes.iter().map(pick).sum::<f64>();
    let base = &rows[0].routes;
    for row in rows {
        let routes = &row.routes;
        let km = sum(routes, |m| m.length_m) / 1000.0;
        let detour: Vec<f64> = routes.iter().zip(shortest).map(|(m, shortest)| m.length_m / shortest).collect();
        let against: Vec<f64> = routes.iter().zip(base).map(|(m, base)| m.length_m / base.length_m).collect();
        let differs =
            routes.iter().zip(base).filter(|(m, base)| shape::far(&m.line, &Near::new(&base.line)) > 0.1 * m.length_m);
        writeln!(
            text,
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.2} | {:.1} | {:.1} | {:.2} | {:.2} | {} | {:.0} |",
            row.name,
            quantile(&detour, 0.5),
            quantile(&detour, 0.95),
            quantile(&detour, 1.0),
            km / (sum(base, |m| m.length_m) / 1000.0),
            quantile(&against, 0.95),
            sum(routes, |m| f64::from(m.turns)) / km,
            100.0 * sum(routes, |m| f64::from(m.events)) / km,
            100.0 * sum(routes, |m| f64::from(m.signals)) / km,
            100.0 * sum(routes, |m| m.fast_main_m) / 1000.0 / km,
            100.0 * sum(routes, |m| m.slow_main_m) / 1000.0 / km,
            percent(differs.count() as f64, routes.len() as f64),
            row.settled,
        )
        .unwrap();
    }
    text.push_str(
        "\n| variant | route km | on a signed cycle route | unknown surface | paved | compacted | gravel | dirt | rough | \
         ascent m per km | bake s | census s | panel s |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for row in rows {
        let metres = sum(&row.routes, |m| m.length_m);
        let surface: Vec<String> =
            (0..6).map(|s| percent(row.routes.iter().map(|m| m.surface_m[s]).sum::<f64>(), metres)).collect();
        writeln!(
            text,
            "| {} | {:.0} | {} | {} | {:.1} | {:.1} | {:.1} | {:.1} |",
            row.name,
            metres / 1000.0,
            percent(sum(&row.routes, |m| m.signed_m), metres),
            surface.join(" | "),
            sum(&row.routes, |m| m.ascent_m) / (metres / 1000.0),
            row.seconds[0],
            row.seconds[1],
            row.seconds[2],
        )
        .unwrap();
    }
    text.push_str(
        "\n`on a signed cycle route`: the way has a cycle route rank in the direction of travel. The surface classes \
         are those of `osm::surface_class`. The seconds are wall time on 4 threads: the bake of one profile (way \
         bases, road pass, snap bits), the census, and the 300 routes with their measures.\n",
    );
}

#[derive(Clone)]
struct Memory(Arc<HashMap<String, Vec<u8>>>);

impl Source for Memory {
    fn read(&self, key: &str) -> planner_router::Result<Vec<u8>> {
        self.0.get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
    }
}

/// Section C: the package of the 10 profiles and the production router, today and with `LIMITS`.
fn limits(graph: &Graph, pairs: &[Pair], rows: &[Row]) -> Result<String, String> {
    let mut text = format!(
        "\n## C. Limits: V0 against {LIMITS}\n\n\
         Each side bakes the 10 preset profiles and writes the package into memory with `crate::package` (the pages \
         of a written package, each zlib page once by its SHA-256). With {LIMITS}, each cycling profile with \
         weights has the turn cost and the class of V1; `road` keeps its own turn cost and the hiking profiles have \
         none. The overlay index, the route catalog and the blocks are not in these bytes.\n"
    );
    let profiles = crate::profiles(&["all"])?;
    let points = graph.points.values();
    let (mut queries, mut tables, mut notes) = (String::new(), String::new(), String::new());
    let mut routers = Vec::new();
    for (name, variant) in variants().into_iter().filter(|(name, _)| ["V0", LIMITS].contains(name)) {
        variant::set(variant);
        let topology = Topology::new(&graph.roads, graph.points.len())?;
        let costs = crate::costs(graph, &topology, &profiles)?;
        let mut objects = HashMap::new();
        let place = ("prototype".to_string(), super::BOUNDS);
        let manifest = crate::package(graph, topology, costs, place, vec![], String::new(), |bytes| {
            let key = digest(bytes);
            objects.insert(key.clone(), bytes.to_vec());
            Ok(key)
        })?;
        variant::set(Variant::default());
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
        let package = objects.values().map(Vec::len).sum::<usize>() + json.len();
        writeln!(
            tables,
            "| {name} | {:.2} | {:.2} | {:.2} | {:.2} | {} |",
            package as f64 / 1e6,
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

        // The production router: snap, landmarks, the search and the answer geometry.
        let package = Package::open(Memory(Arc::new(objects)), &json).map_err(|e| e.to_string())?;
        let selection = Selection::whole(package);
        let budget = selection.default_budget();
        routers.push((name, Router::new(selection, budget)));
    }
    // The first pass fills the caches of each router. Then the two routers take turns for three
    // passes, and each query keeps its fastest time.
    let mut sides: Vec<_> = routers.iter().map(|_| (vec![f64::MAX; pairs.len()], Vec::new(), 0.0, 0)).collect();
    for pass in 0..4 {
        for ((_, router), (millis, settled, metres, failed)) in routers.iter_mut().zip(&mut sides) {
            for (query, pair) in pairs.iter().enumerate() {
                let ends =
                    [pair.from, pair.to].map(|junction| [points[junction as usize].lon, points[junction as usize].lat]);
                let before = PROTOTYPE_SETTLED.load(Relaxed);
                let clock = Instant::now();
                let route = router.route(&shape::request("touring", &ends, vec![]), &Control::default());
                if pass == 0 {
                    continue;
                }
                millis[query] = millis[query].min(1000.0 * clock.elapsed().as_secs_f64());
                if pass == 1 {
                    settled.push((PROTOTYPE_SETTLED.load(Relaxed) - before) as f64);
                    match route {
                        Ok(route) => *metres += route.totals().distance_m as f64,
                        Err(_) => *failed += 1,
                    }
                }
            }
        }
    }
    for ((name, _), (millis, settled, metres, failed)) in routers.iter().zip(&sides) {
        let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
        let plain = rows.iter().find(|row| row.name == *name).expect("a row of the grid");
        writeln!(
            queries,
            "| {name} | {:.1} | {:.1} | {:.1} | {:.0} | {:.0} | {:.0} | {failed} | {:.0} | {:.0} |",
            mean(millis),
            quantile(millis, 0.5),
            quantile(millis, 0.95),
            mean(settled),
            quantile(settled, 0.95),
            plain.settled,
            metres / 1000.0,
            plain.routes.iter().map(|m| m.length_m).sum::<f64>() / 1000.0,
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
