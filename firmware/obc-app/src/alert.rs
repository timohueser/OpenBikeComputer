//! Device failures. Producers raise them into one [`Alerts`] set per pass; the warning card and the
//! cues read that set.

/// A device failure the rider must know about. The device stays usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    /// The GPS module did not answer the boot probe.
    NoGps,
    /// The barometric altimeter did not answer the boot probe.
    NoAltimeter,
    /// The compass / IMU did not answer the boot probe.
    NoCompass,
    /// A ride-log write did not happen while a ride records, so this ride's log is incomplete.
    RecordingFailed,
    /// The boot recovery found an incomplete ride log. Nothing records now.
    RideRecoveredIncomplete,
    /// A settings write did not reach the persistent store. The value stays live in RAM and the
    /// app retries, so only durability is at risk.
    SettingsNotSaved,
    /// The card transport latched off: enough operations failed in a row that the device stopped
    /// attempting the card. It probes the card again on a cool-down.
    StorageLost,
}

// `StorageLost` is the last variant, so this holds every alert inside the one byte of `Alerts`.
const _: () = assert!((Alert::StorageLost as u8) < u8::BITS as u8, "every alert is one bit of `Alerts`");

/// A set of alerts. A raise never displaces what is already raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alerts(u8);

impl Alerts {
    pub const NONE: Alerts = Alerts(0);

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, alert: Alert) -> bool {
        self.0 & 1 << alert as u8 != 0
    }

    /// The alerts here that are not in `seen`.
    pub const fn without(self, seen: Alerts) -> Alerts {
        Alerts(self.0 & !seen.0)
    }

    pub fn raise(&mut self, alerts: impl Into<Alerts>) {
        self.0 |= alerts.into().0;
    }

    pub fn take(&mut self) -> Alerts {
        core::mem::replace(self, Alerts::NONE)
    }
}

impl From<Alert> for Alerts {
    fn from(alert: Alert) -> Alerts {
        Alerts(1 << alert as u8)
    }
}
