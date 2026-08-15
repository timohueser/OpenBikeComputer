//! Board-agnostic `no_std` **storage adapters** — the reusable half of "map/route/track on SD".
//!
//! Split out of `obc-platform` (issue #807) so the SD stack (`embedded-sdmmc`) is pulled by exactly
//! the crate that needs it, and the adapters depend downward on the format/port seams
//! (`obc-route`/`obc-ports`) — never on the renderer or UI. The adapters are generic over
//! `embedded_sdmmc`'s [`BlockDevice`](embedded_sdmmc::BlockDevice) + [`TimeSource`](embedded_sdmmc::TimeSource),
//! so they carry no board/bus types; a board crate picks the concrete `SdCard<SpiDevice, _>`.
//!
//! ## Responsibility / dependency table
//!
//! | Module | Owns | Depends on |
//! |---|---|---|
//! | [`sd`] | FatFs [`ByteSource`](obc_formats::io::ByteSource)/[`ByteSink`](obc_formats::io::ByteSink) and [`TrackSink`](obc_ports::TrackSink) adapters over an [`embedded_sdmmc`] volume — the general seek-per-read path plus the track record encode | `obc-formats`, `obc-ports`, `embedded-sdmmc` |
//! | [`fat_extents`] | the map file's FAT chain resolved once into extent runs → direct-block `read_at` (#500): the fast path for the one big read-only file (`.obcm`) whose scattered reads dominate | `embedded-sdmmc` |
//! | [`ObjectIdSequence`] | monotonic durable-object id candidate, recovery, commit, and persisted-floor handoff | none |
//! | [`RideCatalog`] | fixed-capacity aligned stored-ride id/filename ownership and scan mutation policy | `embedded-sdmmc`, `heapless` |
//! | [`route_name`] | classify uploaded/side-loaded route filenames and durable ids | none |
//! | [`trip_name`] | classify uploaded/side-loaded trip filenames and durable ids | none |
//! | [`TripCatalog`] | fixed-capacity aligned trip id/file/metadata ownership and scan mutation policy | `embedded-sdmmc`, `heapless` |
//! | [`weather`] | transport-neutral inactive-slot upload transaction: running outer CRC, held magic, canonical post-close validation and magic-flush commit | `obc-weather`, `obc-crc` |

#![no_std]

#[cfg(test)]
extern crate std;

pub mod fat_extents;
mod object_id;
mod object_name;
mod ride_catalog;
pub mod route_name;
pub mod sd;
mod trip_catalog;
pub mod trip_name;
pub mod weather;

pub use fat_extents::{ExtentSource, ExtentSourceWithCapacity, ExtentTable, ExtentTableWithCapacity};
pub use object_id::ObjectIdSequence;
pub use ride_catalog::{RideCatalog, RideRemoveAction, RideScanAction, RideScanRead};
pub use sd::{SdByteSink, SdByteSource, SdTrackSink};
pub use trip_catalog::{trip_crc, TripCatalog, TripRemoveAction, TripScanAction, TripScanRead};
