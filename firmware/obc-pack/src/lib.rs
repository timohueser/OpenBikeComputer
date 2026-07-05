//! `obc-pack` — the OBCM map packer: OSM `.osm.pbf` → `.obcm`. Shares one binary
//! format definition with the no_std reader (`obc-reader`).
//!
//! Geometry work (simplify, clip, multipolygon assembly) runs through system GEOS;
//! the quadtree build and serializer are deterministic integer/byte work. Feature
//! selection is config-driven ([`config`]).

pub mod config;
pub mod geom;
pub mod ingest;
pub mod land;
pub mod poi;
pub mod quadtree;
pub mod serialize;

pub use serialize::{
    pack_chunk, pack_feature, pack_style_dict, serialize_lods, serialize_poi_section, serialize_tree,
    validate_chunk_size, Feature, Kind, LodLayer, Node, Style, MAX_SAFE_CHUNK_SIZE, MIN_CHUNK_SIZE,
};
