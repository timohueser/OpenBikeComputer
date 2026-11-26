//! PROTOTYPE. Round five: the rule that joins the signal nodes of one junction, the keys of a
//! through road, and the other profiles. Each region is one run and writes its parts of
//! `<out>/round5/`; `run.sh --round5` joins the parts into `round5.md`.
use super::grid::{self, percent, quantile, Panel, Row};
use super::panel::{self, Measures, Spec, CLUSTERS_M};
use super::round4::{FIRST, TODAY};
use super::variant::{self, Variant, CLUSTER_M};
use crate::base::Topology;
use crate::Graph;
use planner_router::shape::{self, Near};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::Path;
use std::time::Instant;

const KEYS: [(&str, Variant); 4] = [
    ("today", TODAY),
    ("first", FIRST),
    ("first-ii", Variant { keys: variant::KEYS_NO_REF, ..FIRST }),
    ("first-iii", Variant { keys: variant::KEYS_NO_MAIN_REF, ..FIRST }),
];
/// Not in the brief: `first` with no through key at all, and `first` with no turn cost.
const TURNS: [(&str, Variant); 2] =
    [("first-nokey", Variant { through: false, ..FIRST }), ("first-noturn", Variant { turn: 0, ..FIRST })];
const CLUSTERS: [(&str, Variant); 4] = [
    ("first-c40", Variant { cluster: 40, ..FIRST }),
    ("first-c60", Variant { cluster: 60, ..FIRST }),
    ("first-e40", Variant { cluster: 40, entry: true, ..FIRST }),
    ("first-e60", Variant { cluster: 60, entry: true, ..FIRST }),
];
const ROAD_C60: Variant = Variant { signal: 60, cluster: 60, road_signal: true, ..TODAY };
const ROAD: [(&str, Variant); 3] =
    [("today", TODAY), ("road-c60", ROAD_C60), ("road-e60", Variant { entry: true, ..ROAD_C60 })];

const FREIBURG_TOWN: [f64; 4] = [7.78, 47.97, 7.90, 48.03];
const GALWAY_TOWN: [f64; 4] = [-9.12, 53.25, -8.95, 53.30];
const TOWN: &[(f64, f64, usize)] = &[(3.0, 8.0, 100)];

const INTRO_A: &str = "\n# A. The rule that joins the signal nodes of one junction\n\n\
     `today` is V0. `first` is V1 + V2T-100 + S60 with the rule of rounds three and four: `touring` pays S = 60 m on \
     the arc that arrives at a signal node, but not where the arriving road is shorter than 40 m and starts at a \
     signal node too. `first-c40` and `first-c60` are `first` with the rule \"enter a cluster\" in its place, for \
     R = 40 m and R = 60 m. The variants are stand-ins, and nothing is tuned.\n\n\
     `first-e40` and `first-e60` are not in the brief of the round. The rule as written lets a route into a cluster \
     through a junction with no signal and charges nothing (the column `free`), so these two rows measure the same \
     clusters with that door closed: the route pays S on the arc that arrives at any node of a cluster, signal node \
     or not, from a node that is not in that cluster.\n\n\
     - A cluster: the signal nodes that a path of roads of at most R metres in total joins, and the junctions on \
     those paths. A third signal node can join two others, so a chain of signal nodes can be longer than R.\n\
     - The rule: the route pays S on the arc that arrives at a signal node only when the node at the start of the \
     arriving road is not in the same cluster.\n\
     - Shortcuts of the clusters: a path takes any road of the graph in any direction, whatever its access (a \
     footway, a one-way road against its direction). A road counts with its whole length in rounded metres, so no \
     path ends inside a road. Each signal node is a junction of the graph, because the import ends a road there. \
     `m across` is the largest straight-line distance between two nodes of a cluster.\n\
     - `signal nodes`: the signal nodes that the routes pass.\n\
     - `clusters entered`: the arcs where a route arrives at a node of a cluster from a node that is not in that \
     cluster. The clusters are those of R = 60 m for each variant, also for `today`. `first-e60` pays S for each \
     of them. `R = 40`: the same count with the clusters of R = 40 m, where `first-e40` pays.\n\
     - `with a signal`: the clusters entered of which the route passes a signal node: `paid` plus `free`. The rest \
     are clusters that the route touches at junctions with no signal only.\n\
     - `paid`: the clusters that a route enters at a signal node. `first-c60` pays S there, and only there.\n\
     - `free`: the clusters that a route enters at a junction with no signal and then passes a signal node of. \
     `first-c60` charges nothing for them.\n\
     - The first and the last junction of a route do not count. The other columns are those of table B (section B).\n\
     - `differs`: the routes with more than 10 % of their length farther than 30 m from the route of the named \
     row.\n\
     - Shortcuts of the panels: GLO-30 terrain only; plain bidirectional Dijkstra between junctions, no snap to a \
     road position; the rates pool all routes, so the long routes weigh most.\n";

const INTRO_B: &str = "\n# B. What makes a through road\n\n\
     `first` has no turn cost where the next road shares a through key with the road before. The rows differ only in \
     the key rule; each has V1 and S60 with the 40 m rule.\n\n\
     - (i) `first`: the same `name`, the same `ref`, or one same `route=bicycle` relation.\n\
     - (ii) `first-ii`: the same `name` or one same relation. A `ref` is never a key.\n\
     - (iii) `first-iii`: as (i), but the `ref` is no key where the road before or the next road is a main road \
     (class 2: secondary, primary, trunk).\n\
     - Not in the brief, to find where the main road km come from: `first-nokey` is `first` with no through key at \
     all (V1 + V2-100 + S60), and `first-noturn` is `first` with no turn cost (V1 + S60).\n\n\
     Shortcut: the measure `leave and rejoin` uses the keys of (i) in each row, so it is the same measure for each \
     rule, and rules (ii) and (iii) no longer aim at it.\n\n";

const INTRO_C: &str = "\n# C. Other profiles (Freiburg)\n\n\
     `first` for `gravel` and `mtb` is the three switches of `touring`, unchanged: V1, V2T-100 with key rule (i), and \
     S60 with the 40 m rule. The hooks act on each cycling profile with weights. Each profile has its own panel: the \
     seed and the distance classes of the `touring` panel, but the junctions where a road with the snap bit of that \
     profile arrives, so the pairs are not the pairs of `touring`. `/ shortest` is against the shortest route on the \
     roads that the profile may take. The columns are those of table B (section B).\n";

fn save(out: &Path, name: &str, text: &str) -> Result<(), String> {
    let folder = out.join("round5");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(name);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}

/// The panels of `specs` and their rows for `profile` under each variant, with the census of
/// `census`.
fn measure(
    graph: &Graph,
    profile: &str,
    census: Option<(&[i64], f64)>,
    specs: &[Spec],
    variants: &[(&'static str, Variant)],
    (out, prefix): (&Path, &str),
) -> Result<(Vec<Row>, Vec<Panel>), String> {
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let panels = grid::panels(graph, &topology, profile, specs)?;
    let lists = (&*out.join("round5"), prefix);
    let rows = grid::measure_with(graph, &topology, census, profile, variants, &panels, lists)?;
    Ok((rows, panels))
}

/// The clusters of the graph for each R.
fn clusters(text: &mut String, graph: &Graph) {
    let at =
        |size: &variant::Size| format!("{:.5},{:.5}", f64::from(size.at.lat) * 1e-6, f64::from(size.at.lon) * 1e-6);
    text.push_str(
        "\n| R m | signal nodes | clusters | clusters of 2 or more signal nodes | largest cluster: signal nodes | its \
         nodes | its m across | at lat,lon | widest cluster: m across | its signal nodes | its nodes | at lat,lon | \
         clusters over 100 m across | over 200 m |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for radius in CLUSTER_M {
        let sizes = &variant::context().clusters(radius).sizes;
        let largest = sizes.iter().max_by_key(|size| (size.signals, size.nodes));
        let widest = sizes.iter().max_by(|a, b| a.across_m.total_cmp(&b.across_m));
        let (Some(largest), Some(widest)) = (largest, widest) else {
            writeln!(text, "| {radius} | 0 | 0 | 0 | - | - | - | - | - | - | - | - | 0 | 0 |").unwrap();
            continue;
        };
        let over = |metres: f64| sizes.iter().filter(|size| size.across_m > metres).count();
        writeln!(
            text,
            "| {radius} | {} | {} | {} | {} | {} | {:.0} | {} | {:.0} | {} | {} | {} | {} | {} |",
            graph.signals.values().iter().filter(|&&signal| signal != 0).count(),
            sizes.len(),
            sizes.iter().filter(|size| size.signals >= 2).count(),
            largest.signals,
            largest.nodes,
            largest.across_m,
            at(largest),
            widest.across_m,
            widest.signals,
            widest.nodes,
            at(widest),
            over(100.0),
            over(200.0),
        )
        .unwrap();
    }
    text.push_str("\nThe three widest clusters of each R, with the names of the roads inside them:\n\n");
    let roads = &graph.roads;
    for radius in CLUSTER_M {
        let clusters = variant::context().clusters(radius);
        let mut widest: Vec<usize> = (0..clusters.sizes.len()).collect();
        widest.sort_by(|&a, &b| clusters.sizes[b].across_m.total_cmp(&clusters.sizes[a].across_m));
        for &index in widest.iter().take(3) {
            let inside = |road: &usize| {
                clusters.of(roads.from(*road)) == index as u32 + 1 && clusters.of(roads.to(*road)) == index as u32 + 1
            };
            let names: BTreeSet<String> = (0..roads.len())
                .filter(inside)
                .filter_map(|road| {
                    graph.osm.ways.row(roads.way_row(road) as usize).tags.get("name").map(str::to_string)
                })
                .collect();
            let size = &clusters.sizes[index];
            writeln!(
                text,
                "- R = {radius}: {:.0} m across, {} signal nodes, {} nodes, at {}: {}",
                size.across_m,
                size.signals,
                size.nodes,
                at(size),
                names.into_iter().collect::<Vec<_>>().join(", "),
            )
            .unwrap();
        }
    }
}

/// The measures of section A for panel `index` of each row. The first row is `today`; `differs`
/// compares with row `base`.
fn signal_table(text: &mut String, title: &str, note: &str, rows: &[&Row], index: usize, panel: &Panel, base: usize) {
    write!(
        text,
        "\n{title}\n\n{note}\n\n\
         | variant | / today sum | / today p95 | / shortest p50 | / shortest p95 | signal nodes per 100 km | clusters \
         entered per 100 km (R = {CLUSTERS_M}) | with a signal | paid | free | R = 40 | turns per km | slow main km \
         per 100 km | fast main km per 100 km | differs from `{}` |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
        rows[base].name,
    )
    .unwrap();
    let sum = |routes: &[Measures], pick: fn(&Measures) -> f64| routes.iter().map(pick).sum::<f64>();
    let (today, compared) = (&rows[0].panels[index], &rows[base].panels[index]);
    for row in rows {
        let routes = &row.panels[index];
        let km = sum(routes, |m| m.length_m) / 1000.0;
        let detour: Vec<f64> = routes.iter().zip(&panel.shortest).map(|(m, shortest)| m.length_m / shortest).collect();
        let against: Vec<f64> = routes.iter().zip(today).map(|(m, today)| m.length_m / today.length_m).collect();
        let differs = routes
            .iter()
            .zip(compared)
            .filter(|(m, other)| shape::far(&m.line, &Near::new(&other.line)) > 0.1 * m.length_m);
        writeln!(
            text,
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.2} | {:.2} | {:.2} | {} |",
            row.name,
            km / (sum(today, |m| m.length_m) / 1000.0),
            quantile(&against, 0.95),
            quantile(&detour, 0.5),
            quantile(&detour, 0.95),
            100.0 * sum(routes, |m| f64::from(m.signals)) / km,
            100.0 * sum(routes, |m| f64::from(m.entered)) / km,
            100.0 * sum(routes, |m| f64::from(m.paid + m.free)) / km,
            100.0 * sum(routes, |m| f64::from(m.paid)) / km,
            100.0 * sum(routes, |m| f64::from(m.free)) / km,
            100.0 * sum(routes, |m| f64::from(m.entered_40)) / km,
            sum(routes, |m| f64::from(m.turns)) / km,
            100.0 * sum(routes, |m| m.slow_main_m) / 1000.0 / km,
            100.0 * sum(routes, |m| m.fast_main_m) / 1000.0 / km,
            percent(differs.count() as f64, routes.len() as f64),
        )
        .unwrap();
    }
}

fn panel_note(profile: &str, seed: &str) -> String {
    format!(
        "300 pairs of junctions where a road with the snap bit of `{profile}` arrives, the same for each variant: 100 \
         each at 5, 15 and 40 km straight-line distance (within 10 %), {seed}."
    )
}

fn town_note(town: [f64; 4], name: &str) -> String {
    format!(
        "100 pairs of junctions inside the box {town:?} ({name}), 3 to 8 km apart in a straight line, a fixed seed, \
         the same for each variant. A route may leave the town box."
    )
}

/// The rows of section A: `today`, `first` and the cluster rules, from rows in the order of
/// `KEYS`, `TURNS`, `CLUSTERS`.
fn cluster_rows(rows: &[Row]) -> Vec<&Row> {
    rows[..2].iter().chain(&rows[6..]).collect()
}

/// The arcs where the key rules differ, and the turn cost that such an arc then pays.
fn key_arcs(text: &mut String, graph: &Graph) -> Result<(), String> {
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let (first, head, roads, context) =
        (topology.first.values(), topology.head.values(), &graph.roads, variant::context());
    let (mut arcs, mut keyed) = (0usize, 0usize);
    // The arcs with the `ref` as their only key, and those of them with a main road: their count,
    // their count with a turn cost over 10 m, and the sum of their turn costs.
    let mut by_ref = [(0usize, 0usize, 0.0f64); 2];
    for before in 0..roads.len() {
        if context.degree[roads.to(before) as usize] < 3 {
            continue;
        }
        for &after in &head[first[before] as usize..first[before + 1] as usize] {
            let (a, b) = (before, after as usize);
            if roads.way(a) == roads.way(b) && roads.from(a) == roads.to(b) {
                continue;
            }
            arcs += 1;
            let rows = (roads.way_row(a), roads.way_row(b));
            if !context.through(rows.0, rows.1, false, true) {
                continue;
            }
            keyed += 1;
            if context.through(rows.0, rows.1, false, false) {
                continue;
            }
            let cost = 100.0 * (1.0 - variant::cosine(graph, before as u32, after));
            let main = roads.class(a) == 2 || roads.class(b) == 2;
            for count in by_ref.iter_mut().take(1 + usize::from(main)) {
                *count = (count.0 + 1, count.1 + usize::from(cost > 10.0), count.2 + cost);
            }
        }
    }
    let line = |(count, over, sum): (usize, usize, f64)| {
        format!(
            "{count} arcs, {over} of them with a turn cost over 10 m, {:.1} m for each arc",
            sum / count.max(1) as f64
        )
    };
    writeln!(
        text,
        "\nThe arcs where the rules differ: the graph has {arcs} arcs at a junction of three or more road ends (no \
         U-turn; each road of the graph, whatever its access). Rule (i) gives {keyed} of them no turn cost. The `ref` \
         is the only key of {}: rule (ii) charges them. A main road is on one side of {}: rule (iii) charges them.",
        line(by_ref[0]),
        line(by_ref[1]),
    )
    .unwrap();
    Ok(())
}

pub fn freiburg(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let clock = Instant::now();
    let specs = [
        Spec { seed: 3836, classes: panel::DISTANCES, inside: None },
        Spec { seed: 3837, classes: TOWN, inside: Some(FREIBURG_TOWN) },
    ];
    let census = Some((vias, step_m));
    let variants = [&KEYS[..], &TURNS[..], &CLUSTERS[..]].concat();
    let (rows, panels) = measure(graph, "touring", census, &specs, &variants, (out, "freiburg_"))?;

    let mut text = format!("{INTRO_A}\n## A.1 Freiburg (`touring`, box {bounds:?})\n");
    clusters(&mut text, graph);
    let picked = cluster_rows(&rows);
    let note = panel_note("touring", "the panel of `grid.md`");
    signal_table(&mut text, "### A.1 The panel", &note, &picked, 0, &panels[0], 1);
    let note = town_note(FREIBURG_TOWN, "Freiburg town, the town panel of `round3.md`");
    signal_table(&mut text, "### A.1 The town panel", &note, &picked, 1, &panels[1], 1);
    save(out, "A1_freiburg.md", &text)?;

    let mut text = format!(
        "{INTRO_B}What a route of a panel is, and the columns of each table B: {}\n\n## B.1 Freiburg (`touring`, box \
         {bounds:?})\n",
        grid::PANEL_LEGEND
    );
    key_arcs(&mut text, graph)?;
    grid::census_table(&mut text, "### B.1 A. The census", &rows[..6], "round5/splits_freiburg_<variant>.csv");
    let note = panel_note("touring", "the panel of `grid.md`");
    grid::panel_rows(&mut text, "### B.1 B. The panel", &note, &rows[..6], 0, &panels[0], true);
    writeln!(text, "\nThe `touring` rows of A.1 and B.1 took {:.0} s after the bake.", clock.elapsed().as_secs_f64())
        .unwrap();
    save(out, "B1_freiburg.md", &text)?;

    let clock = Instant::now();
    let mut text = INTRO_C.to_string();
    for (number, profile, census) in [(1, "gravel", census), (2, "mtb", None)] {
        let prefix = format!("freiburg_{profile}_");
        let (rows, panels) = measure(graph, profile, census, &specs[..1], &KEYS[..2], (out, &prefix))?;
        if census.is_some() {
            let lists = format!("round5/splits_freiburg_{profile}_<variant>.csv");
            grid::census_table(&mut text, &format!("## C.{number} A. `{profile}`: the census"), &rows, &lists);
            text.push_str(
                "\nThe census is that of `touring` with the costs of the profile: the same relations and points, on \
                 the roads that the profile may use.\n",
            );
        }
        let note = panel_note(profile, "the seed of the `touring` panel");
        grid::panel_rows(
            &mut text,
            &format!("## C.{number} B. `{profile}`: the panel"),
            &note,
            &rows,
            0,
            &panels[0],
            true,
        );
    }
    let (rows, panels) = measure(graph, "road", None, &specs, &ROAD, (out, "freiburg_road_"))?;
    text.push_str(
        "\n## C.3 `road`\n\n`road` has its own turn model, and today it pays 20 m for each signal node that the route \
         passes. `road-c60` pays 60 m by the cluster rule with R = 60 m in place of the 20 m. `road-e60` is not in \
         the brief: it is `road-c60` with the rule of `first-e60` (section A). The turn model of `road` does not \
         change. The census has no row: the split of the cost into terms does not know `road`.\n",
    );
    let note = panel_note("road", "the seed of the `touring` panel");
    grid::panel_rows(&mut text, "### C.3 B. `road`: the panel", &note, &rows, 0, &panels[0], true);
    let town = town_note(FREIBURG_TOWN, "Freiburg town, the seed of the town panel of `touring`");
    grid::panel_rows(&mut text, "### C.3 `road`: the town panel", &town, &rows, 1, &panels[1], false);
    let picked: Vec<&Row> = rows.iter().collect();
    let signals = "The measures of section A for the same routes.";
    signal_table(&mut text, "### C.3 `road`: the signals of the panel", signals, &picked, 0, &panels[0], 0);
    signal_table(&mut text, "### C.3 `road`: the signals of the town panel", signals, &picked, 1, &panels[1], 0);
    writeln!(text, "\nSection C took {:.0} s after the bake.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "C_freiburg.md", &text)
}

pub fn galway(graph: &Graph, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let clock = Instant::now();
    let specs = [
        Spec { seed: 353, classes: panel::DISTANCES, inside: None },
        Spec { seed: 91, classes: TOWN, inside: Some(GALWAY_TOWN) },
    ];
    let variants = [&KEYS[..2], &CLUSTERS[..]].concat();
    let (rows, panels) = measure(graph, "touring", None, &specs, &variants, (out, "galway_"))?;
    let mut text = format!("\n## A.2 Galway, the Burren and Ennis (`touring`, box {bounds:?})\n");
    clusters(&mut text, graph);
    let picked: Vec<&Row> = rows.iter().collect();
    let note = panel_note("touring", "the panel of `round4.md`");
    signal_table(&mut text, "### A.2 The panel", &note, &picked, 0, &panels[0], 1);
    let note = town_note(GALWAY_TOWN, "Galway town, the town panel of `round4.md`");
    signal_table(&mut text, "### A.2 The town panel", &note, &picked, 1, &panels[1], 1);
    writeln!(text, "\nThe Galway rows took {:.0} s after the bake. No census ran.", clock.elapsed().as_secs_f64())
        .unwrap();
    save(out, "A2_galway.md", &text)
}

pub fn engelberg(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let clock = Instant::now();
    let specs = [Spec { seed: 6390, classes: panel::DISTANCES, inside: None }];
    let census = Some((vias, step_m));
    let variants = [&KEYS[..], &TURNS[..], &CLUSTERS[..]].concat();
    let (rows, panels) = measure(graph, "touring", census, &specs, &variants, (out, "engelberg_"))?;
    let area = "the merged extract of Lucerne, Obwalden, Nidwalden, Uri and Schwyz";
    let mut text = format!("\n## A.3 Engelberg (`touring`, {area}, box {bounds:?})\n");
    clusters(&mut text, graph);
    let note = panel_note("touring", "the panel of `round4.md`");
    signal_table(&mut text, "### A.3 The panel", &note, &cluster_rows(&rows), 0, &panels[0], 1);
    save(out, "A3_engelberg.md", &text)?;

    let mut text = format!("\n## B.2 Engelberg (`touring`, {area}, box {bounds:?})\n");
    key_arcs(&mut text, graph)?;
    grid::census_table(&mut text, "### B.2 A. The census", &rows[..6], "round5/splits_engelberg_<variant>.csv");
    grid::panel_rows(&mut text, "### B.2 B. The panel", &note, &rows[..6], 0, &panels[0], true);
    writeln!(text, "\nThe Engelberg rows of A.3 and B.2 took {:.0} s after the bake.", clock.elapsed().as_secs_f64())
        .unwrap();
    save(out, "B2_engelberg.md", &text)
}

pub fn alsace(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let clock = Instant::now();
    let specs = [Spec { seed: 67, classes: panel::DISTANCES, inside: None }];
    let variants = [&KEYS[..], &TURNS[..]].concat();
    let (rows, panels) = measure(graph, "touring", Some((vias, step_m)), &specs, &variants, (out, "alsace_"))?;
    let mut text = format!(
        "\n## A.4 Alsace (box {bounds:?}): the clusters only\n\nNo row of a cluster rule ran in Alsace. The table \
         shows how large a cluster grows in a region with three cities.\n"
    );
    clusters(&mut text, graph);
    save(out, "A4_alsace.md", &text)?;

    let mut text = format!("\n## B.3 Alsace (`touring`, the Geofabrik extract, box {bounds:?})\n");
    key_arcs(&mut text, graph)?;
    grid::census_table(&mut text, "### B.3 A. The census", &rows, "round5/splits_alsace_<variant>.csv");
    let note = panel_note("touring", "the panel of `round3.md`");
    grid::panel_rows(&mut text, "### B.3 B. The panel", &note, &rows, 0, &panels[0], true);
    writeln!(text, "\nThe Alsace rows took {:.0} s after the bake.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "B3_alsace.md", &text)
}
