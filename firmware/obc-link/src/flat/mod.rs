//! Protocol v4: the wire the flat store speaks, and the one engine that speaks it.
//!
//! [`FLAT_Store_Protocol.md`] is the normative contract — §2 is the store seam, §3 is what crosses
//! the link, §4 is the firmware update, §5 binds §3 to BLE and USB — and
//! [`FLAT_Store_Format.md`] is what the card holds. Nothing in this module is negotiated: the major
//! is a transport fact, every message is a fixed layout, and there is no Hello, no capability page
//! and no minor.
//!
//! [`FLAT_Store_Protocol.md`]: ../../../../specs/FLAT_Store_Protocol.md
//! [`FLAT_Store_Format.md`]: ../../../../specs/FLAT_Store_Format.md
//!
//! ## Reading order
//!
//! [`ids`] is the vocabulary — three identities, the kind table, the entry flags. [`wire`] is §3's
//! bytes and nothing else: total decoding, exact encoding, no state and no policy. [`store`] is §2
//! declared from the side that consumes it, because the dependency runs downward and a foundation
//! crate may not name a platform adapter. [`engine`] sits on both: one transfer at a time, no
//! resume, no session, and the catalog as the only durable record of a result.
//!
//! ## What this replaces
//!
//! The Device Object System v2 wire (major 3) and its engine, which the rest of this crate still
//! carries for the OBC2 consumers that have not moved yet. Nothing here forwards to it, shims it, or
//! shares a byte with it: coexistence is compile-time and the two never meet.

pub mod engine;
pub mod ids;
pub mod store;
pub mod wire;

#[cfg(any(test, feature = "std"))]
pub mod vectors;

pub use engine::{CancelCause, Ceilings, Channel, Engine, Reaction, DEFAULT_STAGE};
pub use ids::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision, StoreId};
pub use store::{Mode, Mutation, OpenPolicy, Policy, PutSource, Store, StoreError};
pub use wire::{ErrorCode, Opcode, Refusal, RequestId, WIRE_MAJOR};
