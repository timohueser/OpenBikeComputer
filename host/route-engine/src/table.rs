use crate::{package::Package, package::Source, storage, Error, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};

pub const ENTRIES: usize = 4096;

/// A small directory of independently checked index blocks.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Table {
    pub len: u32,
    pub blocks: Vec<String>,
}

impl Table {
    pub fn write<T: Serialize>(
        values: &[T],
        write: &mut impl FnMut(&[u8]) -> std::result::Result<String, String>,
    ) -> std::result::Result<Self, String> {
        let len = u32::try_from(values.len()).map_err(|_| "Index exceeds u32 entries")?;
        let blocks = values
            .chunks(ENTRIES)
            .map(|page| write(&storage::encode(&page)?))
            .collect::<std::result::Result<_, _>>()?;
        Ok(Self { len, blocks })
    }

    pub fn valid(&self) -> bool {
        self.blocks.len() == (self.len as usize).div_ceil(ENTRIES) && self.blocks.iter().all(|key| valid_digest(key))
    }

    pub fn block_len(&self, block: usize) -> usize {
        (self.len as usize).saturating_sub(block * ENTRIES).min(ENTRIES)
    }
}

pub fn valid_digest(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub struct Cache<T>(VecDeque<(String, Arc<Vec<T>>)>);

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self(VecDeque::new())
    }
}

impl<T: DeserializeOwned> Cache<T> {
    pub fn block<S: Source>(&mut self, package: &Package<S>, table: &Table, block: usize) -> Result<Arc<Vec<T>>> {
        let key = table.blocks.get(block).ok_or_else(|| Error::InvalidData("Index block outside table".into()))?;
        let values = if let Some(at) = self.0.iter().position(|(id, _)| id == key) {
            self.0.remove(at).unwrap().1
        } else {
            Arc::new(package.read::<Vec<T>>(key)?)
        };
        if values.len() != table.block_len(block) {
            return Err(Error::InvalidData("Incomplete index block".into()));
        }
        self.0.push_back((key.clone(), values.clone()));
        while self.0.len() > 8 {
            self.0.pop_front();
        }
        Ok(values)
    }
}
