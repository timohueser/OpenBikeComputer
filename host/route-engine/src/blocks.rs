//! Grid selections: compact road ids over immutable source pages, and the union graph they link.
use crate::{
    base::{Column, Costs, Graph, Numbers, Width},
    package::{self, Package, Source},
    table::{valid_digest, Table, ENTRIES},
    Error, Result,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

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

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.format != 2
            || !valid_digest(&self.source)
            || self.arcs > self.data.graph.head.len
            || self.snap.values().any(|key| !valid_digest(key))
            || self.archives.iter().any(|key| !valid_digest(key))
            || !self.archives.windows(2).all(|p| p[0] < p[1])
        {
            return Err(Error::InvalidData("Invalid routing selection".into()));
        }
        Ok(())
    }
}

/// Sorted, disjoint source ranges; a local id is the position in their concatenation.
#[derive(Clone)]
pub struct Ids {
    ranges: Vec<[u32; 2]>,
    offsets: Vec<u32>,
    pub count: u32,
    source_count: u32,
}
impl Ids {
    pub fn new(ranges: Vec<[u32; 2]>, source_count: u32) -> Result<Self> {
        let mut end = 0;
        let mut count = 0u32;
        let mut offsets = Vec::with_capacity(ranges.len());
        for &[first, last] in &ranges {
            if first < end || first >= last || last > source_count {
                return Err(Error::InvalidData("Overlapping or invalid road ranges".into()));
            }
            offsets.push(count);
            count = count.checked_add(last - first).ok_or(Error::Limit)?;
            end = last;
        }
        if count == 0 || count == u32::MAX {
            return Err(Error::InvalidData("Empty routing selection".into()));
        }
        Ok(Self { ranges, offsets, count, source_count })
    }
    /// Every source id, in place; `count` is validated nonzero and below `u32::MAX` by the manifest.
    pub fn whole(count: u32) -> Self {
        Self { ranges: vec![[0, count]], offsets: vec![0], count, source_count: count }
    }
    fn is_whole(&self) -> bool {
        self.count == self.source_count
    }
    pub fn ranges(&self) -> usize {
        self.ranges.len()
    }
    pub fn iter(&self) -> impl Iterator<Item = u32> + Clone + '_ {
        self.ranges.iter().flat_map(|&[a, z]| a..z)
    }
    pub fn local(&self, id: u32) -> Option<u32> {
        let at = self.ranges.partition_point(|r| r[0] <= id).checked_sub(1)?;
        (id < self.ranges[at][1]).then(|| self.offsets[at] + id - self.ranges[at][0])
    }
    pub fn source(&self, local: u32) -> Result<u32> {
        if local >= self.count {
            return Err(Error::InvalidData("Road outside selection".into()));
        }
        let at = self.offsets.partition_point(|&i| i <= local) - 1;
        Ok(self.ranges[at][0] + local - self.offsets[at])
    }
    pub fn lookup_bytes(&self) -> usize {
        if self.is_whole() {
            return 0;
        }
        let mut previous = usize::MAX;
        let mut count = 0usize;
        for &[a, z] in &self.ranges {
            for page in a as usize / 4096..=(z - 1) as usize / 4096 {
                if page != previous {
                    count += 1;
                    previous = page;
                }
            }
        }
        count.saturating_mul(4096 * 4).saturating_add(
            (self.source_count as usize).div_ceil(4096) * std::mem::size_of::<Option<Box<[u32; 4096]>>>(),
        )
    }
    pub fn fast(&self) -> Result<Lookup> {
        if self.is_whole() {
            return Ok(Lookup::Whole(self.count));
        }
        let mut pages = Vec::new();
        pages.try_reserve_exact((self.source_count as usize).div_ceil(4096)).map_err(|_| Error::Limit)?;
        pages.resize_with((self.source_count as usize).div_ceil(4096), || None);
        for (local, source) in self.iter().enumerate() {
            let page = pages[source as usize / 4096].get_or_insert_with(|| Box::new([u32::MAX; 4096]));
            page[source as usize % 4096] = local as u32;
        }
        Ok(Lookup::Pages(pages))
    }
}
pub enum Lookup {
    Whole(u32),
    Pages(Vec<Option<Box<[u32; 4096]>>>),
}
impl Lookup {
    pub fn get(&self, source: u32) -> Option<u32> {
        match self {
            Self::Whole(count) => (source < *count).then_some(source),
            Self::Pages(pages) => {
                let &local = pages.get(source as usize / 4096)?.as_ref()?.get(source as usize % 4096)?;
                (local != u32::MAX).then_some(local)
            }
        }
    }
}

/// Ascending source indices need only one decoded page at a time.
pub struct Reader<'a, T> {
    table: &'a Table,
    page: usize,
    values: Vec<T>,
}
impl<'a, T: Copy + DeserializeOwned> Reader<'a, T> {
    pub fn new(table: &'a Table) -> Self {
        Self { table, page: usize::MAX, values: Vec::new() }
    }
    pub fn get(&mut self, package: &Package<impl Source>, index: u32) -> Result<T> {
        if index >= self.table.len {
            return Err(Error::InvalidData("Value outside selected column".into()));
        }
        let page = index as usize / ENTRIES;
        if self.page != page {
            self.values = package.read(self.table.key(page)?)?;
            if self.values.len() != self.table.block_len(page) {
                return Err(Error::InvalidData("Incomplete selected column".into()));
            }
            self.page = page;
        }
        Ok(self.values[index as usize % ENTRIES])
    }
}

pub fn numbers(
    package: &Package<impl Source>,
    column: &Column,
    indices: impl Iterator<Item = u32>,
    count: usize,
) -> Result<Numbers> {
    macro_rules! read {
        ($variant:ident, $ty:ty) => {{
            let mut reader = Reader::<$ty>::new(&column.values);
            let mut result = Vec::new();
            result.try_reserve_exact(count).map_err(|_| Error::Limit)?;
            for index in indices {
                if result.len() == count {
                    return Err(Error::InvalidData("Too many selected values".into()));
                }
                result.push(reader.get(package, index)?);
            }
            if result.len() != count {
                return Err(Error::InvalidData("Incomplete selected values".into()));
            }
            Numbers::$variant(result)
        }};
    }
    Ok(match column.width {
        Width::U8 => read!(U8, u8),
        Width::U16 => read!(U16, u16),
        Width::U32 => read!(U32, u32),
        Width::U64 => read!(U64, u64),
    })
}

/// The selected roads' topology. The reverse tables are built from the forward arcs, so the
/// source's stored reverse tables are never read.
pub struct Union {
    pub graph: Arc<Graph>,
    /// Source adjacency ranges and a bit for each retained transition.
    arcs: Vec<[u32; 2]>,
    retained: Vec<u64>,
}
impl Union {
    pub fn open(package: &Package<impl Source>, ids: &Ids, maximum_arcs: u32) -> Result<Self> {
        let topology = &package.manifest().graph;
        let mut rows = Reader::<u32>::new(&topology.first);
        let mut heads = Reader::<u32>::new(&topology.head);
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
        let widest = first.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
        macro_rules! scatter {
            ($variant:ident, $ty:ty) => {{
                let mut offsets = vec![0 as $ty; head.len()];
                for from in 0..ids.count as usize {
                    for arc in first[from]..first[from + 1] {
                        let at = cursor[head[arc as usize] as usize] as usize;
                        reverse_tail[at] = from as u32;
                        offsets[at] = (arc - first[from]) as $ty;
                        cursor[head[arc as usize] as usize] += 1;
                    }
                }
                Numbers::$variant(offsets)
            }};
        }
        let reverse_offsets = if widest < u8::MAX as u32 {
            scatter!(U8, u8)
        } else if widest < u16::MAX as u32 {
            scatter!(U16, u16)
        } else {
            scatter!(U32, u32)
        };
        Ok(Self {
            graph: Arc::new(Graph { first, head, reverse_first, reverse_tail, reverse_offsets }),
            arcs,
            retained,
        })
    }
    pub fn costs(
        &self,
        package: &Package<impl Source>,
        ids: &Ids,
        metric: &str,
        turns: Option<Arc<Numbers>>,
    ) -> Result<Costs> {
        let weights = &package.metric(metric)?.weights;
        let roads = numbers(package, &weights.road_costs, ids.iter(), ids.count as usize)?;
        let turns = match turns {
            Some(turns) => turns,
            None => Arc::new(numbers(
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
