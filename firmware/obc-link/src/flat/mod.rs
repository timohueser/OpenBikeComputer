//! The wire the flat store speaks, and the one engine that speaks it.
//!
//! [`flat-store-protocol.md`] is the normative contract and [`flat-store-format.md`] is what the
//! card holds. Nothing here is negotiated: every version is a fixed fact and every message is a
//! fixed layout.
//!
//! [`ids`] is the vocabulary, [`wire`] is the bytes, [`store`] is the seam declared from the side
//! that consumes it, and [`engine`] sits on both.
//!
//! [`flat-store-protocol.md`]: ../../../../specs/flat-store-protocol.md
//! [`flat-store-format.md`]: ../../../../specs/flat-store-format.md
pub mod engine;
pub mod ids;
pub mod records;
pub mod store;
pub mod wire;

#[cfg(feature = "client")]
pub mod client;

#[cfg(any(test, feature = "std"))]
pub mod vectors;

pub use engine::{
    Admission, CancelCause, Ceilings, Channel, Engine, Link, Reaction, Stall, UploadEnd, UploadProgress, UsbMapBatch,
    DEFAULT_STAGE, STALL_TIMEOUT_MS,
};
pub use ids::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision, StoreId};
pub use records::{
    buffer_len as record_buffer_len, padded_len as padded_record_len, Reassembler, RecordFault,
    PREFIX_LEN as RECORD_PREFIX_LEN, USB_BINDING_MAJOR,
};
pub use store::{
    ArchiveError, ArchiveResult, ArchiveSource, Mode, Mutation, OpenPolicy, Policy, PutSource, Store, StoreError,
};
pub use wire::{ErrorCode, Opcode, Refusal, RequestId, WIRE_MAJOR};
