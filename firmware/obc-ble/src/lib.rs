//! The BLE data-plane core: control-plane descriptor codecs, CRC-32/IEEE, and the whole-object
//! transfer state machine — everything the link needs except the radio. It holds no radio type, so
//! it builds and tests on the host.
//!
//! The control plane is GATT: the fixed 12-byte [`TransferControl`] descriptor the app writes to
//! open a transfer, and the [`StatusMessage`] envelope, the only device-to-app channel. The data
//! plane is the L2CAP CoC, which carries the object's payload bytes and nothing else: there is no
//! per-chunk header. [`Receiver`] and [`StreamSender`] verify one whole-object [`Crc32`] and never
//! buffer the object.
//!
//! The board crate owns the `L2capChannel` and the GATT table. The Swift companion implements the
//! same layouts, and the `specs/vectors/` fixtures pin both.

#![no_std]
#![forbid(unsafe_code)]

pub mod crc32;
pub mod descriptor;
pub mod list;
pub mod sensors;
pub mod transfer;

pub use crc32::Crc32;
pub use descriptor::{
    install_fw_reply, CommandResult, CommandStatus, Config, DescriptorError, ObjectType, Op, SetClock, StatusMessage,
    StoreChanged, TransferControl, TransferResult, TransferStatus, CMD_DELETE_OBJECT, CMD_FORGET_BOND, CMD_INSTALL_FW,
    CMD_SET_CLOCK, SET_CLOCK_MAX_OFFSET_MIN, SET_CLOCK_MIN_UTC,
};
pub use list::{ListHeader, RideListEntry, RouteListEntry, TripListEntry};
pub use sensors::{
    classify_advertisement, parse_battery_level, parse_csc_measurement, parse_hr_measurement, parse_power_measurement,
    power_crank_feeds_cadence, AdvMatch, CrankCadence, CrankRevs, CscSample, HrSample, PowerSample, SensorKind,
    WheelRevs, UUID_BATTERY_LEVEL, UUID_BATTERY_SERVICE, UUID_CSC_MEASUREMENT, UUID_CSC_SERVICE,
    UUID_CYCLING_POWER_MEASUREMENT, UUID_CYCLING_POWER_SERVICE, UUID_HEART_RATE_SERVICE, UUID_HR_MEASUREMENT,
};
pub use transfer::{HeldMagic, Receiver, StreamSender, TransferError, MAGIC_LEN};
