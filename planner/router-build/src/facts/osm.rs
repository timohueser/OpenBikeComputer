//! The read of the facts of the OSM extract: car parks, lifts, places, water and built-up land.
//! Three passes: the relations, then the tagged nodes and the ways, then the nodes of those ways.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::PathBuf;

use osmpbfreader::{OsmId, OsmObj, OsmPbfReader, Tags};
use planner_router::geometry::METRES_PER_UDEG;
use planner_router::shape::P;

/// A lake without a name is a place from this area, in m².
const LAKE_M2: f64 = 10_000.0;
/// A car park without `capacity` is large from this area, in m².
const LARGE_M2: f64 = 1_250.0;
const LARGE_SPACES: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Parking,
    Lift,
    Peak,
    Hut,
    /// A restaurant, a café or a beer garden: a hut with food outside built-up land.
    Food,
    Lake,
    River,
    Waterfall,
    BuiltUp,
}

/// The kind of an element by its tags, or None for an element that no fact reads.
pub(crate) fn kind(tags: &Tags) -> Option<Kind> {
    let tag = |key: &str| tags.get(key).map(|value| value.as_str());
    if tag("amenity") == Some("parking") {
        return (!matches!(tag("access"), Some("private" | "customers"))).then_some(Kind::Parking);
    }
    if matches!(tag("aerialway"), Some("cable_car" | "gondola" | "chair_lift" | "mixed_lift")) {
        return Some(Kind::Lift);
    }
    if tag("natural") == Some("peak") {
        return Some(Kind::Peak);
    }
    if tag("tourism") == Some("alpine_hut") {
        return Some(Kind::Hut);
    }
    if matches!(tag("amenity"), Some("restaurant" | "cafe" | "biergarten")) {
        return Some(Kind::Food);
    }
    match (tag("natural"), tag("water"), tag("waterway")) {
        (Some("water"), Some("lake" | "pond"), _) => return Some(Kind::Lake),
        (Some("water"), Some("river"), _) | (_, _, Some("river" | "canal")) => return Some(Kind::River),
        (_, _, Some("waterfall")) => return Some(Kind::Waterfall),
        _ => {}
    }
    matches!(tag("landuse"), Some("residential" | "commercial" | "retail")).then_some(Kind::BuiltUp)
}

/// A car park and what decides its size class.
#[derive(Clone, Debug, PartialEq)]
pub struct Parking {
    pub at: P,
    pub capacity: Option<u32>,
    pub area_m2: Option<f64>,
}

impl Parking {
    /// `capacity` decides when it is a number; else the mapped area.
    pub fn large(&self) -> bool {
        match self.capacity {
            Some(spaces) => spaces >= LARGE_SPACES,
            None => self.area_m2.is_some_and(|area| area >= LARGE_M2),
        }
    }
}

/// The two ends of a lift, each with the `ele` tag of its node when it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct Lift {
    pub ends: [P; 2],
    pub ele: [Option<f64>; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// The OSM element: `n`, `w` or `r` and its ID.
    pub id: String,
    pub kind: &'static str,
    pub name: Option<String>,
    pub ele_m: Option<i64>,
    pub at: P,
    /// The points of the outline of a lake; empty for another place.
    pub outline: Vec<P>,
}

#[derive(Default)]
pub struct Features {
    pub parkings: Vec<Parking>,
    pub lifts: Vec<Lift>,
    pub places: Vec<Place>,
    /// The lines of the water: lake shores, the outlines of rivers, and rivers and canals.
    pub water: Vec<Vec<P>>,
}

/// An element that a fact reads: a point, or its ways with `true` for an inner ring.
pub(crate) struct Element {
    pub id: String,
    pub kind: Kind,
    pub tags: Vec<(String, String)>,
    pub point: Option<P>,
    pub ways: Vec<(Vec<i64>, bool)>,
}

impl Element {
    pub(crate) fn new(id: OsmId, kind: Kind, tags: &Tags) -> Self {
        let id = match id {
            OsmId::Node(id) => format!("n{}", id.0),
            OsmId::Way(id) => format!("w{}", id.0),
            OsmId::Relation(id) => format!("r{}", id.0),
        };
        let keep = ["name", "ele", "capacity"];
        let tags = tags.iter().filter(|(key, _)| keep.contains(&key.as_str()));
        Self {
            id,
            kind,
            tags: tags.map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            point: None,
            ways: Vec::new(),
        }
    }

    fn tag(&self, key: &str) -> Option<&str> {
        self.tags.iter().find(|(k, _)| k == key).map(|(_, value)| value.as_str())
    }
}

fn reader(path: &PathBuf) -> Result<OsmPbfReader<File>, String> {
    Ok(OsmPbfReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?))
}

/// The facts of the OSM files `paths`.
pub fn read(paths: &[PathBuf]) -> Result<Features, String> {
    let mut elements = Vec::new();
    let mut members = HashSet::new();
    for path in paths {
        for relation in reader(path)?.par_iter_relations() {
            let relation = relation.map_err(|e| e.to_string())?;
            let area = |kind: &Kind| !matches!(kind, Kind::Lift | Kind::Peak | Kind::Waterfall);
            let multipolygon = relation.tags.contains("type", "multipolygon");
            let Some(kind) = kind(&relation.tags).filter(|kind| multipolygon && area(kind)) else { continue };
            let mut element = Element::new(OsmId::Relation(relation.id), kind, &relation.tags);
            for member in &relation.refs {
                if let OsmId::Way(way) = member.member {
                    members.insert(way.0);
                    element.ways.push((vec![way.0], member.role == "inner"));
                }
            }
            elements.push(element);
        }
    }
    let mut ways = HashMap::new();
    for path in paths {
        for object in reader(path)?.par_iter() {
            match object.map_err(|e| e.to_string())? {
                OsmObj::Node(node) => {
                    let point = |kind: &Kind| {
                        matches!(kind, Kind::Parking | Kind::Peak | Kind::Hut | Kind::Food | Kind::Waterfall)
                    };
                    if let Some(kind) = kind(&node.tags).filter(point) {
                        let mut element = Element::new(OsmId::Node(node.id), kind, &node.tags);
                        element.point = Some([node.decimicro_lon / 10, node.decimicro_lat / 10]);
                        elements.push(element);
                    }
                }
                OsmObj::Way(way) => {
                    let kind = kind(&way.tags).filter(|kind| *kind != Kind::Peak && way.nodes.len() >= 2);
                    let member = members.contains(&way.id.0);
                    if kind.is_none() && !member {
                        continue;
                    }
                    let nodes: Vec<i64> = way.nodes.iter().map(|node| node.0).collect();
                    if let Some(kind) = kind {
                        let mut element = Element::new(OsmId::Way(way.id), kind, &way.tags);
                        element.ways.push((nodes.clone(), false));
                        elements.push(element);
                    }
                    if member {
                        ways.insert(way.id.0, nodes);
                    }
                }
                OsmObj::Relation(_) => {}
            }
        }
    }
    // A member way of a relation holds its own ID until here.
    for element in elements.iter_mut().filter(|element| element.id.starts_with('r')) {
        let found = element.ways.drain(..).filter_map(|(way, inner)| Some((ways.get(&way[0])?.clone(), inner)));
        element.ways = found.collect();
    }
    drop(ways);
    let mut needed: Vec<i64> =
        elements.iter().flat_map(|e| &e.ways).flat_map(|(nodes, _)| nodes.iter().copied()).collect();
    needed.sort_unstable();
    needed.dedup();
    let mut points = vec![None; needed.len()];
    let mut heights = HashMap::new();
    for path in paths {
        for node in reader(path)?.par_iter_nodes() {
            let node = node.map_err(|e| e.to_string())?;
            if let Ok(at) = needed.binary_search(&node.id.0) {
                points[at] = Some([node.decimicro_lon / 10, node.decimicro_lat / 10]);
                if let Some(ele) = node.tags.get("ele").and_then(|value| metres(value)) {
                    heights.insert(node.id.0, ele);
                }
            }
        }
    }
    let point = |node: i64| needed.binary_search(&node).ok().and_then(|at| points[at]);
    Ok(features(elements, &point, &|node| heights.get(&node).copied()))
}

/// A height or a distance in metres: a number, with or without the unit `m`.
fn metres(value: &str) -> Option<f64> {
    value.trim().trim_end_matches('m').trim().replace(',', ".").parse().ok().filter(|v: &f64| v.is_finite())
}

/// The features of `elements`, whose nodes `point` places; `ele` gives the `ele` tag of a node.
pub(crate) fn features(
    elements: Vec<Element>,
    point: &dyn Fn(i64) -> Option<P>,
    ele: &dyn Fn(i64) -> Option<f64>,
) -> Features {
    let line = |nodes: &[i64]| nodes.iter().filter_map(|&node| point(node)).collect::<Vec<P>>();
    let built: Vec<Vec<Vec<P>>> = elements
        .iter()
        .filter(|element| element.kind == Kind::BuiltUp)
        .map(|element| element.ways.iter().map(|(nodes, _)| line(nodes)).collect())
        .collect();
    let built = Areas::new(built);
    let mut features = Features::default();
    for element in elements {
        let lines = || element.ways.iter().map(|(nodes, _)| line(nodes));
        match element.kind {
            Kind::BuiltUp => continue,
            Kind::River => {
                features.water.extend(lines());
                continue;
            }
            Kind::Lift => {
                let nodes = &element.ways[0].0;
                let (first, last) = (nodes[0], nodes[nodes.len() - 1]);
                if let (Some(a), Some(b)) = (point(first), point(last)) {
                    features.lifts.push(Lift { ends: [a, b], ele: [ele(first), ele(last)] });
                }
                continue;
            }
            _ => {}
        }
        let rings = rings(&element.ways);
        let (outer, inner): (Vec<_>, Vec<_>) = rings.iter().partition(|(_, inner)| !inner);
        let area = |rings: &[&(Vec<i64>, bool)]| rings.iter().map(|(ring, _)| shoelace(&line(ring))).sum::<f64>();
        let area = (!outer.is_empty()).then(|| area(&outer) - area(&inner));
        // A way that is no ring, such as a waterfall along a stream, takes the mean of its points.
        let at =
            element.point.or_else(|| mean(&outer.iter().flat_map(|(ring, _)| line(&ring[1..])).collect::<Vec<_>>()));
        let Some(at) = at.or_else(|| mean(&lines().flatten().collect::<Vec<_>>())) else { continue };
        let place = |kind: &'static str, outline: Vec<P>| Place {
            id: element.id.clone(),
            kind,
            name: element.tag("name").map(str::to_string),
            ele_m: element.tag("ele").and_then(metres).map(|ele| ele.round() as i64),
            at,
            outline,
        };
        match element.kind {
            Kind::Parking => {
                let capacity = element.tag("capacity").and_then(|value| value.trim().parse().ok());
                features.parkings.push(Parking { at, capacity, area_m2: area });
            }
            Kind::Peak => features.places.push(place("peak", Vec::new())),
            Kind::Hut => features.places.push(place("hut", Vec::new())),
            Kind::Waterfall => features.places.push(place("waterfall", Vec::new())),
            Kind::Food if !built.contains(at) => features.places.push(place("hut", Vec::new())),
            Kind::Lake if element.tag("name").is_some() || area.is_some_and(|area| area >= LAKE_M2) => {
                let shores: Vec<Vec<P>> = lines().collect();
                features.places.push(place("lake", shores.concat()));
                features.water.extend(shores);
            }
            _ => {}
        }
    }
    features
}

/// The closed rings of `ways`, joined at their shared ends, each with the `inner` flag of its
/// first way. A chain that does not close is left out.
fn rings(ways: &[(Vec<i64>, bool)]) -> Vec<(Vec<i64>, bool)> {
    let mut open: Vec<&(Vec<i64>, bool)> = ways.iter().filter(|(nodes, _)| nodes.len() >= 2).collect();
    let mut rings = Vec::new();
    while let Some((first, inner)) = open.pop() {
        let mut ring = first.clone();
        while ring.len() < 4 || ring[0] != ring[ring.len() - 1] {
            let end = ring[ring.len() - 1];
            let Some(next) = open.iter().position(|(nodes, _)| nodes[0] == end || nodes[nodes.len() - 1] == end) else {
                break;
            };
            let (nodes, _) = open.swap_remove(next);
            match nodes[0] == end {
                true => ring.extend(&nodes[1..]),
                false => ring.extend(nodes.iter().rev().skip(1)),
            }
        }
        if ring.len() >= 4 && ring[0] == ring[ring.len() - 1] {
            rings.push((ring, *inner));
        }
    }
    rings
}

fn mean(points: &[P]) -> Option<P> {
    let n = points.len() as i64;
    let sum = points.iter().fold([0i64; 2], |sum, p| [sum[0] + i64::from(p[0]), sum[1] + i64::from(p[1])]);
    (n > 0).then(|| [(sum[0] / n) as i32, (sum[1] / n) as i32])
}

/// The area of a closed ring in m², in a plane scaled at its first point.
fn shoelace(ring: &[P]) -> f64 {
    let Some(first) = ring.first() else { return 0.0 };
    let scale = (first[1] as f64 * 1e-6).to_radians().cos() * METRES_PER_UDEG;
    let xy = |p: &P| [(p[0] - first[0]) as f64 * scale, (p[1] - first[1]) as f64 * METRES_PER_UDEG];
    let twice: f64 = ring.windows(2).map(|w| (xy(&w[0]), xy(&w[1]))).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum();
    twice.abs() / 2.0
}

/// Areas whose rings, outer and inner, hold a point by the even-odd rule.
struct Areas {
    areas: Vec<Vec<Vec<P>>>,
    /// The areas whose box meets each cell of 0.01°.
    cells: HashMap<(i32, i32), Vec<u32>>,
}

const AREA_CELL: i32 = 10_000;

impl Areas {
    fn new(areas: Vec<Vec<Vec<P>>>) -> Self {
        let mut cells = HashMap::<_, Vec<u32>>::new();
        for (i, area) in areas.iter().enumerate() {
            let points = area.iter().flatten();
            let Some(&first) = points.clone().next() else { continue };
            let (low, high) = points.fold((first, first), |(low, high), p| {
                ([low[0].min(p[0]), low[1].min(p[1])], [high[0].max(p[0]), high[1].max(p[1])])
            });
            for x in low[0].div_euclid(AREA_CELL)..=high[0].div_euclid(AREA_CELL) {
                for y in low[1].div_euclid(AREA_CELL)..=high[1].div_euclid(AREA_CELL) {
                    cells.entry((x, y)).or_default().push(i as u32);
                }
            }
        }
        Self { areas, cells }
    }

    fn contains(&self, p: P) -> bool {
        let cell = (p[0].div_euclid(AREA_CELL), p[1].div_euclid(AREA_CELL));
        let inside = |rings: &Vec<Vec<P>>| {
            let crossings = rings.iter().flat_map(|ring| ring.windows(2)).filter(|w| {
                let (a, b) = (w[0], w[1]);
                (a[1] > p[1]) != (b[1] > p[1]) && {
                    let t = (p[1] - a[1]) as f64 / (b[1] - a[1]) as f64;
                    (p[0] as f64) < a[0] as f64 + t * (b[0] - a[0]) as f64
                }
            });
            crossings.count() % 2 == 1
        };
        self.cells.get(&cell).into_iter().flatten().any(|&i| inside(&self.areas[i as usize]))
    }
}
