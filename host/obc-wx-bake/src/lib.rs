//! The OBC weather bakery (WX5, epic #1185).
//!
//! A small stateless service: fetch upstream radar/model products, decode them against the
//! WX1-pinned source contracts, resample nearest-neighbour onto fixed regular lat/lon windows,
//! quantize with the shared WX2 intensity table, tile into OBCG objects, and publish them to R2
//! with an atomically swapped manifest. Upstream weather formats are parsed **here and nowhere
//! else** — never on the phone, never in firmware.
//!
//! Layer map (WX1's prescribed boundaries):
//! - [`source`] — one adapter per upstream; the only provider-aware code.
//! - [`grib`], [`idx`], [`stereo`], [`lcc`] — pinned decode/selection/projection primitives the
//!   adapters share.
//! - [`emit`] — cell grids → OBCG bytes through the `obc-formats` byte authority.
//! - [`manifest`] — the `wx/v1/manifest.json` model and its pinned JSON Schema.
//! - [`publish`] — frames-first, manifest-last object stores (directory and R2).
//! - [`cycle`] — the idempotent orchestrator a systemd timer invokes.

pub mod cycle;
pub mod emit;
pub mod fetch;
pub mod geometry;
pub mod grib;
pub mod idx;
pub mod lcc;
pub mod manifest;
pub mod publish;
pub mod source;
pub mod stereo;
