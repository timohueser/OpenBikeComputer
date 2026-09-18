//! Settlement-name queries over the map's POI spatial index.

use super::Reader;
use crate::Error;
use obc_formats::io::rd_u16;
use obc_formats::obcm::{
    settlement_class_of, PoiMetadata, SettlementClass, SourceId, POI_NAME_LEN, POI_RECORD_LEN, SETTLEMENT_CATEGORY_ID,
    SETTLEMENT_POPULATION_UNKNOWN,
};
use obc_map_scene::BBox;

/// A settlement with its map coordinates, class and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settlement {
    pub source: SourceId,
    pub lat: i32,
    pub lon: i32,
    pub class: SettlementClass,
    /// The stored UTF-8 name. It is never empty.
    pub name: heapless::String<POI_NAME_LEN>,
    /// People, when the map holds a value.
    pub population: Option<u32>,
}

impl Reader<'_> {
    /// Visit the settlements whose coordinates are inside `view`.
    ///
    /// Only the leaves that meet `view` are read, through a 512-byte scratch buffer. The caller
    /// owns each result and chooses its own label budget and ranking. The query order is spatial,
    /// not sorted. A map without settlement data gives no results. A malformed record is dropped
    /// on its own; a read failure ends the query with `Err`.
    pub fn visit_settlements_in(&self, view: &BBox, mut visit: impl FnMut(Settlement)) -> Result<(), Error> {
        let dir = &self.tables.pois;
        let Some(entry) = dir.entries.iter().find(|e| e.category_id == SETTLEMENT_CATEGORY_ID && !e.is_empty()) else {
            return Ok(());
        };
        if dir.chunk_size < POI_RECORD_LEN {
            return Ok(());
        }
        self.scan_poi_leaves(entry, dir.chunk_size, view, |start, record_cap| {
            self.stream_poi_records(start, record_cap, |bytes, off, lat, lon, subtype| {
                let Some(class) = settlement_class_of(subtype) else { return };
                // A leaf box is larger than the view, so check the record itself.
                if lat < view.min_lat || lat > view.max_lat || lon < view.min_lon || lon > view.max_lon {
                    return;
                }
                let len = bytes[off + 9] as usize;
                if len == 0 || len > POI_NAME_LEN {
                    return;
                }
                let Ok(name) = core::str::from_utf8(&bytes[off + 10..off + 10 + len]) else { return };
                if name.chars().any(char::is_control) {
                    return;
                }
                let payload = rd_u16(bytes, off + 34);
                let population = (payload != SETTLEMENT_POPULATION_UNKNOWN).then(|| u32::from(payload) * 100);
                let Some(metadata) = PoiMetadata::decode(&bytes[off + 36..off + 64]) else { return };
                visit(Settlement {
                    source: metadata.source,
                    lat,
                    lon,
                    class,
                    name: heapless::String::try_from(name).expect("name fits its stored field"),
                    population,
                });
            })
        })
    }
}
