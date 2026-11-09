pub mod base;
pub mod blocks;
pub mod catalog;
mod column;
pub mod connectivity;
pub mod cost;
pub mod country;
pub mod grid;
pub mod landmarks;
pub mod layout;
pub mod osm;
pub mod overlays;
mod road_bike;
pub mod sort;
pub mod source;
pub mod step;
mod suitability;
pub mod terrain;
mod traffic;
mod work;

use base::{Dictionary, Topology};
use column::{Column, Lists};
use planner_router::{
    closures::{self, Closures},
    model::{Point, Profile, Road, Surface},
    package::{self, Manifest, CELL, ROADS_PER_PAGE},
    storage,
    table::Table,
};
use std::collections::{BTreeMap, HashMap};

/// The imported region: directed roads, their junctions and turn rules, and the source OSM
/// objects that costs, closures, the overlay index and the route catalog read.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub points: Vec<Point>,
    pub node_ids: Vec<i64>,
    pub node_access: Vec<u8>,
    pub osm: source::Data,
    pub roads: Roads,
    /// Forbidden transitions between directed roads. Sorted for binary search.
    pub forbidden: Vec<(u32, u32)>,
    pub forbidden_foot: Vec<(u32, u32)>,
    pub warnings: Vec<String>,
}

impl Graph {
    pub fn departures(&self) -> Vec<Vec<u32>> {
        let mut result = vec![Vec::new(); self.points.len()];
        for id in 0..self.roads.len() {
            result[self.roads.from(id) as usize].push(id as u32);
        }
        result
    }

    pub fn permits_turn(&self, from: u32, to: u32, walking: bool) -> bool {
        let forbidden = if walking { &self.forbidden_foot } else { &self.forbidden };
        forbidden.binary_search(&(from, to)).is_err()
    }
}

/// The directed roads: one column for each field of `Road`, and the shapes.
#[derive(Clone, Debug, Default)]
pub struct Roads {
    from: Column<u32>,
    to: Column<u32>,
    way: Column<i64>,
    reversed: Column<bool>,
    length_m: Column<u32>,
    ascent_m: Column<u32>,
    descent_m: Column<u32>,
    surface: Column<Surface>,
    class: Column<u8>,
    access: Column<u8>,
    difficulty: Column<u8>,
    hiking_difficulty: Column<Option<u8>>,
    structure: Column<bool>,
    shapes: Lists<Point>,
}

impl Roads {
    pub fn push(&mut self, road: Road) {
        let Road {
            from,
            to,
            way,
            reversed,
            length_m,
            ascent_m,
            descent_m,
            surface,
            class,
            access,
            difficulty,
            hiking_difficulty,
            structure,
            shape,
        } = road;
        self.from.push(from);
        self.to.push(to);
        self.way.push(way);
        self.reversed.push(reversed);
        self.length_m.push(length_m);
        self.ascent_m.push(ascent_m);
        self.descent_m.push(descent_m);
        self.surface.push(surface);
        self.class.push(class);
        self.access.push(access);
        self.difficulty.push(difficulty);
        self.hiking_difficulty.push(hiking_difficulty);
        self.structure.push(structure);
        self.shapes.push(&shape);
    }

    /// Fills `road` with road `index`, reusing the allocation of its shape.
    pub fn get(&self, index: usize, road: &mut Road) {
        let mut shape = std::mem::take(&mut road.shape);
        shape.clear();
        shape.extend_from_slice(self.shapes.get(index));
        *road = self.with_shape(index, shape);
    }

    /// Road `index` with an empty shape, which allocates nothing.
    pub fn attributes(&self, index: usize) -> Road {
        self.with_shape(index, Vec::new())
    }

    fn with_shape(&self, index: usize, shape: Vec<Point>) -> Road {
        Road {
            from: self.from.values()[index],
            to: self.to.values()[index],
            way: self.way.values()[index],
            reversed: self.reversed.values()[index],
            length_m: self.length_m.values()[index],
            ascent_m: self.ascent_m.values()[index],
            descent_m: self.descent_m.values()[index],
            surface: self.surface.values()[index],
            class: self.class.values()[index],
            access: self.access.values()[index],
            difficulty: self.difficulty.values()[index],
            hiking_difficulty: self.hiking_difficulty.values()[index],
            structure: self.structure.values()[index],
            shape,
        }
    }

    /// Each road with a shape of its own: one allocation per road.
    pub fn iter(&self) -> impl Iterator<Item = Road> + '_ {
        (0..self.len()).map(|index| {
            let mut road = Road::default();
            self.get(index, &mut road);
            road
        })
    }

    pub fn len(&self) -> usize {
        self.from.values().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn from(&self, index: usize) -> u32 {
        self.from.values()[index]
    }

    pub fn to(&self, index: usize) -> u32 {
        self.to.values()[index]
    }

    pub fn way(&self, index: usize) -> i64 {
        self.way.values()[index]
    }

    pub fn surface(&self, index: usize) -> Surface {
        self.surface.values()[index]
    }

    pub fn class(&self, index: usize) -> u8 {
        self.class.values()[index]
    }

    pub fn shape(&self, index: usize) -> &[Point] {
        self.shapes.get(index)
    }

    /// The new road `i` is the old road `order[i]`.
    fn permute(&mut self, order: &[u32]) {
        let Self {
            from,
            to,
            way,
            reversed,
            length_m,
            ascent_m,
            descent_m,
            surface,
            class,
            access,
            difficulty,
            hiking_difficulty,
            structure,
            shapes,
        } = self;
        *from = from.permuted(order);
        *to = to.permuted(order);
        *way = way.permuted(order);
        *reversed = reversed.permuted(order);
        *length_m = length_m.permuted(order);
        *ascent_m = ascent_m.permuted(order);
        *descent_m = descent_m.permuted(order);
        *surface = surface.permuted(order);
        *class = class.permuted(order);
        *access = access.permuted(order);
        *difficulty = difficulty.permuted(order);
        *hiking_difficulty = hiking_difficulty.permuted(order);
        *structure = structure.permuted(order);
        *shapes = shapes.permuted(order);
    }
}

impl FromIterator<Road> for Roads {
    fn from_iter<I: IntoIterator<Item = Road>>(roads: I) -> Self {
        let mut result = Self::default();
        for road in roads {
            result.push(road);
        }
        result
    }
}

/// `value` with the keys of each object in byte order: the bytes of the JSON that this crate writes
/// are the same in a binary where another crate enables `serde_json/preserve_order`.
pub fn sort_keys(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(key, value)| (key, sort_keys(value))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_keys).collect()),
        value => value,
    }
}

/// Writes a closed regional package with all legal transitions and complete road geometry.
/// The caller publishes the manifest only after all objects have been written.
pub fn prepare(
    graph: &Graph,
    region: String,
    bounds: [f64; 4],
    profiles: &[Profile],
    source_sha256: Vec<String>,
    attribution: String,
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Manifest, String> {
    let roads = &graph.roads;
    let points = graph.points.len();
    if (0..roads.len()).any(|id| roads.from(id) as usize >= points || roads.to(id) as usize >= points) {
        return Err("Invalid road endpoints".into());
    }
    for restrictions in [&graph.forbidden, &graph.forbidden_foot] {
        if !restrictions.is_sorted() {
            return Err("Unsorted turn restrictions".into());
        }
        for &(before, after) in restrictions {
            if before as usize >= roads.len() || after as usize >= roads.len() {
                return Err("Unknown restriction road".into());
            }
            if roads.to(before as usize) != roads.from(after as usize) {
                return Err("Restriction roads do not meet".into());
            }
        }
    }
    let count = u32::try_from(roads.len()).map_err(|_| "Too many roads")?;
    let topology = Topology::new(roads, points)?;
    let mut manifest = Manifest {
        landmarks: None,
        closures: None,
        format: package::FORMAT,
        region,
        bounds,
        source_sha256,
        attribution,
        warnings: graph.warnings.clone(),
        roads: count,
        graph: planner_router::base::write_topology(topology.first.values(), topology.head.values(), &mut write)?,
        geometry: Table::default(),
        spatial: BTreeMap::new(),
        metrics: BTreeMap::new(),
        costs: Table::default(),
    };
    let mut cells = Vec::<((i32, i32), u32)>::new();
    let mut road_cells = Vec::new();
    for id in 0..roads.len() {
        let shape = roads.shape(id);
        if shape.len() < 2 || roads.class(id) > 6 {
            return Err("Road lacks valid geometry or class".into());
        }
        for pair in shape.windows(2) {
            let (a, b) = (package::cell(pair[0]), package::cell(pair[1]));
            if (a.0.abs_diff(b.0) as u64 + 1) * (a.1.abs_diff(b.1) as u64 + 1) > 4096 {
                return Err(format!("Road segment spans more than 4096 spatial cells of {} microdegrees", CELL));
            }
            for lat in a.0.min(b.0)..=a.0.max(b.0) {
                for lon in a.1.min(b.1)..=a.1.max(b.1) {
                    road_cells.push((lat, lon));
                }
            }
        }
        road_cells.sort_unstable();
        road_cells.dedup();
        cells.extend(road_cells.drain(..).map(|cell| (cell, id as u32)));
    }
    // Unique (cell, road) pairs, by cell and then road.
    cells.sort_unstable();
    let mut geometry = Vec::new();
    let mut page = Vec::new();
    for start in (0..roads.len()).step_by(ROADS_PER_PAGE as usize) {
        let ids = start..roads.len().min(start + ROADS_PER_PAGE as usize);
        page.resize_with(ids.len(), Road::default);
        for (road, id) in page.iter_mut().zip(ids) {
            roads.get(id, road);
        }
        geometry.push(write(&planner_router::geometry::encode(&page)?)?);
    }
    manifest.geometry = Table::write(&geometry, &mut write)?;
    // The package stores its objects in write order: the road lists in cell order, then the
    // directories in key order, so the directories wait until the last road list.
    let mut groups = BTreeMap::<String, BTreeMap<String, String>>::new();
    for run in cells.chunk_by(|a, b| a.0 == b.0) {
        let cell = run[0].0;
        let ids: Vec<u32> = run.iter().map(|&(_, id)| id).collect();
        let group = package::cell_key((cell.0.div_euclid(100), cell.1.div_euclid(100)));
        groups.entry(group).or_default().insert(package::cell_key(cell), write(&storage::encode(&ids)?)?);
    }
    drop(cells);
    for (group, cells) in groups {
        manifest.spatial.insert(group, write(&storage::encode(&cells)?)?);
    }
    let nodes: HashMap<u32, _> = (0..graph.points.len() as u32)
        .filter_map(|point| {
            let node = graph.osm.nodes.get(*graph.node_ids.get(point as usize)?)?;
            let closures = osm::node_closures(node.tags.iter());
            (!closures.is_empty()).then_some((point, closures))
        })
        .collect();
    let mut doubts = cost::Doubts {
        roads: Vec::with_capacity(roads.len()),
        nodes: nodes.iter().map(|(&point, closures)| (point, cost::Doubts::modes(closures))).collect(),
    };
    // A road reports the closures of its way and of the node where it arrives.
    let closures = Closures::build((0..roads.len()).map(|id| {
        let road = roads.attributes(id);
        let direction = if road.reversed { "backward" } else { "forward" };
        let mut closures =
            graph.osm.ways.get(road.way).map_or_else(Vec::new, |way| source::closures(way.tags.iter(), &[direction]));
        // The doubt of a road comes from its way closures only, before the node closures join.
        // `build` reads each road once, in order, so `doubts.roads` gets one byte per road.
        doubts.roads.push(cost::Doubts::modes(&closures));
        closures.extend(nodes.get(&road.to).into_iter().flatten().cloned());
        closures
    }))?;
    let mut dictionary = Dictionary::default();
    let junctions = landmarks::Junctions::new((0..roads.len()).map(|id| (roads.from(id), roads.to(id))))?;
    let mut landmarks = junctions.index(&mut write)?;
    let states = connectivity::States::new(&topology);
    for profile in profiles {
        eprintln!("Preparing profile {}", profile.name);
        if manifest.metrics.contains_key(&profile.name) {
            return Err("Duplicate metric identity".into());
        }
        let costing = cost::Costing::new(graph, profile, &doubts, &mut dictionary)?;
        let road_costs = costing.totals.values();
        let columns = junctions.prepare(road_costs, &mut write)?;
        let turns: Column<u32> = topology
            .edges()
            .map(|(before, after)| base::narrow(costing.transition(before, after)))
            .collect::<Result<_, _>>()?;
        let allowed = states.snappable(road_costs, turns.values());
        let fragments = road_costs.iter().filter(|&&c| c != u32::MAX).count()
            - allowed.iter().map(|word| word.count_ones() as usize).sum::<usize>();
        eprintln!(
            "Profile {}: {fragments} roads in fragments are not snappable; landmark scale {}",
            profile.name, columns.scale
        );
        landmarks.profiles.insert(profile.name.clone(), columns);
        manifest.metrics.insert(
            profile.name.clone(),
            base::metric(profile, costing.bases.values(), road_costs, turns.values(), &allowed, &mut write)?,
        );
    }
    manifest.costs = Table::write(&dictionary.into_values(), &mut write)?;
    manifest.landmarks = Some(landmarks);
    manifest.closures = write_closures(&closures, &mut write)?;
    Ok(manifest)
}

/// Writes the sets object and the per-road column only when a road has a possible closure.
fn write_closures(
    closures: &Closures,
    write: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Option<closures::Index>, String> {
    let ids = (0..closures.roads.len()).map(|road| closures.roads.get(road));
    if ids.clone().all(|id| id == u64::MAX) {
        return Ok(None);
    }
    Ok(Some(closures::Index {
        sets: write(&storage::encode(&closures.sets)?)?,
        roads: planner_router::base::write_column(ids, write)?,
    }))
}
