use crate::{
    base,
    cost::{CostBasis, RoadCost},
    model::{Point, Profile, Road},
    storage,
    table::{self, Table},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub const FORMAT: u32 = 7;
pub const MAX_MANIFEST_BYTES: usize = 128 * 1024 * 1024;
pub const ROADS_PER_PAGE: u32 = 128;
pub const CELL: i32 = 10_000;

/// Hosts read local files, browser storage or application assets through this seam.
/// A source is immutable for the lifetime of a package. A missing object is never an empty page.
pub trait Source {
    fn resident_bytes(&self) -> usize {
        0
    }

    fn read(&self, digest: &str) -> Result<Vec<u8>>;

    /// Packed sources can verify pages in physical order to avoid random disk reads.
    fn order_for_verify(&self, _digests: &mut [String]) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric {
    pub profile: Profile,
    pub weights: base::Weights,
    pub costs: Table,
    /// One eligibility bit per directed road; snapping does not load cost pages.
    pub allowed: Table,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub region: String,
    /// Endpoint coverage; complete intersecting road geometry can extend outside these bounds.
    pub bounds: [f64; 4],
    pub source_sha256: Vec<String>,
    pub attribution: String,
    pub warnings: Vec<String>,
    pub roads: u32,
    pub graph: base::Topology,
    pub geometry: Table,
    pub osm: OsmPages,
    /// One-degree directories contain the fine snap cells.
    pub spatial: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, Metric>,
    pub costs: Table,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landmarks: Option<crate::landmarks::Index>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Endpoint<C = RoadCost> {
    pub cost: Option<C>,
    pub arrival: u32,
    /// States from which this road can legally be entered.
    pub departures: Vec<Departure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Departure {
    pub state: u32,
    pub penalty: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OsmPages {
    pub nodes: Table,
    pub ways: Table,
    pub relations: Table,
}

impl OsmPages {
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        [&self.nodes, &self.ways, &self.relations].into_iter()
    }
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn cell(point: Point) -> (i32, i32) {
    (point.lat.div_euclid(CELL), point.lon.div_euclid(CELL))
}

pub fn cell_key((lat, lon): (i32, i32)) -> String {
    format!("{lat},{lon}")
}

type SpatialDirectory = Arc<BTreeMap<String, String>>;

pub struct Package<S> {
    pub(crate) manifest: Manifest,
    pub(crate) identity: String,
    source: S,
    keys: RefCell<table::Cache<String>>,
    words: RefCell<table::Cache<u64>>,
    numbers: RefCell<table::Cache<u32>>,
    shorts: RefCell<table::Cache<u16>>,
    octets: RefCell<table::Cache<u8>>,
    bases: RefCell<table::Cache<CostBasis>>,
    spatial: RefCell<VecDeque<(String, SpatialDirectory)>>,
    // Geometry has a separate byte budget from the query summaries.
    geometry: HashMap<u32, (Arc<Vec<Road>>, usize)>,
    geometry_order: VecDeque<u32>,
    geometry_bytes: usize,
    graph: Arc<Mutex<Option<Arc<base::Graph>>>>,
    cached_costs: VecDeque<(String, Arc<base::Costs>)>,
    junctions: Arc<Mutex<Option<Arc<Vec<u32>>>>>,
    landmark_columns: RefCell<VecDeque<(Table, Arc<Vec<u16>>)>>,
    landmark_blocks: RefCell<table::Cache<i64>>,
    pub(crate) memory_budget: usize,
}

impl<S: Source> Package<S> {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn open(source: S, manifest: &[u8]) -> Result<Self> {
        if manifest.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Limit);
        }
        let identity = digest(manifest);
        let manifest: Manifest = serde_json::from_slice(manifest).map_err(|e| Error::InvalidData(e.to_string()))?;
        let b = manifest.bounds;
        if manifest.format != FORMAT
            || manifest.roads == 0
            || manifest.metrics.is_empty()
            || b.iter().any(|n| !n.is_finite())
            || b[0] >= b[2]
            || b[1] >= b[3]
            || b[0] < -180.0
            || b[2] > 180.0
            || b[1] < -85.0
            || b[3] > 85.0
            || manifest.geometry.len != manifest.roads.div_ceil(ROADS_PER_PAGE)
        {
            return Err(Error::InvalidData("Unsupported or incomplete regional manifest".into()));
        }
        for (id, metric) in &manifest.metrics {
            metric.profile.validate().map_err(Error::InvalidData)?;
            if id != &metric.profile.name
                || !metric.weights.valid(&manifest.graph, manifest.roads)
                || metric.costs.len != manifest.roads
                || metric.allowed.len != manifest.roads.div_ceil(64)
            {
                return Err(Error::InvalidData("Incomplete prepared metric".into()));
            }
        }
        if !manifest.graph.valid(manifest.roads)
            || !manifest.costs.valid()
            || !manifest.geometry.valid()
            || manifest.osm.tables().any(|t| !t.valid())
            || manifest.metrics.values().any(|m| !m.allowed.valid() || !m.costs.valid())
            || manifest.spatial.values().any(|key| !table::valid_digest(key))
            || manifest.landmarks.as_ref().is_some_and(|index| {
                !index.valid(manifest.roads) || index.profiles.keys().any(|name| !manifest.metrics.contains_key(name))
            })
        {
            return Err(Error::InvalidData("Invalid index directory".into()));
        }
        Ok(Self {
            manifest,
            identity,
            source,
            keys: RefCell::new(table::Cache::default()),
            words: RefCell::new(table::Cache::default()),
            numbers: RefCell::new(table::Cache::default()),
            shorts: RefCell::new(table::Cache::default()),
            octets: RefCell::new(table::Cache::default()),
            bases: RefCell::new(table::Cache::default()),
            spatial: RefCell::new(VecDeque::new()),
            geometry: HashMap::new(),
            geometry_order: VecDeque::new(),
            geometry_bytes: 0,
            graph: Arc::new(Mutex::new(None)),
            cached_costs: VecDeque::new(),
            junctions: Arc::new(Mutex::new(None)),
            landmark_columns: RefCell::new(VecDeque::new()),
            landmark_blocks: RefCell::new(table::Cache::default()),
            memory_budget: usize::MAX,
        })
    }

    /// Create an independent query cache that shares the immutable topology and source.
    pub fn fork(&self) -> Self
    where
        S: Clone,
    {
        Self {
            manifest: self.manifest.clone(),
            identity: self.identity.clone(),
            source: self.source.clone(),
            keys: RefCell::new(table::Cache::default()),
            words: RefCell::new(table::Cache::default()),
            numbers: RefCell::new(table::Cache::default()),
            shorts: RefCell::new(table::Cache::default()),
            octets: RefCell::new(table::Cache::default()),
            bases: RefCell::new(table::Cache::default()),
            spatial: RefCell::new(VecDeque::new()),
            geometry: HashMap::new(),
            geometry_order: VecDeque::new(),
            geometry_bytes: 0,
            graph: Arc::clone(&self.graph),
            cached_costs: VecDeque::new(),
            junctions: Arc::clone(&self.junctions),
            landmark_columns: RefCell::new(VecDeque::new()),
            landmark_blocks: RefCell::new(table::Cache::default()),
            memory_budget: self.memory_budget,
        }
    }

    /// Check the complete object closure before publishing or installing a region.
    pub fn verify(&self) -> Result<()> {
        for key in self.objects()? {
            self.bytes(&key)?;
        }
        Ok(())
    }

    /// All referenced objects, including index blocks, in the source's verification order.
    pub fn objects(&self) -> Result<Vec<String>> {
        let mut keys = std::collections::BTreeSet::new();
        if let Some(index) = &self.manifest.landmarks {
            for table in index.tables() {
                keys.extend(table.blocks.iter().cloned());
            }
        }
        keys.extend(self.manifest.costs.blocks.iter().cloned());
        for block in self.manifest.costs.positions() {
            let costs = self.bases.borrow_mut().block(self, &self.manifest.costs, block)?;
            if costs.iter().any(|cost| !cost.valid()) {
                return Err(Error::InvalidData("Invalid cost dictionary".into()));
            }
        }
        for table in std::iter::once(&self.manifest.geometry).chain(self.manifest.osm.tables()) {
            keys.extend(table.blocks.iter().cloned());
            keys.extend(self.keys(table)?);
        }
        for table in self.manifest.graph.tables() {
            keys.extend(table.blocks.iter().cloned());
        }
        for metric in self.manifest.metrics.values() {
            for table in metric.weights.tables() {
                keys.extend(table.blocks.iter().cloned());
            }
            for table in [&metric.costs] {
                keys.extend(table.blocks.iter().cloned());
                for block in table.positions() {
                    self.numbers.borrow_mut().block(self, table, block)?;
                }
            }
            keys.extend(metric.allowed.blocks.iter().cloned());
            for block in metric.allowed.positions() {
                self.words.borrow_mut().block(self, &metric.allowed, block)?;
            }
        }
        for key in self.manifest.spatial.values() {
            keys.insert(key.clone());
            let cells: BTreeMap<String, String> = self.read(key)?;
            if cells.len() > 10_000 || cells.values().any(|id| !table::valid_digest(id)) {
                return Err(Error::InvalidData("Invalid snap directory".into()));
            }
            keys.extend(cells.into_values());
        }
        let mut keys: Vec<_> = keys.into_iter().collect();
        self.source.order_for_verify(&mut keys)?;
        Ok(keys)
    }

    pub fn keys(&self, table: &Table) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for block in table.positions() {
            let values = self.keys.borrow_mut().block(self, table, block)?;
            if values.iter().any(|key| !table::valid_digest(key)) {
                return Err(Error::InvalidData("Invalid page identity".into()));
            }
            keys.extend(values.iter().cloned());
        }
        Ok(keys)
    }

    pub fn key(&self, table: &Table, index: u32) -> Result<String> {
        if index >= table.len {
            return Err(Error::InvalidData("Page outside index".into()));
        }
        let values = self.keys.borrow_mut().block(self, table, index as usize / table::ENTRIES)?;
        let key = &values[index as usize % table::ENTRIES];
        if !table::valid_digest(key) {
            return Err(Error::InvalidData("Invalid page identity".into()));
        }
        Ok(key.clone())
    }

    pub fn allowed(&self, metric: &str, road: u32) -> Result<bool> {
        if road >= self.manifest.roads {
            return Err(Error::InvalidData("Road outside access index".into()));
        }
        let table = &self.metric(metric)?.allowed;
        let word = road as usize / 64;
        let values = self.words.borrow_mut().block(self, table, word / table::ENTRIES)?;
        Ok(values[word % table::ENTRIES] & (1 << (road % 64)) != 0)
    }

    pub fn spatial_roads(&self, cell: (i32, i32)) -> Result<Vec<u32>> {
        let group = cell_key((cell.0.div_euclid(100), cell.1.div_euclid(100)));
        let Some(key) = self.manifest.spatial.get(&group) else { return Ok(Vec::new()) };
        let mut cache = self.spatial.borrow_mut();
        let cells = if let Some(at) = cache.iter().position(|(id, _)| id == key) {
            cache.remove(at).unwrap().1
        } else {
            let cells: BTreeMap<String, String> = self.read(key)?;
            if cells.len() > 10_000 || cells.values().any(|id| !table::valid_digest(id)) {
                return Err(Error::InvalidData("Invalid snap directory".into()));
            }
            Arc::new(cells)
        };
        let roads = match cells.get(&cell_key(cell)) {
            Some(key) => self.read(key)?,
            None => Vec::new(),
        };
        cache.push_back((key.clone(), cells));
        while cache.len() > 2 {
            cache.pop_front();
        }
        Ok(roads)
    }

    pub fn bytes(&self, key: &str) -> Result<Vec<u8>> {
        let bytes = self.source.read(key)?;
        if bytes.len() > storage::MAX_PAGE_BYTES || digest(&bytes) != key {
            return Err(Error::InvalidData("Object checksum or size mismatch".into()));
        }
        Ok(bytes)
    }

    pub fn read<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<T> {
        storage::decode(&self.bytes(key)?).map_err(Error::InvalidData)
    }

    pub fn metric(&self, name: &str) -> Result<&Metric> {
        self.manifest.metrics.get(name).ok_or_else(|| Error::InvalidRequest(format!("Profile {name} is not installed")))
    }

    pub fn road(&mut self, id: u32) -> Result<Road> {
        if id >= self.manifest.roads {
            return Err(Error::InvalidData("Road outside package".into()));
        }
        let page = id / ROADS_PER_PAGE;
        let value = if let Some((value, _)) = self.geometry.get(&page) {
            Arc::clone(value)
        } else {
            let value = crate::geometry::decode(&self.bytes(&self.key(&self.manifest.geometry, page)?)?)
                .map_err(Error::InvalidData)?;
            if value.len() != (self.manifest.roads - page * ROADS_PER_PAGE).min(ROADS_PER_PAGE) as usize
                || value.iter().any(|r| {
                    r.class > 6
                        || r.shape.len() < 2
                        || r.shape.iter().any(|p| {
                            p.lat.unsigned_abs() > 85_000_000
                                || p.lon.unsigned_abs() > 180_000_000
                                || !p.elevation.is_finite()
                        })
                })
            {
                return Err(Error::InvalidData("Invalid road page".into()));
            }
            let size = value.capacity() * std::mem::size_of::<Road>()
                + value.iter().map(|r| r.shape.capacity() * std::mem::size_of::<Point>()).sum::<usize>();
            let value = Arc::new(value);
            if size <= 32 * 1024 * 1024 {
                while self.geometry_bytes + size > 32 * 1024 * 1024 {
                    let key = self.geometry_order.pop_front().unwrap();
                    self.geometry_bytes -= self.geometry.remove(&key).unwrap().1;
                }
                self.geometry.insert(page, (Arc::clone(&value), size));
                self.geometry_order.push_back(page);
                self.geometry_bytes += size;
            }
            value
        };
        let road = value
            .get((id % ROADS_PER_PAGE) as usize)
            .cloned()
            .ok_or_else(|| Error::InvalidData("Missing road".into()))?;
        Ok(road)
    }

    fn number(&self, table: &Table, index: u32) -> Result<u32> {
        if index >= table.len {
            return Err(Error::InvalidData("Number outside table".into()));
        }
        let values = self.numbers.borrow_mut().block(self, table, index as usize / table::ENTRIES)?;
        Ok(values[index as usize % table::ENTRIES])
    }

    fn graph(&self, metric: &str) -> Result<Arc<base::Graph>> {
        let required = self.fixed_routing_bytes().saturating_add(self.metric(metric)?.weights.decoded_bytes());
        if required > self.memory_budget {
            return Err(Error::Limit);
        }
        {
            let mut shared = self.graph.lock().map_err(|_| Error::Limit)?;
            if shared.is_none() {
                *shared = Some(Arc::new(base::Graph::read(self, &self.manifest.graph)?));
            }
            Ok(Arc::clone(shared.as_ref().unwrap()))
        }
    }

    pub fn base(&mut self, metric: &str) -> Result<(Arc<base::Graph>, Arc<base::Costs>)> {
        let graph = self.graph(metric)?;
        let cached = self
            .cached_costs
            .iter()
            .position(|(name, _)| name == metric)
            .map(|index| self.cached_costs.remove(index).unwrap());
        // Leave space for search queues before retaining other profiles.
        while !self.cached_costs.is_empty()
            && (self.cached_costs.len() >= 3
                || self.routing_bytes(metric)?.saturating_add(16 * 1024 * 1024) > self.memory_budget)
        {
            self.cached_costs.pop_front();
        }
        let cached = if let Some(cached) = cached {
            cached
        } else {
            let weights = &self.metric(metric)?.weights;
            let turns = self.cached_costs.iter().find_map(|(name, costs)| {
                (self.manifest.metrics[name].weights.turns == weights.turns).then(|| Arc::clone(&costs.turns))
            });
            let costs = base::Costs::read_shared(self, weights, &graph, turns)?;
            (metric.into(), Arc::new(costs))
        };
        let costs = Arc::clone(&cached.1);
        self.cached_costs.push_back(cached);
        Ok((graph, costs))
    }

    pub(crate) fn landmarks(
        &self,
        metric: &str,
        starts: &[crate::search::Seed],
        ends: &[crate::search::Seed],
    ) -> Result<Option<crate::landmarks::Prepared>> {
        let Some(index) = &self.manifest.landmarks else {
            return Ok(None);
        };
        let Some(tables) = index.profiles.get(metric) else {
            return Ok(None);
        };
        if starts.is_empty() || ends.is_empty() {
            return Ok(None);
        }
        let mapping = {
            let mut shared = self.junctions.lock().map_err(|_| Error::Limit)?;
            if shared.is_none() {
                *shared = Some(Arc::new(crate::landmarks::read(self, &index.mapping, index.junctions - 1)?));
            }
            Arc::clone(shared.as_ref().unwrap())
        };
        let selected = crate::landmarks::select(tables.len(), index.scale, starts, ends, |i, road| {
            let node =
                *mapping.get(road as usize).ok_or_else(|| Error::InvalidData("Landmark seed outside mapping".into()))?
                    as usize;
            if let Some((_, column)) = self.landmark_columns.borrow().iter().find(|(t, _)| t == &tables[i]) {
                return Ok(column[node]);
            }
            let deltas = self.landmark_blocks.borrow_mut().block(self, &tables[i], node / table::ENTRIES)?;
            let value = deltas[..=node % table::ENTRIES]
                .iter()
                .try_fold(0i64, |sum, &delta| sum.checked_add(delta))
                .and_then(|v| u16::try_from(v).ok())
                .ok_or_else(|| Error::InvalidData("Invalid landmark seed distance".into()))?;
            Ok(value)
        })?;
        let mut columns = Vec::new();
        for (i, from, to) in selected {
            let mut cache = self.landmark_columns.borrow_mut();
            let values = if let Some(at) = cache.iter().position(|(t, _)| t == &tables[i]) {
                cache.remove(at).unwrap().1
            } else {
                while cache.len() >= crate::landmarks::CACHED_COLUMNS {
                    cache.pop_front();
                }
                Arc::new(crate::landmarks::read(self, &tables[i], u16::MAX as u32)?)
            };
            cache.push_back((tables[i].clone(), Arc::clone(&values)));
            columns.push((values, from, to));
        }
        Ok(Some(crate::landmarks::Prepared { mapping, columns, scale: index.scale }))
    }

    fn weight(&self, column: &base::Column, index: usize) -> Result<u64> {
        if index >= column.values.len as usize {
            return Err(Error::InvalidData("Weight outside column".into()));
        }
        let (block, offset) = (index / table::ENTRIES, index % table::ENTRIES);
        macro_rules! read {
            ($cache:ident, $ty:ty) => {{
                let value = self.$cache.borrow_mut().block(self, &column.values, block)?[offset];
                if value == <$ty>::MAX {
                    u64::MAX
                } else {
                    value as u64
                }
            }};
        }
        Ok(match column.width {
            base::Width::U8 => read!(octets, u8),
            base::Width::U16 => read!(shorts, u16),
            base::Width::U32 => read!(numbers, u32),
            base::Width::U64 => read!(words, u64),
        })
    }

    fn road_weight(&self, metric: &str, id: u32) -> Result<u64> {
        match self.cached_costs.iter().find(|(name, _)| name == metric) {
            Some((_, costs)) => Ok(costs.roads.get(id as usize)),
            _ => self.weight(&self.metric(metric)?.weights.road_costs, id as usize),
        }
    }

    fn fixed_routing_bytes(&self) -> usize {
        // Reserve a margin for geometry, index caches, compressed pages and decode scratch.
        self.manifest
            .graph
            .decoded_bytes()
            .saturating_add(crate::search::label_bytes(self.manifest.roads as usize))
            .saturating_add(64 * 1024 * 1024)
            .saturating_add(self.manifest.landmarks.as_ref().map_or(0, |index| index.decoded_bytes()))
    }

    pub(crate) fn routing_bytes(&self, metric: &str) -> Result<usize> {
        let mut bytes = self.fixed_routing_bytes();
        let mut turns = Vec::new();
        for name in std::iter::once(metric)
            .chain(self.cached_costs.iter().map(|(name, _)| name.as_str()).filter(|&name| name != metric))
        {
            let weights = &self.metric(name)?.weights;
            bytes = bytes.saturating_add(weights.road_costs.decoded_bytes());
            if !turns.contains(&&weights.turns) {
                bytes = bytes.saturating_add(weights.turns.decoded_bytes());
                turns.push(&weights.turns);
            }
        }
        Ok(bytes)
    }

    /// Bulk extraction reuses decoded weights instead of thrashing the per-request page caches.
    pub fn prepared_endpoints(&mut self, metric: &str, roads: &[u32]) -> Result<Vec<Endpoint<CostBasis>>> {
        self.base(metric)?;
        roads.iter().map(|&id| self.prepared_endpoint(metric, id)).collect()
    }

    pub fn prepared_cost(&self, metric: &str, id: u32) -> Result<Option<CostBasis>> {
        if id >= self.manifest.roads {
            return Err(Error::InvalidData("Endpoint outside package".into()));
        }
        let prepared = self.metric(metric)?;
        let cost_id = self.number(&prepared.costs, id)?;
        if self.allowed(metric, id)? != (cost_id != 0) {
            return Err(Error::InvalidData("Cost and snap eligibility differ".into()));
        }
        if cost_id == 0 {
            return Ok(None);
        }
        if cost_id > self.manifest.costs.len {
            return Err(Error::InvalidData("Unknown cost basis".into()));
        }
        let index = cost_id as usize - 1;
        let basis =
            self.bases.borrow_mut().block(self, &self.manifest.costs, index / table::ENTRIES)?[index % table::ENTRIES];
        if !basis.valid() {
            return Err(Error::InvalidData("Invalid cost basis".into()));
        }
        Ok(Some(basis))
    }

    pub fn prepared_endpoint(&mut self, metric: &str, id: u32) -> Result<Endpoint<CostBasis>> {
        let Some(basis) = self.prepared_cost(metric, id)? else {
            return Ok(Endpoint { cost: None, arrival: id, departures: Vec::new() });
        };
        let prepared = self.metric(metric)?;
        let graph = self.graph(metric)?;
        if self.road_weight(metric, id)? == u64::MAX {
            return Err(Error::InvalidData("Missing accessible road cost".into()));
        }
        let mut departures = Vec::new();
        for reverse in graph.reverse_first[id as usize]..graph.reverse_first[id as usize + 1] {
            let reverse = reverse as usize;
            let arc = graph.reverse_arc(reverse);
            let penalty = match self.cached_costs.iter().find(|(name, _)| name == metric) {
                Some((_, costs)) => costs.turns.get(arc),
                _ => self.weight(&prepared.weights.turns, arc)?,
            };
            if penalty != u64::MAX {
                departures.push(Departure { state: graph.reverse_tail[reverse], penalty });
            }
        }
        Ok(Endpoint { cost: Some(basis), arrival: id, departures })
    }

    pub fn endpoint(&mut self, metric: &str, id: u32) -> Result<Endpoint> {
        let endpoint = self.prepared_endpoint(metric, id)?;
        let cost = endpoint
            .cost
            .map(|cost| cost.compile(&self.road(id)?, &self.metric(metric)?.profile).map_err(Error::InvalidData))
            .transpose()?;
        if let Some(cost) = &cost {
            if self.road_weight(metric, id)? != cost.total() {
                return Err(Error::InvalidData("Compiled road total differs from base metric".into()));
            }
        }
        Ok(Endpoint { cost, arrival: endpoint.arrival, departures: endpoint.departures })
    }
}
