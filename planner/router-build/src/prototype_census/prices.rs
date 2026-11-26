//! PROTOTYPE. The price list: the cost factor of 1 km of flat road for a list of tag sets, from
//! the scan of the import and `cost::way_basis`. The ways lie near Freiburg, so they get the
//! access defaults of Germany.
use super::terms;
use crate::column::Column;
use crate::osm::{self, Counts};
use crate::source::{Builder, Data, Id, NodeRef, Relations, Sets, Tags, Ways};
use crate::{cost, profiles};
use planner_router::model::{Road, BIKE, FOOT, PUSH};
use std::fmt::Write;

const ROWS: &[&[(&str, &str)]] = &[
    &[("highway", "cycleway"), ("surface", "asphalt")],
    &[("highway", "cycleway")],
    &[("highway", "cycleway"), ("surface", "fine_gravel")],
    &[("highway", "path"), ("bicycle", "designated"), ("foot", "designated"), ("surface", "asphalt")],
    &[("highway", "path"), ("bicycle", "designated"), ("foot", "designated"), ("surface", "fine_gravel")],
    &[("highway", "path"), ("bicycle", "designated"), ("foot", "designated")],
    &[
        ("highway", "path"),
        ("bicycle", "designated"),
        ("foot", "designated"),
        ("segregated", "yes"),
        ("surface", "asphalt"),
    ],
    &[("highway", "path"), ("bicycle", "yes"), ("surface", "asphalt")],
    &[("highway", "path")],
    &[("highway", "path"), ("surface", "ground")],
    &[("highway", "footway"), ("bicycle", "yes"), ("surface", "asphalt")],
    &[("highway", "footway"), ("surface", "asphalt")],
    &[("highway", "residential"), ("surface", "asphalt")],
    &[("highway", "residential")],
    &[("highway", "residential"), ("surface", "asphalt"), ("maxspeed", "30")],
    &[("highway", "residential"), ("surface", "asphalt"), ("bicycle_road", "yes")],
    &[("highway", "living_street")],
    &[("highway", "service")],
    &[("highway", "service"), ("surface", "asphalt")],
    &[("highway", "unclassified"), ("surface", "asphalt")],
    &[("highway", "unclassified"), ("surface", "asphalt"), ("maxspeed", "100")],
    &[("highway", "unclassified")],
    &[("highway", "tertiary"), ("surface", "asphalt"), ("maxspeed", "50")],
    &[("highway", "tertiary"), ("surface", "asphalt"), ("maxspeed", "100")],
    &[("highway", "tertiary"), ("surface", "asphalt")],
    &[("highway", "tertiary")],
    &[("highway", "secondary"), ("surface", "asphalt"), ("maxspeed", "50")],
    &[("highway", "secondary"), ("surface", "asphalt"), ("maxspeed", "100")],
    &[("highway", "secondary"), ("surface", "asphalt"), ("maxspeed", "100"), ("cycleway:right", "track")],
    &[("highway", "secondary"), ("maxspeed", "50")],
    &[("highway", "primary"), ("surface", "asphalt"), ("maxspeed", "100")],
    &[("highway", "track"), ("tracktype", "grade1")],
    &[("highway", "track"), ("surface", "asphalt")],
    &[("highway", "track"), ("tracktype", "grade2")],
    &[("highway", "track"), ("tracktype", "grade2"), ("surface", "compacted")],
    &[("highway", "track"), ("tracktype", "grade3")],
    &[("highway", "track"), ("tracktype", "grade4")],
    &[("highway", "track")],
    &[("highway", "track"), ("surface", "gravel")],
    &[("highway", "pedestrian")],
    &[("highway", "pedestrian"), ("bicycle", "yes"), ("surface", "paving_stones")],
];

const NAMES: [&str; 5] = ["touring", "gravel", "road", "mtb", "hiking"];

/// Way `1000 × variant + row + 1` has the tags of row `row`. The ways of variant 1 are in a
/// regional cycle route, those of variant 2 in a national one.
fn data() -> Data {
    let mut ways = Builder::<Ways>::default();
    let mut points = Column::default();
    let mut members = [Vec::new(), Vec::new()];
    for variant in 0..3i64 {
        for (row, tags) in ROWS.iter().enumerate() {
            let id = 1000 * variant + row as i64 + 1;
            ways.push(id, &[2 * id, 2 * id + 1], &Tags::from_pairs(tags.iter().copied()));
            // 1 km apart at 48° north.
            for lon in [7_850_000, 7_863_430] {
                points.push(NodeRef {
                    lat: 48_000_000,
                    lon,
                    closures: u32::MAX,
                    access: BIKE | FOOT | PUSH,
                    flags: NodeRef::PRESENT,
                    padding: [0; 2],
                });
            }
            if variant > 0 {
                members[variant as usize - 1].push((Id::Way(id), ""));
            }
        }
    }
    let mut ways = ways.finish().0;
    ways.set_points(points);
    let mut relations = Builder::<Relations>::default();
    for (id, network) in [(1, "rcn"), (2, "ncn")] {
        let tags = Tags::from_pairs([("type", "route"), ("route", "bicycle"), ("network", network)]);
        relations.push(id, &tags, members[id as usize - 1].iter().copied());
    }
    let data = Data { ways, relations: relations.finish().0, members: Column::default() };
    osm::scan(data, Sets::default(), Counts::new()).osm
}

pub fn table() -> Result<String, String> {
    let data = data();
    let profiles = profiles(&NAMES)?;
    let profile = |name: &str| profiles.iter().find(|p| p.name == name).expect("a preset");
    let factor = |id: i64, name: &str| {
        let way = data.ways.get(id).expect("a way of the list");
        cost::way_basis(&way, false, profile(name))
            .map_or("excluded".to_string(), |basis| format!("{:.3}", basis.factor))
    };
    let mut out = String::from(
        "# Price list: the cost factor of 1 km of flat road\n\n\
         The factor is `CostBasis.factor` of the real cost code (`cost::way_basis`), for the direction of the way, \
         on a way near Freiburg (access defaults of Germany). The cost of a road is its length times the factor, \
         plus its climb. No way has a signed route, except in the columns `touring rcn` and `touring ncn`.\n\n\
         The four last columns split the `touring` factor: surface × smoothness/tracktype × (1 + 0.6 × traffic \
         stress) × class preference. A `push` way has the factor 4 in place of the traffic stress.\n\n\
         | tags | class | mode | touring | touring rcn | touring ncn | gravel | road | mtb | hiking | surface | \
         smooth/track | stress | class pref |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for (row, tags) in ROWS.iter().enumerate() {
        let id = row as i64 + 1;
        let way = data.ways.get(id).expect("a way of the list");
        let facts = way.facts;
        let label: Vec<String> = tags.iter().map(|(key, value)| format!("{key}={value}")).collect();
        let mode = match facts.access[0] {
            access if access & BIKE != 0 => "ride",
            access if access & PUSH != 0 => "push",
            _ => "none",
        };
        let road = Road {
            way: id,
            surface: facts.surface(),
            class: facts.class,
            access: facts.access[0],
            difficulty: facts.difficulty,
            hiking_difficulty: facts.hiking_difficulty(),
            structure: facts.structure(),
            ..Road::default()
        };
        let split = match terms::factors(&road, &way.tags, facts.country(), profile("touring")) {
            Some(f) if facts.routable() && facts.access[0] != 0 => {
                format!("{:.2} | {:.2} | {:.3} | {:.3}", f.surface, f.rolling, f.stress * f.push, f.class)
            }
            _ => "- | - | - | -".into(),
        };
        writeln!(
            out,
            "| `{}` | {} | {mode} | {} | {} | {} | {} | {} | {} | {} | {split} |",
            label.join(" "),
            facts.class,
            factor(id, "touring"),
            factor(id + 1000, "touring"),
            factor(id + 2000, "touring"),
            factor(id, "gravel"),
            factor(id, "road"),
            factor(id, "mtb"),
            factor(id, "hiking"),
        )
        .expect("a string");
    }
    out.push_str(
        "\nClass: 0 cycleway, 1 quiet road, 2 main road, 3 track, 4 path. The `touring` class preferences are \
         1.0, 1.15, 3.0, 1.2, 2.0. The route bonus takes 10 % per level (rcn 3 levels, ncn 4) from the class \
         preference only, so a way of class 0 gets no discount.\n",
    );
    Ok(out)
}
