//! PROTOTYPE. The switches of the candidate cost changes, and the hooks that the real cost code
//! calls. With every switch off, each hook leaves the cost of today. The candidates measure the
//! reach of an idea; they are not proposals.
use crate::road_bike::tag;
use crate::source::Tags;
use crate::{traffic, Graph};
use planner_router::model::{Point, Profile, Road};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::OnceLock;

/// The length before and after a junction that gives the heading there.
const HEADING_M: f64 = 15.0;
/// Signal nodes that a road shorter than this joins are one signalled junction.
const SIGNAL_GROUP_M: u32 = 40;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Variant {
    /// V1: a `path` or `footway` with `bicycle=designated` has the class of a cycleway.
    pub path_as_cycleway: bool,
    /// V2: the turn cost `T`, at a junction of three or more road ends; 0 for none.
    pub turn: u32,
    /// V2T: no turn cost onto a road with the same name, ref or cycle route.
    pub through: bool,
    /// V3: the class preference of a main road follows its tagged speed limit.
    pub speed_class: bool,
    /// V4: a road of class 1, 3 or 4 in a signed cycle route loses its class preference.
    pub full_bonus: bool,
    /// V5: a road of class 0, 1 or 2 with no surface tag has the surface factor of a paved road.
    pub paved_unknown: bool,
    /// The cost `S` of a signalled junction for a cycling profile with weights; 0 for none.
    pub signal: u32,
    /// Hiking (a): a road of a signed hiking route has no class preference.
    pub walk_bonus: bool,
    /// Hiking (b): the turn cost `T` on foot, with the through rule on names, refs and hiking
    /// routes; 0 for none.
    pub walk_turn: u32,
}

static FLAGS: AtomicU32 = AtomicU32::new(0);
static NUMBERS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

pub fn set(variant: Variant) {
    let flags = [
        variant.path_as_cycleway,
        variant.through,
        variant.speed_class,
        variant.full_bonus,
        variant.paved_unknown,
        variant.walk_bonus,
    ];
    FLAGS.store(flags.iter().enumerate().map(|(bit, on)| u32::from(*on) << bit).sum(), Relaxed);
    for (number, value) in NUMBERS.iter().zip([variant.turn, variant.signal, variant.walk_turn]) {
        number.store(value, Relaxed);
    }
}

pub fn get() -> Variant {
    let flags = FLAGS.load(Relaxed);
    let on = |bit: u32| flags & (1 << bit) != 0;
    Variant {
        path_as_cycleway: on(0),
        through: on(1),
        speed_class: on(2),
        full_bonus: on(3),
        paved_unknown: on(4),
        walk_bonus: on(5),
        turn: NUMBERS[0].load(Relaxed),
        signal: NUMBERS[1].load(Relaxed),
        walk_turn: NUMBERS[2].load(Relaxed),
    }
}

/// The route relations of one kind that list each way row, sorted.
type Routes = HashMap<u32, Vec<i64>>;

/// What the hooks and the measures read of the graph.
pub struct Context {
    /// The undirected road ends at each junction.
    pub degree: Vec<u8>,
    /// The `name` and the `ref` of each way row as numbers; 0 for none.
    names: Vec<[u32; 2]>,
    /// The cycle routes, then the hiking routes.
    routes: [Routes; 2],
    /// The ways of the hiking routes.
    walked: HashSet<i64>,
}

static CONTEXT: OnceLock<Context> = OnceLock::new();

pub fn context() -> &'static Context {
    CONTEXT.get().expect("`prepare` ran before the costs")
}

/// Builds the context of `graph`. The process bakes one graph.
pub fn prepare(graph: &Graph) {
    let roads = &graph.roads;
    let mut ends = Vec::with_capacity(2 * roads.len());
    for road in 0..roads.len() {
        let key = (roads.way_row(road), roads.length_m.values()[road]);
        ends.push((roads.from(road), key, roads.to(road)));
        ends.push((roads.to(road), key, roads.from(road)));
    }
    ends.sort_unstable();
    ends.dedup();
    let mut degree = vec![0u8; graph.points.len()];
    for (junction, ..) in ends {
        degree[junction as usize] = degree[junction as usize].saturating_add(1);
    }
    let mut ids = HashMap::<String, u32>::new();
    let mut id = |value: &str| match value {
        "" => 0,
        value => {
            let next = ids.len() as u32 + 1;
            *ids.entry(value.to_string()).or_insert(next)
        }
    };
    let names = graph.osm.ways.iter().map(|way| [id(tag(&way.tags, "name")), id(tag(&way.tags, "ref"))]).collect();
    let mut routes = [Routes::new(), Routes::new()];
    let mut walked = HashSet::new();
    for relation in graph.osm.relations.iter() {
        let tags = &relation.tags;
        let kind = match tags.get("route") {
            Some("bicycle") => 0,
            Some("hiking" | "foot") => 1,
            _ => continue,
        };
        if !matches!(tags.get("type"), Some("route" | "superroute")) || tags.contains("state", "proposed") {
            continue;
        }
        for (id, row) in
            relation.members().filter_map(|(member, _)| Some((member.way()?, graph.osm.ways.index(member.way()?)?)))
        {
            routes[kind].entry(row as u32).or_default().push(relation.id);
            if kind == 1 {
                walked.insert(id);
            }
        }
    }
    for list in routes.iter_mut().flat_map(HashMap::values_mut) {
        list.sort_unstable();
        list.dedup();
    }
    let _ = CONTEXT.set(Context { degree, names, routes, walked });
}

impl Context {
    /// The name, the ref and the routes of a way row, as tokens that two ways of one road or one
    /// route share: the cycle routes, or the hiking routes for `walking`.
    pub fn labels(&self, row: u32, walking: bool) -> Vec<u64> {
        let [name, reference] = self.names[row as usize];
        let mut labels = Vec::new();
        if name != 0 {
            labels.push(1 << 40 | u64::from(name));
        }
        if reference != 0 {
            labels.push(2 << 40 | u64::from(reference));
        }
        let routes = self.routes[usize::from(walking)].get(&row);
        labels.extend(routes.into_iter().flatten().map(|&id| 3 << 40 | id as u64));
        labels
    }

    fn through(&self, before: u32, after: u32, walking: bool) -> bool {
        let (a, b) = (self.names[before as usize], self.names[after as usize]);
        if a[0] != 0 && a[0] == b[0] || a[1] != 0 && a[1] == b[1] {
            return true;
        }
        let routes = &self.routes[usize::from(walking)];
        match (routes.get(&before), routes.get(&after)) {
            (Some(a), Some(b)) => a.iter().any(|id| b.binary_search(id).is_ok()),
            _ => false,
        }
    }
}

/// Metres east and north from `a` to `b`.
fn vector(a: Point, b: Point) -> (f64, f64) {
    let scale = (f64::from(a.lat) * 1e-6).to_radians().cos();
    (f64::from(b.lon - a.lon) * 0.111_32 * scale, f64::from(b.lat - a.lat) * 0.111_32)
}

/// The cosine of the change of heading from the end of `before` to the start of `after`, each
/// heading over `HEADING_M`.
pub fn cosine(graph: &Graph, before: u32, after: u32) -> f64 {
    let (a, b) = (graph.roads.shape(before as usize), graph.roads.shape(after as usize));
    let end = a[a.len() - 1];
    let back = a.iter().rev().skip(1).find(|p| p.distance(end) >= HEADING_M).unwrap_or(&a[0]);
    let ahead = b.iter().skip(1).find(|p| p.distance(b[0]) >= HEADING_M).unwrap_or(&b[b.len() - 1]);
    let (u, v) = (vector(*back, end), vector(b[0], *ahead));
    let lengths = u.0.hypot(u.1) * v.0.hypot(v.1);
    if lengths == 0.0 {
        return 1.0;
    }
    ((u.0 * v.0 + u.1 * v.1) / lengths).clamp(-1.0, 1.0)
}

/// Whether `road` arrives at a signal node that starts a signalled junction: not where the road
/// is shorter than `SIGNAL_GROUP_M` and starts at a signal node too.
pub fn signalled(graph: &Graph, road: u32) -> bool {
    let (roads, signals, road) = (&graph.roads, graph.signals.values(), road as usize);
    signals[roads.to(road) as usize] != 0
        && !(roads.length_m.values()[road] < SIGNAL_GROUP_M && signals[roads.from(road) as usize] != 0)
}

/// The signal nodes, and the signalled junctions that they form: the groups of signal nodes that
/// roads shorter than `SIGNAL_GROUP_M` join.
pub fn signal_counts(graph: &Graph) -> (usize, usize) {
    let (roads, signals) = (&graph.roads, graph.signals.values());
    let mut group: Vec<u32> = (0..signals.len() as u32).collect();
    fn find(group: &mut [u32], mut node: u32) -> u32 {
        while group[node as usize] != node {
            group[node as usize] = group[group[node as usize] as usize];
            node = group[node as usize];
        }
        node
    }
    for road in 0..roads.len() {
        let (from, to) = (roads.from(road), roads.to(road));
        if roads.length_m.values()[road] < SIGNAL_GROUP_M && signals[from as usize] != 0 && signals[to as usize] != 0 {
            let (a, b) = (find(&mut group, from), find(&mut group, to));
            group[a as usize] = b;
        }
    }
    let nodes: Vec<u32> = (0..signals.len() as u32).filter(|&node| signals[node as usize] != 0).collect();
    let groups: HashSet<u32> = nodes.iter().map(|&node| find(&mut group, node)).collect();
    (nodes.len(), groups.len())
}

/// The hook of `cost::transition`: the turn cost and the signal cost. A U-turn onto the same way
/// is no through road.
pub fn turn_cost(graph: &Graph, profile: &Profile, before: u32, after: u32) -> u64 {
    let variant = get();
    let (turn, signal) = if profile.walking { (variant.walk_turn, 0) } else { (variant.turn, variant.signal) };
    if turn == 0 && signal == 0 {
        return 0;
    }
    let (context, roads) = (context(), &graph.roads);
    let (a, b) = (before as usize, after as usize);
    let mut cost = if signal != 0 && signalled(graph, before) { u64::from(signal) } else { 0 };
    if turn != 0 && context.degree[roads.to(a) as usize] >= 3 {
        let u_turn = roads.way(a) == roads.way(b) && roads.from(a) == roads.to(b);
        let through = (variant.through || profile.walking)
            && !u_turn
            && context.through(roads.way_row(a), roads.way_row(b), profile.walking);
        if !through {
            cost += (f64::from(turn) * (1.0 - cosine(graph, before, after))).round() as u64;
        }
    }
    cost
}

/// The hook of `suitability::way`: V1.
pub fn reclass(road: &Road, tags: &Tags, profile: &Profile) -> Option<Road> {
    let designated = matches!(tag(tags, "highway"), "path" | "footway") && tag(tags, "bicycle") == "designated";
    (get().path_as_cycleway && !profile.walking && road.class == 4 && designated)
        .then(|| Road { class: 0, ..road.clone() })
}

/// The tagged speed limit in the direction of travel, in km/h.
pub fn speed_limit(tags: &Tags, reversed: bool) -> Option<f64> {
    let key = if reversed { "maxspeed:backward" } else { "maxspeed:forward" };
    traffic::speed(tag(tags, key)).or_else(|| traffic::speed(tag(tags, "maxspeed")))
}

/// The hook of `suitability::way`: V3, the class preference of a main road with a tagged limit of
/// 50 km/h or less.
pub fn main_road_preference(tags: &Tags, reversed: bool, profile: &Profile) -> Option<f64> {
    if !get().speed_class || profile.walking {
        return None;
    }
    match speed_limit(tags, reversed)? {
        speed if speed <= 30.0 => Some(1.5),
        speed if speed <= 50.0 => Some(2.0),
        _ => None,
    }
}

/// The hook of `cost::basis`: V4.
pub fn full_bonus(class: u8) -> bool {
    get().full_bonus && matches!(class, 1 | 3 | 4)
}

/// The hook of `cost::basis`: hiking (a), for a way of a signed hiking route.
pub fn walk_bonus(way: i64, profile: &Profile) -> bool {
    profile.walking && get().walk_bonus && context().walked.contains(&way)
}

/// The hook of `suitability::surface_factor`: V5.
pub fn paved_unknown(tags: &Tags) -> bool {
    get().paved_unknown && tags.get("surface").is_none()
}
