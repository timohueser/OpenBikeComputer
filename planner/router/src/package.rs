//! The manifest of a regional package and bounded reads of its objects.
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
use std::sync::Arc;

pub const FORMAT: u32 = 8;
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
    /// One snap bit per directed road: the metric can use the road, and the road is in a large
    /// strongly connected part of the metric's graph, so a snapped point never sits on a fragment
    /// that no route leaves. Snapping reads these words, not the cost pages.
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
    /// One-degree directories contain the fine snap cells.
    pub spatial: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, Metric>,
    pub costs: Table,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landmarks: Option<crate::landmarks::Index>,
    /// Absent when no road has a possible closure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closures: Option<crate::closures::Index>,
}

impl Manifest {
    /// Every table whose blocks a complete package contains.
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.graph
            .tables()
            .into_iter()
            .chain([&self.geometry, &self.costs])
            .chain(self.landmarks.iter().flat_map(|index| index.tables()))
            .chain(self.closures.iter().flat_map(|index| index.tables()))
            .chain(self.metrics.values().flat_map(|m| m.weights.tables().into_iter().chain([&m.allowed, &m.costs])))
    }

    pub fn metric(&self, name: &str) -> Result<&Metric> {
        self.metrics.get(name).ok_or_else(|| Error::InvalidRequest(format!("Profile {name} is not installed")))
    }

    fn valid(&self) -> bool {
        let b = self.bounds;
        self.format == FORMAT
            && self.roads != 0
            && self.roads != u32::MAX
            && !self.metrics.is_empty()
            && b.iter().all(|n| n.is_finite())
            && b[0] < b[2]
            && b[1] < b[3]
            && b[0] >= -180.0
            && b[2] <= 180.0
            && b[1] >= -85.0
            && b[3] <= 85.0
            && self.geometry.len == self.roads.div_ceil(ROADS_PER_PAGE)
            && self.metrics.iter().all(|(id, metric)| {
                metric.profile.validate().is_ok()
                    && id == &metric.profile.name
                    && metric.weights.valid(&self.graph, self.roads)
                    && metric.costs.len == self.roads
                    && metric.allowed.len == self.roads.div_ceil(64)
            })
            && self.graph.valid(self.roads)
            && self.tables().all(Table::valid)
            && self.spatial.values().all(|key| table::valid_digest(key))
            && self.closures.as_ref().is_none_or(|index| index.valid(self.roads))
            && self.landmarks.as_ref().is_none_or(|index| {
                index.valid(self.roads) && index.profiles.keys().all(|name| self.metrics.contains_key(name))
            })
    }
}

/// How a road joins the search. Its arrival state is its road id.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Endpoint {
    pub cost: Option<RoadCost>,
    /// States from which this road can legally be entered.
    pub departures: Vec<Departure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Departure {
    pub state: u32,
    pub penalty: u64,
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

const GEOMETRY_CACHE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
struct Geometry {
    pages: HashMap<u32, (Arc<Vec<Road>>, usize)>,
    order: VecDeque<u32>,
    bytes: usize,
}

/// Reads one package's objects through small per-handle caches. Forks share the manifest.
pub struct Package<S> {
    manifest: Arc<Manifest>,
    identity: String,
    source: S,
    pub(crate) keys: RefCell<table::Cache<String>>,
    pub(crate) words: RefCell<table::Cache<u64>>,
    pub(crate) numbers: RefCell<table::Cache<u32>>,
    pub(crate) bases: RefCell<table::Cache<CostBasis>>,
    spatial: RefCell<VecDeque<(String, SpatialDirectory)>>,
    geometry: RefCell<Geometry>,
}

impl<S: Source> Package<S> {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn source(&self) -> &S {
        &self.source
    }

    /// The package whose manifest is these exact bytes; their digest is its identity.
    pub fn open(source: S, manifest: &[u8]) -> Result<Self> {
        if manifest.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Limit);
        }
        let identity = digest(manifest);
        let manifest: Manifest = serde_json::from_slice(manifest).map_err(|e| Error::InvalidData(e.to_string()))?;
        Self::new(source, manifest, identity)
    }

    pub(crate) fn new(source: S, manifest: Manifest, identity: String) -> Result<Self> {
        if !manifest.valid() {
            return Err(Error::InvalidData("Unsupported or incomplete regional manifest".into()));
        }
        Ok(Self::with(Arc::new(manifest), identity, source))
    }

    fn with(manifest: Arc<Manifest>, identity: String, source: S) -> Self {
        Self {
            manifest,
            identity,
            source,
            keys: RefCell::new(table::Cache::default()),
            words: RefCell::new(table::Cache::default()),
            numbers: RefCell::new(table::Cache::default()),
            bases: RefCell::new(table::Cache::default()),
            spatial: RefCell::new(VecDeque::new()),
            geometry: RefCell::new(Geometry::default()),
        }
    }

    /// A handle with its own caches over the same manifest and source.
    pub fn fork(&self) -> Self
    where
        S: Clone,
    {
        Self::with(Arc::clone(&self.manifest), self.identity.clone(), self.source.clone())
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

    /// The roads of one fine snap cell, through the one-degree directories of the manifest.
    pub fn spatial_roads(&self, cell: (i32, i32)) -> Result<Vec<u32>> {
        let group = cell_key((cell.0.div_euclid(100), cell.1.div_euclid(100)));
        let Some(key) = self.manifest.spatial.get(&group) else { return Ok(Vec::new()) };
        let mut cache = self.spatial.borrow_mut();
        let cells = if let Some(at) = cache.iter().position(|(id, _)| id == key) {
            cache.remove(at).unwrap().1
        } else {
            Arc::new(self.spatial_directory(key)?)
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

    pub(crate) fn spatial_directory(&self, key: &str) -> Result<BTreeMap<String, String>> {
        let cells: BTreeMap<String, String> = self.read(key)?;
        if cells.len() > 10_000 || cells.values().any(|id| !table::valid_digest(id)) {
            return Err(Error::InvalidData("Invalid snap directory".into()));
        }
        Ok(cells)
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
        self.manifest.metric(name)
    }

    pub fn road(&self, id: u32) -> Result<Road> {
        self.with_road(id, Road::clone)
    }

    /// Applies `f` to a road of the cached page, without a copy of its shape.
    pub fn with_road<T>(&self, id: u32, f: impl FnOnce(&Road) -> T) -> Result<T> {
        let page = self.page(id)?;
        page.get((id % ROADS_PER_PAGE) as usize).map(f).ok_or_else(|| Error::InvalidData("Missing road".into()))
    }

    fn page(&self, id: u32) -> Result<Arc<Vec<Road>>> {
        if id >= self.manifest.roads {
            return Err(Error::InvalidData("Road outside package".into()));
        }
        let page = id / ROADS_PER_PAGE;
        let mut geometry = self.geometry.borrow_mut();
        Ok(if let Some((value, _)) = geometry.pages.get(&page) {
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
            if size <= GEOMETRY_CACHE_BYTES {
                while geometry.bytes + size > GEOMETRY_CACHE_BYTES {
                    let key = geometry.order.pop_front().unwrap();
                    geometry.bytes -= geometry.pages.remove(&key).unwrap().1;
                }
                geometry.pages.insert(page, (Arc::clone(&value), size));
                geometry.order.push_back(page);
                geometry.bytes += size;
            }
            value
        })
    }

    fn number(&self, table: &Table, index: u32) -> Result<u32> {
        if index >= table.len {
            return Err(Error::InvalidData("Number outside table".into()));
        }
        let values = self.numbers.borrow_mut().block(self, table, index as usize / table::ENTRIES)?;
        Ok(values[index as usize % table::ENTRIES])
    }

    /// The exact cost factors of a road, or `None` when the metric excludes it.
    pub fn prepared_cost(&self, metric: &str, id: u32) -> Result<Option<CostBasis>> {
        if id >= self.manifest.roads {
            return Err(Error::InvalidData("Endpoint outside package".into()));
        }
        let prepared = self.metric(metric)?;
        let cost_id = self.number(&prepared.costs, id)?;
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
}
