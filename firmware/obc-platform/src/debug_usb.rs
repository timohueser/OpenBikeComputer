//! USB-CDC fake-sensor debug protocol (issue #38) — the board-agnostic half.
//!
//! There's no GPS / compass / altimeter hardware on the prototype (and never a good fix at the
//! bench), so a host streams a recorded ride over USB-CDC and this module turns it into the
//! `obc-app` HAL traits the app already polls — [`DebugLocation`], [`DebugAltimeter`] and
//! [`DebugCompass`]. The app can't tell them from real drivers, and because the protocol +
//! sources live here (not in the board crate) they move to the nRF54L unchanged. The board crate
//! owns only the concrete embassy-usb CDC driver; it feeds received bytes to [`feed_bytes`] and
//! `await`s [`wait_telemetry`] to send the device→host status line.
//!
//! ## Wire format (ASCII, one message per `\n`-terminated line)
//! Host → device:
//! - `F <lat> <lon> [course|-] [speed|-]` — a GPS fix. `lat`/`lon` are integer **microdegrees**
//!   (matching [`Fix`]); `course` (deg CW from north) and `speed` (m/s) are floats, or `-` for
//!   "unknown" (a real receiver drops both at a standstill). Trailing fields may be omitted.
//! - `A <meters>` — a barometric-altitude sample (float metres).
//! - `C <deg>` — a compass heading (float degrees CW from north).
//!
//! Device → host (see [`Telemetry`]): `T <ridden_m> <climb_m> <avg_kmh_x10> <speed_mps_x10>
//! <frame_us> <mode>` — one short line at ~1 Hz, deliberately low-rate so the link never floods.
//!
//! ## Fresh-fix contract (#43)
//! Each parsed sample is handed across to the app through an embassy [`Signal`], whose `try_take`
//! returns a value exactly **once** per signal — so [`DebugLocation::poll`] yields `Some` only on
//! the tick a new fix arrived and `None` between, the same cadence a real ~1 Hz receiver follows.
//! Returning the latest fix on *every* ~8 ms poll would re-trigger the teleport-rejection bug #43
//! fixes; the `Signal` gives the correct semantics for free.

use core::fmt::Write;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_app::{AltimeterSource, CompassSource, Fix, LocationSource};

/// Longest line we accept. The widest message is an `F` with full i32 lat/lon and float
/// course/speed (`F -2147483648 -2147483648 359.99 99.99`) ≈ 45 bytes; 64 leaves slack.
const LINE_MAX: usize = 64;

/// One decoded host→device message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Msg {
    /// A GPS fix (position in microdegrees, optional course/speed).
    Fix(Fix),
    /// A barometric-altitude sample, metres.
    Alt(f32),
    /// A compass heading, degrees CW from north.
    Compass(f32),
}

/// Parse one line into a [`Msg`], or `None` if the tag is unknown or the required fields are
/// missing / malformed. Lenient by design: a corrupt line over the wire is dropped, never fatal.
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
        _ => None,
    }
}

/// `None` for a missing token or the `-` "unknown" sentinel; otherwise the parsed float (or
/// `None` if it doesn't parse). Used for the optional `course`/`speed` of an `F` line.
fn parse_opt_f32(tok: Option<&str>) -> Option<f32> {
    match tok {
        None | Some("-") => None,
        Some(s) => s.parse::<f32>().ok(),
    }
}

/// Accumulates raw CDC bytes into lines, parsing each complete `\n`-terminated line. CDC delivers
/// bytes in arbitrary chunks, so the board calls [`feed`](LineReader::feed) (or the convenience
/// [`feed_bytes`]) with each read; over-long lines (no newline within [`LINE_MAX`]) are dropped to
/// the next newline rather than split.
pub struct LineReader {
    buf: [u8; LINE_MAX],
    len: usize,
    /// Set once the current line overran the buffer; the rest of the line is skipped until the
    /// next newline re-arms a fresh line.
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

    /// Feed a chunk of received bytes; call `on_msg` for each complete line that parses. Kept
    /// generic over the callback (rather than signalling globals directly) so it's pure and
    /// unit-testable; the board passes [`dispatch`] via [`feed_bytes`].
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

// --- the cross-task hand-off: parsed samples in, telemetry out ---

/// Latest GPS fix, with fresh-fix semantics (`try_take` yields it once). See the module docs.
static FIX: Signal<CriticalSectionRawMutex, Fix> = Signal::new();
/// Latest barometric-altitude sample (metres).
static ALT: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Latest compass heading (degrees CW from north).
static COMPASS: Signal<CriticalSectionRawMutex, f32> = Signal::new();
/// Latest device telemetry to send host-ward; the app sets it ~1 Hz, the CDC task awaits it.
static TELEMETRY: Signal<CriticalSectionRawMutex, Telemetry> = Signal::new();

/// Route a decoded [`Msg`] to its sensor signal (the bridge from the USB RX task to the app
/// poll). The board passes this to [`LineReader::feed`]; [`feed_bytes`] bundles both.
pub fn dispatch(msg: Msg) {
    match msg {
        Msg::Fix(f) => FIX.signal(f),
        Msg::Alt(a) => ALT.signal(a),
        Msg::Compass(c) => COMPASS.signal(c),
    }
}

/// Convenience for the board's CDC RX loop: accumulate `bytes` and dispatch every complete line
/// to the sensor signals. `reader` persists across reads (it holds the partial-line buffer).
pub fn feed_bytes(reader: &mut LineReader, bytes: &[u8]) {
    reader.feed(bytes, dispatch);
}

/// The user's location, streamed over USB. Hand `&mut DebugLocation` to `Sensors::loc`.
pub struct DebugLocation;
impl LocationSource for DebugLocation {
    fn poll(&mut self) -> Option<Fix> {
        FIX.try_take()
    }
}

/// The barometric altimeter, streamed over USB. Hand `&mut DebugAltimeter` to `Sensors::altimeter`.
pub struct DebugAltimeter;
impl AltimeterSource for DebugAltimeter {
    fn poll(&mut self) -> Option<f32> {
        ALT.try_take()
    }
}

/// The electronic compass, streamed over USB. Hand `&mut DebugCompass` to `Sensors::compass`.
pub struct DebugCompass;
impl CompassSource for DebugCompass {
    fn poll(&mut self) -> Option<f32> {
        COMPASS.try_take()
    }
}

/// A compact device→host status line — "basic telemetry like the control panel", emitted at
/// ~1 Hz so the link never floods (the explicit performance constraint of issue #38). Integer
/// fields only, so [`format_telemetry`] is allocation- and float-free.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Telemetry {
    /// Distance actually ridden, metres (the `done` stat).
    pub ridden_m: u32,
    /// Climb accumulated, metres (the `climbed` stat).
    pub climb_m: u32,
    /// Average moving speed × 10 (km/h, one decimal); `0` when not yet moving.
    pub avg_kmh_x10: u32,
    /// Last fix's ground speed × 10 (m/s, one decimal); `0` when stopped / unknown.
    pub speed_mps_x10: u32,
    /// Last map-render time, microseconds (the panel's "Render" readout).
    pub frame_us: u32,
    /// Operating mode: `0` Idle, `1` Riding, `2` Paused.
    pub mode: u8,
}

/// Format a telemetry line (`T … \n`) into a small heap-free string the board writes to CDC.
pub fn format_telemetry(t: &Telemetry) -> heapless::String<64> {
    let mut s = heapless::String::new();
    // Infallible for the field count + 64 cap; ignore the Result rather than panic on the MCU.
    let _ = write!(
        s,
        "T {} {} {} {} {} {}\n",
        t.ridden_m, t.climb_m, t.avg_kmh_x10, t.speed_mps_x10, t.frame_us, t.mode
    );
    s
}

/// Publish the latest telemetry (called by the app loop, throttled to ~1 Hz). Overwrites any
/// unsent value, so the host always gets the freshest snapshot.
pub fn set_telemetry(t: Telemetry) {
    TELEMETRY.signal(t);
}

/// Await the next published telemetry (the CDC TX task), so the send cadence is driven by the
/// app's [`set_telemetry`] calls — no polling, no flooding.
pub async fn wait_telemetry() -> Telemetry {
    TELEMETRY.wait().await
}

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
        // A standstill: no course but a (zero-ish) speed — the `-` sentinel keeps the field positional.
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
        // Two lines plus a partial third, fed as separate chunks (as CDC would).
        r.feed(b"F 1 2 - -\nA 100", |m| got.push(m).unwrap());
        r.feed(b".5\nC 45\n", |m| got.push(m).unwrap());
        assert_eq!(
            got.as_slice(),
            &[Msg::Fix(Fix::at(1, 2)), Msg::Alt(100.5), Msg::Compass(45.0)]
        );
    }

    #[test]
    fn line_reader_drops_overlong_lines_without_splitting() {
        let mut r = LineReader::new();
        let mut got = heapless::Vec::<Msg, 4>::new();
        // A junk line far over LINE_MAX, then a good one — only the good one survives.
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
            ridden_m: 1234,
            climb_m: 56,
            avg_kmh_x10: 187,
            speed_mps_x10: 52,
            frame_us: 41000,
            mode: 1,
        };
        assert_eq!(format_telemetry(&t).as_str(), "T 1234 56 187 52 41000 1\n");
    }
}
