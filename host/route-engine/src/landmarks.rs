//! Compressed junction distances guide the exact directed-road search.
use crate::{
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
    pub fn decoded_bytes(&self) -> usize {
        (self.mapping.len as usize)
            .saturating_mul(4)
            .saturating_add((self.junctions as usize).saturating_mul(2).saturating_mul(CACHED_COLUMNS))
    }
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        std::iter::once(&self.mapping).chain(self.profiles.values().flatten())
    }
}

pub fn read<T: TryFrom<i64>>(package: &Package<impl Source>, table: &Table, maximum: u32) -> Result<Vec<T>> {
    if table.pages.is_some() {
        return Err(Error::MissingRegion("Incomplete landmark selection".into()));
    }
    let mut output = Vec::new();
    output.try_reserve_exact(table.len as usize).map_err(|_| Error::Limit)?;
    for (block, key) in table.blocks.iter().enumerate() {
        let deltas: Vec<i64> = package.read(key)?;
        if deltas.len() != table.block_len(block) {
            return Err(Error::InvalidData("Incomplete landmark block".into()));
        }
        let mut value = 0i64;
        for delta in deltas {
            value = value.checked_add(delta).ok_or_else(|| Error::InvalidData("Landmark delta overflow".into()))?;
            if !(0..=maximum as i64).contains(&value) {
                return Err(Error::InvalidData("Landmark outside column".into()));
            }
            output.push(T::try_from(value).map_err(|_| Error::InvalidData("Landmark value exceeds width".into()))?);
        }
    }
    Ok(output)
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

/// Deleting roads preserves feasible potentials when costs and surviving turns stay unchanged.
pub fn project(
    package: &Package<impl Source>,
    roads: &[u32],
    write: &mut impl FnMut(&[u8]) -> std::result::Result<String, String>,
) -> std::result::Result<Option<Index>, String> {
    let Some(index) = &package.manifest().landmarks else { return Ok(None) };
    let mapping: Vec<u32> = read(package, &index.mapping, index.junctions - 1).map_err(|e| e.to_string())?;
    let mut junctions = BTreeMap::new();
    let mut selected = Vec::new();
    let mut remapped = Vec::with_capacity(roads.len());
    for &road in roads {
        let old = *mapping.get(road as usize).ok_or("Invalid projected road")?;
        let next = junctions.len() as u32;
        let new = *junctions.entry(old).or_insert_with(|| {
            selected.push(old);
            next
        });
        remapped.push(new);
    }
    let mut projected = Index {
        scale: index.scale,
        junctions: selected.len() as u32,
        mapping: self::write(remapped, write)?,
        profiles: BTreeMap::new(),
    };
    for (name, tables) in &index.profiles {
        let mut columns = Vec::new();
        for table in tables {
            let values: Vec<u16> = read(package, table, u16::MAX as u32).map_err(|e| e.to_string())?;
            columns.push(self::write(selected.iter().map(|&node| values[node as usize] as u32), write)?);
        }
        projected.profiles.insert(name.clone(), columns);
    }
    Ok(Some(projected))
}

pub(crate) const CACHED_COLUMNS: usize = 6;
pub struct Prepared {
    pub mapping: Arc<Vec<u32>>,
    pub columns: Vec<(Arc<Vec<u16>>, i128, i128)>,
    pub scale: u32,
}
pub(crate) fn select(
    count: usize,
    scale: u32,
    starts: &[Seed],
    ends: &[Seed],
    mut distance: impl FnMut(usize, u32) -> Result<u16>,
) -> Result<Vec<(usize, i128, i128)>> {
    let mut columns = Vec::with_capacity(count);
    for i in 0..count {
        let mut from = i128::MAX;
        let mut to = i128::MIN;
        for seed in starts {
            from = from.min(distance(i, seed.node)? as i128 * scale as i128 + seed.cost as i128);
        }
        for seed in ends {
            to = to.max(distance(i, seed.node)? as i128 * scale as i128 - seed.cost as i128);
        }
        columns.push((i, from, to));
    }
    columns.sort_by_key(|&(i, from, to)| (std::cmp::Reverse(from - to), i));
    columns.truncate(4);
    Ok(columns)
}
impl Prepared {
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

pub fn read_selected<T: TryFrom<i64>>(
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
