//! Thin browser benchmark adapter; the host fetches only pages requested by the search.
use crate::search::Search;
use crate::storage::{Cache, Seed};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct BrowserSearch {
    cache: Cache,
    search: Search,
}

#[wasm_bindgen]
impl BrowserSearch {
    #[wasm_bindgen(constructor)]
    pub fn new(start_rank: u32, start_road: u32, end_rank: u32, end_road: u32, max_labels: u32) -> Self {
        Self {
            cache: Cache::default(),
            search: Search::new(
                &[Seed { node: start_rank, cost: 0, road: start_road }],
                &[Seed { node: end_rank, cost: 0, road: end_road }],
                max_labels as usize,
            ),
        }
    }

    pub fn insert_page(&mut self, id: u32, bytes: &[u8]) -> Result<(), JsValue> {
        self.cache.insert_page(id, bytes, 256 * 1024 * 1024).map_err(|e| JsValue::from_str(&e))
    }

    pub fn poll(&mut self, work: u32) -> Result<String, JsValue> {
        serde_json::to_string(&self.search.poll(&self.cache, work as usize))
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn cancel(&mut self) {
        self.search.cancel();
    }

    pub fn decoded_cache_bytes(&self) -> usize {
        self.cache.decoded_bytes
    }
}
