//! Small, current device facts shown by app chrome.
//!
//! A [`DeviceStatus`] field belongs here when it is a cheap `Copy` fact sampled from a platform
//! port or host snapshot and multiple screens render it. Commands and workflows do not: forgetting
//! a bond remains a host command, while navigation, sensors, weather, catalogs, and transfers keep
//! their domain-specific state. This inclusion rule prevents the status value from becoming a
//! generic bag for anything the device happens to know.

use crate::BleLink;

/// The platform-fed device facts shared by ordinary app chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceStatus {
    /// Battery charge on the app's `0..=100` percent scale.
    pub battery_pct: u8,
    /// Current phone-link phase.
    pub ble_link: BleLink,
    /// Whether the host has a stored phone bond.
    pub ble_paired: bool,
}

impl DeviceStatus {
    /// Whether a phone currently holds the BLE link.
    #[inline]
    pub const fn ble_connected(self) -> bool {
        matches!(self.ble_link, BleLink::Connected)
    }
}
