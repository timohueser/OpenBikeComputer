//! Resumable viewport walk. One step reads one index node or at most 512 record bytes.
use super::{MapPoint, QuadCursor, QuadStep, Reader};
use crate::{Error, PoiCategory, PoiCategorySet};
use obc_formats::{
    io::{rd_i32, rd_u16},
    obcm::{
        poi_directory_category_of, PoiMetadata, POI_RECORD_LEN, SUMMIT_CATEGORY_ID, SUMMIT_ELEVATION_UNKNOWN,
        SUMMIT_SUBTYPE_ID,
    },
};
use obc_map_scene::BBox;

pub struct MapPointQuery {
    generation: u32,
    bounds: BBox,
    categories: PoiCategorySet,
    summits: bool,
    category: usize,
    cursor: QuadCursor,
    started: bool,
    leaf: Option<(u32, usize)>,
}
impl MapPointQuery {
    pub fn new(generation: u32, bounds: BBox, categories: PoiCategorySet, summits: bool) -> Self {
        Self {
            generation,
            bounds,
            categories,
            summits,
            category: 0,
            cursor: QuadCursor::default(),
            started: false,
            leaf: None,
        }
    }
    /// Returns true when complete. A failed query must be discarded by its owner.
    pub fn step(&mut self, reader: &Reader, mut visit: impl FnMut(MapPoint)) -> Result<bool, Error> {
        if reader.generation() != self.generation {
            return Err(Error::BadOffset);
        }
        let Some(entry) = reader.tables.pois.entries.get(self.category) else {
            return Ok(true);
        };
        let selected = if entry.category_id == SUMMIT_CATEGORY_ID {
            self.summits
        } else {
            PoiCategory::from_id(entry.category_id).is_some_and(|cat| self.categories.contains(cat))
        };
        if !selected || entry.is_empty() {
            self.category += 1;
            return Ok(false);
        }
        if let Some((leaf, cursor)) = self.leaf {
            let size = reader.tables.pois.chunk_size;
            let (start, end) = entry.chunk_range(leaf, size).ok_or(Error::BadOffset)?;
            if end > reader.src.len() || size < POI_RECORD_LEN {
                return Err(Error::BadOffset);
            }
            let take = (size / POI_RECORD_LEN - cursor).min(512 / POI_RECORD_LEN);
            let mut scratch = [0; 512];
            reader
                .src
                .read_at(start + (cursor * POI_RECORD_LEN) as u64, &mut scratch[..take * POI_RECORD_LEN])
                .map_err(Error::Source)?;
            let mut ended = cursor + take == size / POI_RECORD_LEN;
            for bytes in scratch[..take * POI_RECORD_LEN].as_chunks::<POI_RECORD_LEN>().0 {
                let subtype = bytes[8];
                if subtype == 0xff {
                    ended = true;
                    break;
                }
                if poi_directory_category_of(subtype) != Some(entry.category_id) {
                    return Err(Error::BadOffset);
                }
                let position = (rd_i32(bytes, 4), rd_i32(bytes, 0));
                if position.0 < self.bounds.min_lon
                    || position.0 > self.bounds.max_lon
                    || position.1 < self.bounds.min_lat
                    || position.1 > self.bounds.max_lat
                {
                    continue;
                }
                let metadata = PoiMetadata::decode(&bytes[36..64]).ok_or(Error::BadOffset)?;
                let elevation = rd_u16(bytes, 34) as i16;
                visit(MapPoint {
                    position,
                    source: metadata.source,
                    subtype,
                    elevation_m: (subtype == SUMMIT_SUBTYPE_ID && elevation != SUMMIT_ELEVATION_UNKNOWN)
                        .then_some(elevation),
                });
            }
            if ended {
                self.leaf = None;
            } else {
                self.leaf = Some((leaf, cursor + take));
            }
            return Ok(false);
        }
        if !self.started {
            self.cursor.reset();
            self.started = true;
        }
        match self
            .cursor
            .step(reader.bbox, &self.bounds, entry.node_count, |index| reader.read_node(entry, index))
            .map_err(|e| Error::from(e.error))?
        {
            QuadStep::Leaf(chunk, _) => self.leaf = Some((chunk, 0)),
            QuadStep::Done => {
                self.category += 1;
                self.started = false;
            }
            QuadStep::Pending => {}
        }
        Ok(false)
    }
}
