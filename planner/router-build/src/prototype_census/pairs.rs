//! PROTOTYPE. Sets of route pairs for a person to judge on a map: the same start and end, the
//! route of two variants, and only pairs whose routes differ.
use super::census::Net;
use super::grid;
use super::panel::{self, Measures};
use super::variant::Variant;
use crate::base::Topology;
use crate::Graph;
use planner_router::search::Workspace;
use planner_router::shape::{self, Near};
use serde_json::{json, Value};

/// The straight-line distances of a set: a third of the pairs in each, within 10 % of 6, 12 and
/// 22 km.
const CLASSES: [(f64, f64); 3] = [(5.4, 6.6), (10.8, 13.2), (19.8, 24.2)];
/// Two routes differ when more than this share of the length of one is farther than 30 m from
/// the other.
const DIFFERS: f64 = 0.10;

pub struct Set {
    pub id: &'static str,
    pub title: &'static str,
    pub area: &'static str,
    pub profile: &'static str,
    pub variants: [(&'static str, Variant); 2],
    /// West, south, east, north: both ends of a pair lie inside.
    pub inside: [f64; 4],
    pub count: usize,
    pub seed: u64,
}

/// What the draw of a set did, for each distance class: the pairs with a route in both variants,
/// and those of them with the same route, which the set leaves out.
pub struct Drawn {
    pub id: &'static str,
    pub drawn: [usize; 3],
    pub same: [usize; 3],
    pub kept: [usize; 3],
}

fn rounded(value: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (value * scale).round() / scale
}

fn route(measures: &Measures, walking: bool) -> Value {
    let line: Vec<[f64; 2]> = measures
        .line
        .iter()
        .map(|p| [rounded(f64::from(p[1]) * 1e-6, 5), rounded(f64::from(p[0]) * 1e-6, 5)])
        .collect();
    let share = |metres: f64| rounded(100.0 * metres / measures.length_m.max(1.0), 1);
    let signed = if walking { "% on signed hiking routes" } else { "% on signed cycle routes" };
    let mut facts = serde_json::Map::new();
    facts.insert("turns".into(), json!(measures.turns));
    facts.insert("signalled junctions".into(), json!(measures.signalled));
    facts.insert("km on slow main roads".into(), json!(rounded(measures.slow_main_m / 1000.0, 1)));
    facts.insert("km on fast main roads".into(), json!(rounded(measures.fast_main_m / 1000.0, 1)));
    facts.insert(signed.into(), json!(share(measures.signed_m)));
    facts.insert("% not paved".into(), json!(share(measures.surface_m[2..].iter().sum())));
    facts.insert("% unknown surface".into(), json!(share(measures.surface_m[0])));
    facts.insert("leave and rejoin".into(), json!(measures.events));
    json!({
        "line": line,
        "km": rounded(measures.length_m / 1000.0, 2),
        "up_m": measures.ascent_m.round(),
        "facts": facts,
    })
}

/// Draws random pairs of junctions inside the box of `set`, with its seed, until each distance
/// class has its third of the pairs whose two routes differ.
pub fn draw(graph: &Graph, topology: &Topology, set: &Set) -> Result<(Value, Drawn), String> {
    let arcs = (topology.first.values(), topology.head.values());
    let profile = &crate::profiles(&[set.profile])?[0];
    let today = grid::bake(graph, topology, set.profile, Variant::default())?;
    let costs = [
        grid::bake(graph, topology, set.profile, set.variants[0].1)?,
        grid::bake(graph, topology, set.profile, set.variants[1].1)?,
    ];
    super::variant::set(Variant::default());
    let nets = costs
        .each_ref()
        .map(|costs| Net::new(graph, profile, arcs, (costs.roads[0].totals.values(), costs.roads[0].turns.values())));
    let junctions = panel::junctions(&nets[0], today.snap[0].values(), Some(set.inside))?;
    let points = graph.points.values();
    let place = |junction: u32| {
        let point = points[junction as usize];
        [rounded(f64::from(point.lat) * 1e-6, 5), rounded(f64::from(point.lon) * 1e-6, 5)]
    };
    let (mut state, mut workspace, mut pairs) = (set.seed, Workspace::default(), Vec::new());
    let mut drawn = Drawn { id: set.id, drawn: [0; 3], same: [0; 3], kept: [0; 3] };
    for (class, &(min_km, max_km)) in CLASSES.iter().enumerate() {
        let quota = set.count / 3 + usize::from(class < set.count % 3);
        let mut tries = 0u64;
        while drawn.kept[class] < quota {
            tries += 1;
            if tries > 50_000_000 {
                return Err(format!("{}: no pair from {min_km} to {max_km} km", set.id));
            }
            let mut pick = || junctions[(panel::random(&mut state) % junctions.len() as u64) as usize];
            let (from, to) = (pick(), pick());
            let apart = points[from as usize].distance(points[to as usize]) / 1000.0;
            if apart < min_km || apart > max_km {
                continue;
            }
            let mut found = Vec::new();
            for net in &nets {
                if let Some((path, _)) = net.route_in(&mut workspace, from, to)? {
                    found.push(panel::measure(net, &path[1..]));
                }
            }
            let [a, b] = found.as_slice() else { continue };
            drawn.drawn[class] += 1;
            let far = |one: &Measures, other: &Measures| {
                shape::far(&one.line, &Near::new(&other.line)) > DIFFERS * one.length_m
            };
            if !far(a, b) && !far(b, a) {
                drawn.same[class] += 1;
                continue;
            }
            drawn.kept[class] += 1;
            let mut routes = serde_json::Map::new();
            routes.insert(set.variants[0].0.into(), route(a, profile.walking));
            routes.insert(set.variants[1].0.into(), route(b, profile.walking));
            pairs.push(json!({
                "id": format!("{}-{:02}", set.id, pairs.len() + 1),
                "from": place(from),
                "to": place(to),
                "routes": routes,
            }));
        }
    }
    let value = json!({
        "id": set.id,
        "title": set.title,
        "area": set.area,
        "profile": set.profile,
        "variants": [set.variants[0].0, set.variants[1].0],
        "pairs": pairs,
    });
    Ok((value, drawn))
}

/// One table row for each set: the pairs drawn, kept and left out.
pub fn table(text: &mut String, sets: &[Drawn]) {
    text.push_str(
        "\nA drawn pair has both ends in the box, a straight-line distance within 10 % of 6, 12 or 22 km, and a route \
         with both variants. It enters the set when more than 10 % of the length of one route is farther than 30 m \
         from the other route; else the routes count as the same and the pair is left out.\n\n\
         | set | pairs in the set (6, 12, 22 km) | drawn | left out: same route | share left out | at 6 km | at 12 km | \
         at 22 km |\n|---|---|---|---|---|---|---|---|\n",
    );
    for set in sets {
        let (drawn, same, kept): (usize, usize, usize) =
            (set.drawn.iter().sum(), set.same.iter().sum(), set.kept.iter().sum());
        let by_class: Vec<String> = (0..3).map(|c| grid::percent(set.same[c] as f64, set.drawn[c] as f64)).collect();
        text.push_str(&format!(
            "| `{}` | {kept} ({}, {}, {}) | {drawn} | {same} | {} | {} |\n",
            set.id,
            set.kept[0],
            set.kept[1],
            set.kept[2],
            grid::percent(same as f64, drawn as f64),
            by_class.join(" | "),
        ));
    }
}
