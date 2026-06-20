//! The Stage-1 validation bridge: a JSON "tree dump" the Python oracle emits
//! (one already-built quadtree per LOD, geometry as raw f64 lon/lat), which this
//! crate re-serializes and byte-compares against `pack.py`'s `.obcm`. It isolates
//! the serializer from ingest/quadtree/GEOS so byte-parity is a meaningful gate.
//!
//! Emitted by `packer/tests/harness/dump_tree.py`; consumed by the
//! `serialize_from_dump` binary. Schema (see that script for the writer):
//!
//! ```json
//! { "marker_color": 63488, "global_bbox": [minlon,minlat,maxlon,maxlat],
//!   "styles": [{"id":1,"z_index":60,"color":64160,"weight":3,"priority":2}],
//!   "lods": [{"max_mpp": null, "chunk_size": 4096, "root": <node>}] }
//! node(leaf)   = {"bbox":[..4], "features":[{"style_id":2,"kind":"polygon",
//!                 "rings":[[[lon_bits,lat_bits],...], ...]}]}
//! node(branch) = {"bbox":[..4], "children":[node,node,node,node]}
//! ```
//!
//! Coordinates are the **u64 bit patterns** of the f64 lon/lat, not decimal text:
//! decimal round-trip is lossy (serde_json can land 1 ULP off Python, flipping a
//! `*1e6` halfway case), so bits keep the serializer test exact.

use serde::Deserialize;

use crate::serialize::{self, Feature, Kind, LodLayer, Node, Style};

#[derive(Deserialize)]
pub struct Dump {
    pub marker_color: u16,
    pub global_bbox: [i64; 4],
    pub styles: Vec<DumpStyle>,
    pub lods: Vec<DumpLod>,
}

#[derive(Deserialize)]
pub struct DumpStyle {
    pub id: u8,
    pub z_index: i8,
    pub color: u16,
    pub weight: u8,
    pub priority: u8,
}

#[derive(Deserialize)]
pub struct DumpLod {
    pub max_mpp: Option<f64>,
    pub chunk_size: usize,
    pub root: DumpNode,
}

#[derive(Deserialize)]
pub struct DumpNode {
    #[allow(dead_code)]
    pub bbox: [i64; 4],
    #[serde(default)]
    pub features: Option<Vec<DumpFeature>>,
    #[serde(default)]
    pub children: Option<Vec<DumpNode>>,
}

#[derive(Deserialize)]
pub struct DumpFeature {
    pub style_id: u8,
    pub kind: String, // "polygon" | "line"
    /// Each vertex is `[lon_bits, lat_bits]` — the exact f64 bit patterns.
    pub rings: Vec<Vec<(u64, u64)>>,
}

impl DumpNode {
    fn into_node(self) -> Node {
        if let Some(children) = self.children {
            let mut it = children.into_iter().map(DumpNode::into_node);
            let arr = [
                it.next().expect("branch needs 4 children"),
                it.next().expect("branch needs 4 children"),
                it.next().expect("branch needs 4 children"),
                it.next().expect("branch needs 4 children"),
            ];
            Node::Branch(Box::new(arr))
        } else {
            let features = self
                .features
                .unwrap_or_default()
                .into_iter()
                .map(|f| Feature {
                    style_id: f.style_id,
                    kind: if f.kind == "polygon" { Kind::Polygon } else { Kind::Line },
                    rings: f
                        .rings
                        .into_iter()
                        .map(|ring| {
                            ring.into_iter()
                                .map(|(lon, lat)| (f64::from_bits(lon), f64::from_bits(lat)))
                                .collect()
                        })
                        .collect(),
                })
                .collect();
            Node::Leaf { bbox: (self.bbox[0], self.bbox[1], self.bbox[2], self.bbox[3]), features }
        }
    }
}

impl Dump {
    /// Serialize this captured pyramid into `.obcm` bytes via the real serializer.
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
        let lods: Vec<LodLayer> = self
            .lods
            .into_iter()
            .map(|l| LodLayer {
                max_mpp: l.max_mpp,
                chunk_size: l.chunk_size,
                root: l.root.into_node(),
            })
            .collect();
        let bbox =
            (self.global_bbox[0], self.global_bbox[1], self.global_bbox[2], self.global_bbox[3]);
        serialize::serialize_lods(&lods, &styles, self.marker_color, bbox)
    }
}
