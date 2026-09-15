use crate::BleLink;

/// The platform-fed device facts shared by ordinary app chrome. [`AppState`](crate::AppState)
/// stores this as its single [`device`](crate::AppState::device) field.
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
