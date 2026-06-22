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
//! - `K t <n>` / `K e <d|u>` / `K b <d|u>` — **input injection**: an encoder turn of `n` detents
//!   (signed), or an encoder/Back button down/up edge. These feed the gesture recogniser exactly
//!   like the physical buttons, so a host can drive the UI (taps and — via a delayed up — holds)
//!   for hardware-in-the-loop work without anyone pressing a button.
//!
//! Device → host (see [`Telemetry`]): `T <frame_us> <lod> <feat_drawn> <feat_tried> <feat_dropped>
//! <chunks> <cache_hits> <cache_misses> <sd_reads> <bytes_read>` — the last map frame's render
//! stats (the same numbers as the RTT `map frame` log / the sim's Render Stats panel), at a low
//! fixed rate so the link never floods.
//!
//! ## Fresh-fix contract (#43)
//! Each parsed *sensor* sample is handed across to the app through an embassy [`Signal`], whose
//! `try_take` returns a value exactly **once** per signal — so [`DebugLocation::poll`] yields
//! `Some` only on the tick a new fix arrived and `None` between, the same cadence a real ~1 Hz
//! receiver follows. Returning the latest fix on *every* ~8 ms poll would re-trigger the
//! teleport-rejection bug #43 fixes; the `Signal` gives the correct semantics for free. Injected
//! input events instead go through a small [`Channel`] (a queue, not a latch) so a burst of edges
//! is delivered in order, exactly as the button debouncer's ring would.

use core::fmt::Write;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use obc_app::{
    AltimeterSource, Button, ButtonEvent, CompassSource, Fix, InputEvent, InputSource, LocationSource,
};

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
    /// An injected raw input event (encoder turn or a button down/up edge).
    Input(InputEvent),
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
        "K" => parse_key(&mut it),
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

/// Parse the tokens after a `K` tag into an injected input event: `K t <n>` an encoder turn of
/// `n` signed detents, `K e <d|u>` an encoder down/up edge, `K b <d|u>` a Back down/up edge.
fn parse_key(it: &mut core::str::SplitAsciiWhitespace) -> Option<Msg> {
    let ev = match it.next()? {
        "t" => InputEvent::Turn(it.next()?.parse::<i32>().ok()?),
        "e" => InputEvent::Button(edge(it.next()?, Button::Encoder)?),
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
/// Latest device telemetry to send host-ward; the app sets it, the CDC task awaits it.
static TELEMETRY: Signal<CriticalSectionRawMutex, Telemetry> = Signal::new();

/// Injected input events (encoder turns / button edges), queued in order for the input plane to
/// drain alongside the physical buttons. A queue, not a latch: a tap is a down+up *pair* and a
/// burst must arrive intact. Sized like the gesture channel — a frame yields at most a couple.
const INPUT_QUEUE: usize = 16;
static INPUT: Channel<CriticalSectionRawMutex, InputEvent, INPUT_QUEUE> = Channel::new();

/// Route a decoded [`Msg`] to its signal/queue (the bridge from the USB RX task to the app poll).
/// The board passes this to [`LineReader::feed`]; [`feed_bytes`] bundles both.
pub fn dispatch(msg: Msg) {
    match msg {
        Msg::Fix(f) => FIX.signal(f),
        Msg::Alt(a) => ALT.signal(a),
        Msg::Compass(c) => COMPASS.signal(c),
        // Drop on the (unreachable) overflow rather than block the RX task.
        Msg::Input(ev) => {
            let _ = INPUT.try_send(ev);
        }
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

/// Injected input, drained by the input plane next to the physical buttons. The board chains this
/// after its `ButtonInput` into the gesture recogniser, so injected turns/edges become gestures
/// (taps and holds) identically to real presses.
pub struct DebugInput;
impl InputSource for DebugInput {
    fn poll(&mut self) -> Option<InputEvent> {
        INPUT.try_receive().ok()
    }
}

/// The last map frame's **render stats** — the same numbers as the RTT `map frame` log and the
/// sim's Render Stats panel (frame time, LOD, feature/chunk counts, map-cache + SD accounting).
/// Snapshotted from [`RenderStats`](obc_render::RenderStats) by the board after each map render and
/// sent host-ward at a low fixed rate. Integer fields only, so [`format_telemetry`] is float-free.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Telemetry {
    /// Last map-render wall time, microseconds.
    pub frame_us: u32,
    /// LOD chosen for the last render.
    pub lod: u8,
    /// Features drawn / tried, and dropped (scratch overflow — want `0`).
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
}

/// Format a telemetry line (`T … \n`) into a small heap-free string the board writes to CDC.
/// Cap sized to the worst case: `T ` + ten `u32::MAX` (10 digits) fields + 9 spaces + `\n` = 105
/// bytes, so the `write!` below truly cannot truncate.
pub fn format_telemetry(t: &Telemetry) -> heapless::String<112> {
    let mut s = heapless::String::new();
    // Infallible for the field count + cap; ignore the Result rather than panic on the MCU.
    let _ = write!(
        s,
        "T {} {} {} {} {} {} {} {} {} {}\n",
        t.frame_us, t.lod, t.feat_drawn, t.feat_tried, t.feat_dropped, t.chunks, t.cache_hits,
        t.cache_misses, t.sd_reads, t.bytes_read
    );
    s
}

/// Publish the latest telemetry (called by the app loop, throttled). Overwrites any unsent value,
/// so the host always gets the freshest snapshot.
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
    fn parses_input_injection() {
        assert_eq!(parse_line("K t 1"), Some(Msg::Input(InputEvent::Turn(1))));
        assert_eq!(parse_line("K t -2"), Some(Msg::Input(InputEvent::Turn(-2))));
        assert_eq!(
            parse_line("K e d"),
            Some(Msg::Input(InputEvent::Button(ButtonEvent::Down(Button::Encoder))))
        );
        assert_eq!(
            parse_line("K b u"),
            Some(Msg::Input(InputEvent::Button(ButtonEvent::Up(Button::Back))))
        );
        assert_eq!(parse_line("K e x"), None); // bad edge
        assert_eq!(parse_line("K z 1"), None); // unknown key
        assert_eq!(parse_line("K t"), None); // missing detents
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
        };
        assert_eq!(format_telemetry(&t).as_str(), "T 41000 2 312 480 0 9 27 3 3 12288\n");
    }
}
