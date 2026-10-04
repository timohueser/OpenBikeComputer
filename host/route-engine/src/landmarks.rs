//! Compressed junction distances guide the exact directed-road search.
use crate::{
    blocks::Ids,
    package::{Package, Source},
    search::Seed,
    table::{self, Table},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Index {
    pub scale: u32,
    pub junctions: u32,
    pub mapping: Table,
    pub profiles: BTreeMap<String, Vec<Table>>,
}

impl Index {
    pub fn valid(&self, roads: u32) -> bool {
        self.scale != 0
            && self.junctions != 0
            && self.mapping.len == roads
            && self.mapping.valid()
            && !self.profiles.is_empty()
            && self.profiles.values().all(|columns| {
                !columns.is_empty()
                    && columns.len() <= 32
                    && columns.iter().all(|c| c.len == self.junctions && c.valid())
            })
    }
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        std::iter::once(&self.mapping).chain(self.profiles.values().flatten())
    }
}

/// The values at ascending `indices` of a delta column. A sparse table yields `MissingRegion`
/// for a page it does not retain.
pub fn read<T: TryFrom<i64>>(
    package: &Package<impl Source>,
    table: &Table,
    indices: impl Iterator<Item = u32> + Clone,
    maximum: u32,
) -> Result<Vec<T>> {
    let mut result = Vec::new();
    result.try_reserve_exact(indices.clone().count()).map_err(|_| Error::Limit)?;
    let mut page = usize::MAX;
    let mut values = Vec::new();
    for index in indices {
        if index >= table.len {
            return Err(Error::InvalidData("Landmark outside selected column".into()));
        }
        let next = index as usize / table::ENTRIES;
        if next != page {
            let deltas: Vec<i64> = package.read(table.key(next)?)?;
            if deltas.len() != table.block_len(next) {
                return Err(Error::InvalidData("Incomplete selected landmark".into()));
            }
            let mut value = 0i64;
            values.clear();
            for delta in deltas {
                value = value.checked_add(delta).ok_or(Error::Limit)?;
                if !(0..=maximum as i64).contains(&value) {
                    return Err(Error::InvalidData("Invalid selected landmark".into()));
                }
                values.push(value);
            }
            page = next;
        }
        result.push(
            T::try_from(values[index as usize % table::ENTRIES])
                .map_err(|_| Error::InvalidData("Landmark exceeds width".into()))?,
        );
    }
    Ok(result)
}

pub fn write(
    values: impl IntoIterator<Item = u32>,
    write: &mut impl FnMut(&[u8]) -> std::result::Result<String, String>,
) -> std::result::Result<Table, String> {
    let mut values = values.into_iter();
    let mut blocks = Vec::new();
    let mut len = 0u32;
    loop {
        let mut previous = 0i64;
        let deltas: Vec<_> = values
            .by_ref()
            .take(table::ENTRIES)
            .map(|v| {
                let delta = v as i64 - previous;
                previous = v as i64;
                delta
            })
            .collect();
        if deltas.is_empty() {
            break;
        }
        len = len.checked_add(deltas.len() as u32).ok_or("Too many landmark entries")?;
        blocks.push(write(&crate::storage::encode(&deltas)?)?);
    }
    Ok(Table { len, blocks, pages: None })
}

/// The junction of each selected road, numbered densely over the junctions the selection uses.
pub struct Junctions {
    pub mapping: Arc<Vec<u32>>,
    /// Source junction ids in local order; column reads follow it.
    pub selected: Ids,
}

impl Junctions {
    pub fn read(package: &Package<impl Source>, index: &Index, ids: &Ids) -> Result<Self> {
        let mut mapping: Vec<u32> = read(package, &index.mapping, ids.iter(), index.junctions - 1)?;
        let mut used = vec![0u64; (index.junctions as usize).div_ceil(64)];
        for &junction in &mapping {
            used[junction as usize / 64] |= 1 << (junction % 64);
        }
        let mut ranges: Vec<[u32; 2]> = Vec::new();
        for junction in (0..index.junctions).filter(|j| used[*j as usize / 64] & (1 << (j % 64)) != 0) {
            if let Some(last) = ranges.last_mut().filter(|r| r[1] == junction) {
                last[1] += 1;
            } else {
                ranges.push([junction, junction + 1]);
            }
        }
        let selected = Ids::new(ranges, index.junctions)?;
        let lookup = selected.fast()?;
        for id in &mut mapping {
            *id = lookup.get(*id).ok_or_else(|| Error::InvalidData("Missing selected junction".into()))?;
        }
        Ok(Self { mapping: Arc::new(mapping), selected })
    }
}

/// One profile's distance columns over the selected junctions.
pub struct Guide {
    pub mapping: Arc<Vec<u32>>,
    pub scale: u32,
    pub columns: Vec<Vec<u16>>,
}

/// Up to four columns bound a query from both sides.
pub struct Potential<'a> {
    mapping: &'a [u32],
    scale: u32,
    columns: Vec<(&'a [u16], i128, i128)>,
}

impl Guide {
    /// The columns whose bounds separate these seeds the most.
    pub fn potential(&self, starts: &[Seed], ends: &[Seed]) -> Result<Potential<'_>> {
        let distance = |column: &[u16], seed: &Seed| -> Result<i128> {
            let junction = *self
                .mapping
                .get(seed.node as usize)
                .ok_or_else(|| Error::InvalidData("Seed outside mapping".into()))?;
            Ok(column[junction as usize] as i128 * self.scale as i128)
        };
        let mut columns = Vec::with_capacity(self.columns.len());
        for (i, column) in self.columns.iter().enumerate() {
            let mut from = i128::MAX;
            let mut to = i128::MIN;
            for seed in starts {
                from = from.min(distance(column, seed)? + seed.cost as i128);
            }
            for seed in ends {
                to = to.max(distance(column, seed)? - seed.cost as i128);
            }
            columns.push((i, from, to));
        }
        columns.sort_by_key(|&(i, from, to)| (std::cmp::Reverse(from.saturating_sub(to)), i));
        columns.truncate(4);
        Ok(Potential {
            mapping: &self.mapping,
            scale: self.scale,
            columns: columns.into_iter().map(|(i, from, to)| (self.columns[i].as_slice(), from, to)).collect(),
        })
    }
}

impl Potential<'_> {
    pub fn get(&self, road: u32) -> i128 {
        let junction = self.mapping[road as usize] as usize;
        let mut forward = 0;
        let mut backward = 0;
        for (column, from, to) in &self.columns {
            let distance = column[junction] as i128 * self.scale as i128;
            forward = forward.max(distance - to);
            backward = backward.max(from - distance);
        }
        forward - backward
    }
}
