//! The instance-owned sensor hub: the board-agnostic embassy-sync bridge between a board's
//! high-priority sensor task, its BLE central manager or debug-uart injection, and the app's
//! `poll`.
//!
//! One [`SensorHub`] owns every semantic stream as a field, is constructed once in static storage
//! at board composition, and is split into typed producer, consumer and control handles that each
//! borrow it. Nothing here is a process-global singleton, so a host test builds as many
//! independent hubs as it likes.
//!
//! The streams, one [`Signal`] mailbox each:
//!
//! - GPS fix, barometric altitude and temperature, published coherently by the sensor task on
//!   each valid fix, so altitude and temperature share the fix's instant. Fresh-fix mailbox:
//!   `try_take` yields once, so a source's `poll` returns `Some` only on the tick a sample
//!   arrived. That is no I²C at the frame rate and no teleport on a stale fix.
//! - GPS time, published on any NAV-PVT whose time the receiver resolved, independent of the
//!   position fix, so the clock can set during acquisition.
//! - Heading, on its own cadence while the rider is stopped, independent of the GPS course.
//! - Heart rate, power and cadence. Two producers, one mailbox each: the board's BLE central
//!   manager and the `debug-uart` injection path both publish through the same
//!   [`SampleInjector`], last-writer-wins, and the app cannot tell them apart.
//! - Rate and GPS power, control latches the ride loop sets and the sensor task awaits.
//! - Event, one payload-less "a datapoint arrived" wake pulsed by every publish above, so the
//!   ride loop needs one await for the whole set. It is a separate signal, so waiting on it never
//!   steals a value from the source polls.
//! - Presence, the boot I²C probe result, published once and drained once into an on-glass
//!   warning for any absent module.
//!
//! The handles, all wired in board composition: [`SensorTaskLink`] goes to the I²C sensor task,
//! [`SampleInjector`] to the BLE central manager and the debug-uart RX task, and
//! [`SensorConsumer`] and [`SensorControl`] to the ride loop.
//!
//! The pure decode this bridges lives in the always-compiled `obc_sensors` and `obc-ble` crates;
//! only this embassy-sync plumbing pulls `embassy-sync`, so it is gated behind `sensor-link`.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_ports::{
    AltimeterSource, CadenceSource, ClockSource, CompassSource, Fix, GpsTime, HeartRateSource, LocationSource,
    PowerSource, TemperatureSource,
};

/// The one raw-mutex `Signal` type every stream in the hub uses. `CriticalSectionRawMutex`,
/// because the producers and the consumer run on different executors and priorities on the board.
type Sig<T> = Signal<CriticalSectionRawMutex, T>;

/// Which sensors answered during startup, carried to the app so a missing module surfaces as a
/// dismissable warning rather than only an RTT line. A missing GPS module is a different thing
/// from "no fix yet".
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
/// and the `power_saver` toggle. The sensor task requests stopped GNSS processing when idle,
/// full-power fixes while riding, or the M10's on-chip low-power tracking when `power_saver` is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpsPower {
    /// Full-power fixes for recording or an on-demand position request.
    Active,
    /// Riding with `power_saver` on: the M10's low-power tracking, at the cost of some fix
    /// latency.
    LowPower,
    /// No position demand: stop GNSS processing and park host polling.
    Sleep,
}

/// Independent receiver and heading demand, delivered as one control update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorDemand {
    pub gps: GpsPower,
    pub compass: bool,
}

impl SensorDemand {
    pub fn for_demand(recording: bool, power_saver: bool, position: bool, compass: bool) -> Self {
        let gps = if position {
            GpsPower::Active
        } else if recording {
            if power_saver {
                GpsPower::LowPower
            } else {
                GpsPower::Active
            }
        } else {
            GpsPower::Sleep
        };
        Self { gps, compass }
    }

    pub fn gnss_running(self) -> bool {
        self.gps != GpsPower::Sleep
    }
}

/// The fix mailbox's stored form: [`Fix`] with its two `Option<f32>`s flattened to raw bits plus
/// presence flags, so every field is niche-free. That keeps the hub's `Signal::new()` state
/// all-zero: `Option<f32>`'s tag byte is a niche the signal's `State::None` would be encoded into
/// as a non-zero value, and one non-zero initializer byte moves a board's whole static hub from
/// `.bss` to `.data`. Private to the hub; the API speaks [`Fix`] on both ends.
#[derive(Clone, Copy)]
struct FixStore {
    lat: i32,
    lon: i32,
    course_bits: u32,
    speed_bits: u32,
    /// bit 0 = course present, bit 1 = speed present.
    flags: u8,
}

impl FixStore {
    const COURSE: u8 = 1 << 0;
    const SPEED: u8 = 1 << 1;

    fn pack(f: Fix) -> Self {
        let mut flags = 0;
        let mut course_bits = 0;
        let mut speed_bits = 0;
        if let Some(c) = f.course {
            flags |= Self::COURSE;
            course_bits = c.to_bits();
        }
        if let Some(s) = f.speed_mps {
            flags |= Self::SPEED;
            speed_bits = s.to_bits();
        }
        Self { lat: f.lat, lon: f.lon, course_bits, speed_bits, flags }
    }

    fn unpack(self) -> Fix {
        Fix {
            lat: self.lat,
            lon: self.lon,
            course: (self.flags & Self::COURSE != 0).then(|| f32::from_bits(self.course_bits)),
            speed_mps: (self.flags & Self::SPEED != 0).then(|| f32::from_bits(self.speed_bits)),
        }
    }
}

/// The instance-owned sensor hand-off: every cross-task sensor stream as a field, so the board
/// owns exactly one and hands out typed handles. Construct it once in static storage and derive
/// handles with the `*` accessors; a host test constructs it as a plain local.
pub struct SensorHub {
    /// Latest GPS fix, fresh-fix. Stored as the niche-free [`FixStore`], which is what keeps the
    /// whole hub zero-initialized.
    fix: Sig<FixStore>,
    /// Latest barometric altitude in metres, coherent with [`SensorHub::fix`].
    alt: Sig<f32>,
    /// Latest ambient temperature in °C, from the BMP581's per-fix reading.
    temp: Sig<f32>,
    /// Latest GPS UTC time, published on any resolved-time NAV-PVT independent of the position
    /// fix, so the clock can set during acquisition.
    gps_time: Sig<GpsTime>,
    /// Latest compass heading in degrees clockwise from north, independent of the GPS course.
    heading: Sig<f32>,
    /// Latest heart rate in bpm, last-writer-wins.
    hr: Sig<u16>,
    /// Latest power in watts; the producer clamps a signed meter reading at 0.
    power: Sig<u16>,
    /// Latest cadence in rpm. A coasting rider publishes a fresh `0`, which is distinct from no
    /// sample at all.
    cadence: Sig<u8>,
    /// Desired GPS fix interval (seconds) — a latch the ride loop sets and the sensor task awaits.
    rate: Sig<u16>,
    /// Desired GPS power state — a latch the ride loop sets and the sensor task awaits.
    sensor_demand: Sig<SensorDemand>,
    /// A single "a datapoint arrived" wake, pulsed by every publish, so one await covers the
    /// whole set. It is payload-less and separate from the value mailboxes, so waiting here never
    /// steals a fix from [`GpsLocation::poll`].
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
            sensor_demand: Signal::new(),
            event: Signal::new(),
            presence: Signal::new(),
        }
    }

    /// The I²C sensor task's handle: publish fix/alt/temp/time/heading/presence, await rate/power.
    pub fn task_link(&self) -> SensorTaskLink<'_> {
        SensorTaskLink(self)
    }

    /// The heart-rate, power and cadence injector, held by both the BLE central manager and the
    /// debug-uart RX task.
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

    /// Publish one datapoint into its mailbox and pulse the shared event. Every producer path
    /// funnels through here, so "a publish always wakes the ride loop" is stated once rather than
    /// re-spelled in each dispatch, where a missing pulse would strand a sample in its mailbox.
    fn publish<T: Send>(&self, mailbox: &Sig<T>, v: T) {
        mailbox.signal(v);
        self.event.signal(());
    }
}

/// The board's high-priority I²C sensor task's handle into the hub: it publishes each coherent
/// datapoint, pulsing the shared event, and awaits the ride loop's rate and power latches.
#[derive(Clone, Copy)]
pub struct SensorTaskLink<'a>(&'a SensorHub);

impl SensorTaskLink<'_> {
    pub fn dispatch_fix(&self, f: Fix) {
        self.0.publish(&self.0.fix, FixStore::pack(f));
    }

    pub fn dispatch_alt(&self, m: f32) {
        self.0.publish(&self.0.alt, m);
    }

    pub fn dispatch_temp(&self, c: f32) {
        self.0.publish(&self.0.temp, c);
    }

    pub fn dispatch_time(&self, t: GpsTime) {
        self.0.publish(&self.0.gps_time, t);
    }

    pub fn dispatch_heading(&self, deg: f32) {
        self.0.publish(&self.0.heading, deg);
    }

    /// Publish the startup probe result once, after GPS responds or its acquisition deadline
    /// passes. The pulse wakes the ride loop to drain it.
    pub fn dispatch_presence(&self, p: SensorPresence) {
        self.0.publish(&self.0.presence, p);
    }

    /// Await the next requested fix interval in seconds, so the task applies a rate change
    /// without sharing the I²C bus with the ride loop.
    pub async fn wait_rate(&self) -> u16 {
        self.0.rate.wait().await
    }

    /// Await the next requested GPS power state, so the task sleeps when a ride ends and wakes
    /// warm when one starts.
    pub async fn wait_power(&self) -> SensorDemand {
        self.0.sensor_demand.wait().await
    }
}

/// The heart-rate, power and cadence injector. Both the board's BLE central manager and the
/// `debug-uart` injection path hold one over the same hub, so the app's `Sensors` wiring is
/// identical whichever is feeding: last-writer-wins. Each dispatch pulses the shared event.
#[derive(Clone, Copy)]
pub struct SampleInjector<'a>(&'a SensorHub);

impl SampleInjector<'_> {
    pub fn dispatch_hr(&self, bpm: u16) {
        self.0.publish(&self.0.hr, bpm);
    }

    /// Publish a fresh power sample in watts; a signed meter reading is clamped at 0 by the
    /// producer.
    pub fn dispatch_power(&self, watts: u16) {
        self.0.publish(&self.0.power, watts);
    }

    /// Publish a fresh cadence sample in rpm. A coasting rider publishes a fresh `0`, which is
    /// not the same as an empty mailbox.
    pub fn dispatch_cadence(&self, rpm: u8) {
        self.0.publish(&self.0.cadence, rpm);
    }
}

/// The ride loop's control handle: the GPS rate and power latches the sensor task awaits. Only
/// the newest value of each matters.
#[derive(Clone, Copy)]
pub struct SensorControl<'a>(&'a SensorHub);

impl SensorControl<'_> {
    /// Request a new GPS fix interval in seconds.
    pub fn set_rate(&self, secs: u16) {
        self.0.rate.signal(secs);
    }

    /// Update receiver power and the independent compass demand.
    pub fn set_power(&self, p: SensorDemand) {
        self.0.sensor_demand.signal(p);
    }
}

/// The ride loop's consumer handle: the app-facing `*Source` drains, the boot presence drain, and
/// the single event wake the loop selects on. The `*Source` accessors return handles bound to the
/// hub's lifetime, so the sources the `Sensors` set holds outlive this transient handle.
#[derive(Clone, Copy)]
pub struct SensorConsumer<'a>(&'a SensorHub);

impl<'a> SensorConsumer<'a> {
    pub fn location(&self) -> GpsLocation<'a> {
        GpsLocation(&self.0.fix)
    }

    pub fn altimeter(&self) -> BaroAltimeter<'a> {
        BaroAltimeter(&self.0.alt)
    }

    pub fn temperature(&self) -> SensorTemp<'a> {
        SensorTemp(&self.0.temp)
    }

    pub fn clock(&self) -> GpsClock<'a> {
        GpsClock(&self.0.gps_time)
    }

    pub fn compass(&self) -> MagCompass<'a> {
        MagCompass(&self.0.heading)
    }

    pub fn hr(&self) -> SensorHr<'a> {
        SensorHr(&self.0.hr)
    }

    pub fn power(&self) -> SensorPower<'a> {
        SensorPower(&self.0.power)
    }

    pub fn cadence(&self) -> SensorCadence<'a> {
        SensorCadence(&self.0.cadence)
    }

    /// Drain the startup probe result: `Some` exactly once, on the pass after the task publishes
    /// it, then `None`. The ride loop maps any absent sensor to a warning flag.
    pub fn take_presence(&self) -> Option<SensorPresence> {
        self.0.presence.try_take()
    }

    /// Await the next datapoint from any sensor: the single wake the event-driven loop selects
    /// on. It completes on any publish, and the loop then drains the typed mailboxes. It is a
    /// separate payload-less signal, so it never steals a value from the source polls.
    pub async fn wait_event(&self) {
        self.0.event.wait().await
    }
}

// Each drain holds a borrow of its one mailbox and drains it on the fresh-mailbox contract, so
// `poll` returns `Some` only on the tick a new sample arrived and the app's staleness gate can
// render a dropped stream as `--`. The names are generic because both the radio manager and the
// debug-uart injection feed the same mailbox.

pub struct GpsLocation<'a>(&'a Sig<FixStore>);
impl LocationSource for GpsLocation<'_> {
    fn poll(&mut self) -> Option<Fix> {
        self.0.try_take().map(FixStore::unpack)
    }
}

/// Declare one drain: a newtype over its mailbox whose `poll` is the fresh-mailbox `try_take`.
/// The bodies are the same body seven times over, and spelling them out invites one of them to
/// grow a peek and break the drain-once contract the app's staleness gate rests on. The GPS fix
/// unpacks its store, so it stays hand-written above.
macro_rules! impl_mailbox_source {
    ($(#[$doc:meta])* $name:ident, $store:ty, $port:ident, $out:ty) => {
        $(#[$doc])*
        pub struct $name<'a>(&'a Sig<$store>);
        impl $port for $name<'_> {
            fn poll(&mut self) -> Option<$out> {
                self.0.try_take()
            }
        }
    };
}

impl_mailbox_source!(BaroAltimeter, f32, AltimeterSource, f32);
impl_mailbox_source!(SensorTemp, f32, TemperatureSource, f32);
impl_mailbox_source!(GpsClock, GpsTime, ClockSource, GpsTime);
impl_mailbox_source!(MagCompass, f32, CompassSource, f32);
impl_mailbox_source!(SensorHr, u16, HeartRateSource, u16);
impl_mailbox_source!(SensorPower, u16, PowerSource, u16);
impl_mailbox_source!(SensorCadence, u8, CadenceSource, u8);

#[cfg(test)]
mod tests {
    use super::*;

    // Multiple independent hubs in one test, with no shared global state. A dispatch on one hub
    // is invisible to the other.
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

    // A publish must leave the shared event signalled so the ride loop's single `wait_event`
    // wakes, yet draining a value mailbox must not consume that wake: they are separate signals.
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

#[cfg(test)]
mod power_tests {
    use super::{GpsPower, SensorDemand};

    #[test]
    fn position_demand_wakes_gps_and_release_preserves_recording_and_compass() {
        for recording in [false, true] {
            for saver in [false, true] {
                for compass in [false, true] {
                    let acquiring = SensorDemand::for_demand(recording, saver, true, compass);
                    assert_eq!(acquiring.gps, GpsPower::Active);
                    assert_eq!(acquiring.compass, compass);
                    let released = SensorDemand::for_demand(recording, saver, false, compass);
                    assert_eq!(released.gnss_running(), recording);
                    assert_eq!(released.compass, compass);
                    assert_eq!(
                        released.gps,
                        if recording {
                            if saver {
                                GpsPower::LowPower
                            } else {
                                GpsPower::Active
                            }
                        } else {
                            GpsPower::Sleep
                        }
                    );
                }
            }
        }
    }
}
