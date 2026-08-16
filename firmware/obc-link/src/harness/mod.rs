//! The fake-link transcript harness: one engine, two bindings, one transaction.
//!
//! This module is host-only (`std`). It exists so the engine can be driven exactly as a board would
//! drive it — records in, commands out, outcomes back — without a radio, a cable, or a card:
//!
//! - [`fake_link`] implements the [`ByteLink`](crate::engine::ByteLink) seam twice, with each
//!   binding's physical facts from §14 and nothing else;
//! - [`transaction`] is an in-memory transaction with the DOS2 lifecycle's shape, so slice 5 can
//!   swap the real kernel in without reshaping the engine;
//! - [`runner`] is the driver loop, generic over the link;
//! - [`transcript`] replays the checked-in semantic transcripts of
//!   `specs/vectors/device-object-v2/transcripts/` through both links.
//!
//! The transport-neutrality proof is mechanical: a scenario is run twice, once per link, and the
//! DOS records the engine emits must be byte-identical. Everything that differs between the two
//! runs is framing, which is exactly the line §14 draws.

pub mod fake_link;
pub mod runner;
pub mod transaction;
pub mod transcript;

#[cfg(test)]
mod tests;

pub use fake_link::{FakeBleLink, FakeLink, FakeUsbLink};
pub use runner::Driver;
pub use transaction::{FakeTransaction, Faults};
