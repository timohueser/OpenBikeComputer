//! OBCR route format: reader + GPX converter.
//!
//! `no_std` so the exact same code runs in the desktop simulator and in the nRF
//! firmware. A route is a single ordered polyline with per-point elevation plus
//! precomputed ride stats — the route-planning sibling of the [`obcm_reader`] map
//! format. The on-disk layout is specified in `OBCR_Spec.md`.
//!
//! Modules:
//! - [`byte_io`] — the [`ByteSource`]/[`ByteSink`] abstractions over file / SD / USB
//!   bytes, plus a [`SliceSource`](byte_io::SliceSource) for in-memory bytes.
//! - [`reader`] — header / chunk-index parsing and on-demand chunk decode
//!   ([`RouteReader`], [`RouteSummary`], [`ChunkMeta`], [`RoutePoint`]).
//!
//! Coordinates are integer microdegrees (1e-6 degrees) like the map; distances and
//! elevations are whole meters. [`obcm_reader::BBox`] is reused for bounding boxes so
//! the renderer can compare a route chunk against the map [`Viewport`]'s bbox without
//! conversion.
//!
//! [`Viewport`]: obcm_render

#![no_std]

pub mod byte_io;
pub mod convert;
pub mod gpx;
pub mod reader;

pub use byte_io::{ByteSink, ByteSource, Error, SliceSource};
pub use convert::{gpx_to_obcr, RouteStats};
pub use gpx::{GpxScanner, RawPoint};
pub use reader::{
    ChunkMeta, RoutePoint, RouteReader, RouteSummary, CHUNK_META_LEN, HEADER_LEN,
    MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS, NAME_CAP,
};

pub use obcm_reader::BBox;
