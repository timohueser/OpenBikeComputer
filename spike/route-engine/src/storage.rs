use crate::model::Point;
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};

pub const NODES_PER_PAGE: u32 = 128;
pub const ROADS_PER_PAGE: u32 = 128;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EdgeRef {
    pub node: u32,
    pub index: u32,
    pub backward: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Arc {
    pub to: u32,
    pub cost: u64,
    pub children: Option<[EdgeRef; 2]>,
    /// Original directed road reached by a leaf transition.
    pub road: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Node {
    pub forward: Vec<Arc>,
    pub backward: Vec<Arc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub first: u32,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Seed {
    pub node: u32,
    pub cost: u64,
    pub road: u32,
}

pub fn snap_cell(p: Point) -> (i32, i32) {
    (p.lat.div_euclid(50_000), p.lon.div_euclid(50_000))
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let raw = postcard::to_allocvec(value).map_err(|e| e.to_string())?;
    let mut writer = ZlibEncoder::new(Vec::new(), Compression::fast());
    writer.write_all(&raw).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let mut raw = Vec::new();
    ZlibDecoder::new(bytes).take((MAX_PAGE_BYTES + 1) as u64).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() > MAX_PAGE_BYTES {
        return Err("Page exceeds decode budget".into());
    }
    postcard::from_bytes(&raw).map_err(|e| e.to_string())
}

#[derive(Default)]
pub struct Cache {
    pub pages: HashMap<u32, Page>,
    pub decoded_bytes: usize,
}

impl Cache {
    pub fn insert_page(&mut self, id: u32, bytes: &[u8], limit: usize) -> Result<(), String> {
        if self.pages.contains_key(&id) {
            return Ok(());
        }
        let first = id.checked_mul(NODES_PER_PAGE).ok_or("Page id outside node range")?;
        let page: Page = decode(bytes)?;
        if page.first != first || page.nodes.len() > NODES_PER_PAGE as usize {
            return Err("Wrong graph page".into());
        }
        let size = std::mem::size_of::<Page>()
            + page.nodes.capacity() * std::mem::size_of::<Node>()
            + page
                .nodes
                .iter()
                .map(|n| (n.forward.capacity() + n.backward.capacity()) * std::mem::size_of::<Arc>())
                .sum::<usize>();
        if size > limit.saturating_sub(self.decoded_bytes) {
            return Err("Graph cache budget exceeded".into());
        }
        self.decoded_bytes += size;
        self.pages.insert(id, page);
        Ok(())
    }

    pub fn node(&self, node: u32) -> Option<&Node> {
        self.pages.get(&(node / NODES_PER_PAGE))?.nodes.get((node % NODES_PER_PAGE) as usize)
    }

    pub fn arc(&self, edge: EdgeRef) -> Option<&Arc> {
        let node = self.node(edge.node)?;
        if edge.backward {
            node.backward.get(edge.index as usize)
        } else {
            node.forward.get(edge.index as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_budget_is_exact_and_invalid_ids_do_not_wrap() {
        let bytes = encode(&Page { first: 0, nodes: vec![Node::default()] }).unwrap();
        let mut measured = Cache::default();
        measured.insert_page(0, &bytes, usize::MAX).unwrap();
        let size = measured.decoded_bytes;
        let mut cache = Cache::default();
        assert!(cache.insert_page(0, &bytes, size - 1).is_err());
        assert_eq!(cache.decoded_bytes, 0);
        assert!(cache.pages.is_empty());
        cache.insert_page(0, &bytes, size).unwrap();
        cache.insert_page(0, &bytes, size).unwrap();
        assert_eq!(cache.decoded_bytes, size);
        assert!(cache.insert_page(u32::MAX, &bytes, usize::MAX).is_err());
        assert!(cache.insert_page(1, &bytes, usize::MAX).is_err());
        assert!(decode::<Page>(&[0, 1, 2]).is_err());
    }
}
