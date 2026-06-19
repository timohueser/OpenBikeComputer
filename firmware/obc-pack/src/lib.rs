//! `obc-pack` — a std host port of the OBCM packer (`packer/obcm/*`): OSM
//! `.osm.pbf` → `.obcm`. Lives in the firmware workspace so it shares one
//! definition of the binary format with the no_std reader (`obc-reader`), which
//! reads back everything this writes.
//!
//! ## Validation strategy (why not byte-identical end-to-end)
//!
//! The port is validated against the Python pipeline (the *oracle*,
//! `packer/pack.py`), but **not** by byte-identity of the whole file. Two things
//! make that impractical: shapely links GEOS 3.13.1 while the system `geos` the
//! Rust side binds is 3.14.1 (simplify/intersection diverge in the last digits),
//! and feature/ring ordering is not reproduced exactly. So the gate is
//! **feature-multiset equivalence + render-diff**, with every difference
//! explained — see `packer/tests/corpus/README.md`.
//!
//! Byte-identity is still used where it *is* achievable and sharp: the
//! [`serialize`] module, fed the same captured quadtree (via [`dump`]), matches
//! the oracle exactly. That is the Stage-1 deliverable.
//!
//! The port also **fixes** a real oracle bug rather than replicating it: closed
//! line-ways (e.g. `highway=residential` loops) are emitted as lines only, not
//! also as filled polygons. See the plan's Amendments section.

pub mod config;
pub mod dump;
pub mod feature_dump;
pub mod geom;
pub mod ingest;
pub mod quadtree;
pub mod serialize;

pub use serialize::{
    pack_chunk, pack_feature, pack_style_dict, serialize_lods, serialize_tree, Feature, Kind,
    LodLayer, Node, Style,
};
