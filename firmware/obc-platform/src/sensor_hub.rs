//! The **instance-owned sensor hub** — the board-agnostic embassy-sync bridge between a board's
//! high-priority sensor task, its BLE central manager / debug-uart injection, and the app's `poll`.
//!
//! This is the successor to the former module-global `sensor_link` + `sensor_values` mailboxes
//! (issue #808): one [`SensorHub`] owns every semantic stream as a field, is constructed once in
//! static storage at board composition, and is split into typed producer/consumer/control handles
//! that each borrow it. Nothing here is a process-global singleton, so a host test constructs as
//! many independent hubs as it likes without shared state (see the tests at the bottom).
//!
//! ## The streams (one [`Signal`] mailbox each)
//!
//! - **GPS fix**, **barometric altitude**, **temperature** — published *coherently* by the sensor
//!   task on each valid fix (the baro is read on the fix), so altitude/temperature share the fix's
//!   instant. Fresh-fix mailbox: `try_take` yields once, so a source's `poll` returns `Some` only
//!   on the tick a sample arrived and `None` between — zero I²C at the frame rate, no teleport on a
//!   stale fix.
//! - **GPS time** — published on any NAV-PVT whose time the receiver resolved, **independent of the
//!   position fix**, so the clock can set during acquisition (before a 3D lock).
//! - **Heading** — the electronic-compass heading, on its own cadence while the rider is stopped;
//!   independent of the GPS course.
//! - **HR / power / cadence** — the raw-value BLE sensors (epic #707). **Two producers, one
//!   mailbox each:** the board's BLE central manager (SE6) *and* the `debug-uart` injection path
//!   (SE8) both publish through the same [`SampleInjector`] — **last-writer-wins**, exactly what a
//!   bench wants when a real strap and an injected line coexist. The app can't tell them apart.
//! - **Rate / GPS-power** — control latches the *ride loop* sets ([`SensorControl`]) and the sensor
//!   task awaits ([`SensorTaskLink`]); only the newest value matters.
//! - **Event** — one payload-less "a datapoint arrived" wake, pulsed by **every** publish above.
//!   The event-driven ride loop selects on [`SensorConsumer::wait_event`] so **one** await covers
//!   the whole set — the fix plus the independently-published heading, GPS time, and BLE samples —
//!   then drains whichever per-stream mailboxes have data via the normal `poll` path. Waiting here
//!   never *steals* a value: it is a separate signal, so `FIX` et al. stay for the source polls.
//! - **Presence** — the boot I²C probe result, published once by the sensor task, drained once by
//!   the ride loop → an on-glass warning for any absent module (issue #504).
//!
//! ## Ownership (who holds which handle — all wired in board composition)
//!
//! | Handle | Held by | Does |
//! |---|---|---|
//! | [`SensorTaskLink`] | the I²C sensor task (`sensors::sensor_task`) | publishes fix/alt/temp/time/heading/presence; awaits rate/power |
//! | [`SampleInjector`] | the BLE central manager **and** the debug-uart RX task | publishes HR/power/cadence (last-writer-wins) |
//! | [`SensorConsumer`] | the ride loop (`run_app`) | the `*Source` drains + presence drain + the one event wake |
//! | [`SensorControl`] | the ride loop (`run_app`) | sets the GPS rate + power latches |
//!
//! The pure decode this bridges lives in the always-compiled `obc_sensors` / `obc-ble` crates; only
//! this embassy-sync plumbing pulls `embassy-sync`, so it is gated behind the `sensor-link` feature.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_ports::{
    AltimeterSource, CadenceSource, ClockSource, CompassSource, Fix, GpsTime, HeartRateSource, LocationSource,
    PowerSource, TemperatureSource,
};

/// The one raw-mutex `Signal` type every stream in the hub uses. `CriticalSectionRawMutex` because
/// producers (the sensor task, the BLE manager, the debug RX task) and the consumer (the ride loop)
/// run on different executors / priorities on the board.
type Sig<T> = Signal<CriticalSectionRawMutex, T>;

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

/// The instance-owned sensor hand-off: every cross-task sensor stream as a field, so the board owns
/// exactly one and hands out typed handles. Construct it once in static storage
/// (`static HUB: SensorHub = SensorHub::new();`) and derive handles with the `*` accessors; a host
/// test constructs it as a plain local (`let hub = SensorHub::new();`) — no shared global state.
pub struct SensorHub {
    /// Latest GPS fix (valid NAV-PVT), fresh-fix — drained by [`GpsLocation`].
    fix: Sig<Fix>,
    /// Latest barometric altitude (metres), coherent with [`SensorHub::fix`] — drained by [`BaroAltimeter`].
    alt: Sig<f32>,
    /// Latest ambient temperature (°C) from the BMP581's per-fix reading — drained by [`SensorTemp`].
    temp: Sig<f32>,
    /// Latest GPS UTC time, published on any resolved-time NAV-PVT **independent of the position
    /// fix** so the clock can set during acquisition — drained by [`GpsClock`].
    gps_time: Sig<GpsTime>,
    /// Latest electronic-compass heading (degrees CW from north), independent of the GPS course —
    /// drained by [`MagCompass`].
    heading: Sig<f32>,
    /// Latest heart rate (bpm), fresh-mailbox last-writer-wins — drained by [`SensorHr`].
    hr: Sig<u16>,
    /// Latest power (watts, non-negative — a signed meter reading is clamped at 0 by the producer) —
    /// drained by [`SensorPower`].
    power: Sig<u16>,
    /// Latest cadence (rpm) — a coasting rider publishes a fresh `0` (feet still), distinct from no
    /// sample at all — drained by [`SensorCadence`].
    cadence: Sig<u8>,
    /// Desired GPS fix interval (seconds) — a latch the ride loop sets and the sensor task awaits.
    rate: Sig<u16>,
    /// Desired GPS power state — a latch the ride loop sets and the sensor task awaits.
    gps_power: Sig<GpsPower>,
    /// A single "a datapoint arrived" wake, pulsed by every publish. The event-driven ride loop
    /// selects on it so one await covers the whole set, then drains the typed mailboxes via `poll`.
    /// Payload-less — purely the "wake the render" edge — and separate from the value mailboxes, so
    /// waiting here never steals a fix from [`GpsLocation::poll`].
    event: Sig<()>,
    /// Which sensors answered the boot I²C probe — published once, drained once (fresh-mailbox).
    presence: Sig<SensorPresence>,
}

impl Default for SensorHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorHub {
    /// A fresh hub with every mailbox empty. `const` so it can live in a `static` at board
    /// composition; also usable as a plain local in host tests.
    pub const fn new() -> Self {
        SensorHub {
            fix: Signal::new(),
            alt: Signal::new(),
            temp: Signal::new(),
            gps_time: Signal::new(),
            heading: Signal::new(),
            hr: Signal::new(),
            power: Signal::new(),
            cadence: Signal::new(),
            rate: Signal::new(),
            gps_power: Signal::new(),
            event: Signal::new(),
            presence: Signal::new(),
        }
    }

    /// The I²C sensor task's handle: publish fix/alt/temp/time/heading/presence, await rate/power.
    pub fn task_link(&self) -> SensorTaskLink<'_> {
        SensorTaskLink(self)
    }

    /// The HR/power/cadence injector — held by both the BLE central manager and the debug-uart RX
    /// task (last-writer-wins into one mailbox each).
    pub fn injector(&self) -> SampleInjector<'_> {
        SampleInjector(self)
    }

    /// The ride loop's consumer handle: the `*Source` drains, the presence drain, and the one
    /// event wake it selects on.
    pub fn consumer(&self) -> SensorConsumer<'_> {
        SensorConsumer(self)
    }

    /// The ride loop's control handle: set the GPS rate + power latches the sensor task awaits.
    pub fn control(&self) -> SensorControl<'_> {
        SensorControl(self)
    }
}

// ============================ Producer: the I²C sensor task ============================

/// The board's high-priority I²C sensor task's handle into the hub. It **publishes** each coherent
/// datapoint (each publish pulses the shared event so the ride loop wakes) and **awaits** the ride
/// loop's rate/power control latches. `Copy` — a bare `&SensorHub`.
#[derive(Clone, Copy)]
pub struct SensorTaskLink<'a>(&'a SensorHub);

impl SensorTaskLink<'_> {
    /// Publish a fresh GPS [`Fix`] (on a valid NAV-PVT) and pulse the event so the loop wakes.
    pub fn dispatch_fix(&self, f: Fix) {
        self.0.fix.signal(f);
        self.0.event.signal(());
    }

    /// Publish a fresh barometric altitude in metres (coherent with the fix).
    pub fn dispatch_alt(&self, m: f32) {
        self.0.alt.signal(m);
        self.0.event.signal(());
    }

    /// Publish a fresh ambient temperature in °C (from the same BMP581 read).
    pub fn dispatch_temp(&self, c: f32) {
        self.0.temp.signal(c);
        self.0.event.signal(());
    }

    /// Publish a fresh GPS UTC time (on a NAV-PVT with resolved time — independent of a position fix).
    pub fn dispatch_time(&self, t: GpsTime) {
        self.0.gps_time.signal(t);
        self.0.event.signal(());
    }

    /// Publish a fresh compass heading in degrees CW from north (from the magnetometer read with each fix).
    pub fn dispatch_heading(&self, deg: f32) {
        self.0.heading.signal(deg);
        self.0.event.signal(());
    }

    /// Publish the boot probe result (once, after the sensor task probes all three chips). Pulses the
    /// event so the ride loop wakes and drains it via [`SensorConsumer::take_presence`].
    pub fn dispatch_presence(&self, p: SensorPresence) {
        self.0.presence.signal(p);
        self.0.event.signal(());
    }

    /// Await the next requested fix interval (seconds) — the task selects on this to apply a rate
    /// change without sharing the I²C bus with the ride loop.
    pub async fn wait_rate(&self) -> u16 {
        self.0.rate.wait().await
    }

    /// Await the next requested GPS power state — the task selects on this to sleep when a ride ends
    /// and wake (warm) when one starts.
    pub async fn wait_power(&self) -> GpsPower {
        self.0.gps_power.wait().await
    }
}

// ============================ Producer: BLE / debug HR-power-cadence ============================

/// The HR/power/cadence injector — the seam the issue calls out for *explicit* ownership of BLE- and
/// debug-injected samples. Both the board's BLE central manager (SE6) and the `debug-uart` injection
/// path (SE8) hold one over the same hub, so the app's `Sensors` wiring is identical whichever is
/// feeding — **last-writer-wins**. Each dispatch pulses the shared event. `Copy`.
#[derive(Clone, Copy)]
pub struct SampleInjector<'a>(&'a SensorHub);

impl SampleInjector<'_> {
    /// Publish a fresh heart-rate sample (bpm) and pulse the shared event so the loop wakes.
    pub fn dispatch_hr(&self, bpm: u16) {
        self.0.hr.signal(bpm);
        self.0.event.signal(());
    }

    /// Publish a fresh power sample (watts). Non-negative — a signed meter reading is clamped at 0
    /// by the producer.
    pub fn dispatch_power(&self, watts: u16) {
        self.0.power.signal(watts);
        self.0.event.signal(());
    }

    /// Publish a fresh cadence sample (rpm). A coasting rider publishes a fresh `0` (feet still),
    /// distinct from no sample at all (the mailbox staying empty).
    pub fn dispatch_cadence(&self, rpm: u8) {
        self.0.cadence.signal(rpm);
        self.0.event.signal(());
    }
}

// ============================ Control: the ride loop ============================

/// The ride loop's control handle: the GPS rate + power *latches* the sensor task awaits. Only the
/// newest value of each matters (a latch, not a queue). `Copy`.
#[derive(Clone, Copy)]
pub struct SensorControl<'a>(&'a SensorHub);

impl SensorControl<'_> {
    /// Request a new GPS fix interval (seconds); the sensor task reconfigures the M10 on the next
    /// [`SensorTaskLink::wait_rate`].
    pub fn set_rate(&self, secs: u16) {
        self.0.rate.signal(secs);
    }

    /// Request a GPS power state; the sensor task transitions the M10 (sleep / wake / power mode) on
    /// the next [`SensorTaskLink::wait_power`].
    pub fn set_power(&self, p: GpsPower) {
        self.0.gps_power.signal(p);
    }
}

// ============================ Consumer: the app poll ============================

/// The ride loop's consumer handle: hands out the app-facing `*Source` drains, drains the boot
/// presence once, and exposes the single event wake the event-driven loop selects on. The `*Source`
/// accessors return handles bound to the hub's lifetime (not this handle's borrow), so the sources
/// the `Sensors` set holds outlive the transient consumer. `Copy`.
#[derive(Clone, Copy)]
pub struct SensorConsumer<'a>(&'a SensorHub);

impl<'a> SensorConsumer<'a> {
    /// The user's location from the real GPS. Hand `&mut` to `Sensors::loc`.
    pub fn location(&self) -> GpsLocation<'a> {
        GpsLocation(&self.0.fix)
    }

    /// The barometric altimeter from the real BMP581. Hand `&mut` to `Sensors::altimeter`.
    pub fn altimeter(&self) -> BaroAltimeter<'a> {
        BaroAltimeter(&self.0.alt)
    }

    /// Ambient temperature from the BMP581. Hand `&mut` to `Sensors::temperature`.
    pub fn temperature(&self) -> SensorTemp<'a> {
        SensorTemp(&self.0.temp)
    }

    /// The GPS UTC clock from the real receiver. Hand `&mut` to `Sensors::clock`.
    pub fn clock(&self) -> GpsClock<'a> {
        GpsClock(&self.0.gps_time)
    }

    /// The electronic compass from the real magnetometer. Hand `&mut` to `Sensors::compass`.
    pub fn compass(&self) -> MagCompass<'a> {
        MagCompass(&self.0.heading)
    }

    /// The rider's heart rate (BLE / injected). Hand `&mut` to `Sensors::hr`.
    pub fn hr(&self) -> SensorHr<'a> {
        SensorHr(&self.0.hr)
    }

    /// The rider's power (BLE / injected). Hand `&mut` to `Sensors::power`.
    pub fn power(&self) -> SensorPower<'a> {
        SensorPower(&self.0.power)
    }

    /// The rider's cadence (BLE / injected). Hand `&mut` to `Sensors::cadence`.
    pub fn cadence(&self) -> SensorCadence<'a> {
        SensorCadence(&self.0.cadence)
    }

    /// Drain the boot probe result — `Some` exactly once, on the pass after the task publishes it,
    /// then `None`. The ride loop maps any absent sensor to a warning flag (issue #504).
    pub fn take_presence(&self) -> Option<SensorPresence> {
        self.0.presence.try_take()
    }

    /// Await the next *any-sensor* datapoint — the single wake the event-driven loop selects on.
    /// Completes on any publish, so one await covers the whole set; the loop then drains the typed
    /// mailboxes via `poll`. Consuming a value mailbox here would steal it from the source polls, so
    /// this is a separate payload-less signal.
    pub async fn wait_event(&self) {
        self.0.event.wait().await
    }
}

// ============================ The app-facing `*Source` drains ============================
//
// Each holds a borrow of its one mailbox and drains it on the fresh-mailbox contract (`try_take`
// yields a value once) — so `poll` returns `Some` only on the tick a new sample arrived and `None`
// between, the cadence a real ~1 Hz receiver / strap follows. The app's staleness gate then renders
// a dropped stream as `--`. Generic names (`SensorHr`, not `BleHr`) because both the radio manager
// and the debug-uart injection feed the same mailbox.

/// The user's location. See [`SensorConsumer::location`].
pub struct GpsLocation<'a>(&'a Sig<Fix>);
impl LocationSource for GpsLocation<'_> {
    fn poll(&mut self) -> Option<Fix> {
        self.0.try_take()
    }
}

/// The barometric altimeter. See [`SensorConsumer::altimeter`].
pub struct BaroAltimeter<'a>(&'a Sig<f32>);
impl AltimeterSource for BaroAltimeter<'_> {
    fn poll(&mut self) -> Option<f32> {
        self.0.try_take()
    }
}

/// Ambient temperature. See [`SensorConsumer::temperature`].
pub struct SensorTemp<'a>(&'a Sig<f32>);
impl TemperatureSource for SensorTemp<'_> {
    fn poll(&mut self) -> Option<f32> {
        self.0.try_take()
    }
}

/// The GPS UTC clock. See [`SensorConsumer::clock`].
pub struct GpsClock<'a>(&'a Sig<GpsTime>);
impl ClockSource for GpsClock<'_> {
    fn poll(&mut self) -> Option<GpsTime> {
        self.0.try_take()
    }
}

/// The electronic compass. See [`SensorConsumer::compass`].
pub struct MagCompass<'a>(&'a Sig<f32>);
impl CompassSource for MagCompass<'_> {
    fn poll(&mut self) -> Option<f32> {
        self.0.try_take()
    }
}

/// The rider's heart rate. See [`SensorConsumer::hr`].
pub struct SensorHr<'a>(&'a Sig<u16>);
impl HeartRateSource for SensorHr<'_> {
    fn poll(&mut self) -> Option<u16> {
        self.0.try_take()
    }
}

/// The rider's power. See [`SensorConsumer::power`].
pub struct SensorPower<'a>(&'a Sig<u16>);
impl PowerSource for SensorPower<'_> {
    fn poll(&mut self) -> Option<u16> {
        self.0.try_take()
    }
}

/// The rider's cadence. See [`SensorConsumer::cadence`].
pub struct SensorCadence<'a>(&'a Sig<u8>);
impl CadenceSource for SensorCadence<'_> {
    fn poll(&mut self) -> Option<u8> {
        self.0.try_take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Multiple independent hubs in one test, no shared global state — the acceptance criterion the
    // old module-global mailboxes could not meet. A dispatch on one hub is invisible to the other.
    #[test]
    fn hubs_are_independent_instances() {
        let a = SensorHub::new();
        let b = SensorHub::new();
        let (ia, ca) = (a.injector(), a.consumer());
        let cb = b.consumer();

        ia.dispatch_hr(158);
        assert_eq!(ca.hr().poll(), Some(158), "hub a drains its own value once");
        assert_eq!(cb.hr().poll(), None, "hub b is untouched by a dispatch on hub a");
        assert_eq!(ca.hr().poll(), None, "fresh-mailbox: empty again until the next dispatch");
    }

    #[test]
    fn hr_mailbox_drains_once() {
        let hub = SensorHub::new();
        let mut src = hub.consumer().hr();
        assert_eq!(src.poll(), None, "empty until a dispatch");
        hub.injector().dispatch_hr(158);
        assert_eq!(src.poll(), Some(158), "drains the published value once");
        assert_eq!(src.poll(), None, "fresh-mailbox: empty again until the next dispatch");
    }

    #[test]
    fn power_mailbox_last_writer_wins() {
        let hub = SensorHub::new();
        let inj = hub.injector();
        inj.dispatch_power(200);
        inj.dispatch_power(275); // a second producer overwrites before the app drains
        assert_eq!(hub.consumer().power().poll(), Some(275), "last-writer-wins in the shared mailbox");
        assert_eq!(hub.consumer().power().poll(), None);
    }

    #[test]
    fn cadence_zero_is_a_real_sample() {
        let hub = SensorHub::new();
        hub.injector().dispatch_cadence(0); // coasting — feet still, a fresh 0, not "no sensor"
        assert_eq!(hub.consumer().cadence().poll(), Some(0), "a coasting 0 is a real reading, distinct from empty");
        assert_eq!(hub.consumer().cadence().poll(), None);
    }

    // The GPS streams are independent mailboxes: a fix does not disturb heading or time, and time is
    // publishable *before* a fix (during acquisition), which the source drains independently.
    #[test]
    fn gps_streams_are_independent() {
        let hub = SensorHub::new();
        let link = hub.task_link();
        let consumer = hub.consumer();

        // Time before any fix — the acquisition case.
        link.dispatch_time(GpsTime { utc: obc_ports::DateTime::default(), second: 0 });
        assert!(consumer.clock().poll().is_some(), "GPS time drains independent of a position fix");
        assert_eq!(consumer.location().poll(), None, "no fix was published");

        link.dispatch_heading(90.0);
        assert_eq!(consumer.compass().poll(), Some(90.0), "heading drains on its own");
        assert!(consumer.clock().poll().is_none(), "the time mailbox was already drained, undisturbed by heading");
    }

    // A publish must leave the shared event signalled so the ride loop's single `wait_event` wakes,
    // yet draining a value mailbox must NOT consume that wake — they are separate signals. A busy
    // poll of the `wait_event` future proves it is ready after a dispatch and that the value survives.
    #[test]
    fn any_publish_signals_the_event_without_stealing_values() {
        use core::future::Future;
        use core::pin::pin;
        use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        // A no-op waker so we can poll the `wait_event` future once on the host.
        const VT: RawWakerVTable =
            RawWakerVTable::new(|_| RawWaker::new(core::ptr::null(), &VT), |_| {}, |_| {}, |_| {});
        let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VT)) };
        let mut cx = Context::from_waker(&waker);

        let hub = SensorHub::new();
        let consumer = hub.consumer();
        assert!(
            matches!(pin!(consumer.wait_event()).as_mut().poll(&mut cx), Poll::Pending),
            "no event before a publish"
        );

        hub.injector().dispatch_hr(140);
        assert!(
            matches!(pin!(consumer.wait_event()).as_mut().poll(&mut cx), Poll::Ready(())),
            "a publish wakes the loop"
        );
        // The value survived the wake — waiting on the event never steals a source's sample.
        assert_eq!(consumer.hr().poll(), Some(140), "the HR sample is still there for the source poll");
    }
}
