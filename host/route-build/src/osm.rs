//! Streaming regional OSM import.
use osmpbfreader::{OsmId, OsmObj, OsmPbfReader, Relation, Tags, Way};
use route_engine::model::{Graph, Point, Road, Surface, BIKE, FOOT, NO_ELEVATION, PUSH};
use route_engine::osm as source;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;

type Counts = BTreeMap<&'static str, usize>;

fn count(counts: &mut Counts, key: &'static str) {
    *counts.entry(key).or_default() += 1;
}

fn tag<'a>(tags: &'a Tags, key: &str) -> Option<&'a str> {
    tags.get(key).map(|v| v.as_str())
}

#[derive(Clone)]
struct Attributes {
    class: u8,
    surface: Surface,
    access: [u8; 2],
    difficulty: u8,
    hiking_difficulty: Option<u8>,
    uncertain_access: bool,
    structure: bool,
}

struct RawWay {
    id: i64,
    nodes: Vec<i64>,
    attributes: Attributes,
    tags: Tags,
}

#[derive(Clone)]
struct RawNode {
    point: Point,
    crossing: u8,
    tags: Tags,
}

struct Restriction {
    from: i64,
    to: i64,
    via: i64,
    only: bool,
    u_turn: bool,
    walking: bool,
}

fn access(tags: &Tags, defaults: u8, direction: &str) -> u8 {
    source::access(|key| tag(tags, key), defaults, direction)
}

fn conditional_modes(tags: &Tags) -> u8 {
    source::conditional_modes(tags.iter().map(|(key, value)| (key.as_str(), value.as_str())))
}

fn attributes(tags: &Tags, counts: &mut Counts) -> Option<Attributes> {
    if tags.contains("area", "yes") {
        count(counts, "excluded area ways");
        return None;
    }
    let ferry = tags.contains("route", "ferry");
    let highway = tag(tags, "highway").unwrap_or("");
    let (class, defaults) = if ferry { (6, BIKE | FOOT | PUSH) } else { source::highway_access(highway)? };
    let mut modes = [access(tags, defaults, "forward"), access(tags, defaults, "backward")];
    let oneway = tag(tags, "oneway").unwrap_or(if tags.contains("junction", "roundabout") { "yes" } else { "no" });
    let opposite = match oneway {
        "yes" | "1" | "true" => source::cycleway(|key| tag(tags, key), true),
        "-1" | "reverse" => source::cycleway(|key| tag(tags, key), false),
        _ => false,
    };
    let bike_oneway = tag(tags, "oneway:bicycle").unwrap_or(if opposite { "no" } else { oneway });
    for (value, mask) in [(bike_oneway, BIKE), (tag(tags, "oneway:foot").unwrap_or("no"), FOOT | PUSH)] {
        match value {
            "yes" | "1" | "true" => modes[1] &= !mask,
            "-1" | "reverse" => modes[0] &= !mask,
            "no" | "0" | "false" => {}
            _ => {
                modes[0] &= !mask;
                modes[1] &= !mask;
                count(counts, "excluded unsupported oneway modes");
            }
        }
    }
    let conditional = conditional_modes(tags);
    if conditional != 0 {
        for mode in &mut modes {
            *mode &= !conditional;
        }
        count(counts, "ways with excluded conditional modes");
    }
    if tags.values().any(|v| matches!(v.as_str(), "destination" | "customers" | "delivery")) {
        count(counts, "ways with excluded destination or customer modes");
    }
    if modes == [0, 0] {
        return None;
    }
    let surface = match tag(tags, "surface").unwrap_or("") {
        "asphalt" | "paved" | "concrete" | "concrete:plates" | "concrete:lanes" | "paving_stones" => Surface::Paved,
        "compacted" | "fine_gravel" => Surface::Compacted,
        "gravel" | "pebblestone" => Surface::Gravel,
        "ground" | "dirt" | "earth" | "grass" | "unpaved" => Surface::Dirt,
        "sand" | "mud" | "rock" | "stone" | "cobblestone" | "sett" => Surface::Rough,
        _ => Surface::Unknown,
    };
    let difficulty = tag(tags, "mtb:scale").and_then(|v| v.parse::<u8>().ok()).filter(|v| *v <= 6).unwrap_or(255);
    let hiking_difficulty = match tag(tags, "sac_scale") {
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
    let uncertain_access = ["access", "bicycle", "foot"].iter().all(|key| tag(tags, key).is_none())
        && matches!(highway, "path" | "track" | "cycleway" | "bridleway");
    Some(Attributes {
        class,
        surface,
        access: modes,
        difficulty,
        hiking_difficulty,
        uncertain_access,
        structure: ["bridge", "tunnel"].iter().any(|key| tag(tags, key).is_some_and(|v| v != "no")),
    })
}

fn crossing(tags: &Tags, counts: &mut Counts) -> u8 {
    let defaults = match tag(tags, "barrier") {
        None | Some("no" | "entrance" | "bollard" | "gate" | "lift_gate" | "swing_gate" | "cycle_barrier") => {
            BIKE | FOOT | PUSH
        }
        Some("stile" | "kissing_gate" | "turnstile") => FOOT,
        Some(_) => {
            count(counts, "conservatively blocked barrier nodes");
            0
        }
    };
    let modes = access(tags, defaults, "forward") & access(tags, defaults, "backward");
    if modes & BIKE == 0 && modes & PUSH != 0 {
        count(counts, "dismount-only crossing nodes");
    }
    let conditional = conditional_modes(tags);
    if conditional != 0 {
        count(counts, "nodes with excluded conditional modes");
    }
    modes & !conditional
}

fn relation_rules(
    relation: &Relation,
    ways: &mut HashMap<i64, RawWay>,
    rules: &mut Vec<Restriction>,
    counts: &mut Counts,
) {
    let mut mode_rules = Vec::new();
    let generic = if tags_foot_restriction(&relation.tags) { None } else { tag(&relation.tags, "restriction") };
    let except_bike = tag(&relation.tags, "except").is_some_and(|v| v.split(';').any(|m| m == "bicycle"));
    if let Some(rule) = tag(&relation.tags, "restriction:bicycle").or(if except_bike { None } else { generic }) {
        mode_rules.push((false, rule));
    }
    if let Some(rule) = tag(&relation.tags, "restriction:foot").or_else(|| {
        if tags_foot_restriction(&relation.tags) {
            tag(&relation.tags, "restriction")
        } else {
            None
        }
    }) {
        mode_rules.push((true, rule));
    }
    let conditional_bike = tag(&relation.tags, "restriction:bicycle:conditional").is_some()
        || !except_bike && tag(&relation.tags, "restriction:conditional").is_some();
    let conditional_foot = tag(&relation.tags, "restriction:foot:conditional").is_some();
    if mode_rules.is_empty() && !conditional_bike && !conditional_foot {
        return;
    }
    let members: Vec<_> = relation.refs.iter().filter_map(|r| r.member.way().map(|id| id.0)).collect();
    if !members.iter().any(|id| ways.contains_key(id)) {
        return;
    }
    let from: Vec<_> =
        relation.refs.iter().filter(|r| r.role == "from").filter_map(|r| r.member.way().map(|id| id.0)).collect();
    let to: Vec<_> =
        relation.refs.iter().filter(|r| r.role == "to").filter_map(|r| r.member.way().map(|id| id.0)).collect();
    let via: Vec<_> = relation.refs.iter().filter(|r| r.role == "via").map(|r| r.member).collect();
    let via_node = if let [OsmId::Node(id)] = via.as_slice() { Some(id.0) } else { None };
    let supported_shape = from.len() == 1 && to.len() == 1 && via_node.is_some();
    let mut excluded = if conditional_bike { BIKE | PUSH } else { 0 } | if conditional_foot { FOOT | PUSH } else { 0 };
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
            if let Some(way) = ways.get_mut(&id) {
                for mode in &mut way.attributes.access {
                    *mode &= !excluded;
                }
            }
        }
    }
}

fn tags_foot_restriction(tags: &Tags) -> bool {
    tags.contains("type", "restriction:foot")
}

/// Bounds are [west, south, east, north]. Missing or outside nodes break ways.
pub fn import(paths: &[PathBuf], bounds: [f64; 4], country: &str) -> Result<Graph, String> {
    if country != "DE" {
        return Err("The importer currently supports German access defaults; use --country DE with German data".into());
    }
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
    let mut counts = Counts::new();
    let mut ways = HashMap::new();
    let mut relations = HashMap::new();
    let mut preserved = source::Data::default();
    for path in paths {
        let mut reader = OsmPbfReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        for item in reader.iter() {
            match item.map_err(|e| e.to_string())? {
                OsmObj::Way(Way { id, tags, nodes }) if nodes.len() >= 2 => {
                    if !tags.contains_key("highway") && !tags.contains("route", "ferry") {
                        continue;
                    }
                    if preserved.ways.contains_key(&id.0) {
                        count(&mut counts, "duplicate input ways");
                        continue;
                    }
                    preserved.ways.insert(
                        id.0,
                        source::Way { id: id.0, nodes: nodes.iter().map(|n| n.0).collect(), tags: copy_tags(&tags) },
                    );
                    if let Some(attributes) = attributes(&tags, &mut counts) {
                        ways.insert(
                            id.0,
                            RawWay { id: id.0, nodes: nodes.into_iter().map(|n| n.0).collect(), attributes, tags },
                        );
                    }
                }
                OsmObj::Relation(relation) => {
                    relations.entry(relation.id.0).or_insert(relation);
                }
                _ => {}
            }
        }
    }
    let mut rules = Vec::new();
    for relation in relations.values() {
        relation_rules(relation, &mut ways, &mut rules, &mut counts);
    }
    ways.retain(|_, w| w.attributes.access != [0, 0]);
    let mut needed = HashMap::<i64, u32>::new();
    for way in preserved.ways.values() {
        for id in &way.nodes {
            *needed.entry(*id).or_default() += 1;
        }
    }
    let mut nodes = HashMap::new();
    for path in paths {
        let mut reader = OsmPbfReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        for item in reader.iter() {
            let OsmObj::Node(node) = item.map_err(|e| e.to_string())? else {
                continue;
            };
            if !needed.contains_key(&node.id.0) || nodes.contains_key(&node.id.0) {
                continue;
            }
            let (lat, lon) = (node.decimicro_lat as f64 * 1e-7, node.decimicro_lon as f64 * 1e-7);
            if lon < bounds[0] || lat < bounds[1] || lon > bounds[2] || lat > bounds[3] {
                continue;
            }
            nodes.insert(
                node.id.0,
                RawNode {
                    point: Point {
                        lat: (lat * 1e6).round() as i32,
                        lon: (lon * 1e6).round() as i32,
                        elevation: NO_ELEVATION,
                    },
                    crossing: crossing(&node.tags, &mut counts),
                    tags: node.tags,
                },
            );
        }
    }
    counts.insert("outside bounds or missing referenced nodes", needed.len() - nodes.len());
    preserved.ways.retain(|_, way| way.nodes.iter().any(|id| nodes.contains_key(id)));
    preserve_relations(&mut preserved, relations, &nodes);
    let mut graph = build_graph(ways, nodes, needed, rules, counts)?;
    graph.osm.ways = preserved.ways;
    graph.osm.relations = preserved.relations;
    Ok(graph)
}

fn copy_tags(tags: &Tags) -> source::Tags {
    tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn preserve_relations(data: &mut source::Data, relations: HashMap<i64, Relation>, nodes: &HashMap<i64, RawNode>) {
    // Retain direct memberships and their parent relations, including unrecognized tags and roles.
    loop {
        let mut changed = false;
        for relation in relations.values() {
            if data.relations.contains_key(&relation.id.0) {
                continue;
            }
            if !relation.refs.iter().any(|r| match r.member {
                OsmId::Way(id) => data.ways.contains_key(&id.0),
                OsmId::Node(id) => nodes.contains_key(&id.0),
                OsmId::Relation(id) => data.relations.contains_key(&id.0),
            }) {
                continue;
            }
            data.relations.insert(
                relation.id.0,
                source::Relation {
                    id: relation.id.0,
                    tags: copy_tags(&relation.tags),
                    members: relation
                        .refs
                        .iter()
                        .map(|r| {
                            (
                                match r.member {
                                    OsmId::Way(id) => source::Id::Way(id.0),
                                    OsmId::Node(id) => source::Id::Node(id.0),
                                    OsmId::Relation(id) => source::Id::Relation(id.0),
                                },
                                r.role.to_string(),
                            )
                        })
                        .collect(),
                },
            );
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

fn build_graph(
    ways: HashMap<i64, RawWay>,
    nodes: HashMap<i64, RawNode>,
    usage: HashMap<i64, u32>,
    rules: Vec<Restriction>,
    mut counts: Counts,
) -> Result<Graph, String> {
    let mut graph = Graph {
        points: Vec::new(),
        node_ids: Vec::new(),
        node_access: Vec::new(),
        osm: source::Data {
            nodes: nodes
                .iter()
                .map(|(&id, n)| (id, source::Node { id, point: n.point, tags: copy_tags(&n.tags) }))
                .collect(),
            ways: ways
                .values()
                .map(|w| (w.id, source::Way { id: w.id, nodes: w.nodes.clone(), tags: copy_tags(&w.tags) }))
                .collect(),
            relations: BTreeMap::new(),
        },
        roads: Vec::new(),
        forbidden: Vec::new(),
        forbidden_foot: Vec::new(),
        warnings: Vec::new(),
    };
    let mut junctions: HashSet<_> = usage.into_iter().filter(|(_, n)| *n > 1).map(|(id, _)| id).collect();
    junctions.extend(
        nodes
            .iter()
            .filter(|(_, n)| {
                n.tags.contains("highway", "traffic_signals")
                    || n.tags.contains("highway", "crossing") && n.tags.contains("crossing", "traffic_signals")
            })
            .map(|(id, _)| *id),
    );
    junctions.extend(rules.iter().map(|r| r.via));
    junctions.extend(nodes.iter().filter(|(_, n)| n.crossing != (BIKE | FOOT | PUSH)).map(|(id, _)| *id));
    let mut point_ids = HashMap::new();
    let mut ordered: Vec<_> = ways.into_values().collect();
    ordered.sort_unstable_by_key(|w| w.id);
    for way in &ordered {
        junctions.insert(way.nodes[0]);
        junctions.insert(*way.nodes.last().unwrap());
        if way.nodes.first() == way.nodes.last() {
            junctions.insert(way.nodes[way.nodes.len() / 2]);
        }
    }
    for way in ordered {
        let mut run = Vec::new();
        for id in &way.nodes {
            if !nodes.contains_key(id) {
                emit_run(&mut graph, &mut point_ids, &nodes, &run, &way)?;
                run.clear();
                continue;
            }
            run.push(*id);
            if junctions.contains(id) && run.len() >= 2 {
                emit_run(&mut graph, &mut point_ids, &nodes, &run, &way)?;
                run.clear();
                run.push(*id);
            }
        }
        emit_run(&mut graph, &mut point_ids, &nodes, &run, &way)?;
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
    counts.insert("roads with uncertain default access", graph.roads.iter().filter(|r| r.uncertain_access).count());
    counts.insert("roads with unknown surface", graph.roads.iter().filter(|r| r.surface == Surface::Unknown).count());
    graph.warnings = counts.into_iter().filter(|(_, n)| *n != 0).map(|(label, n)| format!("{label}: {n}")).collect();
    graph.warnings.push("No DEM applied; elevation is unknown and climbing costs are not validated".into());
    graph.warnings.push("German access defaults; ferry schedules, conditional access and via-way restrictions require conservative handling".into());
    if graph.roads.is_empty() {
        return Err("No routable roads within input bounds".into());
    }
    Ok(graph)
}

fn emit_run(
    graph: &mut Graph,
    point_ids: &mut HashMap<i64, u32>,
    nodes: &HashMap<i64, RawNode>,
    run: &[i64],
    way: &RawWay,
) -> Result<(), String> {
    if run.len() < 2 {
        return Ok(());
    }
    let shape: Vec<_> = run.iter().map(|id| nodes[id].point).collect();
    let length = shape.windows(2).map(|p| p[0].distance(p[1])).sum::<f64>();
    if length < 0.01 {
        return Ok(());
    }
    let mut endpoint = |id: i64| -> Result<u32, String> {
        if let Some(&existing) = point_ids.get(&id) {
            return Ok(existing);
        }
        let index = u32::try_from(graph.points.len()).map_err(|_| "Too many graph nodes")?;
        graph.points.push(nodes[&id].point);
        graph.node_ids.push(id);
        graph.node_access.push(nodes[&id].crossing);
        point_ids.insert(id, index);
        Ok(index)
    };
    let from = endpoint(run[0])?;
    let to = endpoint(*run.last().unwrap())?;
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
            uncertain_access: way.attributes.uncertain_access,
            structure: way.attributes.structure,
            shape,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osmpbfreader::{NodeId, Ref, RelationId, WayId};
    use route_engine::model::Profile;

    fn tags(values: &[(&str, &str)]) -> Tags {
        values.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    fn way(id: i64, nodes: &[i64], values: &[(&str, &str)]) -> RawWay {
        RawWay {
            id,
            nodes: nodes.to_vec(),
            attributes: attributes(&tags(values), &mut Counts::new()).unwrap(),
            tags: tags(values),
        }
    }

    fn fixture(ways: Vec<RawWay>, present: &[i64], rules: Vec<Restriction>) -> Graph {
        let mut usage = HashMap::new();
        for way in &ways {
            for id in &way.nodes {
                *usage.entry(*id).or_default() += 1;
            }
        }
        let nodes = present
            .iter()
            .map(|id| {
                (
                    *id,
                    RawNode {
                        point: Point { lat: 47_000_000, lon: 10_000_000 + *id as i32 * 100, elevation: NO_ELEVATION },
                        crossing: BIKE | FOOT | PUSH,
                        tags: Tags::default(),
                    },
                )
            })
            .collect();
        build_graph(ways.into_iter().map(|w| (w.id, w)).collect(), nodes, usage, rules, Counts::new()).unwrap()
    }

    #[test]
    fn source_tags_and_relation_roles_survive_graph_compilation() {
        let source = way(
            10,
            &[1, 2],
            &[
                ("highway", "tertiary"),
                ("smoothness", "intermediate"),
                ("maxspeed:forward", "70"),
                ("cycleway:right", "lane"),
                ("custom:future", "retained"),
            ],
        );
        let expected = copy_tags(&source.tags);
        let mut graph = fixture(vec![source], &[1, 2], vec![]);
        let relation = Relation {
            id: RelationId(20),
            tags: tags(&[("type", "route"), ("route", "bicycle"), ("network", "lcn")]),
            refs: vec![Ref { member: OsmId::Way(WayId(10)), role: "forward".into() }],
        };
        let parent = Relation {
            id: RelationId(21),
            tags: tags(&[("type", "superroute"), ("route", "bicycle")]),
            refs: vec![Ref { member: OsmId::Relation(RelationId(20)), role: "section".into() }],
        };
        preserve_relations(&mut graph.osm, [(20, relation), (21, parent)].into_iter().collect(), &HashMap::new());
        assert_eq!(graph.osm.ways[&10].tags, expected);
        assert_eq!(graph.osm.ways[&10].nodes, [1, 2]);
        assert_eq!(graph.osm.relations[&20].members, [(source::Id::Way(10), "forward".into())]);
        assert_eq!(graph.osm.relations[&21].members, [(source::Id::Relation(20), "section".into())]);
        let profile = Profile::presets().into_iter().find(|p| p.name == "road/quieter").unwrap();
        let mut objects = HashMap::new();
        let manifest =
            crate::prepare(&graph, "source-test".into(), [9.0, 46.0, 11.0, 48.0], &[profile], vec![], |bytes| {
                let key = route_engine::package::digest(bytes);
                objects.insert(key.clone(), bytes.to_vec());
                Ok(key)
            })
            .unwrap();
        let pages: Vec<String> = route_engine::storage::decode(&objects[&manifest.osm.ways.blocks[0]]).unwrap();
        let retained: Vec<source::Way> = route_engine::storage::decode(&objects[&pages[0]]).unwrap();
        assert_eq!(retained[0].tags, expected);
        let pages: Vec<String> = route_engine::storage::decode(&objects[&manifest.osm.relations.blocks[0]]).unwrap();
        let relations: Vec<source::Relation> = route_engine::storage::decode(&objects[&pages[0]]).unwrap();
        assert_eq!(relations.len(), 2);
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
        let attrs = attributes(
            &tags(&[("highway", "path"), ("bicycle:pushing:conditional", "no @ (wet)")]),
            &mut Counts::new(),
        )
        .unwrap();
        assert_eq!(attrs.access, [BIKE | FOOT; 2]);
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
    fn unsupported_conditions_close_only_affected_modes() {
        let mut counts = Counts::new();
        let attrs =
            attributes(&tags(&[("highway", "path"), ("bicycle:conditional", "no @ (wet)")]), &mut counts).unwrap();
        assert_eq!(attrs.access, [FOOT | PUSH; 2]);
        assert_eq!(counts["ways with excluded conditional modes"], 1);
        let attrs =
            attributes(&tags(&[("highway", "residential"), ("oneway:foot:conditional", "yes @ (Mo-Fr)")]), &mut counts)
                .unwrap();
        assert_eq!(attrs.access, [BIKE; 2]);
        let attrs = attributes(&tags(&[("highway", "path"), ("bicycle", "destination")]), &mut counts).unwrap();
        assert_eq!(attrs.access, [FOOT | PUSH; 2]);
    }

    #[test]
    fn freight_conditions_do_not_close_a_cycling_ascent() {
        for condition in ["agricultural @ hazmat:water", "no @ (hazmat)"] {
            let graph = fixture(
                vec![way(
                    10,
                    &[1, 2, 3],
                    &[("highway", "tertiary"), ("ref", "L 186"), ("access:conditional", condition)],
                )],
                &[1, 2, 3],
                vec![],
            );
            assert_eq!(graph.roads.len(), 2);
            assert!(graph.roads.iter().all(|r| r.access == BIKE | FOOT | PUSH));
        }
        let attrs = attributes(
            &tags(&[("highway", "tertiary"), ("motor_vehicle:conditional", "no @ (Mo-Fr)")]),
            &mut Counts::new(),
        )
        .unwrap();
        assert_eq!(attrs.access, [BIKE | FOOT | PUSH; 2]);
        for condition in ["no @ (wet)", "agricultural @ hazmat:water; no @ (Mo-Fr)", "no @ (!hazmat:water)"] {
            assert!(attributes(
                &tags(&[("highway", "tertiary"), ("access:conditional", condition)]),
                &mut Counts::new()
            )
            .is_none());
        }
        assert!(attributes(&tags(&[("highway", "construction"), ("construction", "tertiary")]), &mut Counts::new())
            .is_none());
    }

    #[test]
    fn topology_uses_ids_and_never_joins_across_missing_nodes() {
        let graph = fixture(
            vec![way(10, &[1, 2, 3, 4, 5, 6, 7], &[("highway", "path")]), way(20, &[2, 8], &[("highway", "path")])],
            &[1, 2, 3, 5, 6, 7, 8],
            vec![],
        );
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
        let mut ways: HashMap<_, _> =
            [way(10, &[1, 2], &[("highway", "path")]), way(20, &[2, 3], &[("highway", "path")])]
                .into_iter()
                .map(|w| (w.id, w))
                .collect();
        let relation = Relation {
            id: RelationId(1),
            tags: tags(&[
                ("type", "restriction"),
                ("restriction", "no_right_turn"),
                ("except", "bicycle"),
                ("restriction:foot", "no_right_turn"),
            ]),
            refs: vec![
                Ref { member: OsmId::Way(WayId(10)), role: "from".into() },
                Ref { member: OsmId::Node(NodeId(2)), role: "via".into() },
                Ref { member: OsmId::Way(WayId(20)), role: "to".into() },
            ],
        };
        let mut rules = Vec::new();
        relation_rules(&relation, &mut ways, &mut rules, &mut Counts::new());
        let graph = fixture(ways.into_values().collect(), &[1, 2, 3], rules);
        assert!(graph.forbidden.is_empty());
        assert_eq!(graph.forbidden_foot.len(), 1);
        let (from, to) = graph.forbidden_foot[0];
        assert!(!graph.permits_turn(from, to, true));
        assert!(graph.permits_turn(from, to, false));
    }

    #[test]
    fn via_way_restriction_exclusion_is_explicit_and_mode_specific() {
        let mut ways: HashMap<_, _> = [
            way(10, &[1, 2], &[("highway", "path")]),
            way(20, &[2, 3], &[("highway", "path")]),
            way(30, &[3, 4], &[("highway", "path")]),
        ]
        .into_iter()
        .map(|w| (w.id, w))
        .collect();
        let relation = Relation {
            id: RelationId(1),
            tags: tags(&[("type", "restriction"), ("restriction", "no_right_turn")]),
            refs: [(10, "from"), (20, "via"), (30, "to")]
                .into_iter()
                .map(|(id, role)| Ref { member: OsmId::Way(WayId(id)), role: role.into() })
                .collect(),
        };
        let mut counts = Counts::new();
        let mut rules = Vec::new();
        relation_rules(&relation, &mut ways, &mut rules, &mut counts);
        assert!(rules.is_empty());
        assert!(ways.values().all(|w| w.attributes.access == [FOOT; 2]));
        assert_eq!(counts["conservatively excluded unsupported restriction relations"], 1);
    }

    #[test]
    fn hiking_difficulty_does_not_use_mtb_scale() {
        let graph = fixture(
            vec![way(10, &[1, 2], &[("highway", "path"), ("mtb:scale", "6"), ("sac_scale", "hiking")])],
            &[1, 2],
            vec![],
        );
        let profiles = Profile::presets();
        assert!(profiles.iter().find(|p| p.walking).unwrap().permits(&graph.roads[0]));
        assert!(profiles.iter().filter(|p| !p.walking).all(|p| !p.permits(&graph.roads[0])));
        let difficult =
            fixture(vec![way(10, &[1, 2], &[("highway", "path"), ("sac_scale", "alpine_hiking")])], &[1, 2], vec![]);
        assert!(profiles.iter().all(|p| !p.permits(&difficult.roads[0])));
    }

    #[test]
    fn node_dismount_permission_does_not_become_cycling_permission() {
        let ways = [(10, way(10, &[1, 2, 3], &[("highway", "path")]))].into_iter().collect();
        let nodes = (1..=3)
            .map(|id| {
                (
                    id,
                    RawNode {
                        point: Point { lat: 47_000_000, lon: 10_000_000 + id as i32 * 100, elevation: NO_ELEVATION },
                        tags: Tags::default(),
                        crossing: if id == 2 {
                            crossing(&tags(&[("bicycle", "dismount")]), &mut Counts::new())
                        } else {
                            BIKE | FOOT | PUSH
                        },
                    },
                )
            })
            .collect();
        let graph =
            build_graph(ways, nodes, [(1, 1), (2, 1), (3, 1)].into_iter().collect(), vec![], Counts::new()).unwrap();
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
        let mut profile = Profile::presets().into_iter().find(|p| p.name == "road").unwrap();
        assert_eq!(crate::cost::Costing::new(&graph, &profile).unwrap().transition(before, after), Some(300));
        profile.pushing = false;
        assert_eq!(crate::cost::Costing::new(&graph, &profile).unwrap().transition(before, after), None);
    }
}
