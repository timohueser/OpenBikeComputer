pub mod base;
pub mod blocks;
pub mod catalog;
pub mod cells;
mod column;
pub mod connectivity;
pub mod cost;
pub mod country;
pub mod grid;
pub mod landmarks;
pub mod layout;
pub mod osm;
pub mod overlays;
pub mod partition;
mod road_bike;
pub mod sort;
pub mod source;
pub mod step;
mod suitability;
pub mod tables;
pub mod terrain;
mod traffic;
mod work;

use base::{Dictionary, Topology};
use column::{Column, Layout, Lists, Plain};
use planner_router::{
    closures,
    cost::CostBasis,
    model::{Point, Profile, Road},
    package::{self, Manifest, CELL, ROADS_PER_PAGE},
    storage,
    table::Table,
};
use rayon::prelude::*;
use std::collections::BTreeMap;
use terrain::Height;

/// The ways or roads of one job of a parallel pass. Each job writes its own range of each output,
/// so the bytes do not depend on the thread count.
#[cfg(not(test))]
const CHUNK: usize = 65_536;
#[cfg(test)]
const CHUNK: usize = 8;

/// The file of the closure sets in the roads layer.
pub(crate) const SETS: &str = "sets.postcard";

/// The imported region: directed roads, their junctions and turn rules, and the source OSM
/// objects that costs, closures, the overlay index and the route catalog read.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// The junctions, in the order of their cells (`partition`).
    pub points: Column<Point>,
    pub node_ids: Column<i64>,
    pub node_access: Column<u8>,
    /// 1 for a traffic signal at the junction.
    pub signals: Column<u8>,
    /// The `sets` ID of the closures of the junction's node, or `u32::MAX`.
    pub node_closures: Column<u32>,
    /// The doubtful modes of those closures (`cost::doubts`).
    pub node_doubts: Column<u8>,
    pub sets: source::Sets,
    pub osm: source::Data,
    pub roads: Roads,
    /// Forbidden transitions between directed roads. Sorted for binary search.
    pub forbidden: Column<Turn>,
    pub forbidden_foot: Column<Turn>,
    pub warnings: Vec<String>,
    pub cells: partition::Cells,
}

/// A transition from road `from` to road `to`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Turn {
    pub from: u32,
    pub to: u32,
}

const _: () = assert!(size_of::<Turn>() == 4 + 4);

impl Graph {
    pub fn permits_turn(&self, from: u32, to: u32, walking: bool) -> bool {
        let forbidden = if walking { &self.forbidden_foot } else { &self.forbidden };
        forbidden.values().binary_search(&Turn { from, to }).is_err()
    }

    /// Saves the junctions, the roads, the turn rules and the closure sets: all but the source
    /// objects and the warnings.
    pub(crate) fn save(self, layout: &mut Layout) -> Result<(), String> {
        layout.save("points", self.points)?;
        layout.save("node_ids", self.node_ids)?;
        layout.save("node_access", self.node_access)?;
        layout.save("signals", self.signals)?;
        layout.save("node_closures", self.node_closures)?;
        layout.save("node_doubts", self.node_doubts)?;
        self.roads.save(layout)?;
        layout.save("forbidden", self.forbidden)?;
        layout.save("forbidden_foot", self.forbidden_foot)?;
        self.cells.save(layout)?;
        let path = layout.path(SETS)?;
        std::fs::write(&path, self.sets.to_bytes()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The graph that `save` saved, over the source objects `osm`.
    pub(crate) fn open(layout: &Layout, osm: source::Data, warnings: Vec<String>) -> Result<Self, String> {
        let path = layout.path(SETS)?;
        let sets = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Graph {
            points: layout.open("points")?,
            node_ids: layout.open("node_ids")?,
            node_access: layout.open("node_access")?,
            signals: layout.open("signals")?,
            node_closures: layout.open("node_closures")?,
            node_doubts: layout.open("node_doubts")?,
            sets: source::Sets::from_bytes(&sets)?,
            osm,
            roads: Roads::open(layout)?,
            forbidden: layout.open("forbidden")?,
            forbidden_foot: layout.open("forbidden_foot")?,
            warnings,
            cells: partition::Cells::open(layout)?,
        })
    }

    /// Puts each junction column into the order `order`: the old ID of each new junction ID.
    pub fn permute_junctions(&mut self, order: &Column<u32>) -> Result<(), String> {
        fn permute<T: Plain>(column: &mut Column<T>, order: &Column<u32>) -> Result<(), String> {
            if column.len() != order.len() {
                return Err(format!("a junction column has {} entries, not {}", column.len(), order.len()));
            }
            let mut permuted = Column::default();
            permuted.extend_permuted(column, order.values());
            *column = permuted;
            Ok(())
        }
        permute(&mut self.points, order)?;
        permute(&mut self.node_ids, order)?;
        permute(&mut self.node_access, order)?;
        permute(&mut self.signals, order)?;
        permute(&mut self.node_closures, order)?;
        permute(&mut self.node_doubts, order)
    }

    /// For a graph that a test builds by hand; the import does this itself. Gives each road the
    /// row of its way in `osm.ways` by a search for its way ID, and each such way the attributes
    /// and the access of its roads and the closures of its tags. A junction column that is empty
    /// gets no signal and no closure.
    pub fn link_ways(&mut self) {
        let points = self.points.len();
        if self.signals.is_empty() {
            self.signals = Column::filled(points, 0);
        }
        if self.node_closures.is_empty() {
            self.node_closures = Column::filled(points, u32::MAX);
        }
        if self.node_doubts.is_empty() {
            self.node_doubts = Column::filled(points, 0);
        }
        let mut facts: Vec<_> = self.osm.ways.iter().map(|way| way.facts).collect();
        for (way, facts) in self.osm.ways.iter().zip(&mut facts) {
            (facts.closures, facts.doubts) = osm::way_closures(&way.tags, &mut self.sets);
        }
        let mut sources = Vec::with_capacity(self.roads.len());
        for id in 0..self.roads.len() {
            let road = self.roads.attributes(id);
            let row = self.osm.ways.index(road.way).unwrap_or_else(|| panic!("road {id} has no way {}", road.way));
            let way = &mut facts[row];
            let side = usize::from(road.reversed);
            way.routable = 1;
            (way.class, way.surface, way.difficulty) =
                (road.class, source::surface_byte(road.surface), road.difficulty);
            way.hiking_difficulty = source::hiking_byte(road.hiking_difficulty);
            way.structure = u8::from(road.structure);
            way.access[side] = road.access;
            let junction = self.node_closures.values()[road.to as usize];
            let closures = osm::road_closures(&mut self.sets, way.closures[side], junction);
            sources.push(RoadSource { way_row: row as u32, closures });
        }
        self.roads.way_row = sources.iter().map(|source| source.way_row).collect();
        self.roads.closures = sources.iter().map(|source| source.closures).collect();
        self.osm.ways.set_facts(facts.into_iter().collect());
    }
}

/// What a road takes from its way and from its arrival junction, beside its `Road` fields.
#[derive(Clone, Copy, Debug)]
pub struct RoadSource {
    /// The row of the way in `osm.ways`.
    pub way_row: u32,
    /// The `sets` ID of the closures of its way in its direction, then of its arrival junction.
    pub closures: u32,
}

impl Default for RoadSource {
    fn default() -> Self {
        RoadSource { way_row: u32::MAX, closures: u32::MAX }
    }
}

/// The directed roads: one column for each field of `Road`, and the shapes.
#[derive(Clone, Debug, Default)]
pub struct Roads {
    from: Column<u32>,
    to: Column<u32>,
    way: Column<i64>,
    /// 1 for the direction against the nodes of the way.
    reversed: Column<u8>,
    length_m: Column<u32>,
    ascent_m: Column<u32>,
    descent_m: Column<u32>,
    /// `source::surface_byte`.
    surface: Column<u8>,
    class: Column<u8>,
    access: Column<u8>,
    difficulty: Column<u8>,
    /// `source::hiking_byte`.
    hiking_difficulty: Column<u8>,
    structure: Column<u8>,
    shapes: Lists<Point>,
    way_row: Column<u32>,
    closures: Column<u32>,
}

impl Roads {
    fn save(self, layout: &mut Layout) -> Result<(), String> {
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
            way_row,
            closures,
        } = self;
        layout.save("roads.from", from)?;
        layout.save("roads.to", to)?;
        layout.save("roads.way", way)?;
        layout.save("roads.reversed", reversed)?;
        layout.save("roads.length_m", length_m)?;
        layout.save("roads.ascent_m", ascent_m)?;
        layout.save("roads.descent_m", descent_m)?;
        layout.save("roads.surface", surface)?;
        layout.save("roads.class", class)?;
        layout.save("roads.access", access)?;
        layout.save("roads.difficulty", difficulty)?;
        layout.save("roads.hiking_difficulty", hiking_difficulty)?;
        layout.save("roads.structure", structure)?;
        layout.save_lists("roads.shapes", shapes)?;
        layout.save("roads.way_row", way_row)?;
        layout.save("roads.closures", closures)
    }

    fn open(layout: &Layout) -> Result<Self, String> {
        let roads = Self {
            from: layout.open("roads.from")?,
            to: layout.open("roads.to")?,
            way: layout.open("roads.way")?,
            reversed: layout.open("roads.reversed")?,
            length_m: layout.open("roads.length_m")?,
            ascent_m: layout.open("roads.ascent_m")?,
            descent_m: layout.open("roads.descent_m")?,
            surface: layout.open("roads.surface")?,
            class: layout.open("roads.class")?,
            access: layout.open("roads.access")?,
            difficulty: layout.open("roads.difficulty")?,
            hiking_difficulty: layout.open("roads.hiking_difficulty")?,
            structure: layout.open("roads.structure")?,
            shapes: layout.open_lists("roads.shapes")?,
            way_row: layout.open("roads.way_row")?,
            closures: layout.open("roads.closures")?,
        };
        let len = roads.len();
        let shapes = roads.shapes.len();
        if [roads.to.len(), roads.way_row.len(), roads.closures.len(), shapes].iter().any(|&n| n != len) {
            return Err("the road columns differ in length".into());
        }
        Ok(roads)
    }

    pub fn push(&mut self, road: Road, source: RoadSource) {
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
        self.reversed.push(u8::from(reversed));
        self.length_m.push(length_m);
        self.ascent_m.push(ascent_m);
        self.descent_m.push(descent_m);
        self.surface.push(source::surface_byte(surface));
        self.class.push(class);
        self.access.push(access);
        self.difficulty.push(difficulty);
        self.hiking_difficulty.push(source::hiking_byte(hiking_difficulty));
        self.structure.push(u8::from(structure));
        self.shapes.push(&shape);
        self.way_row.push(source.way_row);
        self.closures.push(source.closures);
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
            reversed: self.reversed(index),
            length_m: self.length_m.values()[index],
            ascent_m: self.ascent_m.values()[index],
            descent_m: self.descent_m.values()[index],
            surface: self.surface(index),
            class: self.class.values()[index],
            access: self.access.values()[index],
            difficulty: self.difficulty.values()[index],
            hiking_difficulty: source::hiking(self.hiking_difficulty.values()[index]),
            structure: self.structure.values()[index] != 0,
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

    pub fn surface(&self, index: usize) -> planner_router::model::Surface {
        source::surface(self.surface.values()[index])
    }

    pub fn class(&self, index: usize) -> u8 {
        self.class.values()[index]
    }

    pub fn shape(&self, index: usize) -> &[Point] {
        self.shapes.get(index)
    }

    pub fn reversed(&self, index: usize) -> bool {
        self.reversed.values()[index] != 0
    }

    pub fn way_row(&self, index: usize) -> u32 {
        self.way_row.values()[index]
    }

    pub fn closures(&self, index: usize) -> u32 {
        self.closures.values()[index]
    }

    /// Appends road `order[i]` of `bucket` for each `i`. With `terrain`, each appended shape is
    /// resampled and has heights, and its road the ascent and descent of those heights.
    pub fn append_permuted(&mut self, bucket: &Roads, order: &[u32], terrain: Option<Height>) -> Result<(), String> {
        let start = self.len();
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
            way_row,
            closures,
        } = self;
        from.extend_permuted(&bucket.from, order);
        to.extend_permuted(&bucket.to, order);
        way.extend_permuted(&bucket.way, order);
        reversed.extend_permuted(&bucket.reversed, order);
        length_m.extend_permuted(&bucket.length_m, order);
        ascent_m.extend_permuted(&bucket.ascent_m, order);
        descent_m.extend_permuted(&bucket.descent_m, order);
        surface.extend_permuted(&bucket.surface, order);
        class.extend_permuted(&bucket.class, order);
        access.extend_permuted(&bucket.access, order);
        difficulty.extend_permuted(&bucket.difficulty, order);
        hiking_difficulty.extend_permuted(&bucket.hiking_difficulty, order);
        structure.extend_permuted(&bucket.structure, order);
        way_row.extend_permuted(&bucket.way_row, order);
        closures.extend_permuted(&bucket.closures, order);
        let Some(height) = terrain else {
            shapes.extend_permuted(&bucket.shapes, order);
            return Ok(());
        };
        for &old in order {
            shapes.push_filled(terrain::resampled_len(bucket.shape(old as usize)), Point::default());
        }
        // Visit the roads by the GLO-30 square of their first point, then by fine cell, so the
        // terrain keeps only a few squares decoded.
        let degree = |micro: i32| micro.div_euclid(1_000_000);
        let visits = order
            .iter()
            .enumerate()
            .map(|(slot, &old)| {
                let p = bucket.shape(old as usize).first().ok_or("Terrain requires a road with at least two points")?;
                let (lat, lon) = package::cell(*p);
                Ok((([degree(p.lat), degree(p.lon)], [lat, lon]), slot as u32))
            })
            .collect::<Result<Column<_>, String>>()?;
        for (_, slot) in sort::sorted(visits) {
            let (new, old) = (start + slot as usize, order[slot as usize] as usize);
            let shape = bucket.shape(old);
            let structure = bucket.structure.values()[old] != 0;
            let (ascent, descent) = terrain::sample(shape, structure, &mut *height, shapes.get_mut(new))?;
            ascent_m.values_mut()[new] = ascent;
            descent_m.values_mut()[new] = descent;
        }
        Ok(())
    }
}

/// The roads of a test graph, without their source: `Graph::link_ways` gives it.
impl FromIterator<Road> for Roads {
    fn from_iter<I: IntoIterator<Item = Road>>(roads: I) -> Self {
        let mut result = Self::default();
        for road in roads {
            result.push(road, RoadSource::default());
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

/// The costs of the profiles of a package: the content of the costs layer.
pub struct Costs {
    pub profiles: Vec<Profile>,
    /// The costs of each profile, with the IDs of `dictionary`.
    roads: Vec<cost::RoadCosts>,
    /// The snap bits of each profile (`connectivity::States::snappable`).
    snap: Vec<Column<u64>>,
    /// The cost bases that the `bases` columns name, from ID 1.
    dictionary: Vec<CostBasis>,
}

impl Costs {
    const FILE: &'static str = "costs.json";
    const DICTIONARY: &'static str = "dictionary.postcard";

    /// Saves the dictionary, the columns of profile `i` as `<i>.bases`, `<i>.totals`, `<i>.turns`
    /// and `<i>.snap`, and `costs.json` with the profile IDs in their order.
    pub(crate) fn save(self, layout: &mut Layout) -> Result<(), String> {
        let write = |name: &str, bytes: Vec<u8>| {
            let path = layout.path(name)?;
            std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
        };
        write(Self::DICTIONARY, postcard::to_allocvec(&self.dictionary).expect("cost bases encode"))?;
        let ids: Vec<&str> = self.profiles.iter().map(|profile| profile.name.as_str()).collect();
        write(Self::FILE, serde_json::to_vec(&serde_json::json!({"profiles": ids})).expect("JSON values"))?;
        for (i, (costs, snap)) in self.roads.into_iter().zip(self.snap).enumerate() {
            layout.save(&format!("{i}.bases"), costs.bases)?;
            layout.save(&format!("{i}.totals"), costs.totals)?;
            layout.save(&format!("{i}.turns"), costs.turns)?;
            layout.save(&format!("{i}.snap"), snap)?;
        }
        Ok(())
    }

    /// The cost basis of road `road` for profile `profile`; `None` where the profile excludes it.
    pub fn basis(&self, profile: usize, road: usize) -> Option<CostBasis> {
        let id = self.roads[profile].bases.values()[road];
        Some(self.dictionary[id.checked_sub(1)? as usize])
    }

    /// The total cost of road `road` for profile `profile`; `u32::MAX` where it excludes the road.
    pub fn total(&self, profile: usize, road: usize) -> u32 {
        self.roads[profile].totals.values()[road]
    }

    /// The cost of each arc of the topology for profile `profile`; `u32::MAX` where it may not turn.
    pub fn turns(&self, profile: usize) -> &[u32] {
        self.roads[profile].turns.values()
    }

    /// The cost columns of each profile, the input of `tables::build`.
    pub fn columns(&self) -> Vec<tables::Costs<'_>> {
        self.roads
            .iter()
            .map(|costs| tables::Costs { totals: costs.totals.values(), turns: costs.turns.values() })
            .collect()
    }

    /// The snap bit of road `road` for profile `profile`.
    pub fn snappable(&self, profile: usize, road: usize) -> bool {
        self.snap[profile].values()[road / 64] >> (road % 64) & 1 != 0
    }

    pub(crate) fn open(layout: &Layout) -> Result<Self, String> {
        let read = |name: &str| {
            let path = layout.path(name)?;
            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
        };
        let dictionary = postcard::from_bytes(&read(Self::DICTIONARY)?).map_err(|e| format!("cost bases: {e}"))?;
        let ids: serde_json::Value = serde_json::from_slice(&read(Self::FILE)?).map_err(|e| e.to_string())?;
        let ids = ids["profiles"].as_array().ok_or("costs.json lists no profiles")?;
        let names: Vec<&str> = ids.iter().filter_map(serde_json::Value::as_str).collect();
        let profiles = profiles(&names)?;
        let (mut roads, mut snap) = (Vec::new(), Vec::new());
        for i in 0..profiles.len() {
            let bases = layout.open(&format!("{i}.bases"))?;
            let totals = layout.open(&format!("{i}.totals"))?;
            roads.push(cost::RoadCosts { totals, bases, turns: layout.open(&format!("{i}.turns"))? });
            snap.push(layout.open(&format!("{i}.snap"))?);
        }
        Ok(Self { profiles, roads, snap, dictionary })
    }
}

/// The preset profiles that `names` lists, in preset order; `all` lists every preset.
pub fn profiles(names: &[&str]) -> Result<Vec<Profile>, String> {
    let listed = |name: &str| names.iter().any(|listed| *listed == "all" || *listed == name);
    let profiles: Vec<_> = Profile::presets().into_iter().filter(|p| listed(&p.name)).collect();
    if profiles.is_empty() || names.iter().any(|name| *name != "all" && !profiles.iter().any(|p| p.name == *name)) {
        return Err("Unknown profile ID".into());
    }
    Ok(profiles)
}

/// Fails where `graph` and `profiles` cannot give a package.
pub fn check(graph: &Graph, profiles: &[Profile]) -> Result<(), String> {
    let roads = &graph.roads;
    let points = graph.points.values().len();
    if (0..roads.len()).any(|id| roads.from(id) as usize >= points || roads.to(id) as usize >= points) {
        return Err("Invalid road endpoints".into());
    }
    for restrictions in [graph.forbidden.values(), graph.forbidden_foot.values()] {
        if !restrictions.is_sorted() {
            return Err("Unsorted turn restrictions".into());
        }
        for &Turn { from: before, to: after } in restrictions {
            if before as usize >= roads.len() || after as usize >= roads.len() {
                return Err("Unknown restriction road".into());
            }
            if roads.to(before as usize) != roads.from(after as usize) {
                return Err("Restriction roads do not meet".into());
            }
        }
    }
    u32::try_from(roads.len()).map_err(|_| "Too many roads")?;
    if (0..roads.len()).any(|id| roads.shape(id).len() < 2 || roads.class(id) > 6) {
        return Err("Road lacks valid geometry or class".into());
    }
    for (i, profile) in profiles.iter().enumerate() {
        if profiles[..i].iter().any(|other| other.name == profile.name) {
            return Err("Duplicate metric identity".into());
        }
        profile.validate()?;
    }
    cost::check_nodes(graph)
}

/// The profile jobs that run at once: the shared topology and the `place` column of `States` take
/// 8 B for each road and 4 B for each arc once; each job adds its own columns.
fn profile_jobs(budget: u64, graph: &Graph, topology: &Topology) -> usize {
    let (r, a, j) = (graph.roads.len() as u64, topology.head.len() as u64, graph.points.len() as u64);
    let threads = rayon::current_num_threads() as u64;
    (budget.saturating_sub(8 * r + 4 * a) / (25 * r + 4 * a + 45 * j).max(1)).clamp(1, threads) as usize
}

/// The costs of `profiles` on `graph`, which `check` accepted, with its road-state graph
/// `topology`.
pub fn costs(graph: &Graph, topology: &Topology, profiles: &[Profile]) -> Result<Costs, String> {
    let roads = &graph.roads;
    let budget = work::page_budget()?.0;
    work::Guard::stage("way-bases");
    let bases = cost::way_bases(&graph.osm, profiles)?;
    work::Guard::stage("road-pass");
    // The random reads of a road group go to the way bases of its profiles: they take a quarter
    // of the budget at most.
    let ways = graph.osm.ways.len().max(1) as u64;
    let group = (budget / 4 / (8 * ways)).clamp(1, profiles.len().max(1) as u64) as usize;
    let mut costs = cost::road_pass(graph, topology, profiles, &bases, group)?;
    let mut dictionary = Dictionary::default();
    cost::renumber(&mut costs, &bases, &mut dictionary)?;
    drop(bases);
    work::Guard::stage("snap");
    let states = connectivity::States::new(topology, roads, graph.points.len());
    let jobs = profile_jobs(budget, graph, topology);
    eprintln!("Road pass: {group} profiles at a time; profile jobs: {jobs} at a time");
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build().map_err(|e| e.to_string())?;
    let snap = pool.install(|| {
        costs
            .par_iter()
            .with_max_len(1)
            .map(|costs| states.snappable(costs.totals.values(), costs.turns.values()))
            .collect()
    });
    Ok(Costs { profiles: profiles.to_vec(), roads: costs, snap, dictionary: dictionary.into_values() })
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
    write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Manifest, String> {
    check(graph, profiles)?;
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let costs = costs(graph, &topology, profiles)?;
    package(graph, topology, costs, (region, bounds), source_sha256, attribution, write)
}

/// Writes the package of `graph` with `costs`, which `costs` computed on `topology`: the landmark
/// distances of each profile, then the objects in profile order.
pub fn package(
    graph: &Graph,
    topology: Topology,
    costs: Costs,
    (region, bounds): (String, [f64; 4]),
    source_sha256: Vec<String>,
    attribution: String,
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Manifest, String> {
    let roads = &graph.roads;
    let count = u32::try_from(roads.len()).map_err(|_| "Too many roads")?;
    work::Guard::stage("landmarks");
    let junctions = landmarks::Junctions::new((0..roads.len()).map(|id| (roads.from(id), roads.to(id))))?;
    let jobs = profile_jobs(work::page_budget()?.0, graph, &topology);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build().map_err(|e| e.to_string())?;
    let distances = pool.install(|| {
        costs.roads.par_iter().with_max_len(1).map(|costs| junctions.compute(costs.totals.values())).collect()
    });
    let distances = cost::in_order(distances)?;
    let Costs { profiles, roads: costs, snap, dictionary } = costs;
    work::Guard::stage("writer");
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
    drop(topology);
    // The (1° group, cell, road) of each cell that a road passes.
    let mut cells = Column::<(([i32; 2], [i32; 2]), u32)>::default();
    let mut road_cells = Vec::new();
    for id in 0..roads.len() {
        for pair in roads.shape(id).windows(2) {
            let (a, b) = (package::cell(pair[0]), package::cell(pair[1]));
            if (a.0.abs_diff(b.0) as u64 + 1) * (a.1.abs_diff(b.1) as u64 + 1) > 4096 {
                return Err(format!("Road segment spans more than 4096 spatial cells of {} microdegrees", CELL));
            }
            for lat in a.0.min(b.0)..=a.0.max(b.0) {
                for lon in a.1.min(b.1)..=a.1.max(b.1) {
                    road_cells.push((([lat.div_euclid(100), lon.div_euclid(100)], [lat, lon]), id as u32));
                }
            }
        }
        road_cells.sort_unstable();
        road_cells.dedup();
        cells.extend(&road_cells);
        road_cells.clear();
    }
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
    // Each group writes the road lists of its cells, then its directory.
    let mut cells = sort::sorted(cells).peekable();
    let (mut ids, mut directory, mut current) = (Vec::new(), BTreeMap::new(), None);
    loop {
        let next = cells.next();
        let group = next.map(|((group, _), _)| group);
        if let Some([lat, lon]) = current.filter(|&current| Some(current) != group) {
            let directory = storage::encode(&std::mem::take(&mut directory))?;
            manifest.spatial.insert(package::cell_key((lat, lon)), write(&directory)?);
        }
        let Some(((group, [lat, lon]), id)) = next else { break };
        current = Some(group);
        ids.clear();
        ids.push(id);
        while let Some((_, id)) = cells.next_if(|&((_, cell), _)| cell == [lat, lon]) {
            ids.push(id);
        }
        directory.insert(package::cell_key((lat, lon)), write(&storage::encode(&ids)?)?);
    }
    drop(cells);
    let mut landmarks = junctions.index(&mut write)?;
    for ((profile, costs), (allowed, distances)) in profiles.iter().zip(&costs).zip(snap.iter().zip(&distances)) {
        let road_costs = costs.totals.values();
        let fragments = road_costs.iter().filter(|&&c| c != u32::MAX).count()
            - allowed.values().iter().map(|word| word.count_ones() as usize).sum::<usize>();
        eprintln!(
            "Profile {}: {fragments} roads in fragments are not snappable; landmark scale {}",
            profile.name, distances.scale
        );
        landmarks.profiles.insert(profile.name.clone(), distances.write(&mut write)?);
        manifest.metrics.insert(
            profile.name.clone(),
            base::metric(
                profile,
                costs.bases.values(),
                road_costs,
                costs.turns.values(),
                allowed.values(),
                &mut write,
            )?,
        );
    }
    manifest.costs = Table::write(&dictionary, &mut write)?;
    manifest.landmarks = Some(landmarks);
    manifest.closures = write_closures(graph, &mut write)?;
    Ok(manifest)
}

/// The transitions of the roads with the (from, to) junctions `ends`: the roads that arrive at
/// each junction times those that depart.
pub fn turns(ends: impl Iterator<Item = (u32, u32)>, junctions: usize) -> u64 {
    let [mut arriving, mut departing] = [(); 2].map(|_| Column::filled(junctions, 0u32));
    for (from, to) in ends {
        arriving.values_mut()[to as usize] += 1;
        departing.values_mut()[from as usize] += 1;
    }
    arriving.values().iter().zip(departing.values()).map(|(&a, &d)| u64::from(a) * u64::from(d)).sum()
}

/// Prints the counts of the import, and fails when the 32-bit IDs of format 9 cannot number the
/// roads and the transitions. The import checks them after walk 1, before the roads and their
/// terrain. `ways` and `references` are the counts of the ways, `ends` the (from, to) junction of
/// each road.
pub fn check_counts((ways, references): (usize, usize), ends: &[(u32, u32)], junctions: usize) -> Result<(), String> {
    let turns = turns(ends.iter().copied(), junctions);
    let (r, j) = (ends.len() as u64, junctions as u64);
    eprintln!("Imported: {ways} ways, {references} node references, {r} directed roads, {turns} turns, {j} junctions");
    if r >= u64::from(u32::MAX) {
        return Err(format!("Too many roads: {r}; format 9 allows fewer than 2^32 - 1"));
    }
    if turns >= 1 << 32 {
        return Err(format!("Too many road transitions: {turns}; format 9 allows fewer than 2^32"));
    }
    Ok(())
}

/// The manifest limit of format 9, which the costs step checks before its passes.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub profiles: usize,
    /// West, south, east, north: the 1° squares of the spatial index.
    pub bounds: [f64; 4],
}

impl Limits {
    /// Fails when the manifest of a package of `graph` passes the limit of format 9. Returns the
    /// estimate of the manifest size in bytes.
    pub fn check(self, graph: &Graph) -> Result<u64, String> {
        let Self { profiles, bounds } = self;
        let roads = &graph.roads;
        let turns = turns((0..roads.len()).map(|id| (roads.from(id), roads.to(id))), graph.points.len());
        let (r, j) = (roads.len() as u64, graph.points.len() as u64);
        // Each page reference of the manifest takes about 70 bytes: a hash and its JSON.
        let pages = |n: u64| n.div_ceil(4096);
        let squares = |low: f64, high: f64| (high.floor() - low.floor()) as u64 + 1;
        let per_profile = 2 * pages(r) + pages(turns) + pages(r.div_ceil(64)) + 6 * pages(j);
        let references = pages(r + 1)
            + pages(turns)
            + 2 * pages(r)
            + profiles as u64 * per_profile
            + pages(r.div_ceil(128))
            + 1
            + squares(bounds[0], bounds[2]) * squares(bounds[1], bounds[3]);
        let estimate = 70 * references + 64 * 1024;
        eprintln!("Manifest estimate: {estimate} bytes");
        if estimate > package::MAX_MANIFEST_BYTES as u64 {
            return Err(format!(
                "the package of this region needs a manifest of about {} MiB; format 9 allows 128 MiB",
                estimate.div_ceil(1 << 20)
            ));
        }
        Ok(estimate)
    }
}

/// Fails when the closure sets of the roads pass the page limit of format 9.
pub fn check_closures(graph: &Graph) -> Result<(), String> {
    let roads = &graph.roads;
    let mut used = std::collections::BTreeSet::new();
    used.extend((0..roads.len()).map(|id| roads.closures(id)).filter(|&set| set != u32::MAX));
    let sets: Vec<_> = used.into_iter().map(|set| graph.sets.get(set)).collect();
    storage::encode(&sets).map_err(|_| "The closure sets of the roads pass the page limit of format 9")?;
    Ok(())
}

/// Writes the sets object and the per-road column only when a road has a possible closure. A set
/// gets its ID at its first use in road order.
fn write_closures(
    graph: &Graph,
    write: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Option<closures::Index>, String> {
    let roads = &graph.roads;
    let (mut ids, mut sets) = (vec![u32::MAX; graph.sets.len()], Vec::new());
    for road in 0..roads.len() {
        let set = roads.closures(road);
        if let Some(id) = ids.get_mut(set as usize).filter(|id| **id == u32::MAX) {
            *id = sets.len() as u32;
            sets.push(graph.sets.get(set));
        }
    }
    if sets.is_empty() {
        return Ok(None);
    }
    let id = |road| ids.get(roads.closures(road) as usize).map_or(u64::MAX, |&id| u64::from(id));
    Ok(Some(closures::Index {
        sets: write(&storage::encode(&sets)?)?,
        roads: planner_router::base::write_column((0..roads.len()).map(id), write)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::{Surface, BIKE, FOOT, NO_ELEVATION, PUSH};
    use source::{Builder, Tags, Ways};

    /// A ladder: junction `i` meets `i + 1` and `i + 2`, each pair by a way in both directions.
    fn ladder() -> Graph {
        let points: Vec<_> =
            (0..20).map(|i| Point { lat: i % 2 * 1000, lon: i / 2 * 1000, elevation: NO_ELEVATION }).collect();
        let (mut roads, mut ways) = (Vec::new(), Builder::<Ways>::default());
        for a in 0..20u32 {
            for b in [a + 1, a + 2].into_iter().filter(|&b| b < 20) {
                let way = i64::from(100 * a + b);
                // Classes and surfaces that cycle with another period than the chunks, so each
                // chunk has its own first bases.
                let class = (a + b) % 5;
                let highway = ["cycleway", "residential", "primary", "track", "path"][class as usize];
                let surface = [(Surface::Paved, "asphalt"), (Surface::Gravel, "gravel")][(a * b % 2) as usize];
                let tags = Tags::from_pairs([("highway", highway), ("surface", surface.1)]);
                ways.push(way, &[i64::from(a), i64::from(b)], &tags);
                for (from, to) in [(a, b), (b, a)] {
                    let shape = vec![points[from as usize], points[to as usize]];
                    roads.push(Road {
                        from,
                        to,
                        way,
                        reversed: from > to,
                        length_m: shape[0].distance(shape[1]).round() as u32,
                        surface: surface.0,
                        class: class as u8,
                        access: BIKE | FOOT | PUSH,
                        difficulty: 255,
                        shape,
                        ..Road::default()
                    });
                }
            }
        }
        let mut graph = Graph {
            node_ids: (0..20).collect(),
            node_access: Column::filled(20, BIKE | FOOT | PUSH),
            points: points.into_iter().collect(),
            roads: roads.into_iter().collect(),
            ..Graph::default()
        };
        graph.osm.ways = ways.finish().0;
        graph.link_ways();
        graph
    }

    #[test]
    fn one_and_four_threads_give_the_same_package() {
        let graph = ladder();
        let profiles: Vec<_> = Profile::presets().into_iter().take(3).collect();
        let package = |threads| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let mut objects = Vec::new();
            let manifest = pool.install(|| {
                prepare(&graph, "test".into(), [0.0, 0.0, 0.01, 0.01], &profiles, vec![], "test".into(), |bytes| {
                    objects.push(bytes.to_vec());
                    Ok(package::digest(bytes))
                })
            });
            (serde_json::to_vec(&manifest.unwrap()).unwrap(), objects)
        };
        assert!(graph.roads.len() > 4 * CHUNK, "the roads fill several chunks");
        assert_eq!(package(1), package(4));
    }

    #[test]
    fn the_chunks_give_the_costs_of_each_road_computed_alone() {
        let graph = ladder();
        let profiles: Vec<_> = Profile::presets().into_iter().take(3).collect();
        let topology = Topology::new(&graph.roads, graph.points.len()).unwrap();
        let bases = cost::way_bases(&graph.osm, &profiles).unwrap();
        assert!(graph.osm.ways.len() > 2 * CHUNK, "the ways fill several chunks");
        let costs = cost::road_pass(&graph, &topology, &profiles, &bases, 2).unwrap();
        let bits = |basis: Option<CostBasis>| basis.map(|b| (b.factor.to_bits(), b.turn.to_bits(), b.ferry));
        for (profile, costs) in profiles.iter().zip(&costs) {
            let alone = |road: u32| {
                let way = graph.osm.ways.row(graph.roads.way_row(road as usize) as usize);
                cost::way_basis(&way, graph.roads.reversed(road as usize), profile)
            };
            let mut distinct = std::collections::BTreeSet::new();
            for (road, shaped) in graph.roads.iter().enumerate() {
                let basis = alone(road as u32);
                distinct.insert(bits(basis));
                assert_eq!(bits(bases.value(costs.bases.values()[road])), bits(basis), "{}", profile.name);
                let total = basis.map(|basis| basis.compile(&shaped, profile).unwrap().total());
                assert_eq!(costs.totals.values()[road], base::narrow(total).unwrap());
            }
            assert!(distinct.len() > 3, "{}: the ways have different bases", profile.name);
            for (arc, (before, after)) in topology.edges().enumerate() {
                let turn = cost::transition(&graph, profile, (alone(before), alone(after)), before, after);
                assert_eq!(costs.turns.values()[arc], base::narrow(turn).unwrap(), "{}", profile.name);
            }
        }
    }

    #[test]
    fn the_manifest_estimate_is_not_below_the_manifest() {
        let graph = ladder();
        let profiles: Vec<_> = Profile::presets().into_iter().take(2).collect();
        let bounds = [0.0, 0.0, 0.01, 0.01];
        let estimate = Limits { profiles: profiles.len(), bounds }.check(&graph).unwrap();
        check_closures(&graph).unwrap();
        let manifest = prepare(&graph, "test".into(), bounds, &profiles, vec![], "test".into(), |bytes| {
            Ok(package::digest(bytes))
        })
        .unwrap();
        let json = serde_json::to_vec(&manifest).unwrap();
        assert!(json.len() as u64 <= estimate);
        // The page references, each a SHA-256 in hex, are the part of the estimate that grows.
        fn hashes(value: &serde_json::Value) -> u64 {
            match value {
                serde_json::Value::String(text) => {
                    u64::from(text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
                }
                serde_json::Value::Array(items) => items.iter().map(hashes).sum(),
                serde_json::Value::Object(map) => map.values().map(hashes).sum(),
                _ => 0,
            }
        }
        let references = (estimate - 64 * 1024) / 70;
        let found = hashes(&serde_json::from_slice(&json).unwrap());
        assert!(found <= references, "{found} page references, estimate {references}");
    }
}
