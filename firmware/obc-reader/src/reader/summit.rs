//! Named summit queries over the map's existing POI spatial index.

use super::Reader;
use crate::Error;
use obc_formats::io::rd_u16;
use obc_formats::obcm::{
    POI_NAME_LEN, POI_RECORD_LEN, SUMMIT_CATEGORY_ID, SUMMIT_ELEVATION_UNKNOWN, SUMMIT_SUBTYPE_ID,
};
use obc_map_scene::{cos_lat, ground_dist_m_cl, BBox, M_PER_DEG};

/// Maximum distance searched for panorama labels.
pub const MAX_SUMMIT_RADIUS_M: u32 = 100_000;

/// A named summit with the original map coordinates and optional signed height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summit {
    pub source: obc_formats::obcm::SourceId,
    pub lat: i32,
    pub lon: i32,
    pub name: heapless::String<POI_NAME_LEN>,
    pub elevation_m: Option<i16>,
    pub distance_m: u32,
}

impl Reader<'_> {
    /// Visit named summits within `radius_m` of `(lon, lat)` in microdegrees.
    ///
    /// The radius is capped at 100 km. Only intersecting summit leaves are read, through
    /// a 512-byte scratch buffer. The callback owns each result and chooses its own label
    /// budget and ranking; query order is spatial, not distance order. Maps without summit
    /// data produce no results. Distances use the same local ground projection as POI queries.
    pub fn visit_summits_within(
        &self,
        pos: (i32, i32),
        radius_m: u32,
        mut visit: impl FnMut(Summit),
    ) -> Result<(), Error> {
        let dir = &self.tables.pois;
        let Some(entry) = dir.entries.iter().find(|e| e.category_id == SUMMIT_CATEGORY_ID && !e.is_empty()) else {
            return Ok(());
        };
        if dir.chunk_size < POI_RECORD_LEN {
            return Ok(());
        }
        let radius = radius_m.min(MAX_SUMMIT_RADIUS_M) as f32;
        let cl = cos_lat(pos.1).max(1e-3);
        let half_lat = (radius * (1_000_000.0 / M_PER_DEG as f32)) as i32 + 1;
        let half_lon = (half_lat as f32 / cl) as i32 + 1;
        let search = BBox {
            min_lon: pos.0.saturating_sub(half_lon),
            max_lon: pos.0.saturating_add(half_lon),
            min_lat: pos.1.saturating_sub(half_lat),
            max_lat: pos.1.saturating_add(half_lat),
        };
        self.scan_poi_leaves(entry, dir.chunk_size, &search, |start, record_cap| {
            self.stream_poi_records(start, record_cap, |bytes, off, lat, lon, subtype| {
                if subtype != SUMMIT_SUBTYPE_ID {
                    return;
                }
                let distance = ground_dist_m_cl(pos, (lon, lat), cl);
                if distance > radius {
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
                let elevation = rd_u16(bytes, off + 34) as i16;
                let Some(metadata) = obc_formats::obcm::PoiMetadata::decode(&bytes[off + 36..off + 64]) else { return };
                visit(Summit {
                    source: metadata.source,
                    lat,
                    lon,
                    name: heapless::String::try_from(name).expect("name fits its stored field"),
                    elevation_m: (elevation != SUMMIT_ELEVATION_UNKNOWN).then_some(elevation),
                    distance_m: (distance + 0.5) as u32,
                });
            })
        })
    }
}
