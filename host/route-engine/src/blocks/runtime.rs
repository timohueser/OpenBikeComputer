use super::{ids::Ids, Union};
use crate::{
    base,
    data::RoutingData,
    landmarks,
    model::{Point, Profile, Road},
    package::{self, cell_key, digest, Departure, Endpoint, Source, MAX_MANIFEST_BYTES},
    search::Seed,
    snap::{self, Candidates, Policy},
    table::{self, valid_digest, Table},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub source: String,
    pub data: package::Manifest,
    /// Half-open ranges in the source road order.
    pub roads: Vec<[u32; 2]>,
    /// Outgoing source arcs before transitions to absent roads are removed.
    pub arcs: u32,
    pub snap: BTreeMap<String, String>,
    pub archives: Vec<String>,
}

struct Junctions {
    mapping: Arc<Vec<u32>>,
    source: Vec<u32>,
}

pub struct Package<S> {
    manifest: Manifest,
    identity: String,
    input: package::Package<S>,
    ids: Ids,
    union: Arc<Mutex<Option<Arc<Union>>>>,
    costs: VecDeque<(String, Arc<base::Costs>)>,
    turn_groups: Arc<BTreeMap<String, usize>>,
    junctions: RefCell<Option<Junctions>>,
    columns: RefCell<VecDeque<(Table, Arc<Vec<u16>>)>>,
    landmark_blocks: RefCell<table::Cache<i64>>,
    memory_budget: usize,
    resident_bytes: usize,
}

impl<S: Source> Package<S> {
    pub fn open(source: S, bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Limit);
        }
        let manifest: Manifest = serde_json::from_slice(bytes).map_err(|e| Error::InvalidData(e.to_string()))?;
        if manifest.format != 2
            || !valid_digest(&manifest.source)
            || manifest.arcs > manifest.data.graph.head.len
            || manifest.snap.values().any(|key| !valid_digest(key))
            || manifest.archives.iter().any(|key| !valid_digest(key))
            || !manifest.archives.windows(2).all(|p| p[0] < p[1])
        {
            return Err(Error::InvalidData("Invalid routing selection".into()));
        }
        let ids = Ids::new(manifest.roads.clone(), manifest.data.roads)?;
        let resident_bytes = source.resident_bytes().saturating_add(bytes.len().saturating_mul(4));
        let mut turns = Vec::new();
        let turn_groups = manifest
            .data
            .metrics
            .iter()
            .map(|(name, metric)| {
                let group = turns.iter().position(|column| *column == &metric.weights.turns).unwrap_or_else(|| {
                    turns.push(&metric.weights.turns);
                    turns.len() - 1
                });
                (name.clone(), group)
            })
            .collect();
        let input = package::Package::open(
            source,
            &serde_json::to_vec(&manifest.data).map_err(|e| Error::InvalidData(e.to_string()))?,
        )?;
        Ok(Self {
            manifest,
            identity: digest(bytes),
            input,
            ids,
            union: Arc::new(Mutex::new(None)),
            costs: VecDeque::new(),
            turn_groups: Arc::new(turn_groups),
            junctions: RefCell::new(None),
            columns: RefCell::new(VecDeque::new()),
            landmark_blocks: RefCell::new(table::Cache::default()),
            memory_budget: usize::MAX,
            resident_bytes,
        })
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn roads(&self) -> u32 {
        self.ids.count
    }
    pub fn source_id(&self, road: u32) -> Result<u32> {
        self.ids.source(road)
    }
    pub fn local_id(&self, road: u32) -> Option<u32> {
        self.ids.local(road)
    }

    pub fn fork(&self) -> Self
    where
        S: Clone,
    {
        Self {
            manifest: self.manifest.clone(),
            identity: self.identity.clone(),
            input: self.input.fork(),
            ids: self.ids.clone(),
            union: self.union.clone(),
            costs: VecDeque::new(),
            turn_groups: self.turn_groups.clone(),
            junctions: RefCell::new(None),
            columns: RefCell::new(VecDeque::new()),
            landmark_blocks: RefCell::new(table::Cache::default()),
            memory_budget: self.memory_budget,
            resident_bytes: self.resident_bytes,
        }
    }

    pub fn verify(&self) -> Result<()> {
        let data = &self.manifest.data;
        let mut keys = BTreeSet::new();
        for table in data.graph.tables().into_iter().chain([&data.geometry, &data.costs]) {
            keys.extend(table.blocks.iter().cloned());
        }
        for metric in data.metrics.values() {
            for table in metric.weights.tables().into_iter().chain([&metric.allowed, &metric.costs]) {
                keys.extend(table.blocks.iter().cloned());
            }
        }
        if let Some(index) = &data.landmarks {
            for table in index.tables() {
                keys.extend(table.blocks.iter().cloned());
            }
        }
        for page in self.ids.iter().map(|id| id / package::ROADS_PER_PAGE).collect::<BTreeSet<_>>() {
            keys.insert(self.input.key(&data.geometry, page)?);
        }
        keys.extend(self.manifest.snap.values().cloned());
        for key in keys {
            self.input.bytes(&key)?;
        }
        Ok(())
    }

    fn prepare_junctions(&self) -> Result<()> {
        let mut cached = self.junctions.borrow_mut();
        if cached.is_some() {
            return Ok(());
        }
        let Some(index) = &self.manifest.data.landmarks else {
            return Ok(());
        };
        let mut mapping: Vec<u32> =
            landmarks::read_selected(&self.input, &index.mapping, self.ids.iter(), index.junctions - 1)?;
        let mut source = mapping.clone();
        source.sort_unstable();
        source.dedup();
        let mut ranges: Vec<[u32; 2]> = Vec::new();
        for &id in &source {
            if let Some(last) = ranges.last_mut().filter(|r| r[1] == id) {
                last[1] += 1;
            } else {
                ranges.push([id, id + 1]);
            }
        }
        let selected = Ids::new(ranges, index.junctions)?;
        let lookup = selected.fast()?;
        for id in &mut mapping {
            *id = lookup.get(*id).ok_or_else(|| Error::InvalidData("Missing selected junction".into()))?;
        }
        *cached = Some(Junctions { mapping: Arc::new(mapping), source });
        Ok(())
    }
}

impl<S: Source> RoutingData for Package<S> {
    fn identity(&self) -> &str {
        &self.identity
    }
    fn region(&self) -> &str {
        &self.manifest.data.region
    }
    fn bounds(&self) -> [f64; 4] {
        self.manifest.data.bounds
    }
    fn attribution(&self) -> &str {
        &self.manifest.data.attribution
    }
    fn warnings(&self) -> &[String] {
        &self.manifest.data.warnings
    }
    fn profiles(&self) -> Vec<&str> {
        self.manifest.data.metrics.keys().map(String::as_str).collect()
    }
    fn profile(&self, name: &str) -> Result<&Profile> {
        Ok(&self.input.metric(name)?.profile)
    }

    fn snap(&mut self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
        self.profile(metric)?;
        let mut roads = BTreeSet::new();
        for cell in snap::cells(point, policy.radius_m).map_err(Error::InvalidRequest)? {
            if let Some(key) = self.manifest.snap.get(&cell_key(cell)) {
                let found: Vec<u32> = self.input.read(key)?;
                if found.iter().any(|&id| id >= self.manifest.data.roads) {
                    return Err(Error::InvalidData("Snap road outside source".into()));
                }
                roads.extend(found.into_iter().filter_map(|id| self.ids.local(id)));
                if roads.len() > 100_000 {
                    return Err(Error::Limit);
                }
            }
        }
        snap::candidates(roads, point, policy, |local| {
            let source = self.ids.source(local)?;
            if self.input.allowed(metric, source)? {
                self.input.road(source).map(Some)
            } else {
                Ok(None)
            }
        })
    }
    fn road(&mut self, road: u32) -> Result<Road> {
        self.input.road(self.ids.source(road)?)
    }
    fn endpoint(&mut self, metric: &str, road: u32) -> Result<Endpoint> {
        let source = self.ids.source(road)?;
        let Some(basis) = self.input.prepared_cost(metric, source)? else {
            return Ok(Endpoint { cost: None, arrival: road, departures: vec![] });
        };
        let cost = basis.compile(&self.input.road(source)?, self.profile(metric)?).map_err(Error::InvalidData)?;
        let (graph, costs) = self.base(metric)?;
        if cost.total() != costs.roads.get(road as usize) {
            return Err(Error::InvalidData("Road cost differs from its basis".into()));
        }
        let departures = (graph.reverse_first[road as usize]..graph.reverse_first[road as usize + 1])
            .filter_map(|reverse| {
                let reverse = reverse as usize;
                let penalty = costs.turns.get(graph.reverse_arc(reverse));
                (penalty != u64::MAX).then_some(Departure { state: graph.reverse_tail[reverse], penalty })
            })
            .collect();
        Ok(Endpoint { cost: Some(cost), arrival: road, departures })
    }
    fn base(&mut self, metric: &str) -> Result<(Arc<base::Graph>, Arc<base::Costs>)> {
        self.profile(metric)?;
        let cached = self.costs.iter().position(|(name, _)| name == metric).map(|at| self.costs.remove(at).unwrap());
        if cached.is_none() {
            while !self.costs.is_empty()
                && (self.costs.len() >= 3
                    || self.routing_bytes(metric)?.saturating_add(16 * 1024 * 1024) > self.memory_budget)
            {
                self.costs.pop_front();
            }
        }
        if self.routing_bytes(metric)? > self.memory_budget {
            return Err(Error::Limit);
        }
        let union = {
            let mut shared = self.union.lock().map_err(|_| Error::Limit)?;
            if shared.is_none() {
                *shared = Some(Arc::new(Union::open(&self.input, &self.ids, self.manifest.arcs)?));
            }
            shared.as_ref().unwrap().clone()
        };
        let entry = match cached {
            Some(entry) => entry,
            None => {
                let turns = self.costs.iter().find_map(|(name, costs)| {
                    (self.turn_groups[name] == self.turn_groups[metric]).then(|| costs.turns.clone())
                });
                (metric.into(), Arc::new(union.costs(&self.input, &self.ids, metric, turns)?))
            }
        };
        let costs = entry.1.clone();
        self.costs.push_back(entry);
        Ok((union.graph.clone(), costs))
    }
    fn has_landmarks(&self, metric: &str) -> bool {
        self.manifest.data.landmarks.as_ref().is_some_and(|i| i.profiles.contains_key(metric))
    }
    fn landmarks(&self, metric: &str, starts: &[Seed], ends: &[Seed]) -> Result<Option<landmarks::Prepared>> {
        if self.routing_bytes(metric)? > self.memory_budget {
            return Err(Error::Limit);
        }
        let Some(index) = &self.manifest.data.landmarks else {
            return Ok(None);
        };
        let Some(tables) = index.profiles.get(metric) else {
            return Ok(None);
        };
        if starts.is_empty() || ends.is_empty() {
            return Ok(None);
        }
        self.prepare_junctions()?;
        let junctions = self.junctions.borrow();
        let junctions = junctions.as_ref().unwrap();
        let selected = landmarks::select(tables.len(), index.scale, starts, ends, |column, road| {
            let &local = junctions
                .mapping
                .get(road as usize)
                .ok_or_else(|| Error::InvalidData("Landmark seed outside selection".into()))?;
            if let Some((_, values)) = self.columns.borrow().iter().find(|(t, _)| t == &tables[column]) {
                return Ok(values[local as usize]);
            }
            let source = junctions.source[local as usize] as usize;
            let values =
                self.landmark_blocks.borrow_mut().block(&self.input, &tables[column], source / table::ENTRIES)?;
            values[..=source % table::ENTRIES]
                .iter()
                .try_fold(0i64, |sum, &d| sum.checked_add(d))
                .and_then(|v| u16::try_from(v).ok())
                .ok_or_else(|| Error::InvalidData("Invalid landmark seed distance".into()))
        })?;
        let mut columns = Vec::new();
        for (i, from, to) in selected {
            let mut cache = self.columns.borrow_mut();
            let values = if let Some(at) = cache.iter().position(|(t, _)| t == &tables[i]) {
                cache.remove(at).unwrap().1
            } else {
                Arc::new(landmarks::read_selected(
                    &self.input,
                    &tables[i],
                    junctions.source.iter().copied(),
                    u16::MAX as u32,
                )?)
            };
            cache.push_back((tables[i].clone(), values.clone()));
            while cache.len() > landmarks::CACHED_COLUMNS {
                cache.pop_front();
            }
            columns.push((values, from, to));
        }
        Ok(Some(landmarks::Prepared { mapping: junctions.mapping.clone(), columns, scale: index.scale }))
    }
    fn routing_bytes(&self, metric: &str) -> Result<usize> {
        self.profile(metric)?;
        let roads = self.ids.count as usize;
        let arcs = self.manifest.arcs as usize;
        let width = |column: &base::Column| match column.width {
            base::Width::U8 => 1usize,
            base::Width::U16 => 2,
            base::Width::U32 => 4,
            base::Width::U64 => 8,
        };
        let topology = roads
            .saturating_add(1)
            .saturating_mul(8)
            .saturating_add(arcs.saturating_mul(8 + width(&self.manifest.data.graph.reverse_offsets)))
            .saturating_add(arcs.div_ceil(64).saturating_mul(8))
            .saturating_add(self.manifest.roads.len().saturating_mul(16));
        let guide = self.manifest.data.landmarks.as_ref().map_or(0, |index| {
            let nodes = roads.min(index.junctions as usize);
            // Mapping, selected junction ids, six cached columns and decode/remap scratch.
            roads
                .saturating_mul(8)
                .saturating_add(nodes.saturating_mul(4 + 2 * landmarks::CACHED_COLUMNS))
                .saturating_add(nodes.saturating_mul(4))
        });
        let mut weights_bytes = 0usize;
        let mut turns = Vec::new();
        for name in std::iter::once(metric)
            .chain(self.costs.iter().map(|(name, _)| name.as_str()).filter(|&name| name != metric))
        {
            let weights = &self.input.metric(name)?.weights;
            weights_bytes = weights_bytes.saturating_add(roads.saturating_mul(width(&weights.road_costs)));
            let group = self.turn_groups[name];
            if !turns.contains(&group) {
                weights_bytes = weights_bytes.saturating_add(arcs.saturating_mul(width(&weights.turns)));
                turns.push(group);
            }
        }
        let linking = if self.union.lock().map_err(|_| Error::Limit)?.is_none() {
            roads.saturating_mul(4).saturating_add(arcs.saturating_mul(4)).saturating_add(self.ids.lookup_bytes())
        } else {
            0
        };
        Ok(topology
            .saturating_add(guide.max(linking))
            .saturating_add(weights_bytes)
            .saturating_add(crate::search::label_bytes(roads))
            .saturating_add(64 * 1024 * 1024)
            .saturating_add(self.resident_bytes))
    }

    fn memory_budget(&self) -> usize {
        self.memory_budget
    }
    fn set_memory_budget(&mut self, bytes: usize) {
        self.memory_budget = bytes;
    }
}
