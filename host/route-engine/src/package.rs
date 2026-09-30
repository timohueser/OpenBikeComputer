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

pub const FORMAT: u32 = 6;
pub const MAX_MANIFEST_BYTES: usize = 128 * 1024 * 1024;
pub const ROADS_PER_PAGE: u32 = 128;
pub const CELL: i32 = 10_000;

/// Hosts read local files, browser storage or application assets through this seam.
/// A source is immutable for the lifetime of a package. A missing object is never an empty page.
pub trait Source {
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
    bases: RefCell<table::Cache<CostBasis>>,
    spatial: RefCell<VecDeque<(String, SpatialDirectory)>>,
    // Geometry has a separate byte budget from the query summaries.
    geometry: HashMap<u32, (Arc<Vec<Road>>, usize)>,
    geometry_order: VecDeque<u32>,
    geometry_bytes: usize,
    graph: Arc<Mutex<Option<Arc<base::Graph>>>>,
    active_costs: Option<(String, Arc<base::Costs>)>,
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
            bases: RefCell::new(table::Cache::default()),
            spatial: RefCell::new(VecDeque::new()),
            geometry: HashMap::new(),
            geometry_order: VecDeque::new(),
            geometry_bytes: 0,
            graph: Arc::new(Mutex::new(None)),
            active_costs: None,
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
            bases: RefCell::new(table::Cache::default()),
            spatial: RefCell::new(VecDeque::new()),
            geometry: HashMap::new(),
            geometry_order: VecDeque::new(),
            geometry_bytes: 0,
            graph: Arc::clone(&self.graph),
            active_costs: None,
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
        keys.extend(self.manifest.costs.blocks.iter().cloned());
        for block in 0..self.manifest.costs.blocks.len() {
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
                for block in 0..table.blocks.len() {
                    self.numbers.borrow_mut().block(self, table, block)?;
                }
            }
            keys.extend(metric.allowed.blocks.iter().cloned());
            for block in 0..metric.allowed.blocks.len() {
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
        for block in 0..table.blocks.len() {
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

    pub(crate) fn base(&mut self, metric: &str) -> Result<(Arc<base::Graph>, Arc<base::Costs>)> {
        let required = self.routing_bytes(metric)?;
        if required > self.memory_budget {
            return Err(Error::Limit);
        }
        let graph = {
            let mut shared = self.graph.lock().map_err(|_| Error::Limit)?;
            if shared.is_none() {
                *shared = Some(Arc::new(base::Graph::read(self, &self.manifest.graph)?));
            }
            Arc::clone(shared.as_ref().unwrap())
        };
        if self.active_costs.as_ref().is_none_or(|(name, _)| name != metric) {
            self.active_costs = None;
            let weights = base::Costs::read(self, &self.metric(metric)?.weights, &graph)?;
            self.active_costs = Some((metric.into(), Arc::new(weights)));
        }
        Ok((graph, Arc::clone(&self.active_costs.as_ref().unwrap().1)))
    }

    pub(crate) fn routing_bytes(&self, metric: &str) -> Result<usize> {
        // Reserve a margin for geometry, index caches, compressed pages and decode scratch.
        Ok(self
            .manifest
            .graph
            .decoded_bytes()
            .saturating_add(self.metric(metric)?.weights.decoded_bytes())
            .saturating_add((self.manifest.roads as usize).saturating_mul(26))
            .saturating_add(64 * 1024 * 1024))
    }

    pub fn prepared_endpoint(&mut self, metric: &str, id: u32) -> Result<Endpoint<CostBasis>> {
        if id >= self.manifest.roads {
            return Err(Error::InvalidData("Endpoint outside package".into()));
        }
        let prepared = self.metric(metric)?;
        let cost_id = self.number(&prepared.costs, id)?;
        if self.allowed(metric, id)? != (cost_id != 0) {
            return Err(Error::InvalidData("Cost and snap eligibility differ".into()));
        }
        if cost_id == 0 {
            return Ok(Endpoint { cost: None, arrival: id, departures: Vec::new() });
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
        let (graph, costs) = self.base(metric)?;
        if costs.roads.get(id as usize) == u64::MAX {
            return Err(Error::InvalidData("Missing accessible road cost".into()));
        }
        let mut departures = Vec::new();
        for reverse in graph.reverse_first[id as usize]..graph.reverse_first[id as usize + 1] {
            let reverse = reverse as usize;
            let penalty = costs.turns.get(graph.reverse_arc(reverse));
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
            if self.base(metric)?.1.roads.get(id as usize) != cost.total() {
                return Err(Error::InvalidData("Compiled road total differs from base metric".into()));
            }
        }
        Ok(Endpoint { cost, arrival: endpoint.arrival, departures: endpoint.departures })
    }
}
