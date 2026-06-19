//! Stage-2 validation bridge: a JSON dump of the **pre-quadtree** per-LOD feature
//! lists (post-simplify), emitted by `packer/tests/harness/dump_features.py`. The
//! Rust side builds its own quadtree from these and serializes, isolating the
//! quadtree+clip port from ingest and simplify (which stay Python here). See the
//! `build_from_features` binary.
//!
//! Coordinates are exact f64 bit patterns, like [`crate::dump`].

use serde::Deserialize;

use crate::dump::DumpStyle;
use crate::geom::Geom;
use crate::quadtree::build_lod;
use crate::serialize::{serialize_lods, LodLayer, Style};

#[derive(Deserialize)]
pub struct FeatureDump {
    pub marker_color: u16,
    pub global_bbox: [i64; 4],
    pub chunk_size: usize,
    pub styles: Vec<DumpStyle>,
    pub lods: Vec<FeatureLod>,
}

#[derive(Deserialize)]
pub struct FeatureLod {
    pub max_mpp: Option<f64>,
    pub features: Vec<FeatureEntry>,
}

#[derive(Deserialize)]
pub struct FeatureEntry {
    pub style_id: u8,
    pub kind: String, // "polygon" | "line"
    pub rings: Vec<Vec<(u64, u64)>>,
}

impl FeatureEntry {
    fn into_geom(self) -> (u8, Geom) {
        let mut rings = self.rings.into_iter().map(|ring| {
            ring.into_iter().map(|(x, y)| (f64::from_bits(x), f64::from_bits(y))).collect::<Vec<_>>()
        });
        if self.kind == "polygon" {
            let exterior = rings.next().unwrap_or_default();
            let interiors = rings.collect();
            (self.style_id, Geom::Polygon { exterior, interiors })
        } else {
            (self.style_id, Geom::Line(rings.next().unwrap_or_default()))
        }
    }
}

impl FeatureDump {
    /// Build a quadtree per LOD from these features and serialize to `.obcm`.
    pub fn to_obcm(self) -> Vec<u8> {
        let styles: Vec<Style> = self
            .styles
            .iter()
            .map(|s| Style {
                id: s.id,
                z_index: s.z_index,
                color: s.color,
                weight: s.weight,
                priority: s.priority,
            })
            .collect();
        let bbox = (self.global_bbox[0], self.global_bbox[1], self.global_bbox[2], self.global_bbox[3]);
        let chunk_size = self.chunk_size;
        let lods: Vec<LodLayer> = self
            .lods
            .into_iter()
            .map(|l| {
                let root = build_lod(l.features.into_iter().map(FeatureEntry::into_geom), bbox, chunk_size);
                LodLayer { max_mpp: l.max_mpp, chunk_size, root }
            })
            .collect();
        serialize_lods(&lods, &styles, self.marker_color, bbox)
    }
}
