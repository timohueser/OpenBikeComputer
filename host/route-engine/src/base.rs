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

pub enum Numbers {
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
}
impl Numbers {
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
