//! The S0 BLE data-plane core (`obc-ble-interface-spec.md`, epic #267): the control-plane
//! descriptor codecs, CRC-32/IEEE, and the whole-object transfer state machine — everything the
//! link needs *except* the radio. It is `no_std`, no-alloc, and holds **no** trouble-host / SDC
//! types, so it builds and tests on the host (`cargo test`) before it ever touches the DK — the
//! `debug_link` discipline applied to BLE.
//!
//! ## The two planes (spec §1 design principles)
//!
//! The wire has two planes and this crate models both halves of the contract:
//!
//! - **Control plane** (GATT, small + typed): the fixed 16-byte [`TransferControl`] descriptor
//!   (`descriptor`) announces a transfer; the device answers with the [`StatusMessage`] envelope
//!   (`transferResult` / `storeChanged` / `commandResult`). These are the "frame codec" the S0
//!   freeze pins — there is **no per-chunk header on the CoC**.
//! - **Data plane** (L2CAP CoC, raw bytes): the channel carries exactly the object's payload bytes.
//!   [`Receiver`] sinks them with a running [`Crc32`] and verifies **one** whole-object CRC at
//!   commit; [`StreamSender`] streams an object out the same way. Both restart rather than resume,
//!   and neither buffers the whole object — the RAM-limited MCU CRCs as it writes.
//!
//! The board crate (`obc-fw-nrf54l`, `ble` feature) owns the trouble-host `L2capChannel` and the
//! GATT attribute table; it decodes writes and encodes notifications through `descriptor`, and
//! drives the CoC bytes through `transfer`. The companion app's Swift mirror
//! (`OBCKit/OBCTransport`) implements the same layouts; the shared `protocol-vectors/` fixtures
//! pin both, and `tests/vectors.rs` here runs this crate's *production* codecs against them.

#![no_std]
#![forbid(unsafe_code)]

pub mod crc32;
pub mod descriptor;
pub mod list;
pub mod transfer;

pub use crc32::Crc32;
pub use descriptor::{
    CommandResult, CommandStatus, Config, DescriptorError, ObjectStoreDigest, ObjectType, Op, StatusMessage,
    StoreChanged, TransferControl, TransferResult, TransferStatus,
};
pub use list::{ListHeader, RideListEntry, RouteListEntry, LIST_ENTRY_LEN};
pub use transfer::{Receiver, StreamSender, TransferError};

/// The protocol version this crate implements (spec §1). The board serves it on the
/// `protocolVersion` characteristic; the app reads it on connect and stops on a mismatch.
pub const PROTOCOL_VERSION: u16 = 1;
