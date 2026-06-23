//! `obc-pack` — the OBCM map packer: OSM `.osm.pbf` → `.obcm`. Lives in the
//! firmware workspace so it shares one definition of the binary format with the
//! no_std reader (`obc-reader`), which reads back everything this writes.
//!
//! The geometry work (simplify, clip, multipolygon assembly) runs through the
//! system GEOS; the quadtree build and the serializer are deterministic
//! integer/byte work. Feature selection is config-driven — see [`config`].
//!
//! Deliberate correctness rule: a closed line-way (e.g. a `highway=residential`
//! loop) is emitted as a line only, never also as a filled polygon.

pub mod config;
pub mod geom;
pub mod ingest;
pub mod land;
pub mod quadtree;
pub mod serialize;

pub use serialize::{
    pack_chunk, pack_feature, pack_style_dict, serialize_lods, serialize_tree, validate_chunk_size, Feature, Kind,
    LodLayer, Node, Style, MAX_SAFE_CHUNK_SIZE,
};
