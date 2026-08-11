//! The OBC weather bakery (WX5, epic #1185).
//!
//! A small stateless service: fetch upstream radar/model products, decode them against the
//! WX1-pinned source contracts, resample nearest-neighbour onto fixed regular lat/lon windows,
//! quantize with the shared WX2 intensity table, tile into OBCG objects, and publish them to R2
//! with an atomically swapped manifest. Upstream weather formats are parsed **here and nowhere
//! else** — never on the phone, never in firmware.
//!
//! Layer map (WX1's prescribed boundaries):
//! - [`source`] — one adapter per upstream; the only provider-aware code, and the ordered
//!   `MOSAIC_PRIORITY` table that says which source wins a cell where two overlap.
//! - [`grib`], [`idx`], [`tiff`], [`stereo`], [`lcc`], [`laea`] — pinned decode/selection/
//!   projection primitives the adapters share.
//! - [`canonical`] — the lattice, the priority mosaic, the sharded emit and **the cycle** (WXR3
//!   #1242): one global 0.01 degree / 15-minute dataset, no providers, no tiers, no resolutions,
//!   and since #1246 the only thing the bakery publishes.
//! - [`manifest_v2`] — `wx/v2/manifest.json`: the dataset's manifest (WXR4 #1243), with nothing
//!   selectable in it — generation, grid constants, shard presence and deadlines.
//! - [`timefmt`] — the one UTC formatting convention every timestamp and key segment uses.
//! - [`publish`] — objects-first, manifest-last object stores (directory and R2).
//! - [`sweep`] — retention (WXR8 #1247): the one destructive path, deleting the generations the
//!   manifest just published no longer names, and only after it is durably in place.
//!
//! One thing here is *not* the service: [`pack`] freezes a real past event — raw archive bytes,
//! the tree the real baker makes of them, and what actually happened next — so the simulator and
//! the tests can run against real radar. It is driven by the `obc-wx-pack` binary and nothing in
//! the bakery depends on it.

pub mod canonical;
pub mod fetch;
pub mod geometry;
pub mod grib;
pub mod idx;
pub mod laea;
pub mod lcc;
pub mod manifest_v2;
pub mod pack;
pub mod publish;
pub mod s3;
pub mod source;
pub mod stereo;
pub mod sweep;
pub mod tiff;
pub mod timefmt;
