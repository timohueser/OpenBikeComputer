//! Board-agnostic `no_std` storage mechanics.
//!
//! [`health`] is the transport's consecutive-failure latch: the rule that says when a board stops
//! paying a dead card's deadlines. [`flat`] is the raw-card store: the whole card format and the
//! five-operation `Store` seam over a 512-byte block device, with no partition table and no
//! filesystem. [`ObjectIdSequence`] is the monotonic durable-object id band.

#![no_std]

// Only tests and the flat store's host-only faulting card and reference model use `std`.
#[cfg(any(test, feature = "std"))]
extern crate std;

pub mod flat;
pub mod health;
mod object_id;

pub use object_id::ObjectIdSequence;
