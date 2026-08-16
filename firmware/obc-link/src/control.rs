//! The device-control plane, opcodes `0x0400`–`0x0406` (`Device_Object_Protocol_v3.md` §16).
//!
//! "They read or set device state — identity, diagnostics, configuration, clock, bonding — and they
//! are deliberately outside the object system: none of them carries an OperationId, claims a slot,
//! creates a generation, touches the catalog, or occupies a retained-result slot."
//!
//! The whole plane is also card-independent: "Every operation below MUST work with no card
//! inserted, an unsupported filesystem, or a recovery-failed store." `ResetStore` is the one member
//! that needs the medium, and the one that changes durable state.

use crate::codec::{bytes16_at, i64_at, put_bytes, put_i64, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::error::{reject_nonzero, DecodeError};
use crate::ids::StoreId;
use crate::metadata::text_is_clean;
use crate::{BufferTooSmall, EncodeResult};

/// The GetDeviceStatus response.
pub const DEVICE_STATUS_LEN: usize = 64;

/// The device configuration block, in both directions.
pub const CONFIG_BLOCK_LEN: usize = 56;

/// The device name field inside that block.
pub const MAX_DEVICE_NAME: usize = 32;

/// The SetClock request and response.
pub const SET_CLOCK_LEN: usize = 16;

/// The ForgetBond request.
pub const FORGET_BOND_LEN: usize = 8;

/// The ResetStore request and response.
pub const RESET_STORE_LEN: usize = 16;

/// How the store's medium is classified (§16, reproducing `OBC2_Storage_Format.md` §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MountClass {
    /// No medium is present — the one case classification never sees.
    NoCard = 0,
    /// A filesystem this format does not define.
    UnsupportedFilesystem = 1,
    /// Initializing.
    Initializing = 2,
    /// Mounted and healthy.
    Mounted = 3,
    /// Mounted with at least one degraded catalog entry, reached at a failed lazy pin.
    MountedDegradedEntry = 4,
    /// Recovery failed; read-only.
    RecoveryFailedReadOnly = 5,
    /// Mounted, degraded store-wide.
    MountedStoreDegraded = 6,
}

impl MountClass {
    /// Every class, in wire order.
    pub const ALL: [MountClass; 7] = [
        MountClass::NoCard,
        MountClass::UnsupportedFilesystem,
        MountClass::Initializing,
        MountClass::Mounted,
        MountClass::MountedDegradedEntry,
        MountClass::RecoveryFailedReadOnly,
        MountClass::MountedStoreDegraded,
    ];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(MountClass::NoCard),
            1 => Some(MountClass::UnsupportedFilesystem),
            2 => Some(MountClass::Initializing),
            3 => Some(MountClass::Mounted),
            4 => Some(MountClass::MountedDegradedEntry),
            5 => Some(MountClass::RecoveryFailedReadOnly),
            6 => Some(MountClass::MountedStoreDegraded),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// True for the three classes §16 says report a StoreId: `3`, `4`, and `6`.
    pub const fn reports_store_id(self) -> bool {
        matches!(self, MountClass::Mounted | MountClass::MountedDegradedEntry | MountClass::MountedStoreDegraded)
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            MountClass::NoCard => "noCard",
            MountClass::UnsupportedFilesystem => "unsupportedFilesystem",
            MountClass::Initializing => "initializing",
            MountClass::Mounted => "mounted",
            MountClass::MountedDegradedEntry => "mountedDegradedEntry",
            MountClass::RecoveryFailedReadOnly => "recoveryFailedReadOnly",
            MountClass::MountedStoreDegraded => "mountedStoreDegraded",
        }
    }
}

/// GetDeviceStatus status-flag bits (§16).
pub mod status_flags {
    /// Bit 0 — a card is present.
    pub const CARD_PRESENT: u16 = 1 << 0;
    /// Bit 1 — developer/unlocked mode.
    pub const DEVELOPER_UNLOCKED: u16 = 1 << 1;
    /// Every defined bit; the rest are zero.
    pub const ALL: u16 = CARD_PRESENT | DEVELOPER_UNLOCKED;
}

/// The 64-byte GetDeviceStatus response (§16). Its request payload is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceStatus {
    /// Firmware major.
    pub firmware_major: u16,
    /// Firmware minor.
    pub firmware_minor: u16,
    /// Firmware patch.
    pub firmware_patch: u16,
    /// Hardware revision.
    pub hardware_revision: u16,
    /// The 16 opaque serial bytes. "it is not a StoreId: replacing the card changes the StoreId and
    /// never the serial."
    pub device_serial: [u8; 16],
    /// Boots since manufacture.
    pub boot_count: u32,
    /// Seconds since this boot.
    pub uptime_seconds: u64,
    /// Worst observed stack high-water, in bytes. A diagnostic that drives no protocol behaviour.
    pub stack_high_water: u32,
    /// Card-present / developer bits.
    pub status_flags: u16,
    /// How the medium is classified.
    pub mount_class: MountClass,
    /// Firmware build number.
    pub firmware_build: u32,
    /// The StoreId, zero unless the mount class reports one.
    pub store_id: StoreId,
}

impl DeviceStatus {
    /// Decodes exactly [`DEVICE_STATUS_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, DEVICE_STATUS_LEN)?;
        if payload[43] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        let status_flags = u16_at(payload, 40);
        if status_flags & !status_flags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let mount_class = MountClass::from_u8(payload[42]).ok_or_else(DecodeError::unknown_enum)?;
        let store_id = StoreId::new(bytes16_at(payload, 48));
        if !mount_class.reports_store_id() && !store_id.is_zero() {
            // §16: "StoreId; zero unless mount class is `3`, `4`, or `6`".
            return Err(DecodeError::reserved_bits());
        }
        Ok(DeviceStatus {
            firmware_major: u16_at(payload, 0),
            firmware_minor: u16_at(payload, 2),
            firmware_patch: u16_at(payload, 4),
            hardware_revision: u16_at(payload, 6),
            device_serial: bytes16_at(payload, 8),
            boot_count: u32_at(payload, 24),
            uptime_seconds: u64_at(payload, 28),
            stack_high_water: u32_at(payload, 36),
            status_flags,
            mount_class,
            firmware_build: u32_at(payload, 44),
            store_id,
        })
    }

    /// Encodes the response.
    pub fn encode(&self) -> [u8; DEVICE_STATUS_LEN] {
        let mut out = [0u8; DEVICE_STATUS_LEN];
        put_u16(&mut out, 0, self.firmware_major);
        put_u16(&mut out, 2, self.firmware_minor);
        put_u16(&mut out, 4, self.firmware_patch);
        put_u16(&mut out, 6, self.hardware_revision);
        put_bytes(&mut out, 8, &self.device_serial);
        put_u32(&mut out, 24, self.boot_count);
        put_u64(&mut out, 28, self.uptime_seconds);
        put_u32(&mut out, 36, self.stack_high_water);
        put_u16(&mut out, 40, self.status_flags);
        out[42] = self.mount_class.to_u8();
        put_u32(&mut out, 44, self.firmware_build);
        put_bytes(&mut out, 48, self.store_id.as_bytes());
        out
    }
}

/// Config unit-flag bits (§16).
pub mod unit_flags {
    /// Bit 0 — imperial distance, speed, and elevation.
    pub const IMPERIAL: u8 = 1 << 0;
    /// Bit 1 — Fahrenheit.
    pub const FAHRENHEIT: u8 = 1 << 1;
    /// Bit 2 — 12-hour clock.
    pub const TWELVE_HOUR: u8 = 1 << 2;
    /// Every defined bit; the rest are zero.
    pub const ALL: u8 = IMPERIAL | FAHRENHEIT | TWELVE_HOUR;
}

/// The weather refresh cadence (§16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WeatherRefresh {
    /// Never refresh.
    Off = 0,
    /// Every fifteen minutes.
    Minutes15 = 1,
    /// Every thirty minutes.
    Minutes30 = 2,
    /// Hourly.
    Minutes60 = 3,
    /// Every two hours.
    Minutes120 = 4,
}

impl WeatherRefresh {
    /// Every value, in wire order.
    pub const ALL: [WeatherRefresh; 5] = [
        WeatherRefresh::Off,
        WeatherRefresh::Minutes15,
        WeatherRefresh::Minutes30,
        WeatherRefresh::Minutes60,
        WeatherRefresh::Minutes120,
    ];

    /// Decodes a wire `u8`. "a weather-refresh value above `4` is `invalidDescriptor`".
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(WeatherRefresh::Off),
            1 => Some(WeatherRefresh::Minutes15),
            2 => Some(WeatherRefresh::Minutes30),
            3 => Some(WeatherRefresh::Minutes60),
            4 => Some(WeatherRefresh::Minutes120),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }
}

/// The 56-byte device configuration block (§16), carried by GetConfig and SetConfig alike.
///
/// "Because the block is whole and fixed, there is no absent field and no absent-means-leave-
/// untouched rule to reason about."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigBlock {
    /// Imperial / Fahrenheit / 12-hour.
    pub unit_flags: u8,
    /// How often to refresh weather.
    pub weather_refresh: WeatherRefresh,
    /// The device name, `0` through `32` encoded bytes. "a zero length means the device advertises
    /// its factory default name rather than an empty one."
    pub name: [u8; MAX_DEVICE_NAME],
    /// How many of those bytes are the name.
    pub name_len: u8,
}

impl ConfigBlock {
    /// The block's codec version.
    pub const CODEC_VERSION: u8 = 1;

    /// The name bytes.
    pub fn name(&self) -> &[u8] {
        &self.name[..usize::from(self.name_len)]
    }

    /// Decodes exactly [`CONFIG_BLOCK_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, CONFIG_BLOCK_LEN)?;
        if payload[0] != Self::CODEC_VERSION || payload[1] != CONFIG_BLOCK_LEN as u8 {
            return Err(DecodeError::invalid_descriptor(crate::error::detail::descriptor::INVALID_COMBINATION));
        }
        reject_nonzero(payload, 2, 2)?;
        if payload[7] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        reject_nonzero(payload, 40, 16)?;
        let name_len = payload[4];
        if usize::from(name_len) > MAX_DEVICE_NAME {
            return Err(DecodeError::invalid_combination());
        }
        if payload[5] & !unit_flags::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        let weather_refresh = WeatherRefresh::from_u8(payload[6]).ok_or_else(DecodeError::unknown_enum)?;
        let name_start = 8 + usize::from(name_len);
        if !payload[name_start..8 + MAX_DEVICE_NAME].iter().all(|&b| b == 0) {
            // "a nonzero byte at or beyond the stated length is `invalidDescriptor`".
            return Err(DecodeError::reserved_bits());
        }
        if !text_is_clean(&payload[8..name_start]) {
            // "Name bytes obey Section 2.2's text rules."
            return Err(DecodeError::invalid_descriptor(crate::error::detail::descriptor::NONCANONICAL_METADATA));
        }
        let mut name = [0u8; MAX_DEVICE_NAME];
        name.copy_from_slice(&payload[8..8 + MAX_DEVICE_NAME]);
        Ok(ConfigBlock { unit_flags: payload[5], weather_refresh, name, name_len })
    }

    /// Encodes the block.
    pub fn encode(&self) -> [u8; CONFIG_BLOCK_LEN] {
        let mut out = [0u8; CONFIG_BLOCK_LEN];
        out[0] = Self::CODEC_VERSION;
        out[1] = CONFIG_BLOCK_LEN as u8;
        out[4] = self.name_len;
        out[5] = self.unit_flags;
        out[6] = self.weather_refresh.to_u8();
        put_bytes(&mut out, 8, &self.name);
        out
    }
}

/// Which source a SetClock offers (§16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClockSource {
    /// A companion application.
    Companion = 1,
    /// The device's own GPS.
    Gps = 2,
}

impl ClockSource {
    /// Every source, in wire order.
    pub const ALL: [ClockSource; 2] = [ClockSource::Companion, ClockSource::Gps];

    /// Decodes a wire `u8`. "An unknown source value is `invalidDescriptor/unknownEnum`."
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(ClockSource::Companion),
            2 => Some(ClockSource::Gps),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            ClockSource::Companion => "companion",
            ClockSource::Gps => "gps",
        }
    }
}

/// The SetClock request (§16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetClock {
    /// The offered time, in signed Unix seconds.
    pub epoch_seconds: i64,
    /// Where it came from.
    pub source: ClockSource,
}

impl SetClock {
    /// Decodes exactly [`SET_CLOCK_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, SET_CLOCK_LEN)?;
        reject_nonzero(payload, 9, 7)?;
        Ok(SetClock {
            epoch_seconds: i64_at(payload, 0),
            source: ClockSource::from_u8(payload[8]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; SET_CLOCK_LEN] {
        let mut out = [0u8; SET_CLOCK_LEN];
        put_i64(&mut out, 0, self.epoch_seconds);
        out[8] = self.source.to_u8();
        out
    }
}

/// Whether the device's clock is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClockState {
    /// Not yet trusted; "no set of any source is refused while the clock is still untrusted".
    Untrusted = 0,
    /// Trusted.
    Trusted = 1,
}

impl ClockState {
    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ClockState::Untrusted),
            1 => Some(ClockState::Trusted),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            ClockState::Untrusted => "untrusted",
            ClockState::Trusted => "trusted",
        }
    }
}

/// The SetClock response (§16). "the response's clock and state bytes are how a client learns which
/// happened" after a lost response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockStatus {
    /// The device's clock after the request.
    pub epoch_seconds: i64,
    /// The source it is now trusting.
    pub source: ClockSource,
    /// Whether the clock is trusted.
    pub state: ClockState,
}

impl ClockStatus {
    /// Decodes exactly [`SET_CLOCK_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, SET_CLOCK_LEN)?;
        reject_nonzero(payload, 10, 6)?;
        Ok(ClockStatus {
            epoch_seconds: i64_at(payload, 0),
            source: ClockSource::from_u8(payload[8]).ok_or_else(DecodeError::unknown_enum)?,
            state: ClockState::from_u8(payload[9]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the response.
    pub fn encode(&self) -> [u8; SET_CLOCK_LEN] {
        let mut out = [0u8; SET_CLOCK_LEN];
        put_i64(&mut out, 0, self.epoch_seconds);
        out[8] = self.source.to_u8();
        out[9] = self.state.to_u8();
        out
    }
}

/// How much bonding material to remove (§16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ForgetBondScope {
    /// Only this bond.
    ThisBond = 1,
    /// Every bond.
    EveryBond = 2,
}

impl ForgetBondScope {
    /// Every scope, in wire order.
    pub const ALL: [ForgetBondScope; 2] = [ForgetBondScope::ThisBond, ForgetBondScope::EveryBond];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(ForgetBondScope::ThisBond),
            2 => Some(ForgetBondScope::EveryBond),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            ForgetBondScope::ThisBond => "thisBond",
            ForgetBondScope::EveryBond => "everyBond",
        }
    }
}

/// The ForgetBond request (§16). Its response payload is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetBond {
    /// How much to remove.
    pub scope: ForgetBondScope,
}

impl ForgetBond {
    /// Decodes exactly [`FORGET_BOND_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, FORGET_BOND_LEN)?;
        reject_nonzero(payload, 1, 7)?;
        Ok(ForgetBond { scope: ForgetBondScope::from_u8(payload[0]).ok_or_else(DecodeError::unknown_enum)? })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; FORGET_BOND_LEN] {
        let mut out = [0u8; FORGET_BOND_LEN];
        out[0] = self.scope.to_u8();
        out
    }
}

/// An Echo payload: "zero or more bytes with no internal structure", returned byte-identical.
///
/// "the device MUST NOT interpret, log, or store the payload", so this type deliberately has no
/// accessor beyond the bytes themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Echo<'a> {
    /// The bytes.
    pub payload: &'a [u8],
}

impl<'a> Echo<'a> {
    /// Borrows a payload. Its only bound is the negotiated control frame, which every frame already
    /// has, so there is nothing else to check.
    pub fn decode(payload: &'a [u8]) -> crate::Result<Self> {
        Ok(Echo { payload })
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        if out.len() < self.payload.len() {
            return Err(BufferTooSmall { needed: self.payload.len(), available: out.len() });
        }
        out[..self.payload.len()].copy_from_slice(self.payload);
        Ok(self.payload.len())
    }
}

/// The ResetStore request (§16): the StoreId being destroyed, echoed as confirmation.
///
/// "The echo is the confirmation, and it is checked before anything is deleted." The all-zero form
/// is admitted only in the two classes that report no StoreId — initializing `2` and
/// recovery-failed `5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetStore {
    /// The StoreId the client believes it is destroying, or zero.
    pub echoed_store_id: StoreId,
}

impl ResetStore {
    /// Decodes exactly [`RESET_STORE_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, RESET_STORE_LEN)?;
        Ok(ResetStore { echoed_store_id: StoreId::new(bytes16_at(payload, 0)) })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; RESET_STORE_LEN] {
        self.echoed_store_id.to_bytes()
    }

    /// Whether this echo is admissible against the device's current mount class and StoreId.
    ///
    /// A codec cannot know either, so this is a predicate the engine calls rather than a decode
    /// rule; the refusal it produces is `invalidDescriptor/invalidCombination` and nothing is
    /// destroyed.
    pub fn echo_is_admissible(&self, mount_class: MountClass, current: StoreId) -> bool {
        if mount_class.reports_store_id() {
            !self.echoed_store_id.is_zero() && self.echoed_store_id == current
        } else {
            matches!(mount_class, MountClass::Initializing | MountClass::RecoveryFailedReadOnly)
                && self.echoed_store_id.is_zero()
        }
    }
}

/// The ResetStore response: the new StoreId born from the reinitialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetStoreResult {
    /// The new StoreId, sent "only after the first checkpoint gate of the new store is durable".
    pub new_store_id: StoreId,
}

impl ResetStoreResult {
    /// Decodes exactly [`RESET_STORE_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, RESET_STORE_LEN)?;
        Ok(ResetStoreResult { new_store_id: StoreId::new(bytes16_at(payload, 0)) })
    }

    /// Encodes the response.
    pub fn encode(&self) -> [u8; RESET_STORE_LEN] {
        self.new_store_id.to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(mount_class: MountClass) -> DeviceStatus {
        DeviceStatus {
            firmware_major: 1,
            firmware_minor: 4,
            firmware_patch: 2,
            hardware_revision: 3,
            device_serial: [0xC0; 16],
            boot_count: 412,
            uptime_seconds: 86_400,
            stack_high_water: 24_576,
            status_flags: if mount_class == MountClass::NoCard { 0 } else { status_flags::CARD_PRESENT },
            mount_class,
            firmware_build: 9911,
            store_id: if mount_class.reports_store_id() { StoreId::new([0x7E; 16]) } else { StoreId::ZERO },
        }
    }

    #[test]
    fn device_status_is_sixty_four_bytes_for_every_mount_class() {
        for class in MountClass::ALL {
            let body = status(class);
            let bytes = body.encode();
            assert_eq!(bytes.len(), 64);
            assert_eq!(DeviceStatus::decode(&bytes).unwrap(), body);
            assert_eq!(bytes[48..64].iter().all(|&b| b == 0), !class.reports_store_id());
        }
        // A StoreId in a class that does not report one.
        let mut bytes = status(MountClass::NoCard).encode();
        put_bytes(&mut bytes, 48, &[1u8; 16]);
        assert_eq!(DeviceStatus::decode(&bytes).unwrap_err(), DecodeError::reserved_bits());
    }

    fn config(name: &[u8]) -> ConfigBlock {
        let mut padded = [0u8; MAX_DEVICE_NAME];
        padded[..name.len()].copy_from_slice(name);
        ConfigBlock {
            unit_flags: unit_flags::IMPERIAL | unit_flags::TWELVE_HOUR,
            weather_refresh: WeatherRefresh::Minutes30,
            name: padded,
            name_len: name.len() as u8,
        }
    }

    #[test]
    fn the_config_block_round_trips_at_both_name_boundaries() {
        for name in [&b""[..], &b"OBC"[..], &b"abcdefghijklmnopqrstuvwxyz012345"[..]] {
            let block = config(name);
            let bytes = block.encode();
            assert_eq!(bytes.len(), 56);
            let decoded = ConfigBlock::decode(&bytes).unwrap();
            assert_eq!(decoded, block);
            assert_eq!(decoded.name(), name);
        }
    }

    #[test]
    fn the_config_block_rejects_every_negative_the_vectors_require() {
        let block = config(b"OBC");

        let mut padded = block.encode();
        padded[8 + 3] = b'!';
        assert_eq!(ConfigBlock::decode(&padded).unwrap_err(), DecodeError::reserved_bits());

        let mut long_name = block.encode();
        long_name[4] = 33;
        assert_eq!(ConfigBlock::decode(&long_name).unwrap_err(), DecodeError::invalid_combination());

        let mut refresh = block.encode();
        refresh[6] = 5;
        assert_eq!(ConfigBlock::decode(&refresh).unwrap_err(), DecodeError::unknown_enum());

        let mut units = block.encode();
        units[5] = 1 << 3;
        assert_eq!(ConfigBlock::decode(&units).unwrap_err(), DecodeError::unsupported_flags());

        let mut length = block.encode();
        length[1] = 57;
        assert_eq!(ConfigBlock::decode(&length).unwrap_err(), DecodeError::invalid_combination());

        let mut version = block.encode();
        version[0] = 2;
        assert_eq!(ConfigBlock::decode(&version).unwrap_err(), DecodeError::invalid_combination());

        let mut control_name = block.encode();
        control_name[8] = 0x07;
        assert!(ConfigBlock::decode(&control_name).is_err());
    }

    #[test]
    fn the_clock_messages_round_trip_for_both_sources() {
        for source in ClockSource::ALL {
            let request = SetClock { epoch_seconds: 1_763_000_000, source };
            let bytes = request.encode();
            assert_eq!(bytes.len(), 16);
            assert_eq!(SetClock::decode(&bytes).unwrap(), request);

            let response = ClockStatus { epoch_seconds: 1_763_000_000, source, state: ClockState::Trusted };
            assert_eq!(ClockStatus::decode(&response.encode()).unwrap(), response);
        }
        let mut unknown = SetClock { epoch_seconds: 0, source: ClockSource::Gps }.encode();
        unknown[8] = 3;
        assert_eq!(SetClock::decode(&unknown).unwrap_err(), DecodeError::unknown_enum());
        // A negative epoch is an ordinary signed value, not an error.
        let before_epoch = SetClock { epoch_seconds: -1, source: ClockSource::Companion };
        assert_eq!(SetClock::decode(&before_epoch.encode()).unwrap(), before_epoch);
    }

    #[test]
    fn forget_bond_echo_and_reset_round_trip() {
        for scope in ForgetBondScope::ALL {
            let request = ForgetBond { scope };
            assert_eq!(ForgetBond::decode(&request.encode()).unwrap(), request);
        }
        let mut zero_scope = ForgetBond { scope: ForgetBondScope::ThisBond }.encode();
        zero_scope[0] = 0;
        assert_eq!(ForgetBond::decode(&zero_scope).unwrap_err(), DecodeError::unknown_enum());

        let mut out = [0u8; 8];
        for payload in [&b""[..], &b"x"[..], &b"12345678"[..]] {
            let echo = Echo { payload };
            let len = echo.encode_into(&mut out).unwrap();
            assert_eq!(Echo::decode(&out[..len]).unwrap(), echo);
        }

        let store = StoreId::new([0x7E; 16]);
        let request = ResetStore { echoed_store_id: store };
        assert_eq!(ResetStore::decode(&request.encode()).unwrap(), request);
        assert!(request.echo_is_admissible(MountClass::Mounted, store));
        assert!(!request.echo_is_admissible(MountClass::Mounted, StoreId::new([1; 16])));
        let zero_echo = ResetStore { echoed_store_id: StoreId::ZERO };
        assert!(!zero_echo.echo_is_admissible(MountClass::Mounted, store));
        assert!(zero_echo.echo_is_admissible(MountClass::Initializing, StoreId::ZERO));
        assert!(zero_echo.echo_is_admissible(MountClass::RecoveryFailedReadOnly, StoreId::ZERO));
        assert!(!zero_echo.echo_is_admissible(MountClass::NoCard, StoreId::ZERO));

        let result = ResetStoreResult { new_store_id: StoreId::new([0x88; 16]) };
        assert_eq!(ResetStoreResult::decode(&result.encode()).unwrap(), result);
    }
}
