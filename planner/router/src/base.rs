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
    /// The narrowest width that holds every position within an outgoing range. The runtime
    /// builds the reverse arcs from `first` and `head` and stores their offsets at this width.
    pub offsets: Width,
}

impl Topology {
    pub fn tables(&self) -> [&Table; 2] {
        [&self.first, &self.head]
    }
}

impl Width {
    pub fn bytes(self) -> usize {
        match self {
            Width::U8 => 1,
            Width::U16 => 2,
            Width::U32 => 4,
            Width::U64 => 8,
        }
    }
    /// The narrowest width whose all-ones sentinel no finite value up to `max` collides with.
    pub fn holding(max: u64) -> Self {
        if max < u8::MAX as u64 {
            Width::U8
        } else if max < u16::MAX as u64 {
            Width::U16
        } else if max < u32::MAX as u64 {
            Width::U32
        } else {
            Width::U64
        }
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
pub fn write_column(values: &[u64], write: &mut impl FnMut(&[u8]) -> Result<String>) -> Result<Column> {
    let width = Width::holding(values.iter().copied().filter(|&v| v != u64::MAX).max().unwrap_or(0));
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
    let widest = first.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
    Ok(Topology {
        first: Table::write(&first, &mut write)?,
        head: Table::write(&head, &mut write)?,
        offsets: Width::holding(widest.saturating_sub(1) as u64),
    })
}

pub enum Numbers {
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
}
impl Numbers {
    pub fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::U32(v) => v.len(),
            Self::U64(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
impl Topology {
    pub fn valid(&self, roads: u32) -> bool {
        roads.checked_add(1).is_some_and(|n| self.first.len == n) && self.tables().iter().all(|t| t.valid())
    }
}
impl Weights {
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
