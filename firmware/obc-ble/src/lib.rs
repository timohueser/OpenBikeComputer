//! The S0 BLE data-plane core: control-plane descriptor codecs, CRC-32/IEEE, and the whole-object
//! transfer state machine — everything the link needs *except* the radio. `no_std`, no-alloc, and
//! free of any trouble-host / SDC type, so it builds and `cargo test`s on the host.
//!
//! The wire has two planes and this crate models both halves:
//!
//! - **Control plane** (GATT, small + typed): the fixed 16-byte [`TransferControl`] descriptor
//!   announces a transfer; the device answers with the [`StatusMessage`] envelope. There is **no
//!   per-chunk header on the CoC**.
//! - **Data plane** (L2CAP CoC, raw bytes): the channel carries exactly the object's payload bytes.
//!   [`Receiver`] sinks them with a running [`Crc32`] and verifies **one** whole-object CRC at
//!   commit; [`StreamSender`] streams an object out the same way. Both restart rather than resume,
//!   and neither buffers the whole object.
//!
//! The board crate owns the `L2capChannel` and GATT table; the companion app's Swift mirror
//! implements the same layouts, and the shared `protocol-vectors/` fixtures pin both.

#![no_std]
#![forbid(unsafe_code)]

pub mod crc32;
pub mod descriptor;
pub mod list;
pub mod sensors;
pub mod transfer;

pub use crc32::Crc32;
pub use descriptor::{
    install_fw_reply, AckRides, CommandResult, CommandStatus, Config, DescriptorError, ObjectStoreDigest, ObjectType,
    Op, StatusMessage, StoreChanged, TransferControl, TransferResult, TransferStatus, CMD_ACK_RIDES, CMD_DELETE_OBJECT,
    CMD_INSTALL_FW,
};
pub use list::{ListHeader, RideListEntry, RouteListEntry, LIST_ENTRY_LEN};
pub use sensors::{
    classify_advertisement, parse_battery_level, parse_csc_measurement, parse_hr_measurement, parse_power_measurement,
    power_crank_feeds_cadence, AdvMatch, CrankCadence, CrankRevs, CscSample, HrSample, PowerSample, SensorKind,
    WheelRevs, UUID_BATTERY_LEVEL, UUID_BATTERY_SERVICE, UUID_CSC_MEASUREMENT, UUID_CSC_SERVICE,
    UUID_CYCLING_POWER_MEASUREMENT, UUID_CYCLING_POWER_SERVICE, UUID_HEART_RATE_SERVICE, UUID_HR_MEASUREMENT,
};
pub use transfer::{Receiver, StreamSender, TransferError};

/// The protocol version this crate implements. The app reads it on connect and stops on a mismatch.
pub const PROTOCOL_VERSION: u16 = 1;
