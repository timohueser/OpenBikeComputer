//! The device's protocol-v4 codec and transfer engine.
//!
//! [`flat`] implements the [`FLAT store protocol`](../../../specs/FLAT_Store_Protocol.md):
//! control and stream records, the store seam, and one transport-free transfer engine.
//! The codec and engine are allocation-free. Host-only fixture production is enabled by `std`.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod flat;
