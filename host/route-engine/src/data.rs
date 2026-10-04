//! Query access is independent of whether data is one region or a union of published blocks.
use crate::{
    base::{Column, Costs, Graph, Width},
    blocks::{self, Ids, Union},
    closures::Closures,
    cost::CostBasis,
    landmarks::{self, Guide, Junctions},
    model::{Point, Profile, Road},
    package::{cell_key, digest, Departure, Endpoint, Manifest, Package, Source, MAX_MANIFEST_BYTES, ROADS_PER_PAGE},
    snap::{self, Candidates, Policy},
    Error, Result,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

/// What the router reads. `Selection` is its one implementation; the trait keeps the router
/// generic over the storage behind it.
pub trait RoutingData {
    fn identity(&self) -> &str;
    fn region(&self) -> &str;
    fn bounds(&self) -> [f64; 4];
    fn attribution(&self) -> &str;
    fn warnings(&self) -> &[String];
    fn profiles(&self) -> Vec<&str>;
    fn profile(&self, name: &str) -> Result<&Profile>;
    fn snap(&self, point: Point, metric: &str, policy: Policy) -> Result<Candidates>;
    fn road(&self, road: u32) -> Result<Road>;
    fn closures(&self) -> Result<Arc<Closures>>;
    fn endpoint(&self, metric: &str, road: u32) -> Result<Endpoint>;
    /// A metric's graph, costs and landmark columns, loaded and budgeted together.
    fn prepared(&self, metric: &str) -> Result<Arc<Prepared>>;
    /// The estimated working set with `metric` prepared beside the cached metrics.
    fn routing_bytes(&self, metric: &str) -> Result<usize>;
    fn memory_budget(&self) -> usize;
    fn set_memory_budget(&mut self, bytes: usize);
    /// The budget native hosts give a router: the costliest profile's routing bytes plus room
    /// for the search queues and the other cached profiles, at least `MINIMUM_BUDGET`.
    fn default_budget(&self) -> usize {
        let costliest = self.profiles().into_iter().filter_map(|profile| self.routing_bytes(profile).ok()).max();
        MINIMUM_BUDGET.max(costliest.unwrap_or(0).saturating_add(SEARCH_HEAP))
    }
}

/// Room above the costliest profile for search labels and queues and for the cached costs of
/// other profiles, which a router drops first when it needs the room.
const SEARCH_HEAP: usize = 256 * 1024 * 1024;
const MINIMUM_BUDGET: usize = 768 * 1024 * 1024;

/// One metric, ready to search.
pub struct Prepared {
    pub graph: Arc<Graph>,
    pub costs: Costs,
    pub guide: Option<Guide>,
}

/// Up to this many prepared metrics stay cached within the budget.
const CACHED_PROFILES: usize = 3;
/// Room the search queues keep when the cache decides whether to retain another metric.
const QUEUE_HEADROOM: usize = 16 * 1024 * 1024;
/// Geometry, index caches, compressed pages and decode scratch.
const MARGIN: usize = 64 * 1024 * 1024;

/// Where the roads of a fine snap cell are listed.
enum Snap {
    /// The manifest's one-degree directories of a complete package.
    Directory,
    /// The flat cell map of a grid selection.
    Cells(BTreeMap<String, String>),
}

/// Loaded once per package and shared by every fork.
#[derive(Default)]
struct Shared {
    union: Option<Arc<Union>>,
    junctions: Option<Arc<Junctions>>,
    closures: Option<Arc<Closures>>,
    profiles: VecDeque<(String, Arc<Prepared>)>,
}

/// The roads a router sees: all of a package, or the ranges of a grid selection. Road ids are
/// compact and in source order.
pub struct Selection<S> {
    package: Package<S>,
    identity: String,
    ids: Arc<Ids>,
    /// Source arcs that leave the selected roads, before those to absent roads are dropped.
    arcs: u32,
    snap: Arc<Snap>,
    /// Metrics with identical turn tables share one decoded turn column.
    turn_groups: Arc<BTreeMap<String, usize>>,
    shared: Arc<Mutex<Shared>>,
    memory_budget: usize,
}

fn turn_groups(manifest: &Manifest) -> BTreeMap<String, usize> {
    let mut tables: Vec<&Column> = Vec::new();
    manifest
        .metrics
        .iter()
        .map(|(name, metric)| {
            let group = tables.iter().position(|table| **table == metric.weights.turns).unwrap_or_else(|| {
                tables.push(&metric.weights.turns);
                tables.len() - 1
            });
            (name.clone(), group)
        })
        .collect()
}

impl<S: Source> Selection<S> {
    /// A complete package as a selection of all of its roads.
    pub fn whole(package: Package<S>) -> Self {
        let manifest = package.manifest();
        Self {
            identity: package.identity().to_owned(),
            ids: Arc::new(Ids::whole(manifest.roads)),
            arcs: manifest.graph.head.len,
            snap: Arc::new(Snap::Directory),
            turn_groups: Arc::new(turn_groups(manifest)),
            shared: Default::default(),
            memory_budget: usize::MAX,
            package,
        }
    }

    /// A grid selection from its `blocks.json` bytes.
    pub fn open(source: S, bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Limit);
        }
        let manifest: blocks::Manifest =
            serde_json::from_slice(bytes).map_err(|e| Error::InvalidData(e.to_string()))?;
        Self::new(source, manifest, digest(bytes))
    }

    pub fn new(source: S, manifest: blocks::Manifest, identity: String) -> Result<Self> {
        manifest.validate()?;
        let blocks::Manifest { source: source_id, data, roads, arcs, snap, .. } = manifest;
        let ids = Ids::new(roads, data.roads)?;
        let turn_groups = Arc::new(turn_groups(&data));
        Ok(Self {
            package: Package::new(source, data, source_id)?,
            identity,
            ids: Arc::new(ids),
            arcs,
            snap: Arc::new(Snap::Cells(snap)),
            turn_groups,
            shared: Default::default(),
            memory_budget: usize::MAX,
        })
    }

    /// A handle with its own page caches; graph, junctions, closures and prepared metrics are shared.
    pub fn fork(&self) -> Self
    where
        S: Clone,
    {
        Self {
            package: self.package.fork(),
            identity: self.identity.clone(),
            ids: Arc::clone(&self.ids),
            arcs: self.arcs,
            snap: Arc::clone(&self.snap),
            turn_groups: Arc::clone(&self.turn_groups),
            shared: Arc::clone(&self.shared),
            memory_budget: self.memory_budget,
        }
    }

    /// The package reader, in source road ids.
    pub fn package(&self) -> &Package<S> {
        &self.package
    }
    pub fn local_id(&self, source: u32) -> Option<u32> {
        self.ids.local(source)
    }
    pub fn source_id(&self, local: u32) -> Result<u32> {
        self.ids.source(local)
    }

    /// Every object the selection refers to, in the source's verification order. Decodes the
    /// cost dictionary, the per-road cost ids, the access words and the closures on the way.
    pub fn objects(&self) -> Result<Vec<String>> {
        let package = &self.package;
        let manifest = package.manifest();
        let mut keys: BTreeSet<String> = manifest.tables().flat_map(|table| table.blocks.iter().cloned()).collect();
        for block in manifest.costs.positions() {
            if package.bases.borrow_mut().block(package, &manifest.costs, block)?.iter().any(|cost| !cost.valid()) {
                return Err(Error::InvalidData("Invalid cost dictionary".into()));
            }
        }
        for metric in manifest.metrics.values() {
            for block in metric.costs.positions() {
                package.numbers.borrow_mut().block(package, &metric.costs, block)?;
            }
            for block in metric.allowed.positions() {
                package.words.borrow_mut().block(package, &metric.allowed, block)?;
            }
        }
        if let Some(key) = &manifest.closures {
            keys.insert(key.clone());
            self.closures()?;
        }
        for table in manifest.osm.tables() {
            keys.extend(package.keys(table)?);
        }
        let mut page = None;
        for id in self.ids.iter() {
            if page != Some(id / ROADS_PER_PAGE) {
                page = Some(id / ROADS_PER_PAGE);
                keys.insert(package.key(&manifest.geometry, id / ROADS_PER_PAGE)?);
            }
        }
        match &*self.snap {
            Snap::Directory => {
                for key in manifest.spatial.values() {
                    keys.insert(key.clone());
                    keys.extend(package.spatial_directory(key)?.into_values());
                }
            }
            Snap::Cells(cells) => keys.extend(cells.values().cloned()),
        }
        let mut keys: Vec<_> = keys.into_iter().collect();
        package.source().order_for_verify(&mut keys)?;
        Ok(keys)
    }

    /// Check the complete object closure before publishing or installing.
    pub fn verify(&self) -> Result<()> {
        for key in self.objects()? {
            self.package.bytes(&key)?;
        }
        Ok(())
    }

    /// Bulk extraction keeps the exact cost factors instead of compiling each curve.
    pub fn prepared_endpoints(&self, metric: &str, roads: &[u32]) -> Result<Vec<Endpoint<CostBasis>>> {
        let prepared = self.prepared(metric)?;
        roads
            .iter()
            .map(|&road| {
                let Some(basis) = self.package.prepared_cost(metric, self.ids.source(road)?)? else {
                    return Ok(Endpoint { cost: None, arrival: road, departures: Vec::new() });
                };
                if prepared.costs.roads.get(road as usize) == u64::MAX {
                    return Err(Error::InvalidData("Missing accessible road cost".into()));
                }
                Ok(Endpoint { cost: Some(basis), arrival: road, departures: departures(&prepared, road) })
            })
            .collect()
    }

    fn shared(&self) -> Result<std::sync::MutexGuard<'_, Shared>> {
        self.shared.lock().map_err(|_| Error::Limit)
    }

    fn union(&self, shared: &mut Shared) -> Result<Arc<Union>> {
        if shared.union.is_none() {
            shared.union = Some(Arc::new(Union::open(&self.package, &self.ids, self.arcs)?));
        }
        Ok(Arc::clone(shared.union.as_ref().unwrap()))
    }

    fn load(&self, shared: &mut Shared, metric: &str) -> Result<Prepared> {
        let package = &self.package;
        let union = self.union(shared)?;
        let turns = shared.profiles.iter().find_map(|(name, prepared)| {
            (self.turn_groups[name] == self.turn_groups[metric]).then(|| Arc::clone(&prepared.costs.turns))
        });
        let costs = union.costs(package, &self.ids, metric, turns)?;
        let columns =
            package.manifest().landmarks.as_ref().and_then(|index| Some((index, index.profiles.get(metric)?)));
        let guide = match columns {
            None => None,
            Some((index, tables)) => {
                if shared.junctions.is_none() {
                    shared.junctions = Some(Arc::new(Junctions::read(package, index, &self.ids)?));
                }
                let junctions = shared.junctions.as_ref().unwrap();
                let columns = tables
                    .iter()
                    .map(|table| landmarks::read(package, table, junctions.selected.iter(), u16::MAX as u32))
                    .collect::<Result<_>>()?;
                Some(Guide { mapping: Arc::clone(&junctions.mapping), scale: index.scale, columns })
            }
        };
        Ok(Prepared { graph: Arc::clone(&union.graph), costs, guide })
    }

    /// The working set with `metric` beside the cached metrics: the graph, the search labels,
    /// the junction mapping, and for each metric its cost columns and landmark columns.
    fn estimate(&self, shared: &Shared, metric: &str) -> Result<usize> {
        let manifest = self.package.manifest();
        let roads = self.ids.count as usize;
        let arcs = self.arcs as usize;
        let width = |column: &Column| match column.width {
            Width::U8 => 1usize,
            Width::U16 => 2,
            Width::U32 => 4,
            Width::U64 => 8,
        };
        let mut bytes = roads
            .saturating_add(1)
            .saturating_mul(8)
            .saturating_add(arcs.saturating_mul(8 + width(&manifest.graph.reverse_offsets)))
            .saturating_add(arcs.div_ceil(64).saturating_mul(8))
            .saturating_add(self.ids.ranges().saturating_mul(16))
            .saturating_add(crate::search::label_bytes(roads))
            .saturating_add(MARGIN);
        if shared.union.is_none() {
            // Linking scratch: the reverse cursor and the fast id lookup.
            bytes = bytes.saturating_add(roads.saturating_add(1).saturating_mul(4));
            bytes = bytes.saturating_add(self.ids.lookup_bytes());
        }
        let mut nodes = 0;
        if let Some(index) = &manifest.landmarks {
            bytes = bytes.saturating_add(roads.saturating_mul(4));
            nodes = match &shared.junctions {
                Some(junctions) => junctions.selected.count as usize,
                None => roads.min(index.junctions as usize),
            };
        }
        let mut turns: Vec<usize> = Vec::new();
        for name in std::iter::once(metric)
            .chain(shared.profiles.iter().map(|(name, _)| name.as_str()).filter(|&name| name != metric))
        {
            let weights = &manifest.metric(name)?.weights;
            bytes = bytes.saturating_add(roads.saturating_mul(width(&weights.road_costs)));
            if !turns.contains(&self.turn_groups[name]) {
                bytes = bytes.saturating_add(arcs.saturating_mul(width(&weights.turns)));
                turns.push(self.turn_groups[name]);
            }
            if let Some(tables) = manifest.landmarks.as_ref().and_then(|index| index.profiles.get(name)) {
                bytes = bytes.saturating_add(tables.len().saturating_mul(nodes).saturating_mul(2));
            }
        }
        Ok(bytes)
    }
}

fn departures(prepared: &Prepared, road: u32) -> Vec<Departure> {
    let graph = &prepared.graph;
    (graph.reverse_first[road as usize]..graph.reverse_first[road as usize + 1])
        .filter_map(|reverse| {
            let reverse = reverse as usize;
            let penalty = prepared.costs.turns.get(graph.reverse_arc(reverse));
            (penalty != u64::MAX).then_some(Departure { state: graph.reverse_tail[reverse], penalty })
        })
        .collect()
}

impl<S: Source> RoutingData for Selection<S> {
    fn identity(&self) -> &str {
        &self.identity
    }
    fn region(&self) -> &str {
        &self.package.manifest().region
    }
    fn bounds(&self) -> [f64; 4] {
        self.package.manifest().bounds
    }
    fn attribution(&self) -> &str {
        &self.package.manifest().attribution
    }
    fn warnings(&self) -> &[String] {
        &self.package.manifest().warnings
    }
    fn profiles(&self) -> Vec<&str> {
        self.package.manifest().metrics.keys().map(String::as_str).collect()
    }
    fn profile(&self, name: &str) -> Result<&Profile> {
        Ok(&self.package.metric(name)?.profile)
    }

    fn snap(&self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
        let package = &self.package;
        package.metric(metric)?;
        let mut roads = BTreeSet::new();
        for cell in snap::cells(point, policy.radius_m).map_err(Error::InvalidRequest)? {
            let found = match &*self.snap {
                Snap::Directory => package.spatial_roads(cell)?,
                Snap::Cells(cells) => match cells.get(&cell_key(cell)) {
                    Some(key) => package.read(key)?,
                    None => Vec::new(),
                },
            };
            if found.iter().any(|&id| id >= package.manifest().roads) {
                return Err(Error::InvalidData("Snap road outside package".into()));
            }
            roads.extend(found.into_iter().filter_map(|id| self.ids.local(id)));
            if roads.len() > 100_000 {
                return Err(Error::Limit);
            }
        }
        snap::candidates(roads, point, policy, |local| {
            let source = self.ids.source(local)?;
            if package.allowed(metric, source)? {
                package.road(source).map(Some)
            } else {
                Ok(None)
            }
        })
    }

    fn road(&self, road: u32) -> Result<Road> {
        self.package.road(self.ids.source(road)?)
    }

    fn closures(&self) -> Result<Arc<Closures>> {
        let mut shared = self.shared()?;
        if let Some(closures) = &shared.closures {
            return Ok(Arc::clone(closures));
        }
        let source = self.package.closures()?;
        let mut roads: Vec<_> =
            source.roads.iter().filter_map(|&(road, entry)| Some((self.ids.local(road)?, entry))).collect();
        roads.sort_unstable();
        let closures = Arc::new(Closures { roads, entries: source.entries });
        shared.closures = Some(Arc::clone(&closures));
        Ok(closures)
    }

    fn endpoint(&self, metric: &str, road: u32) -> Result<Endpoint> {
        let source = self.ids.source(road)?;
        let Some(basis) = self.package.prepared_cost(metric, source)? else {
            return Ok(Endpoint { cost: None, arrival: road, departures: Vec::new() });
        };
        let cost = basis.compile(&self.package.road(source)?, self.profile(metric)?).map_err(Error::InvalidData)?;
        let prepared = self.prepared(metric)?;
        if cost.total() != prepared.costs.roads.get(road as usize) {
            return Err(Error::InvalidData("Road cost differs from its basis".into()));
        }
        Ok(Endpoint { cost: Some(cost), arrival: road, departures: departures(&prepared, road) })
    }

    fn prepared(&self, metric: &str) -> Result<Arc<Prepared>> {
        self.package.metric(metric)?;
        let mut shared = self.shared()?;
        let cached =
            shared.profiles.iter().position(|(name, _)| name == metric).map(|at| shared.profiles.remove(at).unwrap().1);
        // Leave space for search queues before retaining other metrics.
        while !shared.profiles.is_empty()
            && (shared.profiles.len() >= CACHED_PROFILES
                || self.estimate(&shared, metric)?.saturating_add(QUEUE_HEADROOM) > self.memory_budget)
        {
            shared.profiles.pop_front();
        }
        if self.estimate(&shared, metric)? > self.memory_budget {
            return Err(Error::Limit);
        }
        let prepared = match cached {
            Some(prepared) => prepared,
            None => Arc::new(self.load(&mut shared, metric)?),
        };
        shared.profiles.push_back((metric.into(), Arc::clone(&prepared)));
        Ok(prepared)
    }

    fn routing_bytes(&self, metric: &str) -> Result<usize> {
        self.package.metric(metric)?;
        let shared = self.shared()?;
        self.estimate(&shared, metric)
    }

    fn memory_budget(&self) -> usize {
        self.memory_budget
    }
    fn set_memory_budget(&mut self, bytes: usize) {
        self.memory_budget = bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        base::{write_topology, write_weights, Topology, Weights},
        model::Profile,
        package::{Manifest, Metric, OsmPages, FORMAT},
        table::{self, Table},
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[derive(Clone, Default)]
    struct Memory {
        objects: Rc<RefCell<BTreeMap<String, Vec<u8>>>>,
        reads: Rc<Cell<usize>>,
    }
    impl Source for Memory {
        fn read(&self, key: &str) -> Result<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            self.objects.borrow().get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
        }
    }
    impl Memory {
        fn write(&self, bytes: &[u8]) -> std::result::Result<String, String> {
            let key = digest(bytes);
            self.objects.borrow_mut().insert(key.clone(), bytes.to_vec());
            Ok(key)
        }
        fn manifest(&self, roads: u32, graph: Topology, weights: Weights) -> Manifest {
            let placeholder = |len: u32| Table {
                len,
                blocks: vec!["0".repeat(64); (len as usize).div_ceil(table::ENTRIES)],
                pages: None,
            };
            let profile = Profile::presets().remove(0);
            let metric = Metric {
                profile: profile.clone(),
                weights,
                costs: placeholder(roads),
                allowed: placeholder(roads.div_ceil(64)),
            };
            Manifest {
                format: FORMAT,
                region: "test".into(),
                bounds: [0., 0., 1., 1.],
                source_sha256: vec![],
                attribution: String::new(),
                warnings: vec![],
                roads,
                graph,
                geometry: placeholder(roads.div_ceil(ROADS_PER_PAGE)),
                osm: OsmPages::default(),
                spatial: BTreeMap::new(),
                metrics: BTreeMap::from([(profile.name, metric)]),
                costs: Table::default(),
                landmarks: None,
                closures: None,
            }
        }
        fn selection(&self, manifest: &Manifest) -> Selection<Self> {
            Selection::whole(Package::open(self.clone(), &serde_json::to_vec(manifest).unwrap()).unwrap())
        }
    }

    #[test]
    fn forks_share_one_graph_one_prepared_metric_and_one_closures_table() {
        let memory = Memory::default();
        let mut edges: Vec<_> = (0..5000).map(|to| (0, to)).collect();
        edges.extend([(4095, 0), (4096, 4999), (4999, 1)]);
        let topology = write_topology(5000, &edges, |b| memory.write(b)).unwrap();
        let weights = write_weights(&vec![1; 5000], &vec![0; edges.len()], |b| memory.write(b)).unwrap();
        let selection = memory.selection(&memory.manifest(5000, topology, weights.clone()));
        let fork = selection.fork();
        let prepared = selection.prepared("touring").unwrap();
        let graph = &prepared.graph;
        for to in 0..graph.nodes() {
            for index in graph.reverse_first[to]..graph.reverse_first[to + 1] {
                let arc = graph.reverse_arc(index as usize);
                assert_eq!(edges[arc], (graph.reverse_tail[index as usize], to as u32));
            }
        }
        assert_eq!(prepared.costs.arc(graph, 0), 1);
        for maximum in [254, 255, 65534, 65535, u32::MAX as u64 - 1, u32::MAX as u64, u64::MAX - 1] {
            let original = [0, 1, maximum, u64::MAX];
            let column = write_weights(&original, &[], |b| memory.write(b)).unwrap().road_costs;
            let decoded = blocks::numbers(selection.package(), &column, 0..4, 4).unwrap();
            assert_eq!((0..original.len()).map(|i| decoded.get(i)).collect::<Vec<_>>(), original);
        }
        memory.reads.set(0);
        let again = fork.prepared("touring").unwrap();
        assert!(Arc::ptr_eq(&prepared, &again));
        assert!(Arc::ptr_eq(&selection.closures().unwrap(), &fork.closures().unwrap()));
        assert_eq!(memory.reads.get(), 0);
        assert_eq!(selection.identity(), fork.identity());
        assert!(write_topology(2, &[(1, 0), (0, 1)], |b| memory.write(b)).is_err());
        assert!(write_topology(2, &[(0, 1), (0, 1)], |b| memory.write(b)).is_err());
    }

    #[test]
    fn metrics_share_only_identical_turn_columns_and_the_oldest_leaves_first() {
        let memory = Memory::default();
        let topology = write_topology(2, &[(0, 1)], |b| memory.write(b)).unwrap();
        let weights = write_weights(&[1, 2], &[0], |b| memory.write(b)).unwrap();
        let mut manifest = memory.manifest(2, topology, weights);
        for (name, roads, turns) in [("gravel", [4, 5], [0]), ("hiking", [6, 7], [2]), ("mtb", [8, 9], [0])] {
            let mut metric = manifest.metrics["touring"].clone();
            metric.profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
            metric.weights = write_weights(&roads, &turns, |b| memory.write(b)).unwrap();
            manifest.metrics.insert(name.into(), metric);
        }
        let selection = memory.selection(&manifest);
        let touring = selection.prepared("touring").unwrap();
        memory.reads.set(0);
        let gravel = selection.prepared("gravel").unwrap();
        assert_eq!(memory.reads.get(), 1);
        assert!(Arc::ptr_eq(&touring.costs.turns, &gravel.costs.turns));
        let hiking = selection.prepared("hiking").unwrap();
        assert!(!Arc::ptr_eq(&touring.costs.turns, &hiking.costs.turns));
        let graph = &touring.graph;
        assert_eq!([touring.costs.arc(graph, 0), gravel.costs.arc(graph, 0), hiking.costs.arc(graph, 0)], [2, 5, 9]);
        selection.prepared("mtb").unwrap();
        memory.reads.set(0);
        assert!(Arc::ptr_eq(&selection.prepared("gravel").unwrap(), &gravel));
        assert_eq!(memory.reads.get(), 0);
        assert!(!Arc::ptr_eq(&selection.prepared("touring").unwrap(), &touring));
    }

    #[test]
    fn malformed_graph_costs_and_insufficient_memory_fail_before_search() {
        let memory = Memory::default();
        let topology = write_topology(2, &[(0, 1)], |b| memory.write(b)).unwrap();
        let weights = write_weights(&[1, u64::MAX - 2], &[3], |b| memory.write(b)).unwrap();
        let manifest = memory.manifest(2, topology.clone(), weights);
        let mut selection = memory.selection(&manifest);
        selection.set_memory_budget(selection.routing_bytes("touring").unwrap() - 1);
        assert!(matches!(selection.prepared("touring"), Err(Error::Limit)));
        assert_eq!(memory.reads.get(), 0);
        selection.set_memory_budget(usize::MAX);
        assert!(matches!(selection.prepared("touring"), Err(Error::InvalidData(_))));
        let mut invalid = topology;
        invalid.head.blocks[0] = memory.write(&crate::storage::encode(&Vec::<u32>::new()).unwrap()).unwrap();
        let mut broken = manifest;
        broken.graph = invalid;
        assert!(matches!(memory.selection(&broken).prepared("touring"), Err(Error::InvalidData(_))));
    }
}
