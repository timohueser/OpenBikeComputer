//! `obc-pack` — the OBCM map packer: OSM `.osm.pbf` → `.obcm`. Shares one binary
//! format definition with the no_std reader (`obc-reader`).
//!
//! Geometry work (simplify, clip, multipolygon assembly) runs through libGEOS; the
//! quadtree build and serializer are deterministic integer/byte work. Feature
//! selection is config-driven ([`config`]).
//!
//! **libGEOS is the only native dependency**, and how it is supplied is a property
//! of the *build graph*, not of this crate: the `firmware/` workspace links the
//! system library (fast, and a developer has one), while `obc-desktop` turns on
//! `geos/static` so a shipped app carries GEOS inside its binary and asks a user to
//! install nothing (#907). Everything else the packer needs — HTTP and zip
//! ([`net`]) — is Rust, on purpose: an app cannot assume `curl`, and Windows has no
//! `unzip`.
//!
//! [`pipeline::pack`] is the whole thing end to end and the **only** entry point
//! anyone should build a map through: the `obc-pack` binary is arg parsing around
//! it, and the desktop app (#906) links this crate and calls the same function, so
//! neither can grow a pipeline the other doesn't have. What a run says while it
//! runs, and the token that stops it, live in [`progress`].
//!
//! It also owns the two JSON contracts that hang off the packer: the config's schema
//! ([`config`]) and the map-catalog manifest a bakery publishes ([`catalog`]).

pub mod catalog;
pub mod config;
pub mod cut;
pub mod geom;
pub mod grid;
pub mod hours;
pub mod ingest;
pub mod land;
pub mod merge;
pub mod nav;
pub mod net;
pub mod pipeline;
pub mod poi;
pub mod progress;
pub mod quadtree;
pub mod serialize;

pub use pipeline::{pack, PackOptions, PackSummary};
pub use progress::{CancelToken, PackError, Phase, Progress};

pub use serialize::{
    pack_chunk, pack_feature, pack_style_dict, serialize_lods, serialize_nav_section, serialize_poi_section,
    serialize_tree, validate_chunk_size, Feature, Kind, LodLayer, NavProfile, Node, Style, MAX_SAFE_CHUNK_SIZE,
    MIN_CHUNK_SIZE,
};
