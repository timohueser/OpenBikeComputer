//! Board-agnostic `no_std` storage mechanics and adapters.
//!
//! [`health`] is the transport's consecutive-failure latch: the rule that says when a board stops
//! paying a dead card's deadlines. [`flat`] is the raw-card store: the whole card format and the
//! five-operation `Store` seam over a 512-byte block device, with no partition table and no
//! filesystem. [`sd`] is the FatFs
//! [`ByteSource`](obc_formats::io::ByteSource) / [`ByteSink`](obc_formats::io::ByteSink) adapters
//! over an [`embedded_sdmmc`] volume. Both are generic over the card's own types, so a board crate
//! picks the concrete device. [`ObjectIdSequence`] is the monotonic durable-object id band.

#![no_std]

// Only tests and the flat store's host-only faulting card and reference model use `std`.
#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod flat;
pub mod health;
mod object_id;
pub mod sd;
pub mod shared_device;

pub use object_id::ObjectIdSequence;
pub use sd::{SdByteSink, SdByteSource};
