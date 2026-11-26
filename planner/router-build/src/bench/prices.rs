//! The price list of the route bench: the cost factor of typical roads for each profile.
use crate::column::Column;
use crate::cost;
use crate::osm::{self, Counts};
use crate::source::{Builder, Data, Id, NodeRef, Relations, Sets, Tags, Ways};
use planner_router::model::Profile;

/// The tags of each road of the list.
const ROADS: &[&str] = &[
    "highway=cycleway surface=asphalt",
    "highway=cycleway",
    "highway=cycleway surface=fine_gravel",
    "highway=path bicycle=designated foot=designated surface=asphalt",
    "highway=path bicycle=designated foot=designated surface=fine_gravel",
    "highway=path bicycle=designated foot=designated",
    "highway=path bicycle=yes surface=asphalt",
    "highway=path",
    "highway=path surface=ground",
    "highway=footway bicycle=yes surface=asphalt",
    "highway=footway surface=asphalt",
    "highway=pedestrian",
    "highway=living_street",
    "highway=service",
    "highway=residential surface=asphalt",
    "highway=residential",
    "highway=residential surface=asphalt maxspeed=30",
    "highway=residential surface=asphalt bicycle_road=yes",
    "highway=unclassified surface=asphalt",
    "highway=unclassified surface=asphalt maxspeed=100",
    "highway=unclassified",
    "highway=tertiary surface=asphalt maxspeed=50",
    "highway=tertiary surface=asphalt maxspeed=100",
    "highway=tertiary surface=asphalt",
    "highway=secondary surface=asphalt maxspeed=50",
    "highway=secondary surface=asphalt maxspeed=100",
    "highway=secondary surface=asphalt maxspeed=100 cycleway:right=track",
    "highway=primary surface=asphalt maxspeed=100",
    "highway=track tracktype=grade1",
    "highway=track surface=asphalt",
    "highway=track tracktype=grade2",
    "highway=track tracktype=grade2 surface=compacted",
    "highway=track tracktype=grade3",
    "highway=track tracktype=grade4",
    "highway=track",
    "highway=track surface=gravel",
];

/// A road of the list is in no cycle route, in a regional one and in a national one.
const NETWORKS: [&str; 3] = ["", "rcn", "ncn"];

pub struct Price {
    pub tags: &'static str,
    /// For each profile, its name and the factor of `cost::way_basis` in the direction of the
    /// way: the cost of 1 m of flat road, `None` where the profile excludes the road. A cycling
    /// profile also has the factor on a route of each network, as `<profile> <network>`.
    pub factors: Vec<(String, Option<f64>)>,
}

pub fn price_list(profiles: &[Profile]) -> Vec<Price> {
    // Way `1000 × network + row` has the tags of row `row`.
    let id = |network: usize, row: usize| (1000 * network + row) as i64;
    let (mut ways, mut relations) = (Builder::<Ways>::default(), Builder::<Relations>::default());
    for (network, name) in NETWORKS.into_iter().enumerate() {
        for (row, tags) in ROADS.iter().enumerate() {
            let tags = Tags::from_pairs(tags.split(' ').filter_map(|tag| tag.split_once('=')));
            ways.push(id(network, row), &[0, 1], &tags);
        }
        if network > 0 {
            let tags = Tags::from_pairs([("type", "route"), ("route", "bicycle"), ("network", name)]);
            relations.push(network as i64, &tags, (0..ROADS.len()).map(|row| (Id::Way(id(network, row)), "")));
        }
    }
    let mut ways = ways.finish().0;
    // A way outside every country has the worldwide access defaults.
    let node = NodeRef { flags: NodeRef::PRESENT, ..NodeRef::missing(false) };
    ways.set_points(Column::filled(ways.references().len(), node));
    let data = Data { ways, relations: relations.finish().0, members: Column::default() };
    let ways = osm::scan(data, Sets::default(), Counts::new()).osm.ways;
    let price = |(row, &tags): (usize, &&'static str)| {
        let mut factors = Vec::new();
        for profile in profiles {
            for (network, name) in NETWORKS.into_iter().enumerate().take(if profile.walking { 1 } else { 3 }) {
                let way = ways.get(id(network, row)).expect("a way of the list");
                let factor = cost::way_basis(&way, false, profile).map(|basis| basis.factor);
                factors.push((format!("{} {name}", profile.name).trim_end().to_string(), factor));
            }
        }
        Price { tags, factors }
    };
    ROADS.iter().enumerate().map(price).collect()
}
