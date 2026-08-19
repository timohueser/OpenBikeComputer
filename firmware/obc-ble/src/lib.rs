//! The S0 BLE data-plane core: control-plane descriptor codecs, CRC-32/IEEE, and the whole-object
//! transfer state machine — everything the link needs *except* the radio. `no_std`, no-alloc, and
//! free of any trouble-host / SDC type, so it builds and `cargo test`s on the host.
//!
//! The wire has two planes and this crate models both halves:
//!
//! - **Control plane** (GATT, small + typed): the fixed 12-byte [`TransferControl`] descriptor (the
//!   app writes it to open a transfer) and the [`StatusMessage`] envelope (the sole device → app
//!   channel — it even carries a download's announce). There is **no per-chunk header on the CoC**.
//! - **Data plane** (L2CAP CoC, raw bytes): the channel carries exactly the object's payload bytes.
//!   [`Receiver`] sinks them with a running [`Crc32`] and verifies **one** whole-object CRC at
//!   commit; [`StreamSender`] streams an object out the same way. Both restart rather than resume,
//!   and neither buffers the whole object.
//!
//! The board crate owns the `L2capChannel` and GATT table; the companion app's Swift mirror
//! implements the same layouts, and the shared `specs/vectors/` fixtures pin both.

#![no_std]
#![forbid(unsafe_code)]

pub mod crc32;
pub mod descriptor;
pub mod list;
pub mod sensors;
pub mod transfer;
pub mod weather_request;

pub use crc32::Crc32;
pub use descriptor::{
    install_fw_reply, AckRides, CommandResult, CommandStatus, Config, DescriptorError, ObjectType, Op, SetClock,
    SetRouteRetention, StatusMessage, StoreChanged, TransferControl, TransferResult, TransferStatus, VersionRead,
    WeatherUnchanged, CMD_ACK_RIDES, CMD_DELETE_OBJECT, CMD_FORGET_BOND, CMD_INSTALL_FW, CMD_SET_CLOCK,
    CMD_SET_ROUTE_RETENTION, CMD_WEATHER_UNCHANGED, FEATURE_WEATHER, SET_CLOCK_MAX_OFFSET_MIN, SET_CLOCK_MIN_UTC,
    SET_ROUTE_RETENTION_MAX, WEATHER_UNCHANGED_MAX_RETRY_S,
};
pub use list::{ListHeader, RideListEntry, RouteListEntry, TripListEntry};
pub use sensors::{
    classify_advertisement, parse_battery_level, parse_csc_measurement, parse_hr_measurement, parse_power_measurement,
    power_crank_feeds_cadence, AdvMatch, CrankCadence, CrankRevs, CscSample, HrSample, PowerSample, SensorKind,
    WheelRevs, UUID_BATTERY_LEVEL, UUID_BATTERY_SERVICE, UUID_CSC_MEASUREMENT, UUID_CSC_SERVICE,
    UUID_CYCLING_POWER_MEASUREMENT, UUID_CYCLING_POWER_SERVICE, UUID_HEART_RATE_SERVICE, UUID_HR_MEASUREMENT,
};
pub use transfer::{HeldMagic, Receiver, StreamSender, TransferError, MAGIC_LEN};
pub use weather_request::{
    authenticated_context_was_served, classify_upload, BundleFacts, BundleIdentity, DueScheduler, Raise,
    UploadDisposition, WeatherRefresh, WeatherRequestBudget, WeatherRequestContext, BUNDLE_EXPIRED_S,
    REASON_HOURLY_ONLY, REASON_NO_BUNDLE, REASON_OUT_OF_AREA, REASON_RETRY, REASON_SCHEDULED, REASON_URGENT,
    RETRY_LADDER_S, VALID_BEARING, VALID_BUNDLE, VALID_POSITION, VALID_ROUTE, VALID_SPEED, WEATHER_BUNDLE_OBJECT_ID,
    WEATHER_REQUEST_CONTEXT_UUID, WEATHER_REQUEST_CONTEXT_VERSION, WEATHER_REQUEST_SERVICE_UUID,
    WEATHER_REQUEST_SERVICE_UUID_LE, WEATHER_REQUEST_WINDOW_S,
};

/// The protocol version this crate implements. The app reads it (with the store epoch and the
/// reader's OBCM version, as a [`VersionRead`]) on connect and stops on a mismatch — a v1 peer sees
/// this `u16 = 2` first and surfaces its mismatch path. There is no dual-version serving.
///
/// **Not the map-format version.** `obc_formats::obcm::VERSION` is a different number in a
/// different sequence — this is the wire contract, that is the file format on the card — and
/// [`VersionRead::obcm_version`] carries the latter precisely because neither can be derived from
/// the other. Appending that field did **not** bump this: the identity read is decoded by length
/// (spec §1), so a trailing field is additive in both directions, and a bump would stop two peers
/// that remain fully interoperable.
pub const PROTOCOL_VERSION: u16 = 2;
