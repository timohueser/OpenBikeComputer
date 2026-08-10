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
//! - [`canonical`] — the canonical lattice, the priority mosaic and the sharded emit (WXR3
//!   #1242): one global 0.01 degree / 15-minute dataset, no providers, no tiers, no resolutions.
//! - [`emit`] — cell grids → OBCG bytes through the `obc-formats` byte authority.
//! - [`manifest`] — the `wx/v1/manifest.json` model and its pinned JSON Schema.
//! - [`publish`] — frames-first, manifest-last object stores (directory and R2).
//! - [`cycle`] — the idempotent orchestrator a systemd timer invokes.
//!
//! Two things here are *not* the service. [`pack`] freezes a real past event — raw archive bytes,
//! the tree the real baker makes of them, and what actually happened next — so the simulator and
//! the tests can run against real radar. It is driven by the `obc-wx-pack` binary and nothing in
//! the bakery depends on it. [`spike`] is the WXR1 (#1240) measurement harness: throwaway, off the
//! checked-in fixtures, reachable only through the `spike` subcommand, and deleted by WXR7.

pub mod canonical;
pub mod cycle;
pub mod emit;
pub mod fetch;
pub mod geometry;
pub mod grib;
pub mod idx;
pub mod laea;
pub mod lcc;
pub mod manifest;
pub mod pack;
pub mod publish;
pub mod source;
pub mod spike;
pub mod stereo;
pub mod tiff;
