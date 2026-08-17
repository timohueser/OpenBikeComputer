//! The fake-link transcript harness: one engine, two bindings, one transaction.
//!
//! This module is host-only (`std`). It exists so the engine can be driven exactly as a board would
//! drive it — records in, commands out, outcomes back — without a radio, a cable, or a card:
//!
//! - [`fake_link`] implements the [`ByteLink`](crate::engine::ByteLink) seam twice, with each
//!   binding's physical facts from §14 and nothing else;
//! - [`transaction`] is an in-memory transaction with the DOS2 lifecycle's shape — one of the two
//!   implementations of the [`Transaction`](crate::engine::Transaction) seam, and the one that
//!   needs no card;
//! - [`runner`] is the driver loop, generic over the link **and** over the transaction;
//! - [`scenarios`] is the suite itself, written once and generic over a [`Fixture`]. That is what
//!   makes the harness more than this crate's own test: `obc-storage` runs the identical list
//!   against its kernel-backed transaction, so a divergence between the fake and the real store is
//!   a failing scenario rather than a surprise on a device;
//! - [`transcript`] replays the checked-in semantic transcripts of
//!   `specs/vectors/device-object-v2/transcripts/` through both links. All eleven are held to their
//!   framing and their decoding; **one** — the end-to-end create/upload/publish/download flow — is
//!   driven through the engine from its first event, and the abort transcript's *semantics* are
//!   reproduced from a preamble, with the two rows the restart-only profile changes asserted rather
//!   than skipped. The other nine name the state or profile they would need in
//!   [`transcript::DRIVEN`].
//!
//! The transport-neutrality proof is mechanical: a scenario is run twice, once per link, and the
//! DOS records the engine emits must be byte-identical. Everything that differs between the two
//! runs is framing, which is exactly the line §14 draws.

pub mod fake_link;
pub mod runner;
pub mod scenarios;
pub mod transaction;
pub mod transcript;

#[cfg(test)]
mod tests;

pub use fake_link::{FakeBleLink, FakeLink, FakeUsbLink};
pub use runner::Driver;
pub use scenarios::{Fault, Fixture, Store};
pub use transaction::{FakeTransaction, Faults};
