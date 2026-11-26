//! PROTOTYPE. Round four: sets of route pairs for a person to judge on a map, and the numbers of
//! two new regions. Each region is one run and writes its part of `<out>/round4/`;
//! `run.sh --round4` joins the parts into `pairs.json` and `round4.md`.
use super::grid::{self, Row, BASE};
use super::pairs::{self, Set};
use super::panel::{self, Spec};
use super::variant::{self, Variant};
use crate::base::Topology;
use crate::Graph;
use std::fmt::Write;
use std::path::Path;
use std::time::Instant;

pub const TODAY: Variant = Variant {
    path_as_cycleway: false,
    turn: 0,
    through: false,
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
/// V1 + V2T-100 + S60.
pub const FIRST: Variant = Variant { signal: 60, ..BASE };
/// `FIRST` + V4.
const SIGNED: Variant = Variant { full_bonus: true, ..FIRST };
const TRIO: [(&str, Variant); 3] = [("today", TODAY), ("first", FIRST), ("first+signed", SIGNED)];

const NAMES: &str =
    "`today` is V0. `first` is V1 + V2T-100 + S60 of `grid.md` and `round3.md`. `first+signed` is `first` + \
     V4. The terrain is GLO-30 only.";

const NORTH: [f64; 4] = [7.72, 47.97, 8.02, 48.24];
const ENGELBERG: [f64; 4] = [8.15, 46.72, 8.70, 47.08];
const GALWAY: [f64; 4] = [-9.12, 53.25, -8.95, 53.30];
const TOWN: &[(f64, f64, usize)] = &[(3.0, 8.0, 100)];

fn save(out: &Path, name: &str, text: &str) -> Result<(), String> {
    let folder = out.join("round4");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(name);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}

/// Draws the sets, writes them as `round4/<file>`, and appends their table to `text`.
fn draw(
    graph: &Graph,
    topology: &Topology,
    sets: &[Set],
    out: &Path,
    file: &str,
    text: &mut String,
) -> Result<(), String> {
    let clock = Instant::now();
    let (mut values, mut drawn) = (Vec::new(), Vec::new());
    for set in sets {
        let (value, counts) = pairs::draw(graph, topology, set)?;
        eprintln!("Set {}: {:.1} s", set.id, clock.elapsed().as_secs_f64());
        values.push(value);
        drawn.push(counts);
    }
    pairs::table(text, &drawn);
    writeln!(text, "\nThe draws took {:.0} s.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, file, &serde_json::to_string(&values).map_err(|e| e.to_string())?)
}

/// The relations and the signed length that the box has at all, from the census of the first row.
fn signed_network(text: &mut String, graph: &Graph, row: &Row) {
    let census = &row.census;
    let roads = &graph.roads;
    let (mut ranked, mut all) = (0.0, 0.0);
    for road in 0..roads.len() {
        let way = graph.osm.ways.row(roads.way_row(road) as usize);
        let km = f64::from(roads.length_m.values()[road]) / 2000.0;
        all += km;
        if way.facts.ranks[usize::from(roads.reversed(road))] != 0 {
            ranked += km;
        }
    }
    let short: f64 = census.excluded.iter().map(|level| level.1).sum();
    writeln!(
        text,
        "\nThe box has {} cycle route relations with a way inside (`route=bicycle`, not proposed), with {:.0} km of \
         main line; {} of them have 5 km or more ({:.0} km of line, {:.0} km measured). {:.0} km of line are in \
         relations under 5 km. {ranked:.0} km of the {all:.0} km of roads have a cycle route rank in the cost model \
         (half of the directed length; `osm::cycle_ranks` also ranks the ways of each relation that a cycle route \
         lists, whatever its tags).",
        census.relations_in_box,
        census.line_in_box_m / 1000.0,
        census.total.relations,
        census.total.line_m / 1000.0,
        census.total.measured_m / 1000.0,
        short / 1000.0,
    )
    .unwrap();
}

/// The census and the panels of `touring` for `today`, `first` and `first+signed`.
fn numbers(
    graph: &Graph,
    vias: &[i64],
    step_m: f64,
    specs: &[Spec],
    (out, prefix): (&Path, &str),
) -> Result<(Vec<Row>, Vec<grid::Panel>, Topology), String> {
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let panels = grid::panels(graph, &topology, "touring", specs)?;
    let rows = grid::measure(graph, &topology, vias, step_m, "touring", &TRIO, &panels, (&out.join("round4"), prefix))?;
    Ok((rows, panels, topology))
}

fn counts(graph: &Graph, topology: &Topology) -> String {
    let (nodes, junctions) = variant::signal_counts(graph);
    format!(
        "{} directed roads, {} turns, {} junctions; {nodes} signal nodes in {junctions} signalled junctions.",
        graph.roads.len(),
        topology.head.len(),
        graph.points.len(),
    )
}

pub fn freiburg(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let clock = Instant::now();
    let specs = [
        Spec { seed: 3836, classes: panel::DISTANCES, inside: None },
        Spec { seed: 3837, classes: TOWN, inside: Some([7.78, 47.97, 7.90, 48.03]) },
    ];
    let (rows, panels, topology) = numbers(graph, vias, step_m, &specs, (out, "freiburg_"))?;
    let area = "Freiburg – Emmendingen – Freiamt, box 7.72,47.97,8.02,48.24";
    let sets = [
        Set {
            id: "fr-touring-first",
            title: "Freiburg north, touring: today or first build",
            area,
            profile: "touring",
            variants: [TRIO[0], TRIO[1]],
            inside: NORTH,
            count: 40,
            seed: 41,
        },
        Set {
            id: "fr-touring-signed",
            title: "Freiburg north, touring: first build, or first build with the signed routes",
            area,
            profile: "touring",
            variants: [TRIO[1], TRIO[2]],
            inside: NORTH,
            count: 30,
            seed: 42,
        },
        Set {
            id: "fr-gravel-first",
            title: "Freiburg north, gravel: today or first build",
            area,
            profile: "gravel",
            variants: [TRIO[0], TRIO[1]],
            inside: NORTH,
            count: 20,
            seed: 43,
        },
    ];
    let mut text = format!(
        "\n# 1. The pairs for the map (`pairs.json`)\n\n{NAMES} Each set draws random pairs of junctions where a road \
         with the snap bit of the profile arrives, with a fixed seed, until it is full.\n\n## 1.1 Freiburg north \
         (box {NORTH:?})\n"
    );
    draw(graph, &topology, &sets, out, "pairs_freiburg.json", &mut text)?;
    save(out, "1_pairs_freiburg.md", &text)?;

    let mut text = format!(
        "\n# 4. Freiburg, for comparison (`touring`, box {bounds:?})\n\n{NAMES} {}\n",
        counts(graph, &topology)
    );
    signed_network(&mut text, graph, &rows[0]);
    grid::census_table(&mut text, "## 4 A. The census", &rows, "round4/splits_freiburg_<variant>.csv");
    grid::panel_table(&mut text, "## 4 B. The panel of `grid.md`", grid::PANEL_NOTE, &rows, 0, &panels[0], true);
    let town = "100 pairs of junctions inside the box 7.78,47.97,7.90,48.03 (Freiburg town), 3 to 8 km apart, a fixed \
                seed: the town panel of `round3.md`.";
    grid::panel_table(&mut text, "## 4 C. The town panel", town, &rows, 1, &panels[1], false);
    writeln!(text, "\nThe Freiburg run took {:.0} s after the bake.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "4_freiburg.md", &text)
}

/// The junctions of each cell of `inside`: rows from north to south, columns from west to east.
fn coverage(graph: &Graph, [west, south, east, north]: [f64; 4], (columns, rows): (usize, usize)) -> String {
    let mut cells = vec![vec![0usize; columns]; rows];
    for point in graph.points.values() {
        let (lon, lat) = (f64::from(point.lon) * 1e-6, f64::from(point.lat) * 1e-6);
        if lon < west || lon >= east || lat < south || lat >= north {
            continue;
        }
        let column = ((lon - west) / (east - west) * columns as f64) as usize;
        let row = ((north - lat) / (north - south) * rows as f64) as usize;
        cells[row.min(rows - 1)][column.min(columns - 1)] += 1;
    }
    let mut text = String::from("```\n");
    for (row, counts) in cells.iter().enumerate() {
        let line: String = counts
            .iter()
            .map(|&count| match count {
                0 => " .",
                1..=49 => " o",
                _ => " #",
            })
            .collect();
        let top = north - (north - south) * row as f64 / rows as f64;
        writeln!(text, "{top:6.2}{line}").unwrap();
    }
    writeln!(text, "      {west:<.2} … {east:.2}\n```").unwrap();
    text
}

pub fn engelberg(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let clock = Instant::now();
    let specs = [Spec { seed: 6390, classes: panel::DISTANCES, inside: None }];
    let (rows, panels, topology) = numbers(graph, vias, step_m, &specs, (out, "engelberg_"))?;
    let area = "Engelberg and central Switzerland, box 8.15,46.72,8.70,47.08";
    let sets = [
        Set {
            id: "eb-touring-first",
            title: "Engelberg, touring: today or first build",
            area,
            profile: "touring",
            variants: [TRIO[0], TRIO[1]],
            inside: ENGELBERG,
            count: 30,
            seed: 44,
        },
        Set {
            id: "eb-touring-signed",
            title: "Engelberg, touring: first build, or first build with the signed routes",
            area,
            profile: "touring",
            variants: [TRIO[1], TRIO[2]],
            inside: ENGELBERG,
            count: 20,
            seed: 45,
        },
    ];
    let mut text = format!("\n## 1.2 Engelberg (box {ENGELBERG:?})\n");
    draw(graph, &topology, &sets, out, "pairs_engelberg.json", &mut text)?;
    write!(
        text,
        "\nThe roads of the box, from the canton extracts of Lucerne, Obwalden, Nidwalden, Uri and Schwyz. Each cell is \
         0.025° of longitude by 0.02° of latitude (about 1.9 by 2.2 km), from north to south and from west to east: \
         `#` 50 junctions or more, `o` fewer, `.` none. The label of a row is its north edge.\n\n{}",
        coverage(graph, ENGELBERG, (22, 18))
    )
    .unwrap();
    save(out, "1_pairs_engelberg.md", &text)?;

    let mut text = format!(
        "\n# 2. Engelberg numbers (`touring`, the merged extract of Lucerne, Obwalden, Nidwalden, Uri and Schwyz, box \
         {bounds:?})\n\n{NAMES} A way ends at the border of the five cantons. {}\n",
        counts(graph, &topology)
    );
    signed_network(&mut text, graph, &rows[0]);
    grid::census_table(&mut text, "## 2 A. The census", &rows, "round4/splits_engelberg_<variant>.csv");
    let note = "300 pairs of junctions of the whole merged extract where a road with the snap bit of `touring` arrives, \
                the same for each variant: 100 each at 5, 15 and 40 km straight-line distance (within 10 %), a new seed.";
    grid::panel_table(&mut text, "## 2 B. The panel", note, &rows, 0, &panels[0], true);
    writeln!(text, "\nThe Engelberg run took {:.0} s after the bake.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "2_engelberg.md", &text)
}

/// The km of the roads that `touring` may use, by road class and tagged speed limit: half of the
/// directed length, so a one-way road counts half.
fn limits(graph: &Graph) -> String {
    const CLASSES: [&str; 5] = [
        "0 cycleway",
        "1 quiet road (residential, unclassified, tertiary, service)",
        "2 main road (secondary, primary, trunk)",
        "3 track",
        "4 path",
    ];
    let mut km = [[0.0f64; 6]; 5];
    let roads = &graph.roads;
    for road in 0..roads.len() {
        let class = usize::from(roads.class(road));
        if class >= CLASSES.len() {
            continue;
        }
        let way = graph.osm.ways.row(roads.way_row(road) as usize);
        let bucket = match variant::speed_limit(&way.tags, roads.reversed(road)) {
            None => 0,
            Some(speed) if speed <= 30.0 => 1,
            Some(speed) if speed <= 50.0 => 2,
            Some(speed) if speed < 80.0 => 3,
            Some(speed) if speed < 100.0 => 4,
            Some(_) => 5,
        };
        km[class][bucket] += f64::from(roads.length_m.values()[road]) / 2000.0;
    }
    let mut text = String::from(
        "\n| road class | no tagged limit | 30 or less | 31 to 50 | 51 to 79 | 80 to 99 | 100 or more |\n\
         |---|---|---|---|---|---|---|\n",
    );
    for (name, row) in CLASSES.iter().zip(km) {
        let cells: Vec<String> = row.iter().map(|km| format!("{km:.0}")).collect();
        writeln!(text, "| {name} | {} |", cells.join(" | ")).unwrap();
    }
    text
}

pub fn galway(graph: &Graph, vias: &[i64], step_m: f64, bounds: [f64; 4], out: &Path) -> Result<(), String> {
    let clock = Instant::now();
    let specs = [
        Spec { seed: 353, classes: panel::DISTANCES, inside: None },
        Spec { seed: 91, classes: TOWN, inside: Some(GALWAY) },
    ];
    let (rows, panels, topology) = numbers(graph, vias, step_m, &specs, (out, "galway_"))?;
    let mut text = format!(
        "\n# 3. Ireland numbers (`touring`, Galway, the Burren and Ennis, box {bounds:?})\n\n{NAMES} Ireland drives on \
         the left; the ways have the access defaults of Ireland. {}\n",
        counts(graph, &topology)
    );
    signed_network(&mut text, graph, &rows[0]);
    grid::census_table(&mut text, "## 3 A. The census", &rows, "round4/splits_galway_<variant>.csv");
    let note =
        "300 pairs of junctions of the box where a road with the snap bit of `touring` arrives, the same for each \
                variant: 100 each at 5, 15 and 40 km straight-line distance (within 10 %), a new seed.";
    grid::panel_table(&mut text, "## 3 B. The panel", note, &rows, 0, &panels[0], true);
    let town = format!(
        "100 pairs of junctions inside the box {GALWAY:?} (Galway town), 3 to 8 km apart in a straight line, a fixed \
         seed. A route may leave the town box."
    );
    grid::panel_table(&mut text, "## 3 C. The town panel of Galway", &town, &rows, 1, &panels[1], false);
    write!(
        text,
        "\n## 3 D. The roads of the box by class and tagged speed limit, in km\n\nThe limit is `maxspeed` (or \
         `maxspeed:forward|backward`) as a number; a way with only `maxspeed:type` or a zone code has no tagged \
         limit.\n{}",
        limits(graph)
    )
    .unwrap();
    writeln!(text, "\nThe Ireland run took {:.0} s after the bake.", clock.elapsed().as_secs_f64()).unwrap();
    save(out, "3_ireland.md", &text)
}
