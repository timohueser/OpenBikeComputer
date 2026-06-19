//! OBCR route format: reader + GPX converter.
//!
//! `no_std` so the exact same code runs in the desktop simulator and in the nRF
//! firmware. A route is a single ordered polyline with per-point elevation plus
//! precomputed ride stats — the route-planning sibling of the [`obc_reader`] map
//! format. The on-disk layout is specified in `OBCR_Spec.md`.
//!
//! Modules:
//! - [`byte_io`] — the [`ByteSource`]/[`ByteSink`] abstractions over file / SD / USB
//!   bytes, plus a [`SliceSource`](byte_io::SliceSource) for in-memory bytes.
//! - [`reader`] — header / chunk-index parsing and on-demand chunk decode
//!   ([`RouteReader`], [`RouteSummary`], [`ChunkMeta`], [`RoutePoint`]).
//! - [`profile`] — a route's elevation sampled to a fixed-width [`Profile`] for the
//!   Elevation screen's band + cursor + peak label.
//!
//! Coordinates are integer microdegrees (1e-6 degrees) like the map; distances and
//! elevations are whole meters. [`obc_reader::BBox`] is reused for bounding boxes so
//! the renderer can compare a route chunk against the map [`Viewport`]'s bbox without
//! conversion.
//!
//! [`Viewport`]: obc_render

#![no_std]

pub mod byte_io;
pub mod convert;
mod geo;
pub mod gpx;
pub mod matcher;
pub mod profile;
pub mod reader;
pub mod track;

pub use byte_io::{ByteSink, ByteSource, Error, SliceSource};
pub use convert::{gpx_to_obcr, RouteStats};
pub use geo::ground_dist_m;
pub use gpx::{GpxScanner, RawPoint};
pub use matcher::{Match, RouteMatch};
pub use profile::{Profile, Window, PROFILE_COLS};
pub use track::{decode_record, encode_record, track_to_gpx, TrackPoint, TRACK_RECORD_LEN};
pub use reader::{
    ChunkMeta, RoutePoint, RouteReader, RouteSummary, CHUNK_META_LEN, HEADER_LEN,
    MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS, NAME_CAP,
};

pub use obc_reader::BBox;
