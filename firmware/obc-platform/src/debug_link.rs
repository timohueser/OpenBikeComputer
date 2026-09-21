//! Transport-agnostic fake-sensor debug protocol, the board-agnostic half.
//!
//! A host streams a recorded ride over a debug link and this module turns it into the `obc-ports`
//! HAL traits the app already polls, so the app cannot tell them from real drivers. The board
//! crate owns the concrete transport: it feeds received bytes to [`feed_bytes`] and awaits
//! [`wait_telemetry`] to send the device-to-host status line.
//!
//! Wire format: ASCII, one message per `\n`-terminated line. Host to device:
//! - `F <lat> <lon> [course|-] [speed|-]` — a GPS fix. `lat`/`lon` are integer microdegrees;
//!   `course` (degrees clockwise from north) and `speed` (m/s) are floats, or `-` for unknown,
//!   which is what a real receiver reports at a standstill.
//! - `A <meters>` — a barometric-altitude sample. `C <deg>` — a compass heading.
//! - `H <bpm>` / `P <watts>` / `R <rpm>` — heart rate, power and cadence. They dispatch into the
//!   same [`SensorHub`](crate::sensor_hub::SensorHub) mailboxes the BLE central manager feeds, so
//!   the app wiring is identical for an injected line and a real strap.
//! - `K t <n>` / `K <u|d|s|b> <d|u>` — input injection: `n` signed selection steps, or a down/up
//!   edge for the Up, Down, Select or Back button. An edge is the same event a physical button
//!   produces, so a host can drive the UI over the wire.
//! - `Z <mpp>` — set the map camera to exactly `mpp` metres per pixel and force one redraw.
//! - `N <from_lon> <from_lat> <to_lon> <to_lat>` — route-plan trigger, LON FIRST, unlike the
//!   lat-first `F` line.
//! - `ride-damage payload` / `ride-damage metadata` — fabricate a damaged `RECORDING` object. The
//!   operator issues the reset, because a self-reset would race the confirmation off the wire.
//! - `ride-repair-fail` — arm a one-shot refusal of the next exact removal, without the commit.
//! - `store-census` — print one line per catalog entry plus the count and free extents.
//! - `dfu-install` — post the same install request the UI posts. Each armer phase streams back.
//!
//! Device to host: `T …` carries the last map frame's render stats (see [`Telemetry`]) at a low
//! fixed rate, so the link never floods, and `D <text>` is one free-form DFU status line.
//!
//! Fresh-fix contract, behind `debug-link`: each parsed sensor sample reaches the app through an
//! embassy `Signal`, whose `try_take` returns a value once, so `DebugLocation::poll` yields `Some`
//! only on the tick a new fix arrived. Returning the latest fix on every poll would re-trigger the
//! teleport-rejection bug. Injected input goes through a `Channel`, so a burst arrives in order.

use core::fmt::Write;

// The pure protocol below needs no embassy-sync, so it is always compiled and the host feeder
// reuses one canonical codec.
use obc_ports::{Button, ButtonEvent, Fix, InputEvent};

/// Longest line we accept. The widest message is about 45 bytes, so 64 leaves slack.
const LINE_MAX: usize = 64;

/// Byte cap of one device-to-host DFU status line; anything longer is truncated at push.
pub const DFU_STATUS_MAX: usize = 96;

/// One decoded host→device message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Msg {
    /// A GPS fix (position in microdegrees, optional course/speed).
    Fix(Fix),
    /// A barometric-altitude sample, metres.
    Alt(f32),
    /// A compass heading, degrees CW from north.
    Compass(f32),
    Hr(u16),
    Power(u16),
    Cadence(u8),
    /// An injected raw input event (an Up/Down step or a button down/up edge).
    Input(InputEvent),
    /// A debug camera-scale command: set the map viewport to exactly this meters-per-pixel.
    Zoom(f32),
    /// A debug route-plan trigger: plan from `from` to `to`, both `(lon, lat)` microdegrees.
    Nav {
        from: (i32, i32),
        to: (i32, i32),
    },
    /// A firmware-update install trigger: scan, snapshot the rollback, arm the page, reboot.
    DfuInstall,
    /// Fabricate a damaged `RECORDING` object of this kind.
    RideDamage(RideDamageKind),
    /// Arm the one-shot refusal of the next exact removal.
    RideRepairFail,
    /// Print the catalog census.
    StoreCensus,
}

/// Which of the two logical recovery refusals a `ride-damage` command fabricates. The third cause,
/// a catalog that cannot be listed, cannot be produced on a real card without risking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideDamageKind {
    /// Bytes that are not a ride-v3 sample/footer boundary.
    Payload,
    /// A valid sample boundary with a continuation image that does not decode.
    Metadata,
}

/// Parse one line into a [`Msg`], or `None` for an unknown tag or a malformed field. Lenient by
/// design: a corrupt line over the wire is dropped, never fatal.
pub fn parse_line(line: &str) -> Option<Msg> {
    let mut it = line.split_ascii_whitespace();
    match it.next()? {
        "F" => {
            let lat = it.next()?.parse::<i32>().ok()?;
            let lon = it.next()?.parse::<i32>().ok()?;
            // course/speed are optional: absent, `-` (a standstill), or unparseable → `None`.
            let course = parse_opt_f32(it.next());
            let speed_mps = parse_opt_f32(it.next());
            Some(Msg::Fix(Fix { lat, lon, course, speed_mps }))
        }
        "A" => Some(Msg::Alt(it.next()?.parse::<f32>().ok()?)),
        "C" => Some(Msg::Compass(it.next()?.parse::<f32>().ok()?)),
        // Integer bpm, watts and rpm, parse-tolerant: a malformed value drops the line.
        "H" => Some(Msg::Hr(it.next()?.parse::<u16>().ok()?)),
        "P" => Some(Msg::Power(it.next()?.parse::<u16>().ok()?)),
        "R" => Some(Msg::Cadence(it.next()?.parse::<u8>().ok()?)),
        "Z" => Some(Msg::Zoom(it.next()?.parse::<f32>().ok()?)),
        // LON FIRST (the OBCM `(lon, lat)` order), unlike the lat-first `F` fix line.
        "N" => {
            let from_lon = it.next()?.parse::<i32>().ok()?;
            let from_lat = it.next()?.parse::<i32>().ok()?;
            let to_lon = it.next()?.parse::<i32>().ok()?;
            let to_lat = it.next()?.parse::<i32>().ok()?;
            Some(Msg::Nav { from: (from_lon, from_lat), to: (to_lon, to_lat) })
        }
        "K" => parse_key(&mut it),
        // Word tags, so the command name is greppable and is the harness's whole interface.
        "dfu-install" => Some(Msg::DfuInstall),
        "ride-damage" => match it.next()? {
            "payload" => Some(Msg::RideDamage(RideDamageKind::Payload)),
            "metadata" => Some(Msg::RideDamage(RideDamageKind::Metadata)),
            _ => None,
        },
        "ride-repair-fail" => Some(Msg::RideRepairFail),
        "store-census" => Some(Msg::StoreCensus),
        _ => None,
    }
}

/// `None` for a missing token or the `-` unknown sentinel; otherwise the parsed float.
fn parse_opt_f32(tok: Option<&str>) -> Option<f32> {
    match tok {
        None | Some("-") => None,
        Some(s) => s.parse::<f32>().ok(),
    }
}

/// Parse the tokens after a `K` tag into an injected input event.
fn parse_key(it: &mut core::str::SplitAsciiWhitespace) -> Option<Msg> {
    let ev = match it.next()? {
        "t" => InputEvent::Step(it.next()?.parse::<i32>().ok()?),
        "u" => InputEvent::Button(edge(it.next()?, Button::Up)?),
        "d" => InputEvent::Button(edge(it.next()?, Button::Down)?),
        "s" => InputEvent::Button(edge(it.next()?, Button::Select)?),
        "b" => InputEvent::Button(edge(it.next()?, Button::Back)?),
        _ => return None,
    };
    Some(Msg::Input(ev))
}

/// `d` → down edge, `u` → up edge, for button `b`.
fn edge(tok: &str, b: Button) -> Option<ButtonEvent> {
    match tok {
        "d" => Some(ButtonEvent::Down(b)),
        "u" => Some(ButtonEvent::Up(b)),
        _ => None,
    }
}

/// Encode a [`Fix`] as an `F` line, the exact inverse of the `F` arm of [`parse_line`], with `-`
/// for a missing field. The cap covers the worst case, so the `write!`s cannot truncate.
pub fn format_fix(f: &Fix) -> heapless::String<48> {
    /// Write an optional float at `prec` decimals, or the `-` sentinel.
    fn push_opt(s: &mut heapless::String<48>, v: Option<f32>, prec: usize) {
        match v {
            Some(v) => {
                let _ = write!(s, "{v:.prec$}");
            }
            None => {
                let _ = s.push('-');
            }
        }
    }
    let mut s = heapless::String::new();
    let _ = write!(s, "F {} {} ", f.lat, f.lon);
    push_opt(&mut s, f.course, 1);
    let _ = s.push(' ');
    push_opt(&mut s, f.speed_mps, 2);
    let _ = s.push('\n');
    s
}

/// Encode a heart-rate sample as an `H` line, the inverse of the `H` arm of [`parse_line`]. The
/// device and the host feeder share one codec, so the two halves cannot drift.
pub fn format_hr(bpm: u16) -> heapless::String<16> {
    let mut s = heapless::String::new();
    let _ = writeln!(s, "H {bpm}");
    s
}

/// Encode a power sample as a `P` line, the inverse of the `P` arm of [`parse_line`].
pub fn format_power(watts: u16) -> heapless::String<16> {
    let mut s = heapless::String::new();
    let _ = writeln!(s, "P {watts}");
    s
}

/// Encode a cadence sample as an `R` line, the inverse of the `R` arm of [`parse_line`].
pub fn format_cadence(rpm: u8) -> heapless::String<16> {
    let mut s = heapless::String::new();
    let _ = writeln!(s, "R {rpm}");
    s
}

/// Accumulates raw link bytes into lines and parses each complete `\n`-terminated line. A line
/// with no newline within [`LINE_MAX`] is dropped to the next newline rather than split.
pub struct LineReader {
    buf: [u8; LINE_MAX],
    len: usize,
    /// Set once the current line overran the buffer; the rest is skipped until the next newline.
    overflow: bool,
}

impl Default for LineReader {
    fn default() -> Self {
        Self::new()
    }
}

impl LineReader {
    pub const fn new() -> Self {
        LineReader { buf: [0; LINE_MAX], len: 0, overflow: false }
    }

    /// Feed a chunk of received bytes; call `on_msg` for each complete line that parses.
    pub fn feed(&mut self, bytes: &[u8], mut on_msg: impl FnMut(Msg)) {
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                if !self.overflow && self.len > 0 {
                    if let Ok(s) = core::str::from_utf8(&self.buf[..self.len]) {
                        if let Some(msg) = parse_line(s) {
                            on_msg(msg);
                        }
                    }
                }
                self.len = 0;
                self.overflow = false;
            } else if self.overflow {
                // mid-overrun: skip until the newline above re-arms
            } else if self.len < LINE_MAX {
                self.buf[self.len] = b;
                self.len += 1;
            } else {
                self.overflow = true;
            }
        }
    }
}

/// The last map frame's render stats, snapshotted after each map render. Integer fields only, so
/// [`format_telemetry`] is float-free.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Telemetry {
    /// Last map-render wall time, microseconds.
    pub frame_us: u32,
    /// LOD chosen for the last render.
    pub lod: u8,
    /// Features drawn, tried and dropped on scratch overflow, which should be `0`.
    pub feat_drawn: u32,
    pub feat_tried: u32,
    pub feat_dropped: u32,
    /// Quadtree leaves visited this frame.
    pub chunks: u32,
    /// Streamed-map chunk cache: passes served from cache vs. read from SD.
    pub cache_hits: u32,
    pub cache_misses: u32,
    /// Raw SD-source overhead this frame (reads + bytes).
    pub sd_reads: u32,
    pub bytes_read: u32,
    /// Per-stage map-render wall time, µs. `collect_us` includes `read_us`, the I/O inside it, and
    /// `frame_us` is the whole map frame.
    pub collect_us: u32,
    pub read_us: u32,
    pub sort_us: u32,
    pub draw_us: u32,
    pub overlay_us: u32,
    /// Camera scale of the rendered frame, in milli-mpp, so the line stays float-free.
    pub mpp_milli: u32,
}

/// Format a telemetry line into a heap-free string; the cap covers 16 `u32::MAX` fields.
pub fn format_telemetry(t: &Telemetry) -> heapless::String<192> {
    let mut s = heapless::String::new();
    // Infallible for the field count and cap; ignore the Result rather than panic on the MCU.
    let _ = writeln!(
        s,
        "T {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
        t.frame_us,
        t.lod,
        t.feat_drawn,
        t.feat_tried,
        t.feat_dropped,
        t.chunks,
        t.cache_hits,
        t.cache_misses,
        t.sd_reads,
        t.bytes_read,
        t.collect_us,
        t.read_us,
        t.sort_us,
        t.draw_us,
        t.overlay_us,
        t.mpp_milli
    );
    s
}

/// Parse a `T …` telemetry line back into a [`Telemetry`], the inverse of [`format_telemetry`].
/// `None` for a non-`T` line or a malformed field, so other device chatter is ignored.
pub fn parse_telemetry(line: &str) -> Option<Telemetry> {
    let mut it = line.split_ascii_whitespace();
    if it.next()? != "T" {
        return None;
    }
    Some(Telemetry {
        frame_us: it.next()?.parse().ok()?,
        lod: it.next()?.parse().ok()?,
        feat_drawn: it.next()?.parse().ok()?,
        feat_tried: it.next()?.parse().ok()?,
        feat_dropped: it.next()?.parse().ok()?,
        chunks: it.next()?.parse().ok()?,
        cache_hits: it.next()?.parse().ok()?,
        cache_misses: it.next()?.parse().ok()?,
        sd_reads: it.next()?.parse().ok()?,
        bytes_read: it.next()?.parse().ok()?,
        collect_us: it.next()?.parse().ok()?,
        read_us: it.next()?.parse().ok()?,
        sort_us: it.next()?.parse().ok()?,
        draw_us: it.next()?.parse().ok()?,
        overlay_us: it.next()?.parse().ok()?,
        mpp_milli: it.next()?.parse().ok()?,
    })
}

// The cross-task hand-off, gated behind `debug-link`; the host feeder builds without it.
#[cfg(feature = "debug-link")]
mod handoff {
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_sync::signal::Signal;
    use obc_ports::{AltimeterSource, CompassSource, Fix, InputEvent, InputSource, LocationSource};

    use super::{LineReader, Msg, RideDamageKind, Telemetry};
    #[cfg(feature = "sensor-link")]
    use crate::sensor_hub::SampleInjector;

    /// Latest GPS fix, with fresh-fix semantics. See the module docs.
    static FIX: Signal<CriticalSectionRawMutex, Fix> = Signal::new();
    /// Latest barometric-altitude sample (metres).
    static ALT: Signal<CriticalSectionRawMutex, f32> = Signal::new();
    /// Latest compass heading (degrees CW from north).
    static COMPASS: Signal<CriticalSectionRawMutex, f32> = Signal::new();
    /// Latest device telemetry to send host-ward; the app sets it, the transport's TX task awaits it.
    static TELEMETRY: Signal<CriticalSectionRawMutex, Telemetry> = Signal::new();
    /// Latest debug camera-scale command, `try_take`-once. Drained by the map loop each frame.
    static ZOOM: Signal<CriticalSectionRawMutex, f32> = Signal::new();
    /// A debug route-plan trigger's payload: `(from, to)`, both `(lon, lat)` µdeg.
    type NavTrigger = ((i32, i32), (i32, i32));
    /// Latest debug route-plan trigger, `try_take`-once. Drained by the ride loop.
    static NAV: Signal<CriticalSectionRawMutex, NavTrigger> = Signal::new();
    /// A pending `dfu-install` trigger, `try_take`-once. Drained by the ride loop.
    static DFU_INSTALL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
    /// The acceptance-harness commands, `try_take`-once. Drained by the ride loop.
    static RIDE_DAMAGE: Signal<CriticalSectionRawMutex, RideDamageKind> = Signal::new();
    static RIDE_REPAIR_FAIL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
    static STORE_CENSUS: Signal<CriticalSectionRawMutex, ()> = Signal::new();
    /// DFU status lines device-to-host, queued in order: a `Channel`, not a latch, because one arm
    /// emits several phase lines and each must reach the host. An overflowing push is dropped.
    static DFU_STATUS: Channel<CriticalSectionRawMutex, heapless::String<{ super::DFU_STATUS_MAX }>, 4> =
        Channel::new();
    /// A single "a datapoint arrived" wake, pulsed on any host-streamed sensor sample. Injected
    /// input does not pulse it: that wakes the loop through the gesture channel.
    static EVENT: Signal<CriticalSectionRawMutex, ()> = Signal::new();

    /// Injected input events, queued in order. A queue, not a latch: a tap is a down and up pair
    /// and a burst must arrive intact.
    const INPUT_QUEUE: usize = 16;
    static INPUT: Channel<CriticalSectionRawMutex, InputEvent, INPUT_QUEUE> = Channel::new();

    /// Route a decoded [`Msg`] to its signal or queue. Heart rate, power and cadence go through
    /// the caller-owned [`SampleInjector`], into the same hub the BLE manager feeds.
    pub fn dispatch(msg: Msg, #[cfg(feature = "sensor-link")] injector: SampleInjector) {
        match msg {
            Msg::Fix(f) => {
                FIX.signal(f);
                EVENT.signal(());
            }
            Msg::Alt(a) => {
                ALT.signal(a);
                EVENT.signal(());
            }
            Msg::Compass(c) => {
                COMPASS.signal(c);
                EVENT.signal(());
            }
            // Pulse the debug-link `EVENT` too, so an injected sample pulls a `debug-uart` ride
            // loop out of warm sleep exactly like a fix does.
            Msg::Hr(bpm) => {
                #[cfg(feature = "sensor-link")]
                injector.dispatch_hr(bpm);
                #[cfg(not(feature = "sensor-link"))]
                let _ = bpm;
                EVENT.signal(());
            }
            Msg::Power(w) => {
                #[cfg(feature = "sensor-link")]
                injector.dispatch_power(w);
                #[cfg(not(feature = "sensor-link"))]
                let _ = w;
                EVENT.signal(());
            }
            Msg::Cadence(rpm) => {
                #[cfg(feature = "sensor-link")]
                injector.dispatch_cadence(rpm);
                #[cfg(not(feature = "sensor-link"))]
                let _ = rpm;
                EVENT.signal(());
            }
            Msg::Zoom(z) => {
                ZOOM.signal(z);
                EVENT.signal(());
            }
            Msg::Nav { from, to } => {
                NAV.signal((from, to));
                EVENT.signal(());
            }
            // Pulse EVENT too: a parked device must wake its ride loop to drain the request.
            Msg::DfuInstall => {
                DFU_INSTALL.signal(());
                EVENT.signal(());
            }
            Msg::RideDamage(kind) => {
                RIDE_DAMAGE.signal(kind);
                EVENT.signal(());
            }
            Msg::RideRepairFail => {
                RIDE_REPAIR_FAIL.signal(());
                EVENT.signal(());
            }
            Msg::StoreCensus => {
                STORE_CENSUS.signal(());
                EVENT.signal(());
            }
            // Drop on the unreachable overflow rather than block the RX task. No `EVENT` pulse:
            // injected input wakes the loop through the gesture channel.
            Msg::Input(ev) => {
                let _ = INPUT.try_send(ev);
            }
        }
    }

    /// Await the next host-streamed datapoint: the single sensor wake the main loop selects on.
    pub async fn wait_event() {
        EVENT.wait().await
    }

    /// Accumulate `bytes` and dispatch every complete line. `reader` holds the partial-line buffer.
    pub fn feed_bytes(reader: &mut LineReader, bytes: &[u8], #[cfg(feature = "sensor-link")] injector: SampleInjector) {
        reader.feed(bytes, |msg| {
            dispatch(
                msg,
                #[cfg(feature = "sensor-link")]
                injector,
            )
        });
    }

    /// The user's location, streamed over the debug link.
    pub struct DebugLocation;
    impl LocationSource for DebugLocation {
        fn poll(&mut self) -> Option<Fix> {
            FIX.try_take()
        }
    }

    /// The barometric altimeter, streamed over the debug link.
    pub struct DebugAltimeter;
    impl AltimeterSource for DebugAltimeter {
        fn poll(&mut self) -> Option<f32> {
            ALT.try_take()
        }
    }

    /// The electronic compass, streamed over the debug link.
    pub struct DebugCompass;
    impl CompassSource for DebugCompass {
        fn poll(&mut self) -> Option<f32> {
            COMPASS.try_take()
        }
    }

    /// Injected input, drained by the input plane next to the physical buttons, so injected edges
    /// become gestures identically to real presses.
    pub struct DebugInput;
    impl InputSource for DebugInput {
        fn poll(&mut self) -> Option<InputEvent> {
            INPUT.try_receive().ok()
        }
    }

    /// Take a pending debug `Z` camera-scale command, `try_take`-once.
    pub fn take_zoom() -> Option<f32> {
        ZOOM.try_take()
    }

    /// Take a pending route-plan trigger (`(from, to)`, both `(lon, lat)` µdeg), `try_take`-once.
    pub fn take_nav() -> Option<NavTrigger> {
        NAV.try_take()
    }

    /// Take a pending `dfu-install` trigger, `try_take`-once.
    pub fn take_dfu_install() -> bool {
        DFU_INSTALL.try_take().is_some()
    }

    /// Take a pending `ride-damage` fabrication request, `try_take`-once.
    pub fn take_ride_damage() -> Option<RideDamageKind> {
        RIDE_DAMAGE.try_take()
    }

    /// Take a pending `ride-repair-fail` arming request, `try_take`-once.
    pub fn take_ride_repair_fail() -> bool {
        RIDE_REPAIR_FAIL.try_take().is_some()
    }

    /// Take a pending `store-census` request, `try_take`-once.
    pub fn take_store_census() -> bool {
        STORE_CENSUS.try_take().is_some()
    }

    /// Queue one DFU status line for the host. A full queue drops the line; the RTT log is lossless.
    pub fn dfu_status(text: &str) {
        let mut line: heapless::String<{ super::DFU_STATUS_MAX }> = heapless::String::new();
        // Truncate rather than drop on an over-long message; the prefix carries the meaning.
        let take = text.len().min(super::DFU_STATUS_MAX);
        let mut end = take;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        let _ = line.push_str(&text[..end]);
        let _ = DFU_STATUS.try_send(line);
    }

    /// Await the next queued DFU status line, for the transport's TX task.
    pub async fn wait_dfu_status() -> heapless::String<{ super::DFU_STATUS_MAX }> {
        DFU_STATUS.receive().await
    }

    /// Publish the latest telemetry, overwriting any unsent value.
    pub fn set_telemetry(t: Telemetry) {
        TELEMETRY.signal(t);
    }

    /// Await the next published telemetry, so the send cadence is driven by [`set_telemetry`].
    pub async fn wait_telemetry() -> Telemetry {
        TELEMETRY.wait().await
    }
}

#[cfg(feature = "debug-link")]
pub use handoff::{
    dfu_status, dispatch, feed_bytes, set_telemetry, take_dfu_install, take_nav, take_ride_damage,
    take_ride_repair_fail, take_store_census, take_zoom, wait_dfu_status, wait_event, wait_telemetry, DebugAltimeter,
    DebugCompass, DebugInput, DebugLocation,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_fix() {
        assert_eq!(
            parse_line("F 48122905 7814438 90.5 5.0"),
            Some(Msg::Fix(Fix { lat: 48_122_905, lon: 7_814_438, course: Some(90.5), speed_mps: Some(5.0) }))
        );
    }

    #[test]
    fn parses_stopped_fix_with_dash_course() {
        // A standstill: no course but a small speed, and the `-` sentinel keeps the field positional.
        assert_eq!(
            parse_line("F 1 2 - 0.1"),
            Some(Msg::Fix(Fix { lat: 1, lon: 2, course: None, speed_mps: Some(0.1) }))
        );
    }

    #[test]
    fn parses_fix_without_optional_fields() {
        assert_eq!(
            parse_line("F -2147483648 2147483647"),
            Some(Msg::Fix(Fix { lat: i32::MIN, lon: i32::MAX, course: None, speed_mps: None }))
        );
    }

    #[test]
    fn parses_alt_and_compass() {
        assert_eq!(parse_line("A 612.5"), Some(Msg::Alt(612.5)));
        assert_eq!(parse_line("C 270"), Some(Msg::Compass(270.0)));
    }

    #[test]
    fn parses_sensor_lines() {
        assert_eq!(parse_line("H 156"), Some(Msg::Hr(156)));
        assert_eq!(parse_line("P 240"), Some(Msg::Power(240)));
        assert_eq!(parse_line("R 92"), Some(Msg::Cadence(92)));
        // Extremes: u16 for H/P, u8 for R.
        assert_eq!(parse_line("H 65535"), Some(Msg::Hr(u16::MAX)));
        assert_eq!(parse_line("P 1000"), Some(Msg::Power(1000)));
        assert_eq!(parse_line("R 255"), Some(Msg::Cadence(u8::MAX)));
    }

    #[test]
    fn rejects_malformed_sensor_lines() {
        assert_eq!(parse_line("H"), None, "missing bpm");
        assert_eq!(parse_line("P x"), None, "non-numeric watts");
        assert_eq!(parse_line("R -5"), None, "cadence is unsigned");
        assert_eq!(parse_line("H 70000"), None, "bpm above u16 rejects the line");
        assert_eq!(parse_line("R 300"), None, "rpm above u8 rejects the line");
    }

    #[test]
    fn format_sensor_lines_round_trip_through_parse() {
        // Each `format_*` is the exact inverse of its `parse_line` arm, so the two cannot drift.
        assert_eq!(format_hr(156).as_str(), "H 156\n");
        assert_eq!(format_power(240).as_str(), "P 240\n");
        assert_eq!(format_cadence(92).as_str(), "R 92\n");
        assert_eq!(parse_line(format_hr(156).as_str().trim_end()), Some(Msg::Hr(156)));
        assert_eq!(parse_line(format_power(240).as_str().trim_end()), Some(Msg::Power(240)));
        assert_eq!(parse_line(format_cadence(92).as_str().trim_end()), Some(Msg::Cadence(92)));
        // Widest values still fit their 16-byte caps and survive the round-trip.
        assert_eq!(format_hr(u16::MAX).as_str(), "H 65535\n");
        assert_eq!(parse_line(format_hr(u16::MAX).as_str().trim_end()), Some(Msg::Hr(u16::MAX)));
        assert_eq!(parse_line(format_cadence(u8::MAX).as_str().trim_end()), Some(Msg::Cadence(u8::MAX)));
    }

    #[test]
    fn parses_zoom() {
        assert_eq!(parse_line("Z 0.5"), Some(Msg::Zoom(0.5)));
        assert_eq!(parse_line("Z 5"), Some(Msg::Zoom(5.0)));
        assert_eq!(parse_line("Z"), None); // missing value
        assert_eq!(parse_line("Z x"), None); // non-numeric
    }

    #[test]
    fn parses_nav() {
        // LON FIRST, unlike the lat-first `F` line.
        assert_eq!(
            parse_line("N 7809000 48126000 7808898 48139394"),
            Some(Msg::Nav { from: (7809000, 48126000), to: (7808898, 48139394) })
        );
        assert_eq!(parse_line("N 7809000 48126000 7808898"), None); // missing to_lat
        assert_eq!(parse_line("N"), None); // no coords
    }

    #[test]
    fn parses_dfu_install() {
        assert_eq!(parse_line("dfu-install"), Some(Msg::DfuInstall));
        assert_eq!(parse_line("  dfu-install  "), Some(Msg::DfuInstall), "whitespace tolerated like every tag");
        // Trailing junk is ignored: the tag alone is the command.
        assert_eq!(parse_line("dfu-install now"), Some(Msg::DfuInstall));
        assert_eq!(parse_line("dfu-installx"), None, "the tag must match exactly");
        assert_eq!(parse_line("DFU-INSTALL"), None, "tags are case-sensitive, like F/A/C");
    }

    #[test]
    fn parses_the_recovery_acceptance_commands() {
        assert_eq!(parse_line("ride-damage payload"), Some(Msg::RideDamage(RideDamageKind::Payload)));
        assert_eq!(parse_line("ride-damage metadata"), Some(Msg::RideDamage(RideDamageKind::Metadata)));
        assert_eq!(parse_line("ride-damage"), None, "the kind is not optional — the two are different faults");
        assert_eq!(parse_line("ride-damage catalog"), None, "catalog damage is deliberately not fabricable");
        assert_eq!(parse_line("ride-repair-fail"), Some(Msg::RideRepairFail));
        assert_eq!(parse_line("store-census"), Some(Msg::StoreCensus));
        assert_eq!(parse_line("store-censusx"), None, "tags match exactly, like every other one");
    }

    #[test]
    fn parses_input_injection() {
        assert_eq!(parse_line("K t 1"), Some(Msg::Input(InputEvent::Step(1))));
        assert_eq!(parse_line("K t -2"), Some(Msg::Input(InputEvent::Step(-2))));
        assert_eq!(parse_line("K u d"), Some(Msg::Input(InputEvent::Button(ButtonEvent::Down(Button::Up)))));
        assert_eq!(parse_line("K d u"), Some(Msg::Input(InputEvent::Button(ButtonEvent::Up(Button::Down)))));
        assert_eq!(parse_line("K s d"), Some(Msg::Input(InputEvent::Button(ButtonEvent::Down(Button::Select)))));
        assert_eq!(parse_line("K b u"), Some(Msg::Input(InputEvent::Button(ButtonEvent::Up(Button::Back)))));
        assert_eq!(parse_line("K s x"), None); // bad edge
        assert_eq!(parse_line("K z 1"), None); // unknown key
        assert_eq!(parse_line("K t"), None); // missing steps
    }

    #[test]
    fn extra_whitespace_is_tolerated() {
        assert_eq!(parse_line("  F   1   2  "), Some(Msg::Fix(Fix::at(1, 2))));
    }

    #[test]
    fn rejects_unknown_or_malformed() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("X 1 2"), None); // unknown tag
        assert_eq!(parse_line("F 1"), None); // missing lon
        assert_eq!(parse_line("A"), None); // missing value
        assert_eq!(parse_line("F abc 2"), None); // non-numeric lat
    }

    #[test]
    fn line_reader_splits_and_dispatches_multiple_lines() {
        let mut r = LineReader::new();
        let mut got = heapless::Vec::<Msg, 8>::new();
        // Two lines plus a partial third, fed as separate chunks, as the link would.
        r.feed(b"F 1 2 - -\nA 100", |m| got.push(m).unwrap());
        r.feed(b".5\nC 45\n", |m| got.push(m).unwrap());
        assert_eq!(got.as_slice(), &[Msg::Fix(Fix::at(1, 2)), Msg::Alt(100.5), Msg::Compass(45.0)]);
    }

    #[test]
    fn line_reader_drops_overlong_lines_without_splitting() {
        let mut r = LineReader::new();
        let mut got = heapless::Vec::<Msg, 4>::new();
        // A junk line far over LINE_MAX, then a good one: only the good one survives.
        let mut junk = heapless::String::<256>::new();
        for _ in 0..200 {
            junk.push('Z').unwrap();
        }
        r.feed(junk.as_bytes(), |m| got.push(m).unwrap());
        r.feed(b"\nC 12\n", |m| got.push(m).unwrap());
        assert_eq!(got.as_slice(), &[Msg::Compass(12.0)]);
    }

    #[test]
    fn telemetry_formats_compactly() {
        let t = Telemetry {
            frame_us: 41000,
            lod: 2,
            feat_drawn: 312,
            feat_tried: 480,
            feat_dropped: 0,
            chunks: 9,
            cache_hits: 27,
            cache_misses: 3,
            sd_reads: 3,
            bytes_read: 12288,
            collect_us: 20000,
            read_us: 8000,
            sort_us: 500,
            draw_us: 19000,
            overlay_us: 1500,
            mpp_milli: 500,
        };
        assert_eq!(format_telemetry(&t).as_str(), "T 41000 2 312 480 0 9 27 3 3 12288 20000 8000 500 19000 1500 500\n");
    }

    #[test]
    fn telemetry_round_trips_through_format_and_parse() {
        // `parse_telemetry` is the exact inverse of `format_telemetry`.
        let t = Telemetry {
            frame_us: 51234,
            lod: 4,
            feat_drawn: 1000,
            feat_tried: 1024,
            feat_dropped: 7,
            chunks: 33,
            cache_hits: 480,
            cache_misses: 12,
            sd_reads: 9,
            bytes_read: 65535,
            collect_us: 30000,
            read_us: 12000,
            sort_us: 800,
            draw_us: 18000,
            overlay_us: 2434,
            mpp_milli: 3000,
        };
        assert_eq!(parse_telemetry(format_telemetry(&t).as_str()), Some(t));
    }

    #[test]
    fn parse_telemetry_rejects_non_t_and_short_lines() {
        assert_eq!(parse_telemetry("F 1 2"), None); // not a telemetry line
        assert_eq!(parse_telemetry("T 1 2 3"), None); // too few fields
        assert_eq!(parse_telemetry("T 1 x 3 4 5 6 7 8 9 10"), None); // non-numeric field
    }

    #[test]
    fn format_fix_round_trips_through_parse_line() {
        // Course and speed are re-read at the formatter's precision, so pick values that survive it.
        let f = Fix { lat: 48_122_905, lon: 7_814_438, course: Some(90.5), speed_mps: Some(5.25) };
        assert_eq!(format_fix(&f).as_str(), "F 48122905 7814438 90.5 5.25\n");
        assert_eq!(parse_line(format_fix(&f).as_str().trim_end()), Some(Msg::Fix(f)));
    }

    #[test]
    fn format_fix_uses_dash_for_a_standstill() {
        // No course or speed: the `-` sentinel keeps each field positional.
        let f = Fix::at(1, 2);
        assert_eq!(format_fix(&f).as_str(), "F 1 2 - -\n");
        assert_eq!(parse_line(format_fix(&f).as_str().trim_end()), Some(Msg::Fix(f)));
    }

    /// `feed` treats `\r` and `\n` both as line terminators. A bare `\r`, a `\r\n` and a lone
    /// `\n` each dispatch exactly once, with no phantom blank line.
    #[test]
    fn line_reader_treats_cr_and_crlf_as_terminators() {
        let mut r = LineReader::new();
        let mut got = heapless::Vec::<Msg, 8>::new();
        // A `\r`-terminated line, then a `\r\n`-terminated one (CRLF), then a lone `\n`.
        r.feed(b"A 1\rC 2\r\nZ 3\n", |m| got.push(m).unwrap());
        assert_eq!(
            got.as_slice(),
            &[Msg::Alt(1.0), Msg::Compass(2.0), Msg::Zoom(3.0)],
            "bare CR, CRLF, and LF each terminate exactly one line (CRLF emits no blank)"
        );
    }

    /// Empty lines from the wire are skipped at the `LineReader` level and never reach
    /// `parse_line`.
    #[test]
    fn line_reader_skips_blank_lines() {
        let mut r = LineReader::new();
        let mut got = heapless::Vec::<Msg, 8>::new();
        // Leading blanks, blank between, trailing blanks — only the two real lines dispatch.
        r.feed(b"\n\nA 5\n\n\nC 9\n\n", |m| got.push(m).unwrap());
        assert_eq!(got.as_slice(), &[Msg::Alt(5.0), Msg::Compass(9.0)], "blank lines produce no Msg");
    }

    /// The widest fix line is the worst case the cap is sized for: `format_fix` emits it whole and
    /// `parse_line` reads the exact extremes back.
    #[test]
    fn format_fix_round_trips_i32_extremes() {
        let f = Fix { lat: i32::MIN, lon: i32::MAX, course: None, speed_mps: None };
        assert_eq!(format_fix(&f).as_str(), "F -2147483648 2147483647 - -\n", "widest fix fits the 48-byte cap");
        assert_eq!(
            parse_line(format_fix(&f).as_str().trim_end()),
            Some(Msg::Fix(f)),
            "extremes survive the round-trip"
        );
    }

    /// `lod` is parsed as `u8`, so a value above 255 fails the whole parse rather than wrapping.
    /// Every other field is valid, which isolates the overflow as the cause.
    #[test]
    fn parse_telemetry_rejects_lod_above_u8() {
        // A valid T line shape with an out-of-range lod, so the u8 parse fails the line.
        assert_eq!(
            parse_telemetry("T 41000 999 312 480 0 9 27 3 3 12288 20000 8000 500 19000 1500 500"),
            None,
            "lod > 255 overflows the u8 field and rejects the line"
        );
        // Sanity: the same line with lod = 5 parses, isolating the overflow as the cause.
        assert!(
            parse_telemetry("T 41000 5 312 480 0 9 27 3 3 12288 20000 8000 500 19000 1500 500").is_some(),
            "identical line with an in-range lod parses"
        );
    }

    /// A line that fills the buffer exactly, with no newline yet, must not trip overflow: the
    /// boundary accepts a byte while `len < LINE_MAX` and overflows only on the next one.
    #[test]
    fn line_reader_accepts_a_line_filling_the_buffer_exactly() {
        // `Z 1` plus trailing spaces to exactly LINE_MAX bytes. Trailing whitespace is ignored, so
        // it parses as `Zoom(1.0)`.
        let mut line = heapless::String::<64>::new();
        line.push_str("Z 1").unwrap();
        while line.len() < LINE_MAX {
            line.push(' ').unwrap();
        }
        assert_eq!(line.len(), LINE_MAX, "line is exactly LINE_MAX bytes");

        let mut r = LineReader::new();
        let mut got = heapless::Vec::<Msg, 4>::new();
        r.feed(line.as_bytes(), |m| got.push(m).unwrap()); // fills buffer to the brim, no overflow
        r.feed(b"\n", |m| got.push(m).unwrap()); // newline flushes the full-but-not-overflowed line
        assert_eq!(got.as_slice(), &[Msg::Zoom(1.0)], "a line filling the buffer exactly is not dropped");

        // One byte more before the newline *does* overflow and is dropped.
        let mut over = heapless::String::<80>::new();
        over.push_str("Z 1").unwrap();
        while over.len() < LINE_MAX + 1 {
            over.push(' ').unwrap();
        }
        let mut r2 = LineReader::new();
        let mut got2 = heapless::Vec::<Msg, 4>::new();
        r2.feed(over.as_bytes(), |m| got2.push(m).unwrap());
        r2.feed(b"\n", |m| got2.push(m).unwrap());
        assert!(got2.is_empty(), "LINE_MAX + 1 bytes overflows and the line is dropped");
    }
}
