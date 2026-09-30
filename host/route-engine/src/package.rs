use crate::{
    cost::RoadCost,
    model::{Point, Profile, Road},
    storage,
    table::{self, Table},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

pub const FORMAT: u32 = 3;
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
    pub graph: Table,
    pub endpoints: Table,
    pub states: u32,
    /// One eligibility bit per directed road; snapping does not load cost pages.
    pub allowed: Table,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub region: String,
    /// The graph is clipped to these bounds; optimality is within this graph only.
    pub bounds: [f64; 4],
    pub source_sha256: Vec<String>,
    pub attribution: String,
    pub warnings: Vec<String>,
    pub roads: u32,
    pub geometry: Table,
    pub osm: OsmPages,
    /// One-degree directories contain the fine snap cells.
    pub spatial: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, Metric>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Endpoint {
    pub cost: Option<RoadCost>,
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
    spatial: RefCell<VecDeque<(String, SpatialDirectory)>>,
    // Geometry has a separate byte budget from the query summaries.
    geometry: HashMap<u32, (Arc<Vec<Road>>, usize)>,
    geometry_order: VecDeque<u32>,
    geometry_bytes: usize,
    endpoints: VecDeque<(String, u32, Arc<Vec<Endpoint>>, usize)>,
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
                || metric.states == 0
                || metric.graph.len != metric.states.div_ceil(storage::NODES_PER_PAGE)
                || metric.endpoints.len != manifest.geometry.len
                || metric.allowed.len != manifest.roads.div_ceil(64)
            {
                return Err(Error::InvalidData("Incomplete prepared metric".into()));
            }
        }
        if !manifest.geometry.valid()
            || manifest.osm.tables().any(|t| !t.valid())
            || manifest.metrics.values().any(|m| !m.graph.valid() || !m.endpoints.valid() || !m.allowed.valid())
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
            spatial: RefCell::new(VecDeque::new()),
            geometry: HashMap::new(),
            geometry_order: VecDeque::new(),
            geometry_bytes: 0,
            endpoints: VecDeque::new(),
        })
    }

    /// Check the complete object closure before publishing or installing a region.
    pub fn verify(&self) -> Result<()> {
        let mut keys = std::collections::BTreeSet::new();
        for table in std::iter::once(&self.manifest.geometry)
            .chain(self.manifest.osm.tables())
            .chain(self.manifest.metrics.values().flat_map(|m| [&m.graph, &m.endpoints]))
        {
            keys.extend(self.keys(table)?);
        }
        for metric in self.manifest.metrics.values() {
            for block in 0..metric.allowed.blocks.len() {
                self.words.borrow_mut().block(self, &metric.allowed, block)?;
            }
        }
        for key in self.manifest.spatial.values() {
            let cells: BTreeMap<String, String> = self.read(key)?;
            if cells.len() > 10_000 || cells.values().any(|id| !table::valid_digest(id)) {
                return Err(Error::InvalidData("Invalid snap directory".into()));
            }
            keys.extend(cells.into_values());
        }
        let mut keys: Vec<_> = keys.into_iter().collect();
        self.source.order_for_verify(&mut keys)?;
        for key in keys {
            self.bytes(&key)?;
        }
        Ok(())
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
            let value: Vec<Road> = self.read(&self.key(&self.manifest.geometry, page)?)?;
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

    pub fn endpoint(&mut self, metric: &str, id: u32) -> Result<Endpoint> {
        if id >= self.manifest.roads {
            return Err(Error::InvalidData("Endpoint outside package".into()));
        }
        let page = id / ROADS_PER_PAGE;
        let value =
            if let Some(index) = self.endpoints.iter().position(|(name, key, _, _)| name == metric && *key == page) {
                self.endpoints.remove(index).unwrap().2
            } else {
                let value: Vec<Endpoint> = self.read(&self.key(&self.metric(metric)?.endpoints, page)?)?;
                let states = self.metric(metric)?.states;
                if value.len() != (self.manifest.roads - page * ROADS_PER_PAGE).min(ROADS_PER_PAGE) as usize {
                    return Err(Error::InvalidData("Incomplete endpoint page".into()));
                }
                for (offset, e) in value.iter().enumerate() {
                    let road = page as usize * ROADS_PER_PAGE as usize + offset;
                    let allowed = self.allowed(metric, road as u32)?;
                    if allowed != e.cost.is_some()
                        || e.cost.as_ref().is_some_and(|cost| {
                            !cost.valid() || e.arrival >= states || e.departures.iter().any(|d| d.state >= states)
                        })
                    {
                        return Err(Error::InvalidData("Invalid endpoint page".into()));
                    }
                }
                Arc::new(value)
            };
        let endpoint = value
            .get((id % ROADS_PER_PAGE) as usize)
            .cloned()
            .ok_or_else(|| Error::InvalidData("Missing endpoint".into()))?;
        let size = value.capacity() * std::mem::size_of::<Endpoint>()
            + value
                .iter()
                .map(|e| {
                    e.departures.capacity() * std::mem::size_of::<Departure>()
                        + e.cost.as_ref().map_or(0, |c| c.penalties.capacity() * std::mem::size_of::<(f64, f64)>())
                })
                .sum::<usize>();
        self.endpoints.push_back((metric.into(), page, value, size));
        while self.endpoints.len() > 16 || self.endpoints.iter().map(|entry| entry.3).sum::<usize>() > 32 * 1024 * 1024
        {
            self.endpoints.pop_front();
        }
        Ok(endpoint)
    }
}
