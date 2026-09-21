//! The OBCM map packer: OSM `.osm.pbf` to `.obcm`. Shares one binary format definition with the
//! `no_std` reader (`obc-reader`).
//!
//! Geometry work — simplify, clip, multipolygon assembly — runs through libGEOS; the quadtree build
//! and serializer are deterministic integer and byte work. Feature selection is config-driven
//! ([`config`]).
//!
//! libGEOS is the only native dependency, and how it is supplied is a property of the build graph
//! rather than of this crate: the firmware workspace links the system library, while the desktop app
//! turns on `geos/static` so a shipped binary carries GEOS inside it. Everything else the packer
//! needs, HTTP and zip ([`net`]), is Rust on purpose.
//!
//! [`pipeline::pack`] is the whole thing end to end and the only entry point anyone should build a
//! map through: the binary is arg parsing around it, and the desktop app links this crate and calls
//! the same function. What a run says while it runs, and the token that stops it, live in
//! [`progress`].
//!
//! It also owns the two JSON contracts that hang off the packer: the config's schema ([`config`])
//! and the map-catalog manifest a bakery publishes ([`catalog`]).
// A `debug_assert!` whose arguments have side effects is a release-build hole: the macro does not
// evaluate them at all, so a `w.begin_section()?` tucked inside one silently stops writing filler in
// every shipping build while every debug test still passes.
#![warn(clippy::debug_assert_with_mut_call)]

pub mod catalog;
pub mod config;
pub mod contour;
pub mod coverage;
pub mod cut;
pub mod geom;
pub mod grid;
pub mod hours;
pub mod ingest;
pub mod land;
pub mod landmark_map;
pub mod landmarks;
pub mod merge;
pub mod nav;
pub mod net;
pub mod peak_map;
pub mod pipeline;
pub mod poi;
pub mod progress;
pub mod quadtree;
pub mod semantic;
pub mod serialize;
pub mod terrain;

pub use pipeline::{pack, PackOptions, PackSummary};
pub use progress::{CancelToken, PackError, Phase, Progress};

pub use serialize::{
    pack_chunk, pack_feature, pack_style_dict, serialize_lods, serialize_nav_section, serialize_poi_section,
    serialize_tree, validate_chunk_size, Feature, Kind, LodLayer, NavProfile, Node, Style, MAX_SAFE_CHUNK_SIZE,
    MIN_CHUNK_SIZE,
};
