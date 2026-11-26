//! PROTOTYPE. The switches of the candidate cost changes, and the hooks that the real cost code
//! calls. With every switch off, each hook leaves the cost of today. The candidates measure the
//! reach of an idea; they are not proposals.
use crate::road_bike::tag;
use crate::source::Tags;
use crate::{traffic, Graph};
use planner_router::model::{Point, Profile, Road};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::OnceLock;

/// The length before and after a junction that gives the heading there.
const HEADING_M: f64 = 15.0;

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
}

static FLAGS: AtomicU32 = AtomicU32::new(0);
static TURN: AtomicU32 = AtomicU32::new(0);

pub fn set(variant: Variant) {
    let flags =
        [variant.path_as_cycleway, variant.through, variant.speed_class, variant.full_bonus, variant.paved_unknown];
    FLAGS.store(flags.iter().enumerate().map(|(bit, on)| u32::from(*on) << bit).sum(), Relaxed);
    TURN.store(variant.turn, Relaxed);
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
        turn: TURN.load(Relaxed),
    }
}

/// What the turn hook and the measures read of the graph.
pub struct Context {
    /// The undirected road ends at each junction.
    pub degree: Vec<u8>,
    /// The `name` and the `ref` of each way row as numbers; 0 for none.
    names: Vec<[u32; 2]>,
    /// The cycle route relations that list each way row, sorted.
    routes: HashMap<u32, Vec<i64>>,
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
    let mut routes = HashMap::<u32, Vec<i64>>::new();
    for relation in graph.osm.relations.iter() {
        let tags = &relation.tags;
        if tags.get("route") != Some("bicycle")
            || !matches!(tags.get("type"), Some("route" | "superroute"))
            || tags.contains("state", "proposed")
        {
            continue;
        }
        for row in relation.members().filter_map(|(member, _)| graph.osm.ways.index(member.way()?)) {
            routes.entry(row as u32).or_default().push(relation.id);
        }
    }
    for list in routes.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    let _ = CONTEXT.set(Context { degree, names, routes });
}

impl Context {
    /// The name, the ref and the cycle routes of a way row, as tokens that two ways of one road
    /// or one route share.
    pub fn labels(&self, row: u32) -> Vec<u64> {
        let [name, reference] = self.names[row as usize];
        let mut labels = Vec::new();
        if name != 0 {
            labels.push(1 << 40 | u64::from(name));
        }
        if reference != 0 {
            labels.push(2 << 40 | u64::from(reference));
        }
        labels.extend(self.routes.get(&row).into_iter().flatten().map(|&id| 3 << 40 | id as u64));
        labels
    }

    fn through(&self, before: u32, after: u32) -> bool {
        let (a, b) = (self.names[before as usize], self.names[after as usize]);
        if a[0] != 0 && a[0] == b[0] || a[1] != 0 && a[1] == b[1] {
            return true;
        }
        match (self.routes.get(&before), self.routes.get(&after)) {
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

/// The hook of `cost::transition`: V2 and V2T. A U-turn onto the same way is no through road.
pub fn turn_cost(graph: &Graph, profile: &Profile, before: u32, after: u32) -> u64 {
    let variant = get();
    if variant.turn == 0 || profile.walking {
        return 0;
    }
    let (context, roads) = (context(), &graph.roads);
    let (a, b) = (before as usize, after as usize);
    if context.degree[roads.to(a) as usize] < 3 {
        return 0;
    }
    let u_turn = roads.way(a) == roads.way(b) && roads.from(a) == roads.to(b);
    if variant.through && !u_turn && context.through(roads.way_row(a), roads.way_row(b)) {
        return 0;
    }
    (f64::from(variant.turn) * (1.0 - cosine(graph, before, after))).round() as u64
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

/// The hook of `suitability::surface_factor`: V5.
pub fn paved_unknown(tags: &Tags) -> bool {
    get().paved_unknown && tags.get("surface").is_none()
}
