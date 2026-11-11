//! Streaming regional OSM import. The import reads its large tables only in order or through a
//! sort: it joins the nodes to the ways by a sort of the node references, and it reads the tags of
//! each object once.
use crate::column::Column;
use crate::layout::{self, Buckets};
use crate::source::{
    self, Builder, Data, Id, NodeRef, Oneway, Relations, Rule, Sets, Table, Tags, Way, WayFacts, Ways,
};
use crate::terrain::{self, Height};
use crate::{base, cost, country::Country, sort, Graph, RoadSource, Turn};
use osmpbfreader::{OsmId, OsmObj, OsmPbfReader};
use planner_router::closures::{Closure, Kind};
use planner_router::model::{Road, Surface, BIKE, FOOT, PUSH};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::iter::Peekable;
use std::path::PathBuf;

/// The counts of the import that the warnings of the manifest report, by label.
pub type Counts = BTreeMap<String, usize>;

fn count(counts: &mut Counts, key: &str) {
    *counts.entry(key.into()).or_default() += 1;
}

#[derive(Clone, Copy)]
struct Attributes {
    class: u8,
    surface: Surface,
    access: [u8; 2],
    difficulty: u8,
    hiking_difficulty: Option<u8>,
    structure: bool,
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

/// `facts` holds one row for each way of `ways`. A via-way restriction takes its modes from the
/// facts of its routable member ways.
fn relation_rules(
    relation: &source::Relation,
    ways: &Table<Ways>,
    facts: &mut [WayFacts],
    rules: &mut Column<Rule>,
    counts: &mut Counts,
) {
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
    let find = |facts: &[WayFacts], id: i64| ways.index(id).filter(|&row| facts[row].routable());
    let members: Vec<_> = relation.members().filter_map(|(member, _)| member.way()).collect();
    if !members.iter().any(|&id| find(facts, id).is_some()) {
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
            rules.push(Rule {
                from: from[0],
                to: to[0],
                via: via_node.unwrap(),
                only: u8::from(value.starts_with("only_")),
                u_turn: u8::from(value.ends_with("u_turn")),
                walking: u8::from(walking),
                padding: [0; 5],
            });
        }
    }
    if excluded != 0 {
        count(counts, "conservatively excluded unsupported restriction relations");
        for id in members {
            if let Some(row) = find(facts, id) {
                for mode in &mut facts[row].access {
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

/// The source of the roads: the objects of the import with the facts of each way, the closure sets
/// of the ways and the nodes, the turn rules of the relations, and the counts so far.
pub struct Source {
    pub osm: Data,
    pub sets: Sets,
    pub rules: Column<Rule>,
    pub counts: Counts,
}

/// The import of `source` and then of `roads`.
pub fn import(paths: &[PathBuf], bounds: [f64; 4], terrain: Option<Height>) -> Result<Graph, String> {
    roads(source(paths, bounds)?, terrain)
}

/// Bounds are [west, south, east, north]. Missing or outside nodes break ways. The nodes of the
/// input must come in ID order, as `osmium sort` writes them.
pub fn source(paths: &[PathBuf], bounds: [f64; 4]) -> Result<Source, String> {
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
    let (mut ways, duplicates) = ways.finish();
    counts.insert("duplicate input ways".into(), duplicates);
    // Only a relation with a highway, a node or a relation as a member can restrict a turn or be preserved.
    let relations = relations.finish().0.filter(|relation| {
        relation.members().any(|(member, _)| match member {
            Id::Way(id) => ways.get(id).is_some(),
            Id::Node(_) | Id::Relation(_) => true,
        })
    });
    let mut sets = Sets::default();
    let mut join = Join::new(&ways, &relations, bounds);
    read(paths, |object| {
        if let OsmObj::Node(node) = object {
            join.node(
                node.id.0,
                [node.decimicro_lat, node.decimicro_lon],
                || copy_tags(&node.tags),
                &mut sets,
                &mut counts,
            );
        }
    })?;
    let (points, members) = join.finish(&mut counts)?;
    ways.set_points(points);
    Ok(scan(Data { ways, relations, members }, sets, counts))
}

/// Pass 2 of the import: merges the nodes of the input, in ID order, with the node references of
/// the ways sorted by (node ID, reference position), and with the sorted node members of the
/// relations.
struct Join {
    bounds: [f64; 4],
    references: Peekable<sort::Sorted<(i64, u64)>>,
    members: Peekable<sort::Sorted<i64>>,
    /// (reference position, its node) for each reference.
    records: Column<(u64, NodeRef)>,
    /// The node members that a way references and that are present.
    found: Column<i64>,
    previous: Option<i64>,
    unsorted: bool,
    /// The reference positions of one node.
    positions: Vec<u64>,
}

impl Join {
    fn new(ways: &Table<Ways>, relations: &Table<Relations>, bounds: [f64; 4]) -> Self {
        let references = ways.references().iter().enumerate().map(|(position, &id)| (id, position as u64)).collect();
        let members = relations
            .iter()
            .flat_map(|relation| {
                relation
                    .members()
                    .filter_map(|(member, _)| if let Id::Node(id) = member { Some(id) } else { None })
                    .collect::<Vec<_>>()
            })
            .collect();
        Join {
            bounds,
            references: sort::sorted(references).peekable(),
            members: sort::sorted(members).peekable(),
            records: Column::default(),
            found: Column::default(),
            previous: None,
            unsorted: false,
            positions: Vec::new(),
        }
    }

    /// Takes the reference positions of the next referenced node into `positions`, when its ID is
    /// below `below`. Returns its ID.
    fn next_group(&mut self, below: Option<i64>) -> Option<i64> {
        let &(id, _) = self.references.peek()?;
        if below.is_some_and(|below| id >= below) {
            return None;
        }
        self.positions.clear();
        while let Some((_, position)) = self.references.next_if(|&(next, _)| next == id) {
            self.positions.push(position);
        }
        Some(id)
    }

    /// Each referenced node that is not in the input or not inside the bounds.
    fn missing(&mut self, below: Option<i64>, counts: &mut Counts) {
        while self.next_group(below).is_some() {
            count(counts, "outside bounds or missing referenced nodes");
            let node = NodeRef::missing(self.positions.len() > 1);
            for &position in &self.positions {
                self.records.push((position, node));
            }
        }
    }

    /// `decimicro` is [lat, lon] in 1e-7 degrees. The tag work of a node runs once, and only for
    /// a node that a way references and that is present.
    fn node<'t>(
        &mut self,
        id: i64,
        decimicro: [i32; 2],
        tags: impl FnOnce() -> Tags<'t>,
        sets: &mut Sets,
        counts: &mut Counts,
    ) {
        if self.unsorted || self.previous.is_some_and(|previous| id <= previous) {
            // A second copy of a node keeps the first.
            self.unsorted |= self.previous.is_some_and(|previous| id < previous);
            return;
        }
        self.previous = Some(id);
        self.missing(Some(id), counts);
        if self.references.peek().is_none_or(|&(next, _)| next != id) {
            return;
        }
        self.next_group(None);
        let (lat, lon) = (decimicro[0] as f64 * 1e-7, decimicro[1] as f64 * 1e-7);
        let bounds = self.bounds;
        if lon < bounds[0] || lat < bounds[1] || lon > bounds[2] || lat > bounds[3] {
            count(counts, "outside bounds or missing referenced nodes");
            let node = NodeRef::missing(self.positions.len() > 1);
            for &position in &self.positions {
                self.records.push((position, node));
            }
            return;
        }
        let shared = if self.positions.len() > 1 { NodeRef::SHARED } else { 0 };
        let mut node = NodeRef {
            lat: (lat * 1e6).round() as i32,
            lon: (lon * 1e6).round() as i32,
            closures: u32::MAX,
            access: BIKE | FOOT | PUSH,
            flags: NodeRef::PRESENT | shared,
            padding: [0; 2],
        };
        let tags = tags();
        if !tags.is_empty() {
            node.access = crossing(&tags, counts);
            let signals = tags.contains("highway", "traffic_signals")
                || tags.contains("highway", "crossing") && tags.contains("crossing", "traffic_signals");
            node.closures = sets.insert(node_closures(tags.iter()));
            if signals {
                node.flags |= NodeRef::SIGNALS;
            }
            // A node with a possible closure ends its roads, so the road that arrives there reports it.
            if signals || node.access != BIKE | FOOT | PUSH || node.closures != u32::MAX {
                node.flags |= NodeRef::MARKED;
            }
        }
        for &position in &self.positions {
            self.records.push((position, node));
        }
        while self.members.next_if(|&member| member < id).is_some() {}
        if self.members.next_if_eq(&id).is_some() {
            self.found.push(id);
            while self.members.next_if_eq(&id).is_some() {}
        }
    }

    /// The node of each reference, in reference order, and the node members of `Data`.
    fn finish(mut self, counts: &mut Counts) -> Result<(Column<NodeRef>, Column<i64>), String> {
        if self.unsorted {
            return Err("the nodes of the input are not sorted by ID; sort it with `osmium sort`".into());
        }
        self.missing(None, counts);
        let points = sort::sorted(std::mem::take(&mut self.records)).map(|(_, node)| node).collect();
        Ok((points, self.found))
    }
}

/// The highest cycle route rank of each way in a cycle route, forward and backward, in way ID
/// order.
pub(crate) fn cycle_ranks(relations: &Table<Relations>) -> impl Iterator<Item = (i64, [u8; 2])> {
    let mut pending: Vec<_> = relations
        .iter()
        .filter_map(|relation| {
            let tags = &relation.tags;
            if tags.get("route") != Some("bicycle") || !matches!(tags.get("type"), Some("route" | "superroute")) {
                return None;
            }
            let rank = match tags.get("network") {
                Some("icn") => 5,
                Some("ncn") => 4,
                Some("rcn") => 3,
                Some("lcn") => 2,
                _ => 1,
            };
            Some((relation.id, rank))
        })
        .collect();
    let mut visited = HashMap::<i64, u8>::new();
    // One record for each membership of a way; the merge below keeps the highest of each way.
    let mut ways = Column::<(i64, [u8; 2])>::default();
    while let Some((id, rank)) = pending.pop() {
        if visited.get(&id).is_some_and(|previous| *previous >= rank) {
            continue;
        }
        visited.insert(id, rank);
        let Some(relation) = relations.get(id) else {
            continue;
        };
        if relation.tags.contains("state", "proposed") {
            continue;
        }
        for (member, role) in relation.members() {
            match member {
                Id::Way(id) => {
                    // A `forward` or `backward` role follows the way's node order.
                    let ranks = [role != "backward", role != "forward"].map(|on| if on { rank } else { 0 });
                    ways.push((id, ranks));
                }
                Id::Relation(id) => pending.push((id, rank)),
                _ => {}
            }
        }
    }
    let mut ways = sort::sorted(ways).peekable();
    std::iter::from_fn(move || {
        let (id, mut ranks) = ways.next()?;
        while let Some((_, more)) = ways.next_if(|&(next, _)| next == id) {
            ranks = [ranks[0].max(more[0]), ranks[1].max(more[1])];
        }
        Some((id, ranks))
    })
}

/// One scan of the ways in ID order: what the tags of each way give with no profile.
fn facts(osm: &Data, sets: &mut Sets, counts: &mut Counts) -> Column<WayFacts> {
    let mut ranks = cycle_ranks(&osm.relations).peekable();
    osm.ways
        .iter()
        .map(|way| {
            let country = way
                .points
                .iter()
                .find(|point| point.present())
                .map_or_else(Country::default, |point| Country::at(point.point()));
            let (defaults, left_hand) = country.parts();
            let mut facts = WayFacts { defaults, left_hand, ..WayFacts::default() };
            while ranks.next_if(|(id, _)| *id < way.id).is_some() {}
            if let Some((_, found)) = ranks.next_if(|(id, _)| *id == way.id) {
                facts.ranks = found;
            }
            // The country changes the modes of a road class, never whether it is one.
            let known = source::highway_access(way.tags.get("highway").unwrap_or(""), Country::default());
            if known.is_none() && !way.tags.contains("route", "ferry") {
                return facts;
            }
            let Some(attributes) = attributes(&way.tags, country, counts) else {
                return facts;
            };
            facts.routable = 1;
            facts.class = attributes.class;
            facts.surface = source::surface_byte(attributes.surface);
            facts.difficulty = attributes.difficulty;
            facts.hiking_difficulty = source::hiking_byte(attributes.hiking_difficulty);
            facts.structure = u8::from(attributes.structure);
            facts.access = attributes.access;
            (facts.closures, facts.doubts) = way_closures(&way.tags, sets);
            facts
        })
        .collect()
}

/// The closure sets of a way forward and backward, and the doubtful modes of each.
pub(crate) fn way_closures(tags: &Tags, sets: &mut Sets) -> ([u32; 2], [u8; 2]) {
    let [forward, backward] = ["forward", "backward"].map(|direction| {
        let closures = source::closures(tags.iter(), &[direction]);
        (cost::doubts(&closures), sets.insert(closures))
    });
    ([forward.1, backward.1], [forward.0, backward.0])
}

/// The closure set of a road: that of its way in its direction, then that of the junction where
/// it arrives, so that the road reports both.
pub(crate) fn road_closures(sets: &mut Sets, way: u32, junction: u32) -> u32 {
    let mut set = sets.get(way).to_vec();
    set.extend_from_slice(sets.get(junction));
    sets.insert(set)
}

fn preserve_relations(osm: &Data) -> Table<Relations> {
    // Retain direct memberships and their parent relations, including unrecognized tags and roles.
    let members = osm.members.values();
    let mut kept = HashSet::new();
    loop {
        let before = kept.len();
        for relation in osm.relations.iter() {
            if !kept.contains(&relation.id)
                && relation.members().any(|(member, _)| match member {
                    Id::Way(id) => osm.ways.get(id).is_some(),
                    Id::Node(id) => members.binary_search(&id).is_ok(),
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
    osm.relations.filter(|relation| kept.contains(&relation.id))
}

/// A piece of one way between two junctions, as positions of its node references.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Run {
    pub first: usize,
    pub last: usize,
    pub length: f64,
}

/// The runs of one way that give roads, in node order. A run ends at a missing node and at a
/// junction; a run of fewer than 2 points or under 0.01 m gives no road. A reference is a junction
/// when its node is present and shared, marked, a via node of a turn rule (`vias` is sorted), or
/// the middle node of a closed way. The first and last node of a way start and end a run anyway.
/// A way without access in either direction gives no run. The runs depend only on `way` and
/// `vias`, so each call gives the same runs.
pub(crate) fn split(way: &Way, vias: &[i64]) -> impl Iterator<Item = Run> {
    let mut runs = Vec::new();
    let facts = way.facts;
    if !facts.routable() || facts.access == [0, 0] {
        return runs.into_iter();
    }
    let points = way.points;
    let closed = way.nodes.first() == way.nodes.last();
    let junction = |i: usize| {
        points[i].flags & (NodeRef::SHARED | NodeRef::MARKED) != 0
            || closed && i == way.nodes.len() / 2
            || vias.binary_search(&way.nodes[i]).is_ok()
    };
    let mut push = |first: usize, last: usize| {
        if last > first {
            let length = points[first..=last].windows(2).map(|p| p[0].point().distance(p[1].point())).sum::<f64>();
            if length >= 0.01 {
                runs.push(Run { first, last, length });
            }
        }
    };
    let mut start = None;
    for (i, point) in points.iter().enumerate() {
        if !point.present() {
            if let Some(first) = start.take() {
                push(first, i - 1);
            }
            continue;
        }
        let first = *start.get_or_insert(i);
        if i > first && junction(i) {
            push(first, i);
            start = Some(i);
        }
    }
    if let Some(first) = start {
        push(first, points.len() - 1);
    }
    runs.into_iter()
}

/// The junction of each via node that a road uses.
type Vias = HashMap<i64, u32>;

/// The (from, to) junction of each road in emit order, the junction of each via node that a road
/// uses, and the shape points of all roads before and after the resampling of the terrain.
type Walk = (Column<(u32, u32)>, Vias, [u64; 2]);

/// Walk 1: numbers the junctions that the roads use in the order of their first use, run by run,
/// and writes their columns into `graph`. The geometry pages store the junction numbers of each
/// road, so this order keeps the bytes of the package.
fn junctions(osm: &Data, vias: &[i64], graph: &mut Graph) -> Result<Walk, String> {
    // End `2k` is the first node of run `k`, end `2k + 1` its last.
    let mut ends = Column::<((i64, u64), NodeRef)>::default();
    let mut access = Column::<[u8; 2]>::default();
    let (mut points, mut shape) = ([0u64; 2], Vec::new());
    let mut run = 0u64;
    for way in osm.ways.iter() {
        for Run { first, last, .. } in split(&way, vias) {
            ends.push(((way.nodes[first], 2 * run), way.points[first]));
            ends.push(((way.nodes[last], 2 * run + 1), way.points[last]));
            access.push(way.facts.access);
            shape.clear();
            shape.extend(way.points[first..=last].iter().map(NodeRef::point));
            let roads = way.facts.access.iter().filter(|&&modes| modes != 0).count() as u64;
            points[0] += roads * shape.len() as u64;
            points[1] += roads * terrain::resampled_len(&shape) as u64;
            run += 1;
        }
    }
    // A junction is known by its first end, the smallest end number of its node.
    let mut firsts = Column::<(u64, (i64, NodeRef))>::default();
    let mut owners = Column::<(u64, u64)>::default();
    let mut current = None;
    for ((id, end), node) in sort::sorted(ends) {
        let first = match current {
            Some((known, first)) if known == id => first,
            _ => {
                firsts.push((end, (id, node)));
                end
            }
        };
        current = Some((id, first));
        owners.push((first, end));
    }
    let mut owners = sort::sorted(owners).peekable();
    let mut numbered = Column::<(u64, u32)>::default();
    let mut via_junctions = HashMap::new();
    for (number, (first, (id, node))) in sort::sorted(firsts).enumerate() {
        let number = u32::try_from(number).ok().filter(|&number| number != u32::MAX).ok_or("Too many graph nodes")?;
        graph.points.push(node.point());
        graph.node_ids.push(id);
        graph.node_access.push(node.access);
        graph.signals.push(u8::from(node.flags & NodeRef::SIGNALS != 0));
        graph.node_closures.push(node.closures);
        graph.node_doubts.push(cost::doubts(graph.sets.get(node.closures)));
        if vias.binary_search(&id).is_ok() {
            via_junctions.insert(id, number);
        }
        while let Some((_, end)) = owners.next_if(|&(owner, _)| owner == first) {
            numbered.push((end, number));
        }
    }
    let mut numbers = sort::sorted(numbered).map(|(_, junction)| junction);
    let mut pairs = Column::default();
    for &[forward, backward] in access.values() {
        let (Some(from), Some(to)) = (numbers.next(), numbers.next()) else { unreachable!("two ends per run") };
        if forward != 0 {
            pairs.push((from, to));
        }
        if backward != 0 {
            pairs.push((to, from));
        }
    }
    Ok((pairs, via_junctions, points))
}

/// Walk 2: the roads of each run, in the order of walk 1, into `buckets` by `keys`. `ends` and
/// `keys` have one entry for each road. Returns the roads in their final order.
fn emit(
    osm: &Data,
    vias: &[i64],
    (ends, keys): (&Column<(u32, u32)>, &Column<u64>),
    mut buckets: Buckets,
    terrain: Option<Height>,
    graph: &mut Graph,
) -> Result<(), String> {
    let mut roads = ends.values().iter().zip(keys.values());
    // The road set of each (way set, node set) pair.
    let mut road_sets = HashMap::<(u32, u32), u32>::new();
    for (row, way) in osm.ways.iter().enumerate() {
        let facts = way.facts;
        for Run { first, last, length } in split(&way, vias) {
            let shape: Vec<_> = way.points[first..=last].iter().map(NodeRef::point).collect();
            for direction in 0..2 {
                let access = facts.access[direction];
                if access == 0 {
                    continue;
                }
                let (&(from, to), &sort_key) = roads.next().expect("walk 1 gives each road its ends");
                let mut shape = shape.clone();
                if direction == 1 {
                    shape.reverse();
                }
                let key = (facts.closures[direction], graph.node_closures.values()[to as usize]);
                let sets = &mut graph.sets;
                let closures = *road_sets.entry(key).or_insert_with(|| road_closures(sets, key.0, key.1));
                let road = Road {
                    from,
                    to,
                    way: way.id,
                    reversed: direction == 1,
                    length_m: length.round().max(1.0) as u32,
                    ascent_m: 0,
                    descent_m: 0,
                    surface: facts.surface(),
                    class: facts.class,
                    access,
                    difficulty: facts.difficulty,
                    hiking_difficulty: facts.hiking_difficulty(),
                    structure: facts.structure(),
                    shape,
                };
                buckets.push(road, RoadSource { way_row: row as u32, closures }, sort_key)?;
            }
        }
    }
    graph.roads = buckets.finish(terrain)?.0;
    Ok(())
}

/// `osm` holds every highway with the node of each reference, the relations that may be
/// preserved and the node members of `Data`. Gives each way its facts, and keeps the ways with a
/// present node and the relations that may be preserved.
fn scan(mut osm: Data, mut sets: Sets, mut counts: Counts) -> Source {
    let mut facts = facts(&osm, &mut sets, &mut counts);
    let mut rules = Column::default();
    for relation in osm.relations.iter() {
        relation_rules(&relation, &osm.ways, facts.values_mut(), &mut rules, &mut counts);
    }
    osm.ways.set_facts(facts);
    osm.ways = osm.ways.filter(|way| way.points.iter().any(NodeRef::present));
    osm.relations = preserve_relations(&osm);
    Source { osm, sets, rules, counts }
}

/// The roads of `source` in their final order (`layout`), their junctions and their turn rules.
/// With `terrain`, their shapes are resampled and have heights. The import checks the counts of
/// the roads and their transitions before it emits the roads.
pub fn roads(source: Source, terrain: Option<Height>) -> Result<Graph, String> {
    let Source { osm, sets, rules, mut counts } = source;
    let mut graph = Graph { sets, ..Graph::default() };
    let mut vias: Vec<i64> = rules.values().iter().map(|rule| rule.via).collect();
    vias.sort_unstable();
    vias.dedup();
    let (ends, via_junctions, [shape_points, resampled]) = junctions(&osm, &vias, &mut graph)?;
    crate::check_counts((osm.ways.len(), osm.ways.references().len()), ends.values(), graph.points.len())?;
    crate::work::Guard::stage("emit");
    let points = graph.points.values();
    let keys = layout::keys(&ends, |_, to| layout::spatial_key(points[to as usize]))?;
    let points = shape_points + if terrain.is_some() { resampled } else { 0 };
    let bounds = layout::bounds(&keys, layout::bucket_count(ends.len() as u64, points)?);
    eprintln!(
        "{} the roads into {} buckets",
        if terrain.is_some() { "Sampling terrain and ordering" } else { "Ordering" },
        bounds.len() + 1
    );
    emit(&osm, &vias, (&ends, &keys), Buckets::new(bounds), terrain, &mut graph)?;
    drop((ends, keys));
    // The roads that depart from and arrive at each point, in road order.
    let [departures, arrivals] =
        [&graph.roads.from, &graph.roads.to].map(|ends| base::group(ends.values(), graph.points.len()));
    fn at((start, roads): &(Column<u32>, Column<u32>), point: u32) -> &[u32] {
        let start = start.values();
        &roads.values()[start[point as usize] as usize..start[point as usize + 1] as usize]
    }
    for &rule in rules.values() {
        let Some(&via) = via_junctions.get(&rule.via) else {
            count(&mut counts, "restrictions outside retained graph");
            continue;
        };
        let outgoing = at(&departures, via);
        let arrivals: Vec<_> =
            at(&arrivals, via).iter().copied().filter(|&id| graph.roads.way(id as usize) == rule.from).collect();
        let has_target = outgoing.iter().any(|&id| graph.roads.way(id as usize) == rule.to);
        if arrivals.is_empty() || !has_target {
            count(&mut counts, "restriction members missing at retained junction");
        }
        let forbidden = if rule.walking != 0 { &mut graph.forbidden_foot } else { &mut graph.forbidden };
        for from in arrivals {
            for &to in outgoing {
                let target = graph.roads.way(to as usize) == rule.to
                    && (rule.u_turn == 0 || graph.roads.from(from as usize) == graph.roads.to(to as usize));
                if target != (rule.only != 0) {
                    forbidden.push(Turn { from, to });
                }
            }
        }
    }
    for forbidden in [&mut graph.forbidden, &mut graph.forbidden_foot] {
        *forbidden = sort::sorted(std::mem::take(forbidden)).counted().map(|(turn, _)| turn).collect();
    }
    counts.insert("retained directed roads".into(), graph.roads.len());
    counts.insert("retained graph nodes".into(), graph.points.len());
    let unknown = (0..graph.roads.len()).filter(|&id| graph.roads.surface(id) == Surface::Unknown).count();
    counts.insert("roads with unknown surface".into(), unknown);
    graph.warnings = counts.into_iter().filter(|(_, n)| *n != 0).map(|(label, n)| format!("{label}: {n}")).collect();
    graph.warnings.push("No DEM applied; elevation is unknown and climbing costs are not validated".into());
    graph.warnings.push("Country access defaults; ferry schedules, conditional access and via-way restrictions require conservative handling".into());
    if graph.roads.is_empty() {
        return Err("No routable roads within input bounds".into());
    }
    graph.osm = osm;
    Ok(graph)
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

    /// Paths with the IDs `ids`, and their facts.
    fn routable(ids: &[i64]) -> (Table<Ways>, Vec<WayFacts>) {
        let mut ways = Builder::<Ways>::default();
        for &id in ids {
            ways.push(id, &[1, 2], &tags(&[("highway", "path")]));
        }
        let access = attributes(&tags(&[("highway", "path")]), &mut Counts::new()).unwrap().access;
        (ways.finish().0, vec![WayFacts { routable: 1, access, ..WayFacts::default() }; ids.len()])
    }

    fn relation(values: &[(&str, &str)], members: &[(Id, &str)]) -> Table<Relations> {
        let mut relations = Builder::<Relations>::default();
        relations.push(1, &tags(values), members.iter().copied());
        relations.finish().0
    }

    /// The ways of `ways`, and the joined nodes `present` in the order given: node `id` lies at
    /// longitude `id × 50` µdeg. The nodes of `tagged` have tags.
    fn join(
        ways: &[(i64, &[i64], Tags)],
        present: &[(i64, [i32; 2])],
        tagged: &[(i64, Tags)],
        relations: &Table<Relations>,
        bounds: [f64; 4],
    ) -> Result<(Table<Ways>, Column<i64>, Sets, Counts), String> {
        let mut builder = Builder::<Ways>::default();
        for (id, nodes, tags) in ways {
            builder.push(*id, nodes, tags);
        }
        let mut ways = builder.finish().0;
        let (mut sets, mut counts) = (Sets::default(), Counts::new());
        let mut join = Join::new(&ways, relations, bounds);
        for &(id, decimicro) in present {
            let tags = tagged.iter().find(|(node, _)| *node == id).map(|(_, tags)| tags.clone());
            join.node(id, decimicro, || tags.unwrap_or_default(), &mut sets, &mut counts);
        }
        let (points, members) = join.finish(&mut counts)?;
        ways.set_points(points);
        Ok((ways, members, sets, counts))
    }

    /// The graph of `ways` over the nodes `present`, outside every country; the nodes of `tagged`
    /// have tags.
    fn fixture(
        ways: &[(i64, &[i64], Tags)],
        present: &[i64],
        tagged: &[(i64, Tags)],
        relations: Table<Relations>,
    ) -> Graph {
        let mut present = present.to_vec();
        present.sort_unstable();
        let present: Vec<_> = present.iter().map(|&id| (id, [0, id as i32 * 500])).collect();
        let (ways, members, sets, counts) = join(ways, &present, tagged, &relations, [-1.0, -1.0, 1.0, 1.0]).unwrap();
        roads(scan(Data { ways, relations, members }, sets, counts), None).unwrap()
    }

    #[test]
    fn a_join_gives_each_reference_its_node() {
        let ways = [way(10, &[5, 3, 9], &[("highway", "path")]), way(20, &[3, 7], &[("highway", "path")])];
        let signal = [(5, tags(&[("highway", "traffic_signals")]))];
        let relations = relation(&[("type", "site")], &[(Id::Node(3), ""), (Id::Node(9), ""), (Id::Node(5), "")]);
        // Node 7 lies outside the bounds; node 9 is not in the input.
        let present = [(3, [1000, 3000]), (5, [2000, 5000]), (7, [0, 20_000_000])];
        let (ways, members, _, counts) = join(&ways, &present, &signal, &relations, [-1.0, -1.0, 1.0, 1.0]).unwrap();
        let all = BIKE | FOOT | PUSH;
        let node = |lat, lon, flags| NodeRef { lat, lon, closures: u32::MAX, access: all, flags, padding: [0; 2] };
        let shared = node(100, 300, NodeRef::PRESENT | NodeRef::SHARED);
        let signal = node(200, 500, NodeRef::PRESENT | NodeRef::MARKED | NodeRef::SIGNALS);
        let points: Vec<_> = ways.iter().map(|way| way.points.to_vec()).collect();
        assert_eq!(points, [vec![signal, shared, NodeRef::missing(false)], vec![shared, NodeRef::missing(false)]]);
        assert_eq!(members.values(), [3, 5]);
        assert_eq!(counts["outside bounds or missing referenced nodes"], 2);
        let unsorted = [(5, [0, 0]), (3, [0, 0])];
        let error = join(&[way(10, &[3, 5], &[])], &unsorted, &[], &Table::default(), [-1.0, -1.0, 1.0, 1.0]);
        assert!(error.is_err_and(|e| e.contains("osmium sort")));
    }

    #[test]
    fn junctions_take_their_numbers_in_the_order_of_first_use() {
        // The geometry pages store the junction numbers, so they follow the roads, not the node IDs.
        let ways = [way(10, &[9, 5, 1], &[("highway", "path")]), way(20, &[1, 3], &[("highway", "path")])];
        let graph = fixture(&ways, &[1, 3, 5, 9], &[], Default::default());
        assert_eq!(graph.node_ids.values(), [9, 1, 3]);
        let mut ends: Vec<_> = graph.roads.iter().map(|road| (road.from, road.to, road.reversed)).collect();
        ends.sort_unstable();
        assert_eq!(ends, [(0, 1, false), (1, 0, true), (1, 2, false), (2, 1, true)]);
        for road in graph.roads.iter() {
            let ends = [road.from, road.to].map(|junction| graph.points.values()[junction as usize]);
            assert_eq!(ends, [road.shape[0], road.shape[road.shape.len() - 1]]);
        }
    }

    #[test]
    fn a_profile_list_that_passes_the_manifest_limit_fails() {
        let graph = fixture(&[way(10, &[1, 2], &[("highway", "path")])], &[1, 2], &[], Default::default());
        // A million profiles need a manifest larger than format 9 allows.
        let limits = crate::Limits { profiles: 1_000_000, bounds: [-1.0, -1.0, 1.0, 1.0] };
        assert!(limits.check(&graph).is_err_and(|e| e.contains("manifest")));
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
        let Turn { from, to } = graph.forbidden_foot.values()[0];
        assert!(!graph.permits_turn(from, to, true));
        assert!(graph.permits_turn(from, to, false));
    }

    #[test]
    fn via_way_restriction_exclusion_is_explicit_and_mode_specific() {
        let (ways, mut facts) = routable(&[10, 20, 30]);
        let relation = relation(
            &[("type", "restriction"), ("restriction", "no_right_turn")],
            &[(Id::Way(10), "from"), (Id::Way(20), "via"), (Id::Way(30), "to")],
        );
        let mut counts = Counts::new();
        let mut rules = Column::default();
        relation_rules(&relation.iter().next().unwrap(), &ways, &mut facts, &mut rules, &mut counts);
        assert!(rules.is_empty());
        assert!(facts.iter().all(|w| w.access == [FOOT; 2]));
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
        let point = |id: i64| graph.node_ids.values().iter().position(|&node| node == id).unwrap();
        // Riding through an unknown barrier stays closed; walking and pushing pass.
        assert_eq!(
            (graph.node_access.values()[point(2)], graph.node_access.values()[point(3)]),
            (BIKE | FOOT | PUSH, FOOT | PUSH)
        );
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
        let (ways, mut facts) = routable(&[10, 20]);
        for key in ["restriction:conditional", "restriction:bicycle:conditional", "restriction:foot:conditional"] {
            let relation = relation(
                &[("type", "restriction"), (key, "no_right_turn @ (Mo-Fr 07:00-09:00)")],
                &[(Id::Way(10), "from"), (Id::Node(2), "via"), (Id::Way(20), "to")],
            );
            let mut rules = Column::default();
            relation_rules(&relation.iter().next().unwrap(), &ways, &mut facts, &mut rules, &mut Counts::new());
            assert!(rules.is_empty(), "{key}");
            assert!(facts.iter().all(|w| w.access == [BIKE | FOOT | PUSH; 2]), "{key}");
        }
    }

    #[test]
    fn hiking_difficulty_does_not_use_mtb_scale() {
        let path = |values| fixture(&[way(10, &[1, 2], values)], &[1, 2], &[], Default::default());
        let path = |values| path(values).roads.iter().next().unwrap();
        let road = path(&[("highway", "path"), ("mtb:scale", "6"), ("sac_scale", "hiking")]);
        let profiles = Profile::presets();
        assert!(profiles.iter().find(|p| p.walking).unwrap().permits(&road));
        assert!(profiles.iter().filter(|p| !p.walking).all(|p| p.permits(&road) == (p.name == "mtb/technical")));
        let alpine = path(&[("highway", "path"), ("sac_scale", "alpine_hiking")]);
        assert!(profiles.iter().all(|p| p.permits(&alpine) == (p.name == "hiking/alpine")));
        let climbing = path(&[("highway", "path"), ("sac_scale", "demanding_alpine_hiking")]);
        assert!(profiles.iter().all(|p| !p.permits(&climbing)));
    }

    #[test]
    fn mtb_scale_with_a_sign_counts_as_its_digit() {
        let ways = [way(10, &[1, 2], &[("highway", "path"), ("mtb:scale", "2+")])];
        let graph = fixture(&ways, &[1, 2], &[], Default::default());
        assert_eq!(graph.roads.iter().next().unwrap().mtb_scale(), Some(2));
    }

    #[test]
    fn node_dismount_permission_does_not_become_cycling_permission() {
        let ways = [way(10, &[1, 2, 3], &[("highway", "path")])];
        let graph = fixture(&ways, &[1, 2, 3], &[(2, tags(&[("bicycle", "dismount")]))], Default::default());
        assert!(graph.node_access.values().iter().any(|a| a & BIKE == 0 && a & PUSH != 0));
        assert!(graph.forbidden.is_empty());
        assert!(graph.forbidden_foot.is_empty());
        let before = graph
            .roads
            .iter()
            .position(|r| graph.node_ids.values()[r.from as usize] == 1 && graph.node_ids.values()[r.to as usize] == 2)
            .unwrap() as u32;
        let after = graph
            .roads
            .iter()
            .position(|r| graph.node_ids.values()[r.from as usize] == 2 && graph.node_ids.values()[r.to as usize] == 3)
            .unwrap() as u32;
        let profile = Profile::presets().into_iter().find(|p| p.name == "road").unwrap();
        let transition = |graph: &Graph| {
            let bases = crate::cost::way_bases(&graph.osm, std::slice::from_ref(&profile)).unwrap();
            let mut dictionary = Default::default();
            crate::cost::Costing::new(graph, &profile, &bases, &mut dictionary).unwrap().transition(before, after)
        };
        assert_eq!(transition(&graph), Some(300));
        // A crossing that bans pushing as well closes the transition for a cyclist.
        let mut graph = graph;
        graph.node_access.values_mut()[graph.roads.to(before as usize) as usize] = FOOT;
        assert_eq!(transition(&graph), None);
    }
}
