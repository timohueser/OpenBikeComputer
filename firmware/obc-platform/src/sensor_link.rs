//! The cross-task hand-off for the **real** GPS + altimeter sensors — the board-agnostic
//! embassy-sync bridge between the board's high-priority sensor task and the app's `poll`.
//!
//! The real-hardware sibling of [`crate::debug_link`]'s `handoff`: a high-priority embassy task
//! drives the I²C bus (SAM-M10Q GPS + BMP581 baro + ICM-20948 magnetometer) and on each coherent
//! sample [`signal`](embassy_sync::signal::Signal)s the values across to these statics. The
//! HAL-trait impls here just **drain** them with `try_take`, so each source's `poll` yields `Some`
//! only on the tick a fresh sample arrived and `None` between — the fresh-fix mailbox semantics the
//! seam documents, with zero I²C traffic at the frame rate and no teleport on a stale fix.
//!
//! The pure decode this bridges lives in the always-compiled [`crate::ubx`] / [`crate::bmp581`] /
//! [`crate::compass`] / [`crate::icm20948`] modules; only this embassy-sync plumbing pulls
//! `embassy-sync`, so it is gated behind the `sensor-link` feature.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_ports::{AltimeterSource, ClockSource, CompassSource, Fix, GpsTime, LocationSource, TemperatureSource};

/// Latest GPS fix, fresh-fix semantics (`try_take` yields it once) — set by the sensor task on a
/// valid NAV-PVT, drained by [`GpsLocation`]. **Public** so the event-driven main loop (issue #219)
/// can `select` on [`wait_fix`] directly, waking the render exactly when a fix lands.
static FIX: Signal<CriticalSectionRawMutex, Fix> = Signal::new();
/// Latest barometric altitude (metres), set coherently with [`FIX`] (the baro is read on the GPS
/// fix), drained by [`BaroAltimeter`].
static ALT: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Latest ambient temperature (°C) from the BMP581's per-fix reading, drained by [`SensorTemp`].
static TEMP: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Latest GPS UTC time, set on any NAV-PVT whose time the receiver has fully resolved —
/// **independent of the position fix** ([`FIX`]), so the clock can set during acquisition (before a
/// 3D lock). Drained by [`GpsClock`].
static GPS_TIME: Signal<CriticalSectionRawMutex, GpsTime> = Signal::new();
/// Latest electronic-compass heading (degrees CW from north), set from the magnetometer read
/// coincident with each fix, drained by [`MagCompass`]. Independent of the GPS course — the heading
/// the app uses while stopped.
static HEADING: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Desired GPS fix interval (seconds) — set by the ride loop, awaited by the sensor task
/// ([`wait_rate`]) to re-issue the M10 `CFG-RATE` VALSET. A latch: only the newest rate matters.
static RATE: Signal<CriticalSectionRawMutex, u16> = Signal::new();
/// Desired GPS power state — set by the ride loop from tracking state + the `power_saver` toggle,
/// awaited by the sensor task ([`wait_power`]). A latch: only the newest state matters.
static POWER: Signal<CriticalSectionRawMutex, GpsPower> = Signal::new();
/// A single "a datapoint arrived" wake, pulsed by **every** `dispatch_*` above. The event-driven
/// main loop selects on [`wait_event`] so **one** await covers the whole sensor set — the fix plus
/// the independently-published heading and GPS time — then drains whichever per-source mailboxes
/// have data via the normal `poll` path. Payload-less: purely the "wake the render" edge.
static EVENT: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Which sensors answered the boot I²C probe, published **once** by the sensor task after it probes
/// all three chips, drained once by the ride loop ([`take_presence`]) → an on-glass warning for any
/// that are absent (issue #504). Fresh-mailbox like the sample signals: `try_take` yields it once.
static PRESENCE: Signal<CriticalSectionRawMutex, SensorPresence> = Signal::new();

/// Which sensors answered the boot I²C probe — the sensor task's probe results, carried to the app
/// so a missing module surfaces as a dismissable warning rather than only an RTT line. A missing
/// GPS is distinct from "no fix yet" (the receiver is there, just no sky): this is the *module*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorPresence {
    /// The SAM-M10Q GPS answered its probe.
    pub gps: bool,
    /// The BMP581 barometric altimeter answered its probe.
    pub altimeter: bool,
    /// The ICM-20948 (compass / IMU) answered its probe.
    pub compass: bool,
}

/// The GPS receiver's requested power state. The ride loop derives one from whether a ride is active
/// and the `power_saver` toggle, and the sensor task drives the M10 to match: deep sleep when idle
/// (~µA vs. the ~20 mA of continuous tracking), full-power fixes while riding, or the M10's on-chip
/// low-power tracking when `power_saver` is on.
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

/// Publish a fresh GPS [`Fix`] (on a valid NAV-PVT) and pulse [`EVENT`] so the event-driven loop
/// wakes.
pub fn dispatch_fix(f: Fix) {
    FIX.signal(f);
    EVENT.signal(());
}

/// Publish a fresh barometric altitude in metres (coherent with the fix).
pub fn dispatch_alt(m: f32) {
    ALT.signal(m);
    EVENT.signal(());
}

/// Publish a fresh ambient temperature in °C (from the same BMP581 read).
pub fn dispatch_temp(c: f32) {
    TEMP.signal(c);
    EVENT.signal(());
}

/// Publish a fresh GPS UTC time (on a NAV-PVT with resolved time — independent of a position fix).
pub fn dispatch_time(t: GpsTime) {
    GPS_TIME.signal(t);
    EVENT.signal(());
}

/// Publish a fresh compass heading in degrees CW from north (from the magnetometer read with each
/// fix).
pub fn dispatch_heading(deg: f32) {
    HEADING.signal(deg);
    EVENT.signal(());
}

/// Publish the boot probe result (once, after the sensor task probes all three chips). Pulses
/// [`EVENT`] so the event-driven ride loop wakes and drains it via [`take_presence`].
pub fn dispatch_presence(p: SensorPresence) {
    PRESENCE.signal(p);
    EVENT.signal(());
}

/// Drain the boot probe result — `Some` exactly once, on the pass after [`dispatch_presence`], then
/// `None`. The ride loop maps any absent sensor to a warning flag.
pub fn take_presence() -> Option<SensorPresence> {
    PRESENCE.try_take()
}

/// Request a new GPS fix interval (seconds); the sensor task reconfigures the M10 on the next
/// [`wait_rate`].
pub fn set_rate(secs: u16) {
    RATE.signal(secs);
}

/// Request a GPS power state; the sensor task transitions the M10 (sleep / wake / power mode) on the
/// next [`wait_power`].
pub fn set_power(p: GpsPower) {
    POWER.signal(p);
}

/// Await the next published fix — consumes the signal like [`GpsLocation::poll`], so a loop that
/// waits here drives the source instead of polling it.
pub async fn wait_fix() -> Fix {
    FIX.wait().await
}

/// Pulse the shared "a datapoint arrived" wake ([`EVENT`]) without publishing a GPS value — the hook
/// [`crate::sensor_values`] uses so a BLE-fed HR/power/cadence sample wakes the same event-driven
/// ride loop the GPS fix does. `pub(crate)` so only the sibling sensor mailboxes reach it; external
/// callers publish through a typed `dispatch_*`.
pub(crate) fn wake_event() {
    EVENT.signal(());
}

/// Await the next *any-sensor* datapoint — the single wake the event-driven main loop selects on.
/// Completes on any `dispatch_*`, so one await covers the whole set; the loop then drains the typed
/// mailboxes via `poll`. Prefer this over [`wait_fix`]: a heading- or time-only update (no position
/// fix) must still wake the render, and consuming `FIX` here would steal it from
/// [`GpsLocation::poll`].
pub async fn wait_event() {
    EVENT.wait().await
}

/// Await the next requested fix interval (seconds) — the sensor task selects on this to apply a rate
/// change without sharing the I²C bus with the ride loop.
pub async fn wait_rate() -> u16 {
    RATE.wait().await
}

/// Await the next requested GPS power state — the sensor task selects on this to sleep when a ride
/// ends and wake (warm) when one starts.
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
