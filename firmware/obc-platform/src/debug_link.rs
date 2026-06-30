//! Transport-agnostic fake-sensor debug protocol (issue #38) — the board-agnostic half.
//!
//! There's no GPS / compass / altimeter hardware on the prototype (and never a good fix at the
//! bench), so a host streams a recorded ride over a debug link and this module turns it into the
//! `obc-app` HAL traits the app already polls — [`DebugLocation`], [`DebugAltimeter`] and
//! [`DebugCompass`]. The app can't tell them from real drivers, and because the protocol +
//! sources live here (not in the board crate) they move to any board unchanged. The board crate
//! owns only the concrete transport driver — on the shipping nRF54L it's a UART/VCOM link, not USB
//! (the nRF54L has no USB peripheral); it feeds received bytes to [`feed_bytes`] and `await`s
//! [`wait_telemetry`] to send the device→host status line.
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
//! - `Z <mpp>` — set the map camera to exactly `mpp` meters-per-pixel (float). A
//!   debug/benchmark hook: it drives the zoom directly instead of stepping the encoder (which
//!   only moves in fixed 1.2× detents) and always forces one map redraw, so a host sweep can
//!   pin an exact scale and read back one fresh render-stats line per setting.
//!
//! Device → host (see [`Telemetry`]): `T <frame_us> <lod> <feat_drawn> <feat_tried> <feat_dropped>
//! <chunks> <cache_hits> <cache_misses> <sd_reads> <bytes_read> <collect_us> <read_us> <sort_us>
//! <draw_us> <overlay_us> <mpp_milli>` — the last map frame's render stats (the same numbers as
//! the RTT `map frame` log / the sim's Render Stats panel), at a low fixed rate so the link never
//! floods. The trailing six are the render-benchmark fields: the per-stage wall-time breakdown
//! and the frame's camera scale (see [`Telemetry`]).
//!
//! ## Fresh-fix contract (#43) — behind `debug-link`
//! Each parsed *sensor* sample is handed across to the app through an embassy `Signal`, whose
//! `try_take` returns a value exactly **once** per signal — so `DebugLocation::poll` yields
//! `Some` only on the tick a new fix arrived and `None` between, the same cadence a real ~1 Hz
//! receiver follows. Returning the latest fix on *every* ~8 ms poll would re-trigger the
//! teleport-rejection bug #43 fixes; the `Signal` gives the correct semantics for free. Injected
//! input events instead go through a small `Channel` (a queue, not a latch) so a burst of edges
//! is delivered in order, exactly as the button debouncer's ring would. This hand-off (the
//! `Signal`/`Channel` statics + the [`DebugLocation`]/[`DebugAltimeter`]/[`DebugCompass`]/
//! [`DebugInput`] sources) lives behind the `debug-link` feature; the pure codec above does not.

use core::fmt::Write;

// The pure protocol below (parser, encoders, `LineReader`, `Telemetry`) needs only these app
// types — no embassy-sync — so it is **always** compiled and the host feeder (`obc-usb-host`) can
// reuse the one canonical codec with default features. The `Signal`/`Channel` plumbing that
// bridges parsed samples to the app's HAL traits pulls embassy-sync, so it stays behind
// `debug-link` (with its source-trait imports) at the bottom of the file.
use obc_app::{Button, ButtonEvent, Fix, InputEvent};

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
    /// A debug camera-scale command: set the map viewport to exactly this meters-per-pixel.
    Zoom(f32),
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
        "Z" => Some(Msg::Zoom(it.next()?.parse::<f32>().ok()?)),
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

/// Encode a [`Fix`] as an `F` line (the exact inverse of the `F` arm of [`parse_line`]): `F <lat>
/// <lon> <course|-> <speed|->\n`, with `course` at `{:.1}` and `speed` at `{:.2}`, and the `-`
/// sentinel for a missing (standstill) field so each stays positional. The host feeder builds its
/// outgoing fix lines through this so device and host share one encoder. Cap sized to the worst
/// case: `F ` + two i32 (11 chars each) + two spaces + `360.0` + ` ` + `99.99` + `\n` ≈ 38 bytes;
/// 48 leaves slack, so the `write!`s below truly cannot truncate.
pub fn format_fix(f: &Fix) -> heapless::String<48> {
    /// Write an optional float at `prec` decimals, or the `-` "unknown" sentinel — the inverse of
    /// [`parse_opt_f32`], keeping each field positional. (All `write!`/`push` are infallible for
    /// the cap above; ignore the Result rather than panic on the MCU.)
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

/// Accumulates raw link bytes into lines, parsing each complete `\n`-terminated line. The transport
/// delivers bytes in arbitrary chunks, so the board calls [`feed`](LineReader::feed) (or the convenience
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
    /// Per-stage map-render wall time, microseconds — the render-benchmark breakdown (filled from
    /// [`RenderStats`](obc_render::RenderStats) plus the board's read timer). `collect_us` is the
    /// visible-feature collection (quadtree walk + decode + cull + span build, and **includes**
    /// `read_us`); `read_us` is the SD/cache I/O within collect; `sort_us` the painter-order span
    /// sort; `draw_us` the full-screen clear + rasterize; `overlay_us` the route + breadcrumb +
    /// marker. `frame_us` is the whole map frame ≈ `collect_us + sort_us + draw_us + overlay_us`.
    pub collect_us: u32,
    pub read_us: u32,
    pub sort_us: u32,
    pub draw_us: u32,
    pub overlay_us: u32,
    /// Camera scale of the rendered frame, **milli-mpp** (meters-per-pixel × 1000) — lets the host
    /// label each sample by zoom without echoing the `Z` command back. Integer to keep the line
    /// float-free; e.g. `500` = 0.5 m/px.
    pub mpp_milli: u32,
}

/// Format a telemetry line (`T … \n`) into a small heap-free string the board writes to the link.
/// Cap sized to the worst case: `T` + sixteen `u32::MAX` (10-digit) fields each space-separated
/// (16 spaces) + `\n` = 1 + 160 + 16 + 1 = 178 bytes, so the `write!` below truly cannot truncate.
pub fn format_telemetry(t: &Telemetry) -> heapless::String<192> {
    let mut s = heapless::String::new();
    // Infallible for the field count + cap; ignore the Result rather than panic on the MCU.
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

/// Parse a `T …` telemetry line back into a [`Telemetry`] — the exact inverse of
/// [`format_telemetry`] — or `None` for a non-`T` line or one with a missing / malformed field (so
/// other device chatter is simply ignored). The host feeder reads its render-stats readout through
/// this so device and host share one telemetry codec; matching [`Telemetry::lod`] it parses `lod`
/// as a `u8`.
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

// --- the cross-task hand-off: parsed samples in, telemetry out ---
//
// Everything below pulls embassy-sync (`Signal`/`Channel`) and the app's source traits, so it is
// gated behind `debug-link`. The board crate enables the feature and owns the concrete transport
// driver (UART/VCOM on the nRF54L); the host feeder builds without it and reuses only the pure codec
// above.
#[cfg(feature = "debug-link")]
mod handoff {
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_sync::signal::Signal;
    use obc_app::{AltimeterSource, CompassSource, Fix, InputEvent, InputSource, LocationSource};

    use super::{LineReader, Msg, Telemetry};

    /// Latest GPS fix, with fresh-fix semantics (`try_take` yields it once). See the module docs.
    static FIX: Signal<CriticalSectionRawMutex, Fix> = Signal::new();
    /// Latest barometric-altitude sample (metres).
    static ALT: Signal<CriticalSectionRawMutex, f32> = Signal::new();
    /// Latest compass heading (degrees CW from north).
    static COMPASS: Signal<CriticalSectionRawMutex, f32> = Signal::new();
    /// Latest device telemetry to send host-ward; the app sets it, the transport's TX task awaits it.
    static TELEMETRY: Signal<CriticalSectionRawMutex, Telemetry> = Signal::new();
    /// Latest debug camera-scale command (meters-per-pixel), `try_take`-once like a sensor. The
    /// board's map loop drains it each frame and applies it via `App::set_map_mpp` (render
    /// benchmark — see the `Z` wire command).
    static ZOOM: Signal<CriticalSectionRawMutex, f32> = Signal::new();

    /// Injected input events (encoder turns / button edges), queued in order for the input plane to
    /// drain alongside the physical buttons. A queue, not a latch: a tap is a down+up *pair* and a
    /// burst must arrive intact. Sized like the gesture channel — a frame yields at most a couple.
    const INPUT_QUEUE: usize = 16;
    static INPUT: Channel<CriticalSectionRawMutex, InputEvent, INPUT_QUEUE> = Channel::new();

    /// Route a decoded [`Msg`] to its signal/queue (the bridge from the link RX task to the app
    /// poll). The board passes this to [`LineReader::feed`]; [`feed_bytes`] bundles both.
    pub fn dispatch(msg: Msg) {
        match msg {
            Msg::Fix(f) => FIX.signal(f),
            Msg::Alt(a) => ALT.signal(a),
            Msg::Compass(c) => COMPASS.signal(c),
            Msg::Zoom(z) => ZOOM.signal(z),
            // Drop on the (unreachable) overflow rather than block the RX task.
            Msg::Input(ev) => {
                let _ = INPUT.try_send(ev);
            }
        }
    }

    /// Convenience for the board's link RX loop: accumulate `bytes` and dispatch every complete line
    /// to the sensor signals. `reader` persists across reads (it holds the partial-line buffer).
    pub fn feed_bytes(reader: &mut LineReader, bytes: &[u8]) {
        reader.feed(bytes, dispatch);
    }

    /// The user's location, streamed over the debug link. Hand `&mut DebugLocation` to `Sensors::loc`.
    pub struct DebugLocation;
    impl LocationSource for DebugLocation {
        fn poll(&mut self) -> Option<Fix> {
            FIX.try_take()
        }
    }

    /// The barometric altimeter, streamed over the debug link. Hand `&mut DebugAltimeter` to
    /// `Sensors::altimeter`.
    pub struct DebugAltimeter;
    impl AltimeterSource for DebugAltimeter {
        fn poll(&mut self) -> Option<f32> {
            ALT.try_take()
        }
    }

    /// The electronic compass, streamed over the debug link. Hand `&mut DebugCompass` to `Sensors::compass`.
    pub struct DebugCompass;
    impl CompassSource for DebugCompass {
        fn poll(&mut self) -> Option<f32> {
            COMPASS.try_take()
        }
    }

    /// Injected input, drained by the input plane next to the physical buttons. The board chains
    /// this after its `ButtonInput` into the gesture recogniser, so injected turns/edges become
    /// gestures (taps and holds) identically to real presses.
    pub struct DebugInput;
    impl InputSource for DebugInput {
        fn poll(&mut self) -> Option<InputEvent> {
            INPUT.try_receive().ok()
        }
    }

    /// Take a pending debug `Z` camera-scale command (meters-per-pixel), if one arrived since the
    /// last call — `try_take`-once, like a sensor poll. The board's map loop calls this each frame
    /// and applies any value via `App::set_map_mpp` (render benchmark).
    pub fn take_zoom() -> Option<f32> {
        ZOOM.try_take()
    }

    /// Publish the latest telemetry (called by the app loop, throttled). Overwrites any unsent
    /// value, so the host always gets the freshest snapshot.
    pub fn set_telemetry(t: Telemetry) {
        TELEMETRY.signal(t);
    }

    /// Await the next published telemetry (the transport's TX task), so the send cadence is driven by
    /// the app's [`set_telemetry`] calls — no polling, no flooding.
    pub async fn wait_telemetry() -> Telemetry {
        TELEMETRY.wait().await
    }
}

// Re-export the gated hand-off at the module root so the board crate's call sites
// (`debug_link::DebugLocation`, `debug_link::feed_bytes`, …) are unchanged by the split.
#[cfg(feature = "debug-link")]
pub use handoff::{
    dispatch, feed_bytes, set_telemetry, take_zoom, wait_telemetry, DebugAltimeter, DebugCompass, DebugInput,
    DebugLocation,
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
    fn parses_zoom() {
        assert_eq!(parse_line("Z 0.5"), Some(Msg::Zoom(0.5)));
        assert_eq!(parse_line("Z 5"), Some(Msg::Zoom(5.0)));
        assert_eq!(parse_line("Z"), None); // missing value
        assert_eq!(parse_line("Z x"), None); // non-numeric
    }

    #[test]
    fn parses_input_injection() {
        assert_eq!(parse_line("K t 1"), Some(Msg::Input(InputEvent::Turn(1))));
        assert_eq!(parse_line("K t -2"), Some(Msg::Input(InputEvent::Turn(-2))));
        assert_eq!(parse_line("K e d"), Some(Msg::Input(InputEvent::Button(ButtonEvent::Down(Button::Encoder)))));
        assert_eq!(parse_line("K b u"), Some(Msg::Input(InputEvent::Button(ButtonEvent::Up(Button::Back)))));
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
        // Two lines plus a partial third, fed as separate chunks (as the link would).
        r.feed(b"F 1 2 - -\nA 100", |m| got.push(m).unwrap());
        r.feed(b".5\nC 45\n", |m| got.push(m).unwrap());
        assert_eq!(got.as_slice(), &[Msg::Fix(Fix::at(1, 2)), Msg::Alt(100.5), Msg::Compass(45.0)]);
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
        // `parse_telemetry` is the exact inverse of `format_telemetry` — the host reads back what
        // the device wrote. A `\n`-terminated line is fine: `parse_telemetry` splits on whitespace.
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
        // `format_fix` is the exact inverse of the `F` arm of `parse_line`. Course/speed are
        // re-read at the formatter's precision (`{:.1}` / `{:.2}`), so pick values that survive it.
        let f = Fix { lat: 48_122_905, lon: 7_814_438, course: Some(90.5), speed_mps: Some(5.25) };
        assert_eq!(format_fix(&f).as_str(), "F 48122905 7814438 90.5 5.25\n");
        assert_eq!(parse_line(format_fix(&f).as_str().trim_end()), Some(Msg::Fix(f)));
    }

    #[test]
    fn format_fix_uses_dash_for_a_standstill() {
        // No course/speed → the `-` sentinel keeps each field positional, exactly as the host's
        // old `fix_line` produced and `parse_line` accepts.
        let f = Fix::at(1, 2);
        assert_eq!(format_fix(&f).as_str(), "F 1 2 - -\n");
        assert_eq!(parse_line(format_fix(&f).as_str().trim_end()), Some(Msg::Fix(f)));
    }

    /// Item 5 (`\r` / CRLF, `feed` ~183): `feed` treats `\r` and `\n` *both* as line
    /// terminators, but every existing feed test uses only `\n`. A refactor that checked
    /// only `b == b'\n'` would pass them all yet break a host on CRLF. Feed a bare `\r`
    /// and a full `\r\n` and prove each line still dispatches exactly once (the `\n` after
    /// a `\r` lands on an already-reset empty buffer, so no phantom blank line).
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

    /// Item 6 (blank-line guard, `feed` ~184 `self.len > 0`): empty lines from the wire
    /// (a stray `\n`, or a CRLF's `\n` after the `\r` already flushed) must be skipped at
    /// the `LineReader` level, not handed to `parse_line`. Only bare `parse_line("")` is
    /// tested elsewhere; this proves the reader's guard so blank lines never even reach the
    /// parser and produce no spurious `Msg`.
    #[test]
    fn line_reader_skips_blank_lines() {
        let mut r = LineReader::new();
        let mut got = heapless::Vec::<Msg, 8>::new();
        // Leading blanks, blank between, trailing blanks — only the two real lines dispatch.
        r.feed(b"\n\nA 5\n\n\nC 9\n\n", |m| got.push(m).unwrap());
        assert_eq!(got.as_slice(), &[Msg::Alt(5.0), Msg::Compass(9.0)], "blank lines produce no Msg");
    }

    /// Item 7 (i32 extremes through format→parse, the worst case the 48-byte cap is sized
    /// for, ~129): `parse_line` parses `i32::MIN`/`i32::MAX` and a mid value round-trips,
    /// but the *extreme* lat/lon has never gone format→parse. `F -2147483648 2147483647`
    /// is the widest fix line; this proves `format_fix` emits it within the cap (no
    /// truncation) and `parse_line` reads the exact extremes back.
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

    /// Item 8 (lod u8 overflow, `parse_telemetry` ~285): `lod` is parsed as `u8`, so a
    /// line whose `lod` field exceeds 255 must make the whole parse fail (`None`), not
    /// wrap or truncate. A value of 999 in the lod slot is the guard — every other field
    /// is valid, so only the u8 overflow can reject the line.
    #[test]
    fn parse_telemetry_rejects_lod_above_u8() {
        // Valid T line shape, but lod = 999 (> u8::MAX) — the u8 parse fails the line.
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

    /// Item 9 (LINE_MAX boundary, `feed` ~195 `self.len < LINE_MAX`): a line that *fills*
    /// the 64-byte buffer exactly (64 chars, no newline yet) must NOT trip overflow — the
    /// boundary is `< LINE_MAX` for accepting a byte, `else` overflow only on the 65th.
    /// Only a 200-char overrun is tested elsewhere; this pins the exact-fit edge so a line
    /// padded to precisely LINE_MAX still parses when its newline arrives.
    #[test]
    fn line_reader_accepts_a_line_filling_the_buffer_exactly() {
        // `Z 1` plus enough trailing spaces to total exactly LINE_MAX=64 bytes. Trailing
        // whitespace is ignored by `split_ascii_whitespace`, so it parses as `Zoom(1.0)`.
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
