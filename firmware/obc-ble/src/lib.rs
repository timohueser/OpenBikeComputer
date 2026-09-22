//! Radio-free BLE codecs shared by the board firmware and host tests: the live GATT config,
//! command/status and sensor layouts, plus the CRC-32 seam used by persisted link settings.

#![no_std]
#![forbid(unsafe_code)]

pub mod crc32;
pub mod descriptor;
pub mod sensors;

pub use crc32::Crc32;
pub use descriptor::{
    install_fw_reply, CommandResult, CommandStatus, Config, DescriptorError, SetClock, StatusMessage, CMD_FORGET_BOND,
    CMD_INSTALL_FW, CMD_SET_CLOCK, SET_CLOCK_MAX_OFFSET_MIN, SET_CLOCK_MIN_UTC,
};
pub use sensors::{
    classify_advertisement, parse_battery_level, parse_csc_measurement, parse_hr_measurement, parse_power_measurement,
    power_crank_feeds_cadence, AdvMatch, CrankCadence, CrankRevs, CscSample, HrSample, PowerSample, SensorKind,
    WheelRevs, UUID_BATTERY_LEVEL, UUID_BATTERY_SERVICE, UUID_CSC_MEASUREMENT, UUID_CSC_SERVICE,
    UUID_CYCLING_POWER_MEASUREMENT, UUID_CYCLING_POWER_SERVICE, UUID_HEART_RATE_SERVICE, UUID_HR_MEASUREMENT,
};
