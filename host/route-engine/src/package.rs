use crate::{
    cost::RoadCost,
    model::{Point, Profile, Road},
    storage, Error, Result,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

pub const FORMAT: u32 = 2;
pub const MAX_MANIFEST_BYTES: usize = 128 * 1024 * 1024;
pub const ROADS_PER_PAGE: u32 = 128;
pub const CELL: i32 = 10_000;

/// Hosts read local files, browser storage or application assets through this seam.
/// A source is immutable for the lifetime of a package. A missing object is never an empty page.
pub trait Source {
    fn read(&self, digest: &str) -> Result<Vec<u8>>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric {
    pub profile: Profile,
    pub graph: Vec<String>,
    pub endpoints: Vec<String>,
    pub states: u32,
    /// One eligibility bit per directed road; snapping does not load cost pages.
    pub allowed: Vec<u64>,
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
    pub geometry: Vec<String>,
    pub osm: OsmPages,
    /// Cell keys are latitude_index,longitude_index.
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
    pub nodes: Vec<String>,
    pub ways: Vec<String>,
    pub relations: Vec<String>,
}

impl OsmPages {
    pub fn objects(&self) -> impl Iterator<Item = &String> {
        self.nodes.iter().chain(&self.ways).chain(&self.relations)
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

pub struct Package<S> {
    pub(crate) manifest: Manifest,
    pub(crate) identity: String,
    source: S,
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
            || manifest.geometry.len() != manifest.roads.div_ceil(ROADS_PER_PAGE) as usize
        {
            return Err(Error::InvalidData("Unsupported or incomplete regional manifest".into()));
        }
        for (id, metric) in &manifest.metrics {
            metric.profile.validate().map_err(Error::InvalidData)?;
            if id != &metric.profile.name
                || metric.states == 0
                || metric.graph.len() != metric.states.div_ceil(storage::NODES_PER_PAGE) as usize
                || metric.endpoints.len() != manifest.geometry.len()
                || metric.allowed.len() != (manifest.roads as usize).div_ceil(64)
            {
                return Err(Error::InvalidData("Incomplete prepared metric".into()));
            }
        }
        for key in manifest
            .geometry
            .iter()
            .chain(manifest.osm.objects())
            .chain(manifest.spatial.values())
            .chain(manifest.metrics.values().flat_map(|m| m.graph.iter().chain(&m.endpoints)))
        {
            if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
                return Err(Error::InvalidData("Invalid object identity".into()));
            }
        }
        Ok(Self {
            manifest,
            identity,
            source,
            geometry: HashMap::new(),
            geometry_order: VecDeque::new(),
            geometry_bytes: 0,
            endpoints: VecDeque::new(),
        })
    }

    /// Check the complete object closure before publishing or installing a region.
    pub fn verify(&self) -> Result<()> {
        let keys: std::collections::BTreeSet<_> = self
            .manifest
            .geometry
            .iter()
            .chain(self.manifest.osm.objects())
            .chain(self.manifest.spatial.values())
            .chain(self.manifest.metrics.values().flat_map(|m| m.graph.iter().chain(&m.endpoints)))
            .collect();
        for key in keys {
            self.bytes(key)?;
        }
        Ok(())
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
            let value: Vec<Road> = self.read(&self.manifest.geometry[page as usize])?;
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
                let value: Vec<Endpoint> = self.read(&self.metric(metric)?.endpoints[page as usize])?;
                let states = self.metric(metric)?.states;
                if value.len() != (self.manifest.roads - page * ROADS_PER_PAGE).min(ROADS_PER_PAGE) as usize
                    || value.iter().enumerate().any(|(offset, e)| {
                        let road = page as usize * ROADS_PER_PAGE as usize + offset;
                        let allowed = self.metric(metric).unwrap().allowed[road / 64] & (1 << (road % 64)) != 0;
                        allowed != e.cost.is_some()
                            || e.cost.as_ref().is_some_and(|cost| {
                                !cost.valid() || e.arrival >= states || e.departures.iter().any(|d| d.state >= states)
                            })
                    })
                {
                    return Err(Error::InvalidData("Invalid endpoint page".into()));
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
