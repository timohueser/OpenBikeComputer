//! Selections share immutable source pages and link compact arrays only while they are open.
mod columns;
#[cfg(not(target_arch = "wasm32"))]
mod files;
mod ids;
mod runtime;
use crate::{
    base::{Costs, Graph, Numbers},
    package::{Package as SourcePackage, Source},
    Error, Result,
};
#[cfg(not(target_arch = "wasm32"))]
pub use files::Files;
use ids::Ids;
pub use runtime::{Manifest, Package};
use std::sync::Arc;

pub struct Union {
    pub graph: Arc<Graph>,
    /// Source adjacency ranges and a bit for each retained transition.
    arcs: Vec<[u32; 2]>,
    retained: Vec<u64>,
}
impl Union {
    fn open(package: &SourcePackage<impl Source>, ids: &Ids, maximum_arcs: u32) -> Result<Self> {
        let topology = &package.manifest().graph;
        let mut rows = columns::Reader::<u32>::new(&topology.first);
        let mut heads = columns::Reader::<u32>::new(&topology.head);
        let lookup = ids.fast()?;
        let mut first = Vec::new();
        first.try_reserve_exact(ids.count as usize + 1).map_err(|_| Error::Limit)?;
        let mut head = Vec::new();
        head.try_reserve_exact(maximum_arcs as usize).map_err(|_| Error::Limit)?;
        let mut arcs: Vec<[u32; 2]> = Vec::new();
        let mut retained = vec![0u64; (maximum_arcs as usize).div_ceil(64)];
        let mut previous_end = 0;
        let mut source_arcs = 0u32;
        for road in ids.iter() {
            first.push(head.len() as u32);
            let a = rows.get(package, road)?;
            let z = rows.get(package, road + 1)?;
            if a > z || a < previous_end || z > topology.head.len {
                return Err(Error::InvalidData("Invalid selected adjacency range".into()));
            }
            previous_end = z;
            let ordinal = source_arcs;
            source_arcs = source_arcs.checked_add(z - a).ok_or(Error::Limit)?;
            if a < z {
                if let Some(last) = arcs.last_mut().filter(|r| r[1] == a) {
                    last[1] = z;
                } else {
                    arcs.push([a, z]);
                }
            }
            if source_arcs > maximum_arcs {
                return Err(Error::InvalidData("Selection arc count differs".into()));
            }
            let mut previous = None;
            for arc in a..z {
                let source = heads.get(package, arc)?;
                if source >= package.manifest().roads || previous.is_some_and(|old| old >= source) {
                    return Err(Error::InvalidData("Invalid selected transition".into()));
                }
                previous = Some(source);
                if let Some(to) = lookup.get(source) {
                    head.push(to);
                    let bit = (ordinal + (arc - a)) as usize;
                    retained[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
        if source_arcs != maximum_arcs {
            return Err(Error::InvalidData("Incomplete selection adjacency".into()));
        }
        first.push(head.len() as u32);
        let mut reverse_first = vec![0u32; first.len()];
        for &to in &head {
            reverse_first[to as usize + 1] += 1;
        }
        for i in 0..ids.count as usize {
            reverse_first[i + 1] += reverse_first[i];
        }
        let mut cursor = reverse_first.clone();
        let mut reverse_tail = vec![0u32; head.len()];
        let mut offsets = vec![0u32; head.len()];
        let mut maximum = 0;
        for from in 0..ids.count as usize {
            for arc in first[from]..first[from + 1] {
                let to = head[arc as usize] as usize;
                let at = cursor[to] as usize;
                reverse_tail[at] = from as u32;
                offsets[at] = arc - first[from];
                maximum = maximum.max(offsets[at]);
                cursor[to] += 1;
            }
        }
        let reverse_offsets = if maximum < u8::MAX as u32 {
            Numbers::U8(offsets.into_iter().map(|n| n as u8).collect())
        } else if maximum < u16::MAX as u32 {
            Numbers::U16(offsets.into_iter().map(|n| n as u16).collect())
        } else {
            Numbers::U32(offsets)
        };
        Ok(Self {
            graph: Arc::new(Graph { first, head, reverse_first, reverse_tail, reverse_offsets }),
            arcs,
            retained,
        })
    }
    fn costs(
        &self,
        package: &SourcePackage<impl Source>,
        ids: &Ids,
        metric: &str,
        turns: Option<Arc<Numbers>>,
    ) -> Result<Costs> {
        let weights = &package.metric(metric)?.weights;
        let roads = columns::numbers(package, &weights.road_costs, ids.iter(), ids.count as usize)?;
        let turns = match turns {
            Some(turns) => turns,
            None => Arc::new(columns::numbers(
                package,
                &weights.turns,
                self.arcs
                    .iter()
                    .flat_map(|&[a, z]| a..z)
                    .enumerate()
                    .filter(|(bit, _)| self.retained[bit / 64] & (1 << (bit % 64)) != 0)
                    .map(|(_, arc)| arc),
                self.graph.head.len(),
            )?),
        };
        for (arc, &to) in self.graph.head.iter().enumerate() {
            let road = roads.get(to as usize);
            let turn = turns.get(arc);
            if road != u64::MAX && turn != u64::MAX && road.checked_add(turn).is_none_or(|v| v == 0 || v == u64::MAX) {
                return Err(Error::InvalidData("Invalid selected transition cost".into()));
            }
        }
        Ok(Costs { roads, turns })
    }
}
