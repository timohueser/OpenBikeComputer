//! Shared directed topology and exact per-profile road and turn costs.
use crate::{
    storage,
    table::{self, Table},
};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Width {
    U8,
    U16,
    U32,
    U64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub width: Width,
    pub values: Table,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Topology {
    pub first: Table,
    pub head: Table,
    pub reverse_first: Table,
    pub reverse_tail: Table,
    pub reverse_offsets: Column,
}

impl Topology {
    pub fn tables(&self) -> [&Table; 5] {
        [&self.first, &self.head, &self.reverse_first, &self.reverse_tail, &self.reverse_offsets.values]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Weights {
    pub road_costs: Column,
    pub turns: Column,
}

impl Weights {
    pub fn tables(&self) -> [&Table; 2] {
        [&self.road_costs.values, &self.turns.values]
    }
}

/// All-ones values represent absent costs. A finite value never shares that encoding.
fn write_column(values: &[u64], write: &mut impl FnMut(&[u8]) -> Result<String>) -> Result<Column> {
    let max = values.iter().copied().filter(|&v| v != u64::MAX).max().unwrap_or(0);
    let width = if max < u8::MAX as u64 {
        Width::U8
    } else if max < u16::MAX as u64 {
        Width::U16
    } else if max < u32::MAX as u64 {
        Width::U32
    } else {
        Width::U64
    };
    let mut blocks = Vec::new();
    for page in values.chunks(table::ENTRIES) {
        let bytes = match width {
            Width::U8 => storage::encode(&page.iter().map(|&v| v as u8).collect::<Vec<_>>())?,
            Width::U16 => storage::encode(&page.iter().map(|&v| v as u16).collect::<Vec<_>>())?,
            Width::U32 => storage::encode(&page.iter().map(|&v| v as u32).collect::<Vec<_>>())?,
            Width::U64 => storage::encode(&page)?,
        };
        blocks.push(write(&bytes)?);
    }
    Ok(Column {
        width,
        values: Table { len: u32::try_from(values.len()).map_err(|_| "Column exceeds u32")?, blocks, pages: None },
    })
}

pub fn write_weights(
    road_costs: &[u64],
    turns: &[u64],
    mut write: impl FnMut(&[u8]) -> Result<String>,
) -> Result<Weights> {
    Ok(Weights { road_costs: write_column(road_costs, &mut write)?, turns: write_column(turns, &mut write)? })
}

/// Edges are unique and sorted by (source road, destination road). Turn costs use this exact order.
pub fn write_topology(
    roads: u32,
    edges: &[(u32, u32)],
    mut write: impl FnMut(&[u8]) -> Result<String>,
) -> Result<Topology> {
    if roads == 0 || roads == u32::MAX || edges.len() > u32::MAX as usize || !edges.windows(2).all(|p| p[0] < p[1]) {
        return Err("Invalid base graph dimensions or arc order".into());
    }
    let mut first = vec![0u32; roads as usize + 1];
    let mut head = Vec::with_capacity(edges.len());
    for &(from, to) in edges {
        if from >= roads || to >= roads {
            return Err("Base arc outside road graph".into());
        }
        first[from as usize + 1] += 1;
        head.push(to);
    }
    for i in 0..roads as usize {
        first[i + 1] += first[i];
    }
    let mut reverse_first = vec![0u32; first.len()];
    for &to in &head {
        reverse_first[to as usize + 1] += 1;
    }
    for i in 0..roads as usize {
        reverse_first[i + 1] += reverse_first[i];
    }
    let mut cursor = reverse_first.clone();
    let mut reverse_tail = vec![0u32; edges.len()];
    let mut reverse_offsets = vec![0u64; edges.len()];
    for (arc, &(from, to)) in edges.iter().enumerate() {
        let at = cursor[to as usize] as usize;
        reverse_tail[at] = from;
        reverse_offsets[at] = arc as u64 - first[from as usize] as u64;
        cursor[to as usize] += 1;
    }
    Ok(Topology {
        first: Table::write(&first, &mut write)?,
        head: Table::write(&head, &mut write)?,
        reverse_first: Table::write(&reverse_first, &mut write)?,
        reverse_tail: Table::write(&reverse_tail, &mut write)?,
        reverse_offsets: write_column(&reverse_offsets, &mut write)?,
    })
}

use crate::{
    package::{Package, Source},
    Error,
};
use serde::de::DeserializeOwned;

fn read_table<T: DeserializeOwned>(package: &Package<impl Source>, table: &Table) -> crate::Result<Vec<T>> {
    if !table.valid() || table.pages.is_some() {
        return Err(Error::InvalidData("Invalid base column directory".into()));
    }
    let mut values = Vec::new();
    values.try_reserve_exact(table.len as usize).map_err(|_| Error::Limit)?;
    for (block, key) in table.blocks.iter().enumerate() {
        let page: Vec<T> = package.read(key)?;
        if page.len() != table.block_len(block) {
            return Err(Error::InvalidData("Incomplete base column".into()));
        }
        values.extend(page);
    }
    Ok(values)
}

pub enum Numbers {
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
}
impl Numbers {
    fn read(package: &Package<impl Source>, column: &Column) -> crate::Result<Self> {
        Ok(match column.width {
            Width::U8 => Self::U8(read_table(package, &column.values)?),
            Width::U16 => Self::U16(read_table(package, &column.values)?),
            Width::U32 => Self::U32(read_table(package, &column.values)?),
            Width::U64 => Self::U64(read_table(package, &column.values)?),
        })
    }
    pub fn get(&self, index: usize) -> u64 {
        match self {
            Self::U8(v) => {
                if v[index] == u8::MAX {
                    u64::MAX
                } else {
                    v[index] as u64
                }
            }
            Self::U16(v) => {
                if v[index] == u16::MAX {
                    u64::MAX
                } else {
                    v[index] as u64
                }
            }
            Self::U32(v) => {
                if v[index] == u32::MAX {
                    u64::MAX
                } else {
                    v[index] as u64
                }
            }
            Self::U64(v) => v[index],
        }
    }
}
impl Column {
    pub fn decoded_bytes(&self) -> usize {
        (self.values.len as usize).saturating_mul(match self.width {
            Width::U8 => 1,
            Width::U16 => 2,
            Width::U32 => 4,
            Width::U64 => 8,
        })
    }
}
impl Topology {
    pub fn decoded_bytes(&self) -> usize {
        [&self.first, &self.head, &self.reverse_first, &self.reverse_tail]
            .iter()
            .fold(self.reverse_offsets.decoded_bytes(), |sum, t| sum.saturating_add((t.len as usize).saturating_mul(4)))
    }
    pub fn valid(&self, roads: u32) -> bool {
        roads.checked_add(1).is_some_and(|n| self.first.len == n && self.reverse_first.len == n)
            && self.head.len == self.reverse_tail.len
            && self.head.len == self.reverse_offsets.values.len
            && self.tables().iter().all(|t| t.valid())
    }
}
impl Weights {
    pub fn decoded_bytes(&self) -> usize {
        self.road_costs.decoded_bytes().saturating_add(self.turns.decoded_bytes())
    }
    pub fn valid(&self, graph: &Topology, roads: u32) -> bool {
        self.road_costs.values.len == roads
            && self.turns.values.len == graph.head.len
            && self.tables().iter().all(|t| t.valid())
    }
}

pub struct Graph {
    pub first: Vec<u32>,
    pub head: Vec<u32>,
    pub reverse_first: Vec<u32>,
    pub reverse_tail: Vec<u32>,
    pub reverse_offsets: Numbers,
}
impl Graph {
    pub fn read(package: &Package<impl Source>, topology: &Topology) -> crate::Result<Self> {
        if !topology.valid(package.manifest().roads) {
            return Err(Error::InvalidData("Invalid base topology columns".into()));
        }
        let graph = Self {
            first: read_table(package, &topology.first)?,
            head: read_table(package, &topology.head)?,
            reverse_first: read_table(package, &topology.reverse_first)?,
            reverse_tail: read_table(package, &topology.reverse_tail)?,
            reverse_offsets: Numbers::read(package, &topology.reverse_offsets)?,
        };
        let n = graph.first.len() - 1;
        let invalid = || Error::InvalidData("Invalid directed base topology".into());
        for offsets in [&graph.first, &graph.reverse_first] {
            if offsets.first() != Some(&0)
                || offsets.last().copied() != Some(graph.head.len() as u32)
                || !offsets.windows(2).all(|v| v[0] <= v[1])
            {
                return Err(invalid());
            }
        }
        for node in 0..n {
            let heads = &graph.head[graph.first[node] as usize..graph.first[node + 1] as usize];
            if heads.iter().any(|&to| to as usize >= n) || !heads.windows(2).all(|v| v[0] < v[1]) {
                return Err(invalid());
            }
            let mut previous = None;
            for index in graph.reverse_first[node]..graph.reverse_first[node + 1] {
                let index = index as usize;
                let from = graph.reverse_tail[index];
                let offset = graph.reverse_offsets.get(index);
                if from as usize >= n || offset >= (graph.first[from as usize + 1] - graph.first[from as usize]) as u64
                {
                    return Err(invalid());
                }
                let arc = graph.first[from as usize] as usize + offset as usize;
                if graph.head[arc] != node as u32 || previous.is_some_and(|old| old >= from) {
                    return Err(invalid());
                }
                previous = Some(from);
            }
        }
        Ok(graph)
    }
    pub fn reverse_arc(&self, index: usize) -> usize {
        self.first[self.reverse_tail[index] as usize] as usize + self.reverse_offsets.get(index) as usize
    }
    pub fn nodes(&self) -> usize {
        self.first.len() - 1
    }
}

pub struct Costs {
    pub roads: Numbers,
    pub turns: std::sync::Arc<Numbers>,
}
impl Costs {
    pub fn read(package: &Package<impl Source>, weights: &Weights, graph: &Graph) -> crate::Result<Self> {
        Self::read_shared(package, weights, graph, None)
    }
    pub(crate) fn read_shared(
        package: &Package<impl Source>,
        weights: &Weights,
        graph: &Graph,
        turns: Option<std::sync::Arc<Numbers>>,
    ) -> crate::Result<Self> {
        if !weights.valid(&package.manifest().graph, graph.nodes() as u32) {
            return Err(Error::InvalidData("Invalid base cost columns".into()));
        }
        let costs = Self {
            roads: Numbers::read(package, &weights.road_costs)?,
            turns: match turns {
                Some(turns) => turns,
                None => std::sync::Arc::new(Numbers::read(package, &weights.turns)?),
            },
        };
        for (arc, &road) in graph.head.iter().enumerate() {
            let total = costs.roads.get(road as usize);
            let turn = costs.turns.get(arc);
            if total != u64::MAX && turn != u64::MAX && total.checked_add(turn).is_none_or(|n| n == 0 || n == u64::MAX)
            {
                return Err(Error::InvalidData("Invalid base transition cost".into()));
            }
        }
        Ok(costs)
    }
    pub fn arc(&self, graph: &Graph, arc: usize) -> u64 {
        let road = self.roads.get(graph.head[arc] as usize);
        let turn = self.turns.get(arc);
        if road == u64::MAX || turn == u64::MAX {
            u64::MAX
        } else {
            road + turn
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::Profile,
        package::{digest, Manifest, Metric, OsmPages, FORMAT},
    };
    use std::{
        cell::{Cell, RefCell},
        collections::BTreeMap,
        rc::Rc,
    };

    #[derive(Clone, Default)]
    struct Memory {
        objects: Rc<RefCell<BTreeMap<String, Vec<u8>>>>,
        reads: Rc<Cell<usize>>,
    }
    impl Source for Memory {
        fn read(&self, key: &str) -> crate::Result<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            self.objects.borrow().get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
        }
    }
    impl Memory {
        fn write(&self, bytes: &[u8]) -> Result<String> {
            let key = digest(bytes);
            self.objects.borrow_mut().insert(key.clone(), bytes.to_vec());
            Ok(key)
        }
        fn package(&self, roads: u32, graph: Topology, weights: Weights) -> Package<Self> {
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
            let manifest = Manifest {
                format: FORMAT,
                region: "test".into(),
                bounds: [0., 0., 1., 1.],
                source_sha256: vec![],
                attribution: String::new(),
                warnings: vec![],
                roads,
                graph,
                geometry: placeholder(roads.div_ceil(crate::package::ROADS_PER_PAGE)),
                osm: OsmPages::default(),
                spatial: BTreeMap::new(),
                metrics: BTreeMap::from([(profile.name, metric)]),
                costs: Table::default(),
                landmarks: None,
                closures: None,
            };
            Package::open(self.clone(), &serde_json::to_vec(&manifest).unwrap()).unwrap()
        }
    }

    #[test]
    fn columns_preserve_sentinels_large_costs_and_reverse_offsets() {
        let memory = Memory::default();
        let mut edges: Vec<_> = (0..5000).map(|to| (0, to)).collect();
        edges.extend([(4095, 0), (4096, 4999), (4999, 1)]);
        let topology = write_topology(5000, &edges, |b| memory.write(b)).unwrap();
        let weights = write_weights(&vec![1; 5000], &vec![0; edges.len()], |b| memory.write(b)).unwrap();
        let mut package = memory.package(5000, topology.clone(), weights.clone());
        let mut fork = package.fork();
        let graph = Graph::read(&package, &topology).unwrap();
        for to in 0..graph.nodes() {
            for index in graph.reverse_first[to]..graph.reverse_first[to + 1] {
                let arc = graph.reverse_arc(index as usize);
                assert_eq!(edges[arc], (graph.reverse_tail[index as usize], to as u32));
            }
        }
        for maximum in [254, 255, 65534, 65535, u32::MAX as u64 - 1, u32::MAX as u64, u64::MAX - 1] {
            let original = [0, 1, maximum, u64::MAX];
            let column = write_column(&original, &mut |b| memory.write(b)).unwrap();
            let decoded = Numbers::read(&package, &column).unwrap();
            assert_eq!((0..original.len()).map(|i| decoded.get(i)).collect::<Vec<_>>(), original);
        }
        assert_eq!(Costs::read(&package, &weights, &graph).unwrap().arc(&graph, 0), 1);
        let (shared, first_costs) = package.base("touring").unwrap();
        memory.reads.set(0);
        let (same_graph, independent_costs) = fork.base("touring").unwrap();
        assert!(std::sync::Arc::ptr_eq(&shared, &same_graph));
        assert!(!std::sync::Arc::ptr_eq(&first_costs, &independent_costs));
        assert_eq!(memory.reads.get(), weights.tables().iter().map(|t| t.blocks.len()).sum::<usize>());
        assert_eq!(package.identity(), fork.identity());
        assert!(write_topology(2, &[(1, 0), (0, 1)], |b| memory.write(b)).is_err());
        assert!(write_topology(2, &[(0, 1), (0, 1)], |b| memory.write(b)).is_err());
    }

    #[test]
    fn profiles_share_only_identical_turn_columns() {
        let memory = Memory::default();
        let topology = write_topology(2, &[(0, 1)], |b| memory.write(b)).unwrap();
        let weights = write_weights(&[1, 2], &[0], |b| memory.write(b)).unwrap();
        let mut manifest = memory.package(2, topology, weights).manifest().clone();
        for (name, roads, turns) in [("gravel", [4, 5], [0]), ("hiking", [6, 7], [2])] {
            let mut metric = manifest.metrics["touring"].clone();
            metric.profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
            metric.weights = write_weights(&roads, &turns, |b| memory.write(b)).unwrap();
            manifest.metrics.insert(name.into(), metric);
        }
        let mut package = Package::open(memory.clone(), &serde_json::to_vec(&manifest).unwrap()).unwrap();
        let (graph, touring) = package.base("touring").unwrap();
        memory.reads.set(0);
        let (_, gravel) = package.base("gravel").unwrap();
        assert_eq!(memory.reads.get(), 1);
        assert!(std::sync::Arc::ptr_eq(&touring.turns, &gravel.turns));
        let (_, hiking) = package.base("hiking").unwrap();
        assert!(!std::sync::Arc::ptr_eq(&touring.turns, &hiking.turns));
        assert_eq!([touring.arc(&graph, 0), gravel.arc(&graph, 0), hiking.arc(&graph, 0)], [2, 5, 9]);
    }

    #[test]
    fn malformed_graph_costs_and_insufficient_memory_fail_before_search() {
        let memory = Memory::default();
        let topology = write_topology(2, &[(0, 1)], |b| memory.write(b)).unwrap();
        let weights = write_weights(&[1, u64::MAX - 2], &[3], |b| memory.write(b)).unwrap();
        let mut package = memory.package(2, topology.clone(), weights.clone());
        package.memory_budget = package.routing_bytes("touring").unwrap() - 1;
        assert!(matches!(package.base("touring"), Err(Error::Limit)));
        assert_eq!(memory.reads.get(), 0);
        let graph = Graph::read(&package, &topology).unwrap();
        assert!(matches!(Costs::read(&package, &weights, &graph), Err(Error::InvalidData(_))));
        let mut invalid = topology.clone();
        invalid.reverse_offsets = write_column(&[1], &mut |b| memory.write(b)).unwrap();
        assert!(matches!(Graph::read(&package, &invalid), Err(Error::InvalidData(_))));
        let mut invalid = topology;
        invalid.head.blocks[0] = memory.write(&storage::encode(&Vec::<u32>::new()).unwrap()).unwrap();
        assert!(matches!(Graph::read(&package, &invalid), Err(Error::InvalidData(_))));
    }
}
