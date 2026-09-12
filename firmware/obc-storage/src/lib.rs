//! Board-agnostic `no_std` storage mechanics and adapters.
//!
//! Split out of `obc-platform` (issue #807) so the SD stack (`embedded-sdmmc`) is pulled by exactly
//! the crate that needs it, and the adapters depend downward on the format seam — never on the
//! renderer or UI. The adapters are generic over
//! `embedded_sdmmc`'s [`BlockDevice`](embedded_sdmmc::BlockDevice) + [`TimeSource`](embedded_sdmmc::TimeSource),
//! so they carry no board/bus types; a board crate picks the concrete `SdCard<SpiDevice, _>`.
//!
//! ## Responsibility / dependency table
//!
//! | Module | Owns | Depends on |
//! |---|---|---|
//! | [`flat`] | the flat card store: the whole raw-card format and the five-operation `Store` seam, over a 512-byte block device and nothing else — no partition table, no filesystem — plus [`flat::StoreSource`], the one adapter that presents an open object as a [`ByteSource`](obc_formats::io::ByteSource) | `obc-crc`, `obc-formats` |
//! | [`sd`] | FatFs [`ByteSource`](obc_formats::io::ByteSource)/[`ByteSink`](obc_formats::io::ByteSink) adapters over an [`embedded_sdmmc`] volume | `obc-formats`, `embedded-sdmmc` |
//! | [`ObjectIdSequence`] | monotonic durable-object id candidate, recovery, commit, and persisted-floor handoff | none |

#![no_std]

// Only tests and the flat store's host-only faulting card and reference model use `std`.
#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod flat;
mod object_id;
pub mod sd;
pub mod shared_device;

pub use object_id::ObjectIdSequence;
pub use sd::{SdByteSink, SdByteSource};
