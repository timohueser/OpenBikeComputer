//! The cross-task hand-off for the **real** GPS + altimeter sensors (issue #218) — the
//! board-agnostic embassy-sync bridge between the board's high-priority sensor task and the app's
//! `poll`.
//!
//! This is the real-hardware sibling of [`crate::debug_link`]'s `handoff`: a high-priority embassy
//! task in the board crate drives the I²C bus (SAM-M10Q GPS + BMP581 baro + an ICM-20948 magnetometer),
//! and on each coherent sample [`signal`](embassy_sync::signal::Signal)s the values across to these
//! statics. The HAL-trait impls here — [`GpsLocation`], [`BaroAltimeter`], [`SensorTemp`], [`GpsClock`],
//! [`MagCompass`] — just **drain** them with `try_take`, so the app's `LocationSource::poll` /
//! `AltimeterSource::poll` / `TemperatureSource::poll` / `ClockSource::poll` / `CompassSource::poll`
//! yield `Some` only on the tick a fresh sample arrived and `None` between (a cold start, a GPS
//! dropout, or the gap between fixes). That gives the app the exact fresh-fix mailbox semantics the
//! seam documents — **zero I²C traffic at the frame rate**, and no teleport on a stale fix (issue
//! #43) — for free.
//!
//! The pure decode this bridges (UBX framing, NAV-PVT → [`Fix`], BMP581 raw → metres, magnetometer
//! axes → heading) lives in the always-compiled [`crate::ubx`] / [`crate::bmp581`] / [`crate::compass`]
//! / [`crate::icm20948`] modules; only this embassy-sync plumbing pulls `embassy-sync`, so it is gated
//! behind the `sensor-link` feature (the board firmware enables it in its default features; the host
//! workspace never pulls it).

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_app::{AltimeterSource, ClockSource, CompassSource, Fix, GpsTime, LocationSource, TemperatureSource};

/// Latest GPS fix, fresh-fix semantics (`try_take` yields it once) — set by the sensor task on a
/// valid NAV-PVT, drained by [`GpsLocation`]. **Public** so the event-driven main loop (issue #219)
/// can `select` on [`wait_fix`] directly, waking the render exactly when a fix lands.
static FIX: Signal<CriticalSectionRawMutex, Fix> = Signal::new();
/// Latest barometric altitude (metres), set coherently with [`FIX`] (the baro is read on the GPS
/// fix), drained by [`BaroAltimeter`].
static ALT: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Latest ambient temperature (°C) from the BMP581's per-fix reading, drained by [`SensorTemp`].
static TEMP: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Latest GPS UTC time (issue #223), set by the sensor task on any NAV-PVT whose time the receiver
/// has fully resolved — **independent of the position fix** ([`FIX`]), so the clock can set during
/// acquisition (before a 3D lock). Drained by [`GpsClock`].
static GPS_TIME: Signal<CriticalSectionRawMutex, GpsTime> = Signal::new();
/// Latest electronic-compass heading (degrees CW from north), set by the sensor task from the
/// magnetometer read coincident with each fix, drained by [`MagCompass`]. Independent of the GPS
/// course — it's the heading the app uses while stopped.
static HEADING: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Desired GPS fix interval (seconds) — set by the ride loop when the #117 Power-screen setting
/// changes, awaited by the sensor task ([`wait_rate`]) to re-issue the M10 `CFG-RATE` VALSET. A
/// `Signal` (latch), not a queue: only the newest requested rate matters.
static RATE: Signal<CriticalSectionRawMutex, u16> = Signal::new();
/// Desired GPS power state (issue #225) — set by the ride loop from the tracking state + the
/// `power_saver` toggle, awaited by the sensor task ([`wait_power`]). A `Signal` (latch): only the
/// newest requested state matters.
static POWER: Signal<CriticalSectionRawMutex, GpsPower> = Signal::new();

/// The GPS receiver's requested power state (issue #225). The ride loop derives one from whether a
/// ride is active and the Power-screen `power_saver` toggle, and the sensor task drives the M10 to
/// match: deep sleep when idle (so an idle device draws ~µA, not the ~20 mA of continuous tracking),
/// full-power fixes while riding, or the M10's on-chip low-power tracking when `power_saver` is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpsPower {
    /// Riding, full-power continuous fixes at the configured rate.
    Active,
    /// Riding with `power_saver` on — the M10's low-power tracking mode (lower power, same rate, at
    /// the cost of some fix latency).
    LowPower,
    /// Not tracking — deep sleep (`RXM-PMREQ` backup); woken on the next [`Active`](GpsPower::Active)
    /// / [`LowPower`](GpsPower::LowPower) request for a fast warm fix.
    Sleep,
}

/// Publish a fresh GPS [`Fix`] (the sensor task, on a valid NAV-PVT). Overwrites any unconsumed
/// value — the app only wants the freshest.
pub fn dispatch_fix(f: Fix) {
    FIX.signal(f);
}

/// Publish a fresh barometric altitude in metres (the sensor task, coherent with the fix).
pub fn dispatch_alt(m: f32) {
    ALT.signal(m);
}

/// Publish a fresh ambient temperature in °C (the sensor task, from the same BMP581 read).
pub fn dispatch_temp(c: f32) {
    TEMP.signal(c);
}

/// Publish a fresh GPS UTC time (the sensor task, on a NAV-PVT with resolved time — independent of
/// a valid position fix). Overwrites any unconsumed value; the app only wants the freshest.
pub fn dispatch_time(t: GpsTime) {
    GPS_TIME.signal(t);
}

/// Publish a fresh compass heading in degrees CW from north (the sensor task, from the magnetometer
/// read taken with each fix). Overwrites any unconsumed value; the app only wants the freshest.
pub fn dispatch_heading(deg: f32) {
    HEADING.signal(deg);
}

/// Request a new GPS fix interval (seconds) — the ride loop calls this when the persisted
/// `fix_interval_s` setting changes; the sensor task reconfigures the M10 on the next [`wait_rate`].
pub fn set_rate(secs: u16) {
    RATE.signal(secs);
}

/// Request a GPS power state (issue #225) — the ride loop calls this when the ride starts/stops or
/// `power_saver` changes; the sensor task transitions the M10 (sleep / wake / power mode) on the
/// next [`wait_power`].
pub fn set_power(p: GpsPower) {
    POWER.signal(p);
}

/// Await the next published fix — for the event-driven main loop (issue #219) to `select` on, so a
/// fix both updates state and wakes the render exactly once. (Consumes the signal like
/// [`GpsLocation::poll`]; a loop that waits here drives the source instead of polling it.)
pub async fn wait_fix() -> Fix {
    FIX.wait().await
}

/// Await the next requested fix interval (seconds) — the sensor task selects on this to apply a
/// #117 rate change without sharing the I²C bus with the ride loop.
pub async fn wait_rate() -> u16 {
    RATE.wait().await
}

/// Await the next requested GPS power state (issue #225) — the sensor task selects on this to sleep
/// when a ride ends and wake (warm) when one starts.
pub async fn wait_power() -> GpsPower {
    POWER.wait().await
}

/// The user's location from the real GPS. Hand `&mut GpsLocation` to `Sensors::loc`.
pub struct GpsLocation;
impl LocationSource for GpsLocation {
    fn poll(&mut self) -> Option<Fix> {
        FIX.try_take()
    }
}

/// The barometric altimeter from the real BMP581. Hand `&mut BaroAltimeter` to `Sensors::altimeter`.
pub struct BaroAltimeter;
impl AltimeterSource for BaroAltimeter {
    fn poll(&mut self) -> Option<f32> {
        ALT.try_take()
    }
}

/// Ambient temperature from the BMP581. Hand `&mut SensorTemp` to `Sensors::temperature`.
pub struct SensorTemp;
impl TemperatureSource for SensorTemp {
    fn poll(&mut self) -> Option<f32> {
        TEMP.try_take()
    }
}

/// The GPS UTC clock from the real receiver. Hand `&mut GpsClock` to `Sensors::clock`; the app
/// stamps the wall clock from it when "Set from GPS" is on (issue #223).
pub struct GpsClock;
impl ClockSource for GpsClock {
    fn poll(&mut self) -> Option<GpsTime> {
        GPS_TIME.try_take()
    }
}

/// The electronic compass from the real magnetometer. Hand `&mut MagCompass` to `Sensors::compass`;
/// the app adopts it as the heading-up orientation while the rider is stopped (no GPS course).
pub struct MagCompass;
impl CompassSource for MagCompass {
    fn poll(&mut self) -> Option<f32> {
        HEADING.try_take()
    }
}
