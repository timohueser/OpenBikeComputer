//! PROTOTYPE. Round three: a signal cost, a second region, a baseline for hiking, and the cause of
//! the slower production query. Each part is one file of `<out>/round3/`; `run.sh --round3` joins
//! them into `round3.md`.
use super::grid::{self, mean, percent, quantile, Row, BASE};
use super::panel::{self, Spec};
use super::report;
use super::variant::{self, Variant};
use crate::base::Topology;
use crate::Graph;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::time::Instant;

/// Freiburg town: west, south, east, north.
const TOWN: [f64; 4] = [7.78, 47.97, 7.90, 48.03];

fn signal(signal: u32, on: Variant) -> Variant {
    Variant { signal, ..on }
}

const SIGNAL_NOTE: &str = "The signal cost S is a stand-in, not a proposal: `touring` pays S on the arc that passes a \
     signal node (`graph.signals`), but not where the arriving road is shorter than 40 m and starts at a signal \
     node too. The base change is V1+V2T-100 of `grid.md`.";

fn save(out: &Path, name: &str, text: &str) -> Result<(), String> {
    let folder = out.join("round3");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(name);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}

fn signals(text: &mut String, graph: &Graph) {
    let (nodes, junctions) = variant::signal_counts(graph);
    writeln!(
        text,
        "\nThe box has {nodes} signal nodes on roads of the graph (`highway=traffic_signals`, or a crossing with \
         `crossing=traffic_signals`). They form {junctions} signalled junctions when the signal nodes that a road \
         shorter than 40 m joins count as one."
    )
    .unwrap();
}

pub fn freiburg(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let lists = out.join("round3");
    let v0 = Variant::default();

    // 1. The signal cost.
    let clock = Instant::now();
    let specs = [
        Spec { seed: 3836, classes: panel::DISTANCES, inside: None },
        Spec { seed: 3837, classes: &[(3.0, 8.0, 100)], inside: Some(TOWN) },
    ];
    let panels = grid::panels(graph, &topology, "touring", &specs)?;
    let variants = [
        ("V0", v0),
        ("base", BASE),
        ("base+S60", signal(60, BASE)),
        ("base+S120", signal(120, BASE)),
        ("V0+S60", signal(60, v0)),
    ];
    let rows = grid::measure(graph, &topology, vias, step_m, "touring", &variants, &panels, (&lists, "freiburg_"))?;
    let mut text = format!("\n# 1. Signal cost (`touring`, Freiburg box {bounds:?})\n\n{SIGNAL_NOTE}\n");
    signals(&mut text, graph);
    grid::census_table(&mut text, "## 1 A. The census", &rows, "round3/splits_freiburg_<variant>.csv");
    grid::panel_table(&mut text, "## 1 B. The panel of `grid.md`", grid::PANEL_NOTE, &rows, 0, &panels[0], true);
    let town = format!(
        "100 pairs of junctions inside the box {TOWN:?} (Freiburg town), 3 to 8 km apart in a straight line, a fixed \
         seed, the same for each variant. A route may leave the town box."
    );
    grid::panel_table(&mut text, "## 1 C. The town panel", &town, &rows, 1, &panels[1], false);
    writeln!(text, "\nSection 1 took {:.0} s.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "1_signal.md", &text)?;

    // 4. The production query, with the counters of the search.
    let clock = Instant::now();
    let packages =
        grid::touring_packages(graph, bounds, &[("V0", v0), ("base", BASE), ("base+S60", signal(60, BASE))])?;
    let races = grid::race(graph, &panels[0].pairs, packages)?;
    let mut text = String::from(
        "\n# 4. Where the time of the production query goes (`touring`, Freiburg)\n\n\
         `Router::route` on a package of `touring` alone in memory, for the 300 pairs of the panel of `grid.md`, one \
         thread: one pass that fills the caches, then three passes where the routers take turns; each query keeps \
         its fastest time. The other columns are from the first measured pass, from counters that the prototype \
         adds to `search::Workspace::run_with_potential`: the time inside the search, the labels that it takes \
         from its queues (`settled`), the labels that it sets (each is one queue entry), and the stale queue \
         entries that it drops. `outside` is the mean time less the time in the search: snap, landmark potential \
         and answer.\n\n\
         | variant | mean ms | p95 ms | in the search ms | outside ms | settled | labels set | stale entries dropped | \
         labels set for each settled | ns in the search for each settled |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for race in &races {
        let [search, settled, pushed, stale] = [0, 1, 2, 3].map(|i| mean(&race.counters[i]));
        writeln!(
            text,
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.0} | {:.0} | {:.0} | {:.2} | {:.0} |",
            race.name,
            mean(&race.millis),
            quantile(&race.millis, 0.95),
            search,
            mean(&race.millis) - search,
            settled,
            pushed,
            stale,
            pushed / settled,
            1e6 * search / settled,
        )
        .unwrap();
    }
    writeln!(
        text,
        "\nShortcut: the time in the search is of one pass, the mean time is the fastest of three, so `outside` can \
         be a little low. Section 4 took {:.0} s.",
        clock.elapsed().as_secs_f64()
    )
    .unwrap();
    save(out, "4_query.md", &text)?;

    // 3. The hiking baseline.
    let clock = Instant::now();
    let both = Variant { walk_bonus: true, walk_turn: 100, ..v0 };
    let variants = [
        ("V0", v0),
        ("(a)", Variant { walk_bonus: true, ..v0 }),
        ("(b)", Variant { walk_turn: 100, ..v0 }),
        ("(a)+(b)", both),
    ];
    let rows = grid::measure(graph, &topology, vias, step_m, "hiking", &variants, &[], (&lists, "hiking_"))?;
    let mut text = String::from(
        "\n# 3. Hiking baseline (`hiking`, Freiburg)\n\n\
         The census of `census.md` for the profile `hiking` on the relations with `route=hiking|foot`, \
         `type=route|superroute`, not `state=proposed`, and at least 5 km of main line in the box. `hiking` has no \
         route bonus today. The level is the highest network of the relation or of a route above it (iwn, nwn, rwn, \
         lwn). Stand-ins, not proposals: (a) a way that is a member of such a relation has no class preference; \
         (b) a turn cost of 100 × (1 − cos of the change of heading) at a junction of three or more road ends, but \
         none onto a road with the same `name`, the same `ref` or one same hiking relation.\n",
    );
    grid::census_table(&mut text, "## 3 A. The census", &rows, "round3/splits_hiking_<variant>.csv");
    baseline(&mut text, &rows[0]);
    writeln!(text, "\nSection 3 took {:.0} s.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "3_hiking.md", &text)
}

/// The ranking of the largest cost term and the most avoided signatures of the first row.
fn baseline(text: &mut String, row: &Row) {
    let census = &row.census;
    let total = &census.total;
    writeln!(
        text,
        "\nV0: {} relations with the tags, {} with less than 5 km. {} km of line; {} km with no road that `hiking` \
         may use, {} km in chains under 1 km, {} km measured in {} legs. `hiking` follows {} within 30 m and {} on \
         the same directed road; {} splits ({} with repeats) cover {} of the measured length.",
        census.relations_seen,
        census.relations_under_5km,
        report::km(total.line_m),
        report::km(total.lost_m),
        report::km(total.short_m),
        report::km(total.measured_m),
        total.legs,
        percent(total.near_m, total.measured_m),
        percent(total.same_road_m, total.measured_m),
        census.splits.len(),
        total.splits,
        percent(total.split_m, total.measured_m),
    )
    .unwrap();
    let splits: Vec<_> = census.splits.iter().collect();
    let title = "## 3 B. The largest cost term of each split (V0)\n\nThe term with the largest part of (cost of the \
                 signed stretch − cost of the router's stretch). The terms are those of `census.md`; for `hiking` the \
                 difficulty is the SAC scale and the trail visibility, and the climb is 4 per metre of ascent.";
    report::ranking(text, title, &splits, report::top_term);
    let mut signatures = BTreeMap::<&str, (f64, f64, usize, f64)>::new();
    for split in &splits {
        for (name, metres) in &split.signed.signature {
            let entry = signatures.entry(name.as_str()).or_default();
            *entry = (entry.0 + metres, entry.1 + metres * split.seen as f64, entry.2 + 1, entry.3);
        }
        for (name, metres) in &split.router.signature {
            signatures.entry(name.as_str()).or_default().3 += metres;
        }
    }
    let mut ranked: Vec<_> = signatures.into_iter().collect();
    ranked.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
    text.push_str(
        "\n## 3 C. The 10 tag signatures with the most signed length that `hiking` avoided (V0)\n\n\
         Signature: `highway|surface|foot|tracktype|sac_scale`. `avoided share`: the length in splits, with repeats, \
         as a share of the measured signed length with this signature.\n\n\
         | signature | signed km in splits | splits with it | km on the router's stretches | measured signed km | \
         avoided share |\n|---|---|---|---|---|---|\n",
    );
    for (name, (metres, repeats, with, router)) in ranked.iter().take(10) {
        let measured = census.measured_signature.get(*name).copied().unwrap_or(0.0);
        writeln!(
            text,
            "| `{name}` | {} | {with} | {} | {} | {} |",
            report::km(*metres),
            report::km(*router),
            report::km(measured),
            percent(*repeats, measured),
        )
        .unwrap();
    }
}

pub fn alsace(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let clock = Instant::now();
    let panels =
        grid::panels(graph, &topology, "touring", &[Spec { seed: 67, classes: panel::DISTANCES, inside: None }])?;
    let variants = [("V0", Variant::default()), ("base", BASE), ("base+S60", signal(60, BASE))];
    let lists = out.join("round3");
    let rows = grid::measure(graph, &topology, vias, step_m, "touring", &variants, &panels, (&lists, "alsace_"))?;
    let mut text = format!(
        "\n# 2. A region that no choice has seen (`touring`, Alsace, box {bounds:?})\n\n\
         The whole Geofabrik extract of Alsace, with the access defaults of France and GLO-30 terrain. No number of \
         the candidates was chosen with this region. {SIGNAL_NOTE}\n\n\
         {} directed roads, {} turns, {} junctions.\n",
        graph.roads.len(),
        topology.head.len(),
        graph.points.len(),
    );
    signals(&mut text, graph);
    grid::census_table(&mut text, "## 2 A. The census", &rows, "round3/splits_alsace_<variant>.csv");
    let note =
        "300 pairs of junctions where a road with the snap bit of `touring` arrives, the same for each variant: \
                100 each at 5, 15 and 40 km straight-line distance (within 10 %), SplitMix64 with a new seed.";
    grid::panel_table(&mut text, "## 2 B. The panel", note, &rows, 0, &panels[0], true);
    writeln!(text, "\nSection 2 took {:.0} s after the bake.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "2_alsace.md", &text)
}
