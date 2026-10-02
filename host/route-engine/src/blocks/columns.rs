use crate::{
    base::{Column, Numbers, Width},
    package::{Package, Source},
    table::{Table, ENTRIES},
    Error, Result,
};
use serde::de::DeserializeOwned;

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
