use crate::{package::Package, package::Source, storage, Error, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};

pub const ENTRIES: usize = 4096;

/// A small directory of independently checked index blocks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub len: u32,
    pub blocks: Vec<String>,
    /// Published selections retain source page numbers without retaining absent page descriptors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<u32>>,
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
        Ok(Self { len, blocks, pages: None })
    }

    pub fn valid(&self) -> bool {
        let count = (self.len as usize).div_ceil(ENTRIES);
        self.blocks.iter().all(|key| valid_digest(key))
            && match &self.pages {
                None => self.blocks.len() == count,
                Some(pages) => {
                    pages.len() == self.blocks.len()
                        && pages.windows(2).all(|p| p[0] < p[1])
                        && pages.last().is_none_or(|&p| (p as usize) < count)
                }
            }
    }

    pub fn key(&self, page: usize) -> Result<&String> {
        let index = match &self.pages {
            Some(pages) => pages
                .binary_search(&u32::try_from(page).map_err(|_| Error::Limit)?)
                .map_err(|_| Error::MissingRegion(format!("Index page {page}")))?,
            None => page,
        };
        self.blocks.get(index).ok_or_else(|| Error::InvalidData("Index block outside table".into()))
    }

    pub fn positions(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.blocks.len()).map(|i| self.pages.as_ref().map_or(i, |pages| pages[i] as usize))
    }

    pub fn retain(&mut self, pages: &std::collections::BTreeSet<usize>) -> Result<()> {
        let blocks = pages.iter().map(|&page| self.key(page).cloned()).collect::<Result<Vec<_>>>()?;
        self.pages =
            Some(pages.iter().map(|&page| u32::try_from(page).map_err(|_| Error::Limit)).collect::<Result<_>>()?);
        self.blocks = blocks;
        Ok(())
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
        let key = table.key(block)?;
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
