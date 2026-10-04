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
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
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
    /// This router's estimated working set with `metric` prepared: its own label blocks and its
    /// share of the data the forks hold together.
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

/// Cached prepared metrics per router sharing the cache.
const CACHED_PROFILES: usize = 3;
/// Room each router keeps for its search queues when the cache decides what to retain.
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

/// Loaded once per package and shared by every fork. No load runs while `state` is held, so
/// hot reads never wait for a cold load.
#[derive(Default)]
struct Shared {
    union: OnceLock<Arc<Union>>,
    junctions: OnceLock<Arc<Junctions>>,
    closures: OnceLock<Arc<Closures>>,
    /// Cold loads take this lock, so forks that need the same data wait for one copy.
    loading: Mutex<()>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    profiles: VecDeque<(String, Arc<Prepared>)>,
    /// Evicted metrics a router still uses; they count until their last reference drops.
    retired: Vec<(String, Arc<Prepared>)>,
    /// Live handles and the sum of their budgets: the cache is sized for all of them.
    handles: usize,
    budgets: u128,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The data behind every lock is complete once inserted, so a panic elsewhere leaves it usable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
    shared: Arc<Shared>,
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
    fn with(package: Package<S>, identity: String, ids: Ids, arcs: u32, snap: Snap) -> Self {
        let state = State { profiles: VecDeque::new(), retired: Vec::new(), handles: 1, budgets: usize::MAX as u128 };
        Self {
            turn_groups: Arc::new(turn_groups(package.manifest())),
            package,
            identity,
            ids: Arc::new(ids),
            arcs,
            snap: Arc::new(snap),
            shared: Arc::new(Shared { state: Mutex::new(state), ..Default::default() }),
            memory_budget: usize::MAX,
        }
    }

    /// A complete package as a selection of all of its roads.
    pub fn whole(package: Package<S>) -> Self {
        let manifest = package.manifest();
        let (roads, arcs) = (manifest.roads, manifest.graph.head.len);
        let identity = package.identity().to_owned();
        Self::with(package, identity, Ids::whole(roads), arcs, Snap::Directory)
    }

    /// A grid selection from its `blocks.json` bytes.
    pub fn open(source: S, bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Limit);
        }
        let manifest: blocks::Manifest =
            serde_json::from_slice(bytes).map_err(|e| Error::InvalidData(e.to_string()))?;
        Self::new(manifest, digest(bytes), |_| Ok(source))
    }

    /// A grid selection over the source that `source` opens for its validated pack ids.
    pub fn new(
        manifest: blocks::Manifest,
        identity: String,
        source: impl FnOnce(&[String]) -> Result<S>,
    ) -> Result<Self> {
        manifest.validate()?;
        let source = source(&manifest.archives)?;
        let blocks::Manifest { source: source_id, data, roads, arcs, snap, .. } = manifest;
        let ids = Ids::new(roads, data.roads)?;
        Ok(Self::with(Package::new(source, data, source_id)?, identity, ids, arcs, Snap::Cells(snap)))
    }

    /// A handle with its own page caches; graph, junctions, closures and prepared metrics are shared.
    pub fn fork(&self) -> Self
    where
        S: Clone,
    {
        {
            let mut state = lock(&self.shared.state);
            state.handles += 1;
            state.budgets += self.memory_budget as u128;
        }
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

    /// Bulk extraction reads a metric's costs once and keeps the exact cost factors. It fills no
    /// cache and loads no landmark columns.
    pub fn prepared_endpoints(&self, metric: &str, roads: &[u32]) -> Result<Vec<Endpoint<CostBasis>>> {
        let _loading = lock(&self.shared.loading);
        let union = self.union()?;
        let costs = union.costs(&self.package, &self.ids, metric, None)?;
        roads
            .iter()
            .map(|&road| {
                let Some(basis) = self.package.prepared_cost(metric, self.ids.source(road)?)? else {
                    return Ok(Endpoint { cost: None, arrival: road, departures: Vec::new() });
                };
                if costs.roads.get(road as usize) == u64::MAX {
                    return Err(Error::InvalidData("Missing accessible road cost".into()));
                }
                Ok(Endpoint { cost: Some(basis), arrival: road, departures: departures(&union.graph, &costs, road) })
            })
            .collect()
    }

    /// Bytes every router needs for itself: its label blocks and the decode margin.
    pub fn router_bytes(&self) -> usize {
        crate::search::label_bytes(self.ids.count as usize).saturating_add(MARGIN)
    }

    /// Bytes the forks hold together with these metrics prepared: the graph, the junction
    /// mapping, and each metric's cost columns (turn columns once per identical table) and
    /// landmark columns.
    pub fn shared_bytes<'a>(&self, metrics: impl IntoIterator<Item = &'a str>) -> Result<usize> {
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
            .saturating_add(self.ids.ranges().saturating_mul(16));
        if self.shared.union.get().is_none() {
            // Linking scratch: the reverse cursor and the fast id lookup.
            bytes = bytes.saturating_add(roads.saturating_add(1).saturating_mul(4));
            bytes = bytes.saturating_add(self.ids.lookup_bytes());
        }
        let mut nodes = 0;
        if let Some(index) = &manifest.landmarks {
            bytes = bytes.saturating_add(roads.saturating_mul(4));
            nodes = match self.shared.junctions.get() {
                Some(junctions) => junctions.selected.count as usize,
                None => roads.min(index.junctions as usize),
            };
        }
        let mut turns: Vec<usize> = Vec::new();
        for name in metrics {
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

    /// The metrics the cache holds with `metric` beside them.
    fn held<'a>(&self, state: &'a State, metric: &'a str) -> BTreeSet<&'a str> {
        let retired = state.retired.iter().filter(|(_, prepared)| Arc::strong_count(prepared) > 1);
        state.profiles.iter().chain(retired).map(|(name, _)| name.as_str()).chain(std::iter::once(metric)).collect()
    }

    /// One router's view: its own blocks plus its share of what the forks hold together.
    fn share(&self, state: &State, metric: &str) -> Result<usize> {
        let shared = self.shared_bytes(self.held(state, metric))?;
        Ok(self.router_bytes().saturating_add(shared.div_ceil(state.handles.max(1))))
    }

    /// Drops the oldest metrics until the cache fits the routers that share it: at most
    /// `CACHED_PROFILES` per router, and the shared data plus every router's own blocks and
    /// queue headroom within the sum of their budgets.
    fn retain(&self, state: &mut State, metric: &str) -> Result<()> {
        state.retired.retain(|(_, prepared)| Arc::strong_count(prepared) > 1);
        loop {
            let slots = state.profiles.len() < CACHED_PROFILES.saturating_mul(state.handles.max(1));
            let routers = (state.handles as u128) * (self.router_bytes().saturating_add(QUEUE_HEADROOM) as u128);
            let fits = slots && self.shared_bytes(self.held(state, metric))? as u128 + routers <= state.budgets;
            if fits || state.profiles.is_empty() {
                return Ok(());
            }
            let old = state.profiles.pop_front().unwrap();
            if Arc::strong_count(&old.1) > 1 {
                state.retired.push(old);
            }
        }
    }

    fn cached(&self, metric: &str) -> Option<Arc<Prepared>> {
        let mut state = lock(&self.shared.state);
        let at = state.profiles.iter().position(|(name, _)| name == metric)?;
        let entry = state.profiles.remove(at).unwrap();
        let prepared = Arc::clone(&entry.1);
        state.profiles.push_back(entry);
        Some(prepared)
    }

    /// Callers hold `loading`, so one fork builds the union.
    fn union(&self) -> Result<Arc<Union>> {
        if let Some(union) = self.shared.union.get() {
            return Ok(Arc::clone(union));
        }
        let union = Arc::new(Union::open(&self.package, &self.ids, self.arcs)?);
        Ok(Arc::clone(self.shared.union.get_or_init(|| union)))
    }

    /// Callers hold `loading`, so one fork remaps the junctions.
    fn junctions(&self, index: &landmarks::Index) -> Result<Arc<Junctions>> {
        if let Some(junctions) = self.shared.junctions.get() {
            return Ok(Arc::clone(junctions));
        }
        let junctions = Arc::new(Junctions::read(&self.package, index, &self.ids)?);
        Ok(Arc::clone(self.shared.junctions.get_or_init(|| junctions)))
    }

    /// Callers hold `loading`. Reads through this fork's page caches only.
    fn load(&self, metric: &str) -> Result<Prepared> {
        let package = &self.package;
        let union = self.union()?;
        let turns = {
            let state = lock(&self.shared.state);
            state.profiles.iter().chain(&state.retired).find_map(|(name, prepared)| {
                (self.turn_groups[name] == self.turn_groups[metric]).then(|| Arc::clone(&prepared.costs.turns))
            })
        };
        let costs = union.costs(package, &self.ids, metric, turns)?;
        let columns =
            package.manifest().landmarks.as_ref().and_then(|index| Some((index, index.profiles.get(metric)?)));
        let guide = match columns {
            None => None,
            Some((index, tables)) => {
                let junctions = self.junctions(index)?;
                let columns = tables
                    .iter()
                    .map(|table| landmarks::read(package, table, junctions.selected.iter(), u16::MAX as u32))
                    .collect::<Result<_>>()?;
                Some(Guide { mapping: Arc::clone(&junctions.mapping), scale: index.scale, columns })
            }
        };
        Ok(Prepared { graph: Arc::clone(&union.graph), costs, guide })
    }
}

impl<S> Drop for Selection<S> {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        state.handles = state.handles.saturating_sub(1);
        state.budgets = state.budgets.saturating_sub(self.memory_budget as u128);
    }
}

fn departures(graph: &Graph, costs: &Costs, road: u32) -> Vec<Departure> {
    (graph.reverse_first[road as usize]..graph.reverse_first[road as usize + 1])
        .filter_map(|reverse| {
            let reverse = reverse as usize;
            let penalty = costs.turns.get(graph.reverse_arc(reverse));
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
        if let Some(closures) = self.shared.closures.get() {
            return Ok(Arc::clone(closures));
        }
        let source = self.package.closures()?;
        let mut roads: Vec<_> =
            source.roads.iter().filter_map(|&(road, entry)| Some((self.ids.local(road)?, entry))).collect();
        roads.sort_unstable();
        let closures = Arc::new(Closures { roads, entries: source.entries });
        Ok(Arc::clone(self.shared.closures.get_or_init(|| closures)))
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
        Ok(Endpoint { cost: Some(cost), arrival: road, departures: departures(&prepared.graph, &prepared.costs, road) })
    }

    fn prepared(&self, metric: &str) -> Result<Arc<Prepared>> {
        self.package.metric(metric)?;
        if let Some(prepared) = self.cached(metric) {
            return Ok(prepared);
        }
        {
            // Make room first, so a metric that cannot fit fails before any page is read.
            let mut state = lock(&self.shared.state);
            self.retain(&mut state, metric)?;
            if self.share(&state, metric)? > self.memory_budget {
                return Err(Error::Limit);
            }
        }
        let _loading = lock(&self.shared.loading);
        if let Some(prepared) = self.cached(metric) {
            return Ok(prepared);
        }
        let prepared = Arc::new(self.load(metric)?);
        let mut state = lock(&self.shared.state);
        self.retain(&mut state, metric)?;
        state.profiles.push_back((metric.into(), Arc::clone(&prepared)));
        Ok(prepared)
    }

    fn routing_bytes(&self, metric: &str) -> Result<usize> {
        self.package.metric(metric)?;
        let state = lock(&self.shared.state);
        self.share(&state, metric)
    }

    fn memory_budget(&self) -> usize {
        self.memory_budget
    }
    fn set_memory_budget(&mut self, bytes: usize) {
        let mut state = lock(&self.shared.state);
        state.budgets = state.budgets.saturating_sub(self.memory_budget as u128) + bytes as u128;
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
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Barrier,
    };

    /// A read of this key waits at the barrier twice: once to report, once to be released.
    type Gate = Option<(String, Arc<Barrier>)>;

    #[derive(Clone, Default)]
    struct Memory {
        objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        reads: Arc<AtomicUsize>,
        gate: Arc<Mutex<Gate>>,
    }
    impl Source for Memory {
        fn read(&self, key: &str) -> Result<Vec<u8>> {
            let gate = lock(&self.gate).clone();
            if let Some((gated, barrier)) = gate {
                if gated == key {
                    barrier.wait();
                    barrier.wait();
                }
            }
            self.reads.fetch_add(1, Ordering::Relaxed);
            lock(&self.objects).get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
        }
    }
    impl Memory {
        fn write(&self, bytes: &[u8]) -> std::result::Result<String, String> {
            let key = digest(bytes);
            lock(&self.objects).insert(key.clone(), bytes.to_vec());
            Ok(key)
        }
        fn reads(&self) -> usize {
            self.reads.load(Ordering::Relaxed)
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
        /// Adds metrics with the given road costs and turn penalties to a one-metric manifest.
        fn metrics(&self, manifest: &mut Manifest, extra: &[(&str, &[u64], &[u64])]) {
            for &(name, roads, turns) in extra {
                let mut metric = manifest.metrics["touring"].clone();
                metric.profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
                metric.weights = write_weights(roads, turns, |b| self.write(b)).unwrap();
                manifest.metrics.insert(name.into(), metric);
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
        memory.reads.store(0, Ordering::Relaxed);
        let again = fork.prepared("touring").unwrap();
        assert!(Arc::ptr_eq(&prepared, &again));
        assert!(Arc::ptr_eq(&selection.closures().unwrap(), &fork.closures().unwrap()));
        assert_eq!(memory.reads(), 0);
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
        memory.metrics(&mut manifest, &[("gravel", &[4, 5], &[0]), ("hiking", &[6, 7], &[2]), ("mtb", &[8, 9], &[0])]);
        let selection = memory.selection(&manifest);
        let touring = selection.prepared("touring").unwrap();
        memory.reads.store(0, Ordering::Relaxed);
        let gravel = selection.prepared("gravel").unwrap();
        assert_eq!(memory.reads(), 1);
        assert!(Arc::ptr_eq(&touring.costs.turns, &gravel.costs.turns));
        let hiking = selection.prepared("hiking").unwrap();
        assert!(!Arc::ptr_eq(&touring.costs.turns, &hiking.costs.turns));
        let graph = &touring.graph;
        assert_eq!([touring.costs.arc(graph, 0), gravel.costs.arc(graph, 0), hiking.costs.arc(graph, 0)], [2, 5, 9]);
        selection.prepared("mtb").unwrap();
        memory.reads.store(0, Ordering::Relaxed);
        assert!(Arc::ptr_eq(&selection.prepared("gravel").unwrap(), &gravel));
        assert_eq!(memory.reads(), 0);
        assert!(!Arc::ptr_eq(&selection.prepared("touring").unwrap(), &touring));
    }

    #[test]
    fn the_cache_is_sized_for_every_fork_and_counts_evicted_metrics_still_in_use() {
        let memory = Memory::default();
        let topology = write_topology(2, &[(0, 1)], |b| memory.write(b)).unwrap();
        let weights = write_weights(&[1, 2], &[0], |b| memory.write(b)).unwrap();
        let mut manifest = memory.manifest(2, topology, weights);
        memory.metrics(&mut manifest, &[("gravel", &[4, 5], &[0]), ("hiking", &[6, 7], &[0]), ("mtb", &[8, 9], &[0])]);
        let mut selection = memory.selection(&manifest);
        // The first load builds the union; the estimate drops its linking scratch after that.
        let touring = selection.prepared("touring").unwrap();
        let router = selection.router_bytes();
        let one = selection.shared_bytes(["touring"]).unwrap();
        let two = selection.shared_bytes(["touring", "gravel"]).unwrap();
        assert!(one < two && two < 2 * one);
        // Room for one metric beside this router's own blocks and queue headroom.
        selection.set_memory_budget(router + QUEUE_HEADROOM + one);
        let gravel = selection.prepared("gravel").unwrap();
        // The evicted metric counts while a router still holds it.
        assert_eq!(selection.routing_bytes("gravel").unwrap(), router + two);
        assert!(!Arc::ptr_eq(&selection.prepared("touring").unwrap(), &touring));
        drop((touring, gravel));
        assert_eq!(selection.routing_bytes("touring").unwrap(), router + one);
        // A second router with the same budget doubles the room, and both see one cache.
        let fork = selection.fork();
        let hiking = fork.prepared("hiking").unwrap();
        let gravel = fork.prepared("gravel").unwrap();
        assert!(Arc::ptr_eq(&selection.prepared("hiking").unwrap(), &hiking));
        assert!(Arc::ptr_eq(&selection.prepared("gravel").unwrap(), &gravel));
        let three = selection.shared_bytes(["touring", "hiking", "gravel"]).unwrap();
        assert_eq!(selection.routing_bytes("gravel").unwrap(), router + three.div_ceil(2));
        drop((fork, hiking, gravel));
        // Alone again, the next load shrinks the cache back to one metric.
        selection.prepared("mtb").unwrap();
        assert_eq!(selection.routing_bytes("mtb").unwrap(), router + selection.shared_bytes(["mtb"]).unwrap());
    }

    #[test]
    fn a_cold_load_in_one_fork_does_not_block_the_others() {
        let memory = Memory::default();
        let topology = write_topology(2, &[(0, 1)], |b| memory.write(b)).unwrap();
        let weights = write_weights(&[1, 2], &[0], |b| memory.write(b)).unwrap();
        let mut manifest = memory.manifest(2, topology, weights);
        memory.metrics(&mut manifest, &[("gravel", &[4, 5], &[0])]);
        let selection = memory.selection(&manifest);
        let touring = selection.prepared("touring").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let gated = manifest.metrics["gravel"].weights.road_costs.values.blocks[0].clone();
        *lock(&memory.gate) = Some((gated, Arc::clone(&barrier)));
        let loader = selection.fork();
        let loader = std::thread::spawn(move || loader.prepared("gravel"));
        // The loader now sits in its first page read with the loading lock held.
        barrier.wait();
        let (done, hot) = mpsc::channel();
        let probe = selection.fork();
        let probe = std::thread::spawn(move || {
            let bytes = probe.routing_bytes("touring").is_ok();
            let same = probe.prepared("touring").map(|p| Arc::ptr_eq(&p, &touring)).unwrap_or(false);
            let closures = probe.closures().is_ok();
            done.send((bytes, same, closures)).unwrap();
        });
        let hot = hot.recv_timeout(std::time::Duration::from_secs(10)).expect("hot reads waited for a cold load");
        assert_eq!(hot, (true, true, true));
        barrier.wait();
        let gravel = loader.join().unwrap().unwrap();
        probe.join().unwrap();
        assert!(Arc::ptr_eq(&selection.prepared("gravel").unwrap(), &gravel));
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
        assert_eq!(memory.reads(), 0);
        selection.set_memory_budget(usize::MAX);
        assert!(matches!(selection.prepared("touring"), Err(Error::InvalidData(_))));
        let mut invalid = topology;
        invalid.head.blocks[0] = memory.write(&crate::storage::encode(&Vec::<u32>::new()).unwrap()).unwrap();
        let mut broken = manifest;
        broken.graph = invalid;
        assert!(matches!(memory.selection(&broken).prepared("touring"), Err(Error::InvalidData(_))));
    }
}
