//! Streaming regional OSM import.
use crate::source::{self, Builder, Data, Id, Oneway, Relations, Table, Tags, Ways};
use crate::{country::Country, Graph};
use osmpbfreader::{OsmId, OsmObj, OsmPbfReader};
use planner_router::closures::{Closure, Kind};
use planner_router::model::{Point, Road, Surface, BIKE, FOOT, NO_ELEVATION, PUSH};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;

type Counts = BTreeMap<&'static str, usize>;

fn count(counts: &mut Counts, key: &'static str) {
    *counts.entry(key).or_default() += 1;
}

#[derive(Clone)]
struct Attributes {
    class: u8,
    surface: Surface,
    access: [u8; 2],
    difficulty: u8,
    hiking_difficulty: Option<u8>,
    structure: bool,
}

/// A routable way; its nodes and tags stay in the source data.
struct RawWay {
    id: i64,
    attributes: Attributes,
}

struct Restriction {
    from: i64,
    to: i64,
    via: i64,
    only: bool,
    u_turn: bool,
    walking: bool,
}

/// Conditional restrictions do not count: the router keeps them open, and the route reports them.
fn access(tags: &Tags, defaults: u8, direction: &str) -> u8 {
    source::access(|key| tags.get(key), defaults, direction, true)
}

fn attributes(tags: &Tags, country: Country, counts: &mut Counts) -> Option<Attributes> {
    if tags.contains("area", "yes") {
        count(counts, "excluded area ways");
        return None;
    }
    let ferry = tags.contains("route", "ferry");
    let highway = tags.get("highway").unwrap_or("");
    let (class, defaults) = if ferry { (6, BIKE | FOOT | PUSH) } else { source::highway_access(highway, country)? };
    let mut modes = [access(tags, defaults, "forward"), access(tags, defaults, "backward")];
    for (mode, mask) in [("bicycle", BIKE), ("foot", FOOT | PUSH)] {
        match source::oneway(|key| tags.get(key), mode) {
            Oneway::Forward => modes[1] &= !mask,
            Oneway::Backward => modes[0] &= !mask,
            Oneway::Both | Oneway::Reversible => {}
            Oneway::Unknown => {
                modes[0] &= !mask;
                modes[1] &= !mask;
                count(counts, "excluded unsupported oneway modes");
            }
        }
    }
    if modes == [0, 0] {
        return None;
    }
    let surface = match tags.get("surface").unwrap_or("") {
        "asphalt" | "paved" | "concrete" | "concrete:plates" | "concrete:lanes" | "paving_stones" => Surface::Paved,
        "compacted" | "fine_gravel" => Surface::Compacted,
        "gravel" | "pebblestone" => Surface::Gravel,
        "ground" | "dirt" | "earth" | "grass" | "unpaved" => Surface::Dirt,
        "sand" | "mud" | "rock" | "stone" | "cobblestone" | "sett" => Surface::Rough,
        _ => Surface::Unknown,
    };
    let difficulty = tags
        .get("mtb:scale")
        .and_then(|v| v.trim_end_matches(['+', '-']).parse::<u8>().ok())
        .filter(|v| *v <= 6)
        .unwrap_or(255);
    let hiking_difficulty = match tags.get("sac_scale") {
        Some("strolling") => Some(0),
        Some("hiking") => Some(1),
        Some("mountain_hiking") => Some(2),
        Some("demanding_mountain_hiking") => Some(3),
        Some("alpine_hiking") => Some(4),
        Some("demanding_alpine_hiking") => Some(5),
        Some("difficult_alpine_hiking") => Some(6),
        Some(_) => {
            count(counts, "unknown hiking classifications");
            Some(255)
        }
        None => None,
    };
    Some(Attributes {
        class,
        surface,
        access: modes,
        difficulty,
        hiking_difficulty,
        structure: ["bridge", "tunnel"].iter().any(|key| tags.get(key).is_some_and(|v| v != "no")),
    })
}

/// The modes that pass a barrier, or `None` for a barrier the router does not know.
fn barrier(value: Option<&str>) -> Option<u8> {
    match value {
        None
        | Some(
            "no" | "entrance" | "bollard" | "gate" | "lift_gate" | "swing_gate" | "cycle_barrier" | "toll_booth"
            | "border_control" | "cattle_grid" | "kerb" | "block" | "chain" | "height_restrictor",
        ) => Some(BIKE | FOOT | PUSH),
        Some("stile" | "kissing_gate" | "turnstile") => Some(FOOT),
        Some(_) => None,
    }
}

/// The possible closures of a node: its access tags, and an unknown barrier that a rider may
/// have to push through.
pub fn node_closures<'a>(tags: impl Iterator<Item = (&'a str, &'a str)> + Clone) -> Vec<(u8, Closure)> {
    let mut closures = source::closures(tags.clone(), &["forward", "backward"]);
    if let Some((_, value)) = tags.into_iter().find(|&(key, value)| key == "barrier" && barrier(Some(value)).is_none())
    {
        closures.push((FOOT | PUSH, Closure { kind: Kind::Unclear, condition: format!("barrier={value}") }));
    }
    closures
}

fn crossing(tags: &Tags, counts: &mut Counts) -> u8 {
    let defaults = barrier(tags.get("barrier")).unwrap_or_else(|| {
        count(counts, "unknown barrier nodes passed on foot");
        FOOT | PUSH
    });
    let modes = access(tags, defaults, "forward") & access(tags, defaults, "backward");
    if modes & BIKE == 0 && modes & PUSH != 0 {
        count(counts, "dismount-only crossing nodes");
    }
    modes
}

fn relation_rules(relation: &source::Relation, ways: &mut [RawWay], rules: &mut Vec<Restriction>, counts: &mut Counts) {
    let mut mode_rules = Vec::new();
    let generic = if tags_foot_restriction(&relation.tags) { None } else { relation.tags.get("restriction") };
    let except_bike = relation.tags.get("except").is_some_and(|v| v.split(';').any(|m| m == "bicycle"));
    if let Some(rule) = relation.tags.get("restriction:bicycle").or(if except_bike { None } else { generic }) {
        mode_rules.push((false, rule));
    }
    if let Some(rule) = relation.tags.get("restriction:foot").or_else(|| {
        if tags_foot_restriction(&relation.tags) {
            relation.tags.get("restriction")
        } else {
            None
        }
    }) {
        mode_rules.push((true, rule));
    }
    // A conditional restriction restricts a turn only sometimes, so the router ignores it.
    if mode_rules.is_empty() {
        return;
    }
    let find = |ways: &[RawWay], id: i64| ways.binary_search_by_key(&id, |way| way.id).ok();
    let members: Vec<_> = relation.members().filter_map(|(member, _)| member.way()).collect();
    if !members.iter().any(|&id| find(ways, id).is_some()) {
        return;
    }
    let from: Vec<_> = relation.members().filter(|(_, role)| *role == "from").filter_map(|(m, _)| m.way()).collect();
    let to: Vec<_> = relation.members().filter(|(_, role)| *role == "to").filter_map(|(m, _)| m.way()).collect();
    let via: Vec<_> = relation.members().filter(|(_, role)| *role == "via").map(|(member, _)| member).collect();
    let via_node = if let [Id::Node(id)] = via.as_slice() { Some(*id) } else { None };
    let supported_shape = from.len() == 1 && to.len() == 1 && via_node.is_some();
    let mut excluded = 0;
    for (walking, value) in mode_rules {
        let supported_rule = matches!(
            value,
            "no_left_turn"
                | "no_right_turn"
                | "no_straight_on"
                | "no_u_turn"
                | "only_left_turn"
                | "only_right_turn"
                | "only_straight_on"
                | "only_u_turn"
        );
        if !supported_shape || !supported_rule {
            excluded |= if walking { FOOT | PUSH } else { BIKE | PUSH };
        } else {
            rules.push(Restriction {
                from: from[0],
                to: to[0],
                via: via_node.unwrap(),
                only: value.starts_with("only_"),
                u_turn: value.ends_with("u_turn"),
                walking,
            });
        }
    }
    if excluded != 0 {
        count(counts, "conservatively excluded unsupported restriction relations");
        for id in members {
            if let Some(index) = find(ways, id) {
                for mode in &mut ways[index].attributes.access {
                    *mode &= !excluded;
                }
            }
        }
    }
}

fn tags_foot_restriction(tags: &Tags) -> bool {
    tags.contains("type", "restriction:foot")
}

fn read(paths: &[PathBuf], mut visit: impl FnMut(OsmObj)) -> Result<(), String> {
    for path in paths {
        let mut reader = OsmPbfReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        for item in reader.iter() {
            visit(item.map_err(|e| e.to_string())?);
        }
    }
    Ok(())
}

fn copy_tags(tags: &osmpbfreader::Tags) -> Tags<'static> {
    Tags::from_pairs(tags.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}

fn member(id: OsmId) -> Id {
    match id {
        OsmId::Node(id) => Id::Node(id.0),
        OsmId::Way(id) => Id::Way(id.0),
        OsmId::Relation(id) => Id::Relation(id.0),
    }
}

/// Bounds are [west, south, east, north]. Missing or outside nodes break ways.
pub fn import(paths: &[PathBuf], bounds: [f64; 4]) -> Result<Graph, String> {
    if paths.is_empty()
        || bounds.iter().any(|v| !v.is_finite())
        || bounds[0] >= bounds[2]
        || bounds[1] >= bounds[3]
        || bounds[0] < -180.0
        || bounds[2] > 180.0
        || bounds[1] < -90.0
        || bounds[3] > 90.0
    {
        return Err("Provide input files and nonempty [west,south,east,north] bounds".into());
    }
    let mut ways = Builder::<Ways>::default();
    let mut relations = Builder::<Relations>::default();
    read(paths, |object| match object {
        OsmObj::Way(way)
            if way.nodes.len() >= 2 && (way.tags.contains_key("highway") || way.tags.contains("route", "ferry")) =>
        {
            let nodes: Vec<_> = way.nodes.iter().map(|id| id.0).collect();
            ways.push(way.id.0, &nodes, &copy_tags(&way.tags));
        }
        OsmObj::Relation(relation) => {
            let members = relation.refs.iter().map(|r| (member(r.member), r.role.as_str()));
            relations.push(relation.id.0, &copy_tags(&relation.tags), members);
        }
        _ => {}
    })?;
    let mut counts = Counts::new();
    let (ways, duplicates) = ways.finish();
    counts.insert("duplicate input ways", duplicates);
    // Only a relation with a highway, a node or a relation as a member can restrict a turn or be preserved.
    let relations = relations.finish().0.retain(|relation| {
        relation.members().any(|(member, _)| match member {
            Id::Way(id) => ways.get(id).is_some(),
            Id::Node(_) | Id::Relation(_) => true,
        })
    });
    let (needed, uses) = references(&ways);
    let mut nodes = Builder::<source::Nodes>::default();
    read(paths, |object| {
        let OsmObj::Node(node) = object else {
            return;
        };
        let (lat, lon) = (node.decimicro_lat as f64 * 1e-7, node.decimicro_lon as f64 * 1e-7);
        if needed.binary_search(&node.id.0).is_err()
            || lon < bounds[0]
            || lat < bounds[1]
            || lon > bounds[2]
            || lat > bounds[3]
        {
            return;
        }
        let point = Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION };
        nodes.push(node.id.0, point, &copy_tags(&node.tags));
    })?;
    let (nodes, _) = nodes.finish();
    counts.insert("outside bounds or missing referenced nodes", needed.len() - nodes.len());
    build_graph(Data { nodes, ways, relations }, needed, uses, counts)
}

/// The sorted IDs of the nodes that the ways reference, and the number of references to each.
fn references(ways: &Table<Ways>) -> (Vec<i64>, Vec<u32>) {
    let mut ids: Vec<i64> = ways.iter().flat_map(|way| way.nodes.iter().copied()).collect();
    ids.sort_unstable();
    let uses = ids.chunk_by(|a, b| a == b).map(|run| run.len() as u32).collect();
    ids.dedup();
    (ids, uses)
}

fn preserve_relations(osm: &Data) -> Table<Relations> {
    // Retain direct memberships and their parent relations, including unrecognized tags and roles.
    let mut kept = HashSet::new();
    loop {
        let before = kept.len();
        for relation in osm.relations.iter() {
            if !kept.contains(&relation.id)
                && relation.members().any(|(member, _)| match member {
                    Id::Way(id) => osm.ways.get(id).is_some(),
                    Id::Node(id) => osm.nodes.get(id).is_some(),
                    Id::Relation(id) => kept.contains(&id),
                })
            {
                kept.insert(relation.id);
            }
        }
        if kept.len() == before {
            break;
        }
    }
    osm.relations.retain(|relation| kept.contains(&relation.id))
}

/// `osm` holds the nodes inside the bounds, every highway, and the relations that may be
/// preserved; `needed` and `uses` are the `references` of its ways.
fn build_graph(mut osm: Data, needed: Vec<i64>, uses: Vec<u32>, mut counts: Counts) -> Result<Graph, String> {
    let mut ways = Vec::new();
    for way in osm.ways.iter() {
        // The country changes the modes of a road class, never whether it is one.
        let known = source::highway_access(way.tags.get("highway").unwrap_or(""), Country::default());
        if known.is_none() && !way.tags.contains("route", "ferry") {
            continue;
        }
        if let Some(attributes) = attributes(&way.tags, osm.country(&way), &mut counts) {
            ways.push(RawWay { id: way.id, attributes });
        }
    }
    let mut rules = Vec::new();
    for relation in osm.relations.iter() {
        relation_rules(&relation, &mut ways, &mut rules, &mut counts);
    }
    ways.retain(|w| w.attributes.access != [0, 0]);
    osm.ways = osm.ways.retain(|way| way.nodes.iter().any(|&id| osm.nodes.get(id).is_some()));
    osm.relations = preserve_relations(&osm);

    let mut junctions: Vec<bool> = uses.iter().map(|&uses| uses > 1).collect();
    let mut mark = |id: i64| {
        if let Ok(index) = needed.binary_search(&id) {
            junctions[index] = true;
        }
    };
    let mut crossings = HashMap::new();
    for node in osm.nodes.iter().filter(|node| !node.tags.is_empty()) {
        let tags = &node.tags;
        let modes = crossing(tags, &mut counts);
        let signals = tags.contains("highway", "traffic_signals")
            || tags.contains("highway", "crossing") && tags.contains("crossing", "traffic_signals");
        // A node with a possible closure ends its roads, so the road that arrives there reports it.
        if signals || modes != BIKE | FOOT | PUSH || !node_closures(tags.iter()).is_empty() {
            mark(node.id);
        }
        if modes != BIKE | FOOT | PUSH {
            crossings.insert(node.id, modes);
        }
    }
    for rule in &rules {
        mark(rule.via);
    }
    for way in ways.iter().filter_map(|way| osm.ways.get(way.id)) {
        mark(way.nodes[0]);
        mark(way.nodes[way.nodes.len() - 1]);
        if way.nodes.first() == way.nodes.last() {
            mark(way.nodes[way.nodes.len() / 2]);
        }
    }
    let junction = |id: i64| needed.binary_search(&id).is_ok_and(|index| junctions[index]);
    let mut graph = Graph::default();
    let mut point_ids = HashMap::new();
    for raw in &ways {
        let Some(way) = osm.ways.get(raw.id) else {
            continue;
        };
        let mut run = Vec::new();
        for &id in way.nodes {
            let Some(node) = osm.nodes.get(id) else {
                emit_run(&mut graph, &mut point_ids, &crossings, &run, raw)?;
                run.clear();
                continue;
            };
            run.push((id, node.point));
            if junction(id) && run.len() >= 2 {
                emit_run(&mut graph, &mut point_ids, &crossings, &run, raw)?;
                run.clear();
                run.push((id, node.point));
            }
        }
        emit_run(&mut graph, &mut point_ids, &crossings, &run, raw)?;
    }
    let outgoing = graph.departures();
    let mut incoming = vec![Vec::new(); graph.points.len()];
    for (id, road) in graph.roads.iter().enumerate() {
        incoming[road.to as usize].push(id as u32);
    }
    for rule in rules {
        let Some(&via) = point_ids.get(&rule.via) else {
            count(&mut counts, "restrictions outside retained graph");
            continue;
        };
        let arrivals: Vec<_> =
            incoming[via as usize].iter().copied().filter(|id| graph.roads[*id as usize].way == rule.from).collect();
        let has_target = outgoing[via as usize].iter().any(|id| graph.roads[*id as usize].way == rule.to);
        if arrivals.is_empty() || !has_target {
            count(&mut counts, "restriction members missing at retained junction");
        }
        let forbidden = if rule.walking { &mut graph.forbidden_foot } else { &mut graph.forbidden };
        for from in arrivals {
            for &to in &outgoing[via as usize] {
                let target = graph.roads[to as usize].way == rule.to
                    && (!rule.u_turn || graph.roads[from as usize].from == graph.roads[to as usize].to);
                if target != rule.only {
                    forbidden.push((from, to));
                }
            }
        }
    }
    for forbidden in [&mut graph.forbidden, &mut graph.forbidden_foot] {
        forbidden.sort_unstable();
        forbidden.dedup();
    }
    counts.insert("retained directed roads", graph.roads.len());
    counts.insert("retained graph nodes", graph.points.len());
    counts.insert("roads with unknown surface", graph.roads.iter().filter(|r| r.surface == Surface::Unknown).count());
    graph.warnings = counts.into_iter().filter(|(_, n)| *n != 0).map(|(label, n)| format!("{label}: {n}")).collect();
    graph.warnings.push("No DEM applied; elevation is unknown and climbing costs are not validated".into());
    graph.warnings.push("Country access defaults; ferry schedules, conditional access and via-way restrictions require conservative handling".into());
    if graph.roads.is_empty() {
        return Err("No routable roads within input bounds".into());
    }
    graph.osm = osm;
    Ok(graph)
}

fn emit_run(
    graph: &mut Graph,
    point_ids: &mut HashMap<i64, u32>,
    crossings: &HashMap<i64, u8>,
    run: &[(i64, Point)],
    way: &RawWay,
) -> Result<(), String> {
    if run.len() < 2 {
        return Ok(());
    }
    let shape: Vec<_> = run.iter().map(|&(_, point)| point).collect();
    let length = shape.windows(2).map(|p| p[0].distance(p[1])).sum::<f64>();
    if length < 0.01 {
        return Ok(());
    }
    let mut endpoint = |(id, point): (i64, Point)| -> Result<u32, String> {
        if let Some(&existing) = point_ids.get(&id) {
            return Ok(existing);
        }
        let index = u32::try_from(graph.points.len()).map_err(|_| "Too many graph nodes")?;
        graph.points.push(point);
        graph.node_ids.push(id);
        graph.node_access.push(crossings.get(&id).copied().unwrap_or(BIKE | FOOT | PUSH));
        point_ids.insert(id, index);
        Ok(index)
    };
    let from = endpoint(run[0])?;
    let to = endpoint(run[run.len() - 1])?;
    for direction in 0..2 {
        let access = way.attributes.access[direction];
        if access == 0 {
            continue;
        }
        let mut shape = shape.clone();
        if direction == 1 {
            shape.reverse();
        }
        graph.roads.push(Road {
            from: if direction == 0 { from } else { to },
            to: if direction == 0 { to } else { from },
            way: way.id,
            reversed: direction == 1,
            length_m: length.round().max(1.0) as u32,
            ascent_m: 0,
            descent_m: 0,
            surface: way.attributes.surface,
            class: way.attributes.class,
            access,
            difficulty: way.attributes.difficulty,
            hiking_difficulty: way.attributes.hiking_difficulty,
            structure: way.attributes.structure,
            shape,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::Profile;

    /// Tests use the worldwide defaults.
    fn attributes(tags: &Tags, counts: &mut Counts) -> Option<Attributes> {
        super::attributes(tags, Country::default(), counts)
    }

    fn tags(values: &[(&str, &str)]) -> Tags<'static> {
        Tags::from_pairs(values.iter().copied())
    }

    fn way<'a>(id: i64, nodes: &'a [i64], values: &'a [(&'a str, &'a str)]) -> (i64, &'a [i64], Tags<'static>) {
        (id, nodes, tags(values))
    }

    fn raw(id: i64, values: &[(&str, &str)]) -> RawWay {
        RawWay { id, attributes: attributes(&tags(values), &mut Counts::new()).unwrap() }
    }

    fn relation(values: &[(&str, &str)], members: &[(Id, &str)]) -> Table<Relations> {
        let mut relations = Builder::<Relations>::default();
        relations.push(1, &tags(values), members.iter().copied());
        relations.finish().0
    }

    /// The graph of `ways` over the nodes `present`, outside every country; the nodes of `tagged`
    /// have tags.
    fn fixture(
        ways: &[(i64, &[i64], Tags)],
        present: &[i64],
        tagged: &[(i64, Tags)],
        relations: Table<Relations>,
    ) -> Graph {
        let mut builder = Builder::<Ways>::default();
        for (id, nodes, tags) in ways {
            builder.push(*id, nodes, tags);
        }
        let ways = builder.finish().0;
        let mut nodes = Builder::<source::Nodes>::default();
        for &id in present {
            let point = Point { lat: 0, lon: id as i32 * 50, elevation: NO_ELEVATION };
            let tags = tagged.iter().find(|(node, _)| *node == id).map(|(_, tags)| tags.clone());
            nodes.push(id, point, &tags.unwrap_or_default());
        }
        let (needed, uses) = references(&ways);
        build_graph(Data { nodes: nodes.finish().0, ways, relations }, needed, uses, Counts::new()).unwrap()
    }

    #[test]
    fn source_tags_and_relation_roles_survive_graph_compilation() {
        let values = [
            ("highway", "tertiary"),
            ("smoothness", "intermediate"),
            ("maxspeed:forward", "70"),
            ("cycleway:right", "lane"),
            ("custom:future", "retained"),
        ];
        let mut relations = Builder::<Relations>::default();
        relations.push(
            20,
            &tags(&[("type", "route"), ("route", "bicycle"), ("network", "lcn")]),
            [(Id::Way(10), "forward")],
        );
        relations.push(21, &tags(&[("type", "superroute"), ("route", "bicycle")]), [(Id::Relation(20), "section")]);
        relations.push(22, &tags(&[("type", "route")]), [(Id::Way(30), "")]);
        let ways = [way(10, &[1, 2], &values), way(30, &[8, 9], &[("highway", "path")])];
        let graph = fixture(&ways, &[1, 2], &[], relations.finish().0);
        let source = graph.osm.ways.get(10).unwrap();
        assert_eq!((source.tags, source.nodes), (tags(&values), &[1, 2][..]));
        // A way without a node inside the bounds leaves, and so does a relation of only such ways.
        assert!(graph.osm.ways.get(30).is_none());
        let members = |id| graph.osm.relations.get(id).map(|r| r.members().collect::<Vec<_>>());
        assert_eq!(members(20), Some(vec![(Id::Way(10), "forward")]));
        assert_eq!(members(21), Some(vec![(Id::Relation(20), "section")]));
        assert_eq!(members(22), None);
    }

    #[test]
    fn access_direction_and_dismount_are_independent() {
        let attrs = attributes(
            &tags(&[
                ("highway", "residential"),
                ("access", "no"),
                ("foot", "yes"),
                ("bicycle", "yes"),
                ("oneway", "yes"),
                ("oneway:bicycle", "no"),
            ]),
            &mut Counts::new(),
        )
        .unwrap();
        assert_eq!(attrs.access, [BIKE | FOOT | PUSH; 2]);
        let attrs = attributes(&tags(&[("highway", "residential"), ("oneway", "-1")]), &mut Counts::new()).unwrap();
        assert_eq!(attrs.access, [FOOT | PUSH, BIKE | FOOT | PUSH]);
        let attrs = attributes(&tags(&[("highway", "footway"), ("bicycle", "no")]), &mut Counts::new()).unwrap();
        assert_eq!(attrs.access, [FOOT | PUSH; 2]);
        let attrs = attributes(
            &tags(&[("highway", "footway"), ("bicycle", "no"), ("bicycle:pushing", "no")]),
            &mut Counts::new(),
        )
        .unwrap();
        assert_eq!(attrs.access, [FOOT; 2]);
        let attrs = attributes(&tags(&[("highway", "footway"), ("bicycle", "dismount")]), &mut Counts::new()).unwrap();
        assert_eq!(attrs.access, [FOOT | PUSH; 2]);
        assert!(attributes(&tags(&[("highway", "path"), ("bicycle", "dismount"), ("foot", "no")]), &mut Counts::new())
            .is_none());
        let attrs = attributes(&tags(&[("highway", "cycleway"), ("bicycle", "dismount")]), &mut Counts::new()).unwrap();
        assert_eq!(attrs.access, [FOOT | PUSH; 2]);
        let attrs = attributes(
            &tags(&[("highway", "cycleway"), ("bicycle", "dismount"), ("bicycle:pushing", "no")]),
            &mut Counts::new(),
        )
        .unwrap();
        assert_eq!(attrs.access, [FOOT; 2]);
        assert!(attributes(
            &tags(&[("highway", "cycleway"), ("bicycle", "dismount"), ("foot", "no")]),
            &mut Counts::new()
        )
        .is_none());
        assert!(attributes(&tags(&[("highway", "motorway")]), &mut Counts::new()).is_none());
        assert!(attributes(&tags(&[("highway", "motorway"), ("access", "yes")]), &mut Counts::new()).is_none());
        let attrs =
            attributes(&tags(&[("highway", "trunk"), ("access", "yes"), ("bicycle", "yes")]), &mut Counts::new())
                .unwrap();
        assert_eq!(attrs.access, [BIKE | FOOT | PUSH; 2]);
        let attrs =
            attributes(&tags(&[("highway", "residential"), ("bicycle:forward", "no")]), &mut Counts::new()).unwrap();
        assert_eq!(attrs.access, [FOOT | PUSH, BIKE | FOOT | PUSH]);
    }

    #[test]
    fn cycle_lane_direction_only_opens_the_opposing_direction_when_permitted() {
        for (oneway, lane, expected) in [
            ("yes", "yes", [BIKE | FOOT | PUSH, FOOT | PUSH]),
            ("yes", "-1", [BIKE | FOOT | PUSH; 2]),
            ("-1", "-1", [FOOT | PUSH, BIKE | FOOT | PUSH]),
            ("-1", "yes", [BIKE | FOOT | PUSH; 2]),
            ("-1", "no", [BIKE | FOOT | PUSH; 2]),
        ] {
            let attrs = attributes(
                &tags(&[
                    ("highway", "residential"),
                    ("oneway", oneway),
                    ("cycleway:left", "lane"),
                    ("cycleway:left:oneway", lane),
                ]),
                &mut Counts::new(),
            )
            .unwrap();
            assert_eq!(attrs.access, expected, "oneway={oneway}, lane={lane}");
            let attrs = attributes(
                &tags(&[
                    ("highway", "residential"),
                    ("oneway", oneway),
                    ("cycleway:both", "lane"),
                    ("cycleway:left:oneway", lane),
                ]),
                &mut Counts::new(),
            )
            .unwrap();
            assert_eq!(attrs.access, expected, "oneway={oneway}, both lanes, left={lane}");
        }
    }

    #[test]
    fn only_no_closes_a_way() {
        let open = |pairs: &[(&str, &str)]| {
            let pairs = [&[("highway", "secondary")], pairs].concat();
            attributes(&tags(&pairs), &mut Counts::new()).map(|attrs| attrs.access)
        };
        for condition in ["no @ (Nov-May)", "no @ (2025 Mar 10-2025 Oct 1)", "no @ (Mo-Fr)", "no @ (wet)", "no"] {
            assert_eq!(open(&[("access:conditional", condition)]), Some([BIKE | FOOT | PUSH; 2]), "{condition}");
        }
        for value in ["permit", "private", "agricultural;forestry", "destination", "military", "discouraged", "unknown"]
        {
            assert_eq!(open(&[("access", value)]), Some([BIKE | FOOT | PUSH; 2]), "{value}");
        }
        assert_eq!(open(&[("access", "permit"), ("bicycle", "no")]), Some([FOOT | PUSH; 2]));
        assert_eq!(open(&[("bicycle", "use_sidepath")]), Some([BIKE | FOOT | PUSH; 2]));
        assert_eq!(open(&[("access", "no"), ("foot", "private")]), Some([FOOT | PUSH; 2]));
        assert_eq!(open(&[("access", "no")]), None);
        assert!(attributes(&tags(&[("highway", "construction")]), &mut Counts::new()).is_none());
        assert_eq!(crossing(&tags(&[("barrier", "toll_booth")]), &mut Counts::new()), BIKE | FOOT | PUSH);
        assert_eq!(
            crossing(&tags(&[("barrier", "gate"), ("access", "private")]), &mut Counts::new()),
            BIKE | FOOT | PUSH
        );
        assert_eq!(crossing(&tags(&[("barrier", "gate"), ("access", "no")]), &mut Counts::new()), 0);
    }

    #[test]
    fn topology_uses_ids_and_never_joins_across_missing_nodes() {
        let ways = [way(10, &[1, 2, 3, 4, 5, 6, 7], &[("highway", "path")]), way(20, &[2, 8], &[("highway", "path")])];
        let graph = fixture(&ways, &[1, 2, 3, 5, 6, 7, 8], &[], Default::default());
        assert_eq!(graph.points.len(), 6);
        assert_eq!(graph.roads.len(), 8);
        assert_eq!(graph.roads.iter().filter(|r| r.shape.len() == 3).count(), 2);
        assert!(graph
            .roads
            .iter()
            .filter(|r| r.way == 10)
            .all(|r| !r.shape.windows(2).any(|p| p[0].distance(p[1]) > 10.0)));
    }

    #[test]
    fn explicit_foot_turn_and_bicycle_exception_survive_import() {
        let relation = relation(
            &[
                ("type", "restriction"),
                ("restriction", "no_right_turn"),
                ("except", "bicycle"),
                ("restriction:foot", "no_right_turn"),
            ],
            &[(Id::Way(10), "from"), (Id::Node(2), "via"), (Id::Way(20), "to")],
        );
        let ways = [way(10, &[1, 2], &[("highway", "path")]), way(20, &[2, 3], &[("highway", "path")])];
        let graph = fixture(&ways, &[1, 2, 3], &[], relation);
        assert!(graph.forbidden.is_empty());
        assert_eq!(graph.forbidden_foot.len(), 1);
        let (from, to) = graph.forbidden_foot[0];
        assert!(!graph.permits_turn(from, to, true));
        assert!(graph.permits_turn(from, to, false));
    }

    #[test]
    fn via_way_restriction_exclusion_is_explicit_and_mode_specific() {
        let mut ways = [10, 20, 30].map(|id| raw(id, &[("highway", "path")]));
        let relation = relation(
            &[("type", "restriction"), ("restriction", "no_right_turn")],
            &[(Id::Way(10), "from"), (Id::Way(20), "via"), (Id::Way(30), "to")],
        );
        let mut counts = Counts::new();
        let mut rules = Vec::new();
        relation_rules(&relation.iter().next().unwrap(), &mut ways, &mut rules, &mut counts);
        assert!(rules.is_empty());
        assert!(ways.iter().all(|w| w.attributes.access == [FOOT; 2]));
        assert_eq!(counts["conservatively excluded unsupported restriction relations"], 1);
    }

    #[test]
    fn a_node_closure_stays_routable_and_the_arriving_road_reports_it() {
        let tagged = [
            (2, tags(&[("barrier", "gate"), ("access:conditional", "no @ (Nov-Apr)")])),
            (3, tags(&[("barrier", "yes")])),
        ];
        let ways = [way(10, &[1, 2, 3, 4], &[("highway", "residential")])];
        let graph = fixture(&ways, &[1, 2, 3, 4], &tagged, Default::default());
        let point = |id: i64| graph.node_ids.iter().position(|&node| node == id).unwrap();
        // Riding through an unknown barrier stays closed; walking and pushing pass.
        assert_eq!((graph.node_access[point(2)], graph.node_access[point(3)]), (BIKE | FOOT | PUSH, FOOT | PUSH));
        let profile = Profile::presets().into_iter().find(|p| p.name == "road").unwrap();
        let mut objects = HashMap::new();
        let manifest = crate::prepare(
            &graph,
            "nodes".into(),
            [-1.0, -1.0, 1.0, 1.0],
            &[profile],
            vec![],
            "test".into(),
            |bytes| {
                let key = planner_router::package::digest(bytes);
                objects.insert(key.clone(), bytes.to_vec());
                Ok(key)
            },
        )
        .unwrap();
        struct Memory(HashMap<String, Vec<u8>>);
        impl planner_router::package::Source for Memory {
            fn read(&self, key: &str) -> planner_router::Result<Vec<u8>> {
                self.0.get(key).cloned().ok_or_else(|| planner_router::Error::MissingRegion(key.into()))
            }
        }
        use planner_router::data::RoutingData;
        let package = planner_router::package::Package::open(Memory(objects), &serde_json::to_vec(&manifest).unwrap());
        let table = planner_router::data::Selection::whole(package.unwrap()).closures().unwrap();
        let arriving =
            |id: i64| graph.roads.iter().position(|r| !r.reversed && r.to as usize == point(id)).unwrap() as u32;
        let closure = |kind, condition: &str| Some(vec![Closure { kind, condition: condition.into() }]);
        assert_eq!(table.closing(arriving(2), BIKE), closure(Kind::Seasonal, "Nov-Apr"));
        assert_eq!(table.closing(arriving(3), PUSH), closure(Kind::Unclear, "barrier=yes"));
        assert_eq!((table.closing(arriving(3), BIKE), table.closing(arriving(4), BIKE)), (None, None));
    }

    #[test]
    fn a_conditional_turn_restriction_closes_neither_turn_nor_way() {
        let mut ways = [10, 20].map(|id| raw(id, &[("highway", "path")]));
        for key in ["restriction:conditional", "restriction:bicycle:conditional", "restriction:foot:conditional"] {
            let relation = relation(
                &[("type", "restriction"), (key, "no_right_turn @ (Mo-Fr 07:00-09:00)")],
                &[(Id::Way(10), "from"), (Id::Node(2), "via"), (Id::Way(20), "to")],
            );
            let mut rules = Vec::new();
            relation_rules(&relation.iter().next().unwrap(), &mut ways, &mut rules, &mut Counts::new());
            assert!(rules.is_empty(), "{key}");
            assert!(ways.iter().all(|w| w.attributes.access == [BIKE | FOOT | PUSH; 2]), "{key}");
        }
    }

    #[test]
    fn hiking_difficulty_does_not_use_mtb_scale() {
        let path = |values| fixture(&[way(10, &[1, 2], values)], &[1, 2], &[], Default::default());
        let graph = path(&[("highway", "path"), ("mtb:scale", "6"), ("sac_scale", "hiking")]);
        let profiles = Profile::presets();
        assert!(profiles.iter().find(|p| p.walking).unwrap().permits(&graph.roads[0]));
        assert!(profiles
            .iter()
            .filter(|p| !p.walking)
            .all(|p| p.permits(&graph.roads[0]) == (p.name == "mtb/technical")));
        let alpine = path(&[("highway", "path"), ("sac_scale", "alpine_hiking")]);
        assert!(profiles.iter().all(|p| p.permits(&alpine.roads[0]) == (p.name == "hiking/alpine")));
        let climbing = path(&[("highway", "path"), ("sac_scale", "demanding_alpine_hiking")]);
        assert!(profiles.iter().all(|p| !p.permits(&climbing.roads[0])));
    }

    #[test]
    fn mtb_scale_with_a_sign_counts_as_its_digit() {
        let ways = [way(10, &[1, 2], &[("highway", "path"), ("mtb:scale", "2+")])];
        let graph = fixture(&ways, &[1, 2], &[], Default::default());
        assert_eq!(graph.roads[0].mtb_scale(), Some(2));
    }

    #[test]
    fn node_dismount_permission_does_not_become_cycling_permission() {
        let ways = [way(10, &[1, 2, 3], &[("highway", "path")])];
        let graph = fixture(&ways, &[1, 2, 3], &[(2, tags(&[("bicycle", "dismount")]))], Default::default());
        assert!(graph.node_access.iter().any(|a| a & BIKE == 0 && a & PUSH != 0));
        assert!(graph.forbidden.is_empty());
        assert!(graph.forbidden_foot.is_empty());
        let before = graph
            .roads
            .iter()
            .position(|r| graph.node_ids[r.from as usize] == 1 && graph.node_ids[r.to as usize] == 2)
            .unwrap() as u32;
        let after = graph
            .roads
            .iter()
            .position(|r| graph.node_ids[r.from as usize] == 2 && graph.node_ids[r.to as usize] == 3)
            .unwrap() as u32;
        let profile = Profile::presets().into_iter().find(|p| p.name == "road").unwrap();
        assert_eq!(
            crate::cost::Costing::new(&graph, &profile, &Default::default()).unwrap().transition(before, after),
            Some(300)
        );
        // A crossing that bans pushing as well closes the transition for a cyclist.
        let mut graph = graph;
        graph.node_access[graph.roads[before as usize].to as usize] = FOOT;
        assert_eq!(
            crate::cost::Costing::new(&graph, &profile, &Default::default()).unwrap().transition(before, after),
            None
        );
    }
}
