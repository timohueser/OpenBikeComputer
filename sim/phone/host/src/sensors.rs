//! The phone's sensors as the app's pull-only ports.
//!
//! CoreLocation and CoreMotion deliver on their own cadence and the app polls once per pass, so
//! each value sits in a mailbox and leaves on the first poll that takes it. That is the
//! fresh-sample contract in [`obc_ports`]: `Some` means new, and a value read twice would be a
//! second fix at the same instant.

use obc_ports::{
    AltimeterSource, ClockSource, CompassSource, DateTime, Fix, FuelGauge, GpsTime, LocationSource, Sensors,
};

/// One sensor value waiting for its poll.
struct Mailbox<T>(Option<T>);

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> Mailbox<T> {
    /// Supersede whatever the app has not taken yet: the newer sample is the true one.
    fn push(&mut self, value: T) {
        self.0 = Some(value);
    }

    fn take(&mut self) -> Option<T> {
        self.0.take()
    }
}

impl LocationSource for Mailbox<Fix> {
    fn poll(&mut self) -> Option<Fix> {
        self.take()
    }
}

impl ClockSource for Mailbox<GpsTime> {
    fn poll(&mut self) -> Option<GpsTime> {
        self.take()
    }
}

impl CompassSource for Mailbox<f32> {
    fn poll(&mut self) -> Option<f32> {
        self.take()
    }
}

impl AltimeterSource for Mailbox<f32> {
    fn poll(&mut self) -> Option<f32> {
        self.take()
    }
}

impl FuelGauge for Mailbox<u8> {
    fn poll(&mut self) -> Option<u8> {
        self.take()
    }
}

/// What the phone can tell the device: position, wall clock, heading, altitude and charge.
///
/// There is no temperature, heart rate, power or cadence, because the phone has none of them, and
/// an honest absence is what makes the screens show a blank instead of a fabricated number.
#[derive(Default)]
pub struct PhoneSensors {
    fix: Mailbox<Fix>,
    clock: Mailbox<GpsTime>,
    heading: Mailbox<f32>,
    altitude: Mailbox<f32>,
    battery: Mailbox<u8>,
}

impl PhoneSensors {
    /// One `CLLocation`. Its timestamp stamps the wall clock, as the board's GPS does, and a fix
    /// without one leaves the clock alone.
    pub fn push_fix(&mut self, fix: Fix, unix_secs: Option<u32>) {
        self.fix.push(fix);
        if let Some(unix) = unix_secs {
            // `DateTime` is minute-resolution, so the seconds into the minute ride beside it.
            self.clock.push(GpsTime { utc: DateTime::from_unix(unix), second: (unix % 60) as u8 });
        }
    }

    /// `CLHeading.trueHeading`, in degrees clockwise from north. CoreLocation reports an invalid
    /// heading as a negative value, which is the absence of a direction, so dropping it leaves the
    /// app on its last real heading.
    pub fn push_heading(&mut self, degrees: f32) {
        if !degrees.is_finite() || degrees < 0.0 {
            return;
        }
        self.heading.push(degrees.rem_euclid(360.0));
    }

    /// `CMAltimeter` absolute altitude in metres. There is no cadence latch, because CoreMotion
    /// delivers at its own rate and the app's staleness gate covers a gap.
    pub fn push_altitude(&mut self, metres: f32) {
        self.altitude.push(metres);
    }

    /// `UIDevice.batteryLevel` as whole percent.
    pub fn push_battery(&mut self, percent: u8) {
        self.battery.push(percent.min(100));
    }

    /// The mailboxes as one pass's ports.
    pub(crate) fn ports(&mut self) -> Sensors<'_> {
        Sensors {
            clock: Some(&mut self.clock),
            compass: Some(&mut self.heading),
            altimeter: Some(&mut self.altitude),
            fuel: Some(&mut self.battery),
            ..Sensors::new(&mut self.fix)
        }
    }
}
