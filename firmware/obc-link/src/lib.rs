//! The protocol-v4 codec, device engine and optional host client.
//!
//! [`flat`] implements the [`FLAT store protocol`](../../../specs/FLAT_Store_Protocol.md):
//! control and stream records, the store seam, and one transport-free transfer engine.
//! The device codec and engine are allocation-free. Host-only fixture production is enabled by
//! `std`; `client` enables the transport-free host client with owned records and catalog results.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(any(test, feature = "std"))]
extern crate std;

#[cfg(feature = "client")]
extern crate alloc;

pub mod flat;
