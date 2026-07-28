//! Streaming GPX scanners (`no_std`): track points and waypoints.
//!
//! [`GpxScanner`] pulls `<trkpt lat=".." lon="..">…<ele>…</ele></trkpt>` points out of
//! a [`ByteSource`] one at a time; [`WptScanner`] does the same for top-level
//! `<wpt>` waypoints (name, optional elevation, and the `<sym>`/`<type>` symbol the
//! converter maps to a category). Both read the file in fixed blocks
//! with compaction so an element that straddles a block boundary is handled
//! transparently. RAM is O(1) (one [`SCAN_BUF`]-sized buffer per scanner) regardless
//! of route length, so converting a hundreds-of-km GPX on-device is feasible.
//!
//! A deliberately small hand-rolled scan, not a full XML stack: GPX elements are a
//! regular shape and that is all the converter needs. Elevation is optional;
//! timestamps are ignored (a route has no time). Waypoint names are taken verbatim
//! (no entity unescaping — the phone-side importer runs a real XML parser; this path
//! only backs the on-device GPX upload).

use heapless::String;

use obc_formats::io::{ByteSource, Error};
use obc_formats::obcr::WAYPOINT_NAME_CAP;

/// Scan buffer size. Must comfortably exceed one `<trkpt>…</trkpt>` / `<wpt>…</wpt>`
/// element (a few hundred bytes) so at least one whole element is always resident
/// after a refill.
const SCAN_BUF: usize = 4096;

/// Stored bytes of a `<wpt>`'s symbol. Real `<sym>`/`<type>` values are one or two words
/// ("Drinking Water", "Convenience Store"); a longer one is freeform prose that no
/// [`category_for_symbol`](crate::symbol::category_for_symbol) row could match, so truncating it
/// here costs nothing and keeps [`RawWaypoint`] small enough for a bounded resident set.
pub const WAYPOINT_SYMBOL_CAP: usize = 32;

/// One raw track point straight from the GPX: microdegree position + optional
/// elevation in meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: Option<f32>,
}

/// One raw `<wpt>` waypoint straight from the GPX: position, optional elevation, its `<name>`
/// truncated to [`WAYPOINT_NAME_CAP`] bytes (on a char boundary), and its raw symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct RawWaypoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: Option<f32>,
    pub name: String<WAYPOINT_NAME_CAP>,
    /// The waypoint's icon as the exporter wrote it — `<sym>` if non-empty, else `<type>`, else
    /// empty. Kept verbatim (truncated to [`WAYPOINT_SYMBOL_CAP`]); the mapping onto a category
    /// is [`category_for_symbol`](crate::symbol::category_for_symbol)'s job, not the scanner's.
    pub symbol: String<WAYPOINT_SYMBOL_CAP>,
}

/// The shared block-buffered scan state: a window over the source that refills with
/// compaction and locates whole `<tag …>[body</tag>]` elements. Both scanners are
/// thin element-parsers over this core.
struct ScanCore<'a> {
    src: &'a dyn ByteSource,
    buf: [u8; SCAN_BUF],
    filled: usize,
    pos: usize,
    next_read: u32,
    src_len: u32,
}

/// A located element, as index ranges into the core's buffer (valid until the next
/// [`ScanCore::next_element`] call): the opening tag `<tag …` (attributes), and the
/// body for a non-self-closing element.
struct Element {
    attr: core::ops::Range<usize>,
    body: Option<core::ops::Range<usize>>,
}

impl<'a> ScanCore<'a> {
    fn new(src: &'a dyn ByteSource) -> Self {
        let src_len = src.len();
        ScanCore { src, buf: [0; SCAN_BUF], filled: 0, pos: 0, next_read: 0, src_len }
    }

    /// Drop the consumed prefix `buf[..pos]` and read more from the source into the
    /// freed space. Returns the number of new bytes read (0 at end of source / when the
    /// buffer is full).
    fn refill(&mut self) -> Result<usize, Error> {
        if self.pos > 0 {
            self.buf.copy_within(self.pos..self.filled, 0);
            self.filled -= self.pos;
            self.pos = 0;
        }
        let space = SCAN_BUF - self.filled;
        let avail = (self.src_len - self.next_read) as usize;
        let n = space.min(avail);
        if n == 0 {
            return Ok(0);
        }
        self.src.read_at(self.next_read, &mut self.buf[self.filled..self.filled + n])?;
        self.filled += n;
        self.next_read += n as u32;
        Ok(n)
    }

    fn at_source_end(&self) -> bool {
        self.next_read >= self.src_len
    }

    /// Locate the next whole `open`-tag element (e.g. `open = b"<trkpt"`,
    /// `close = b"</trkpt>"`), refilling as needed, and advance past it. Returns
    /// `None` once the source is exhausted (a trailing truncated element is dropped,
    /// matching a truncated file's other losses).
    fn next_element(&mut self, open: &[u8], close: &[u8]) -> Result<Option<Element>, Error> {
        loop {
            let window = &self.buf[self.pos..self.filled];
            let Some(rel) = find(window, open) else {
                // No start tag here. Keep a short tail (a split `open`) across the refill.
                if self.at_source_end() {
                    return Ok(None);
                }
                let keep = (open.len() - 1).min(self.filled - self.pos);
                self.pos = self.filled - keep;
                if self.refill()? == 0 {
                    return Ok(None);
                }
                continue;
            };
            let start = self.pos + rel;

            // Tag-name boundary: `<wpt` must not match a longer tag name.
            if start + open.len() >= self.filled {
                if self.at_source_end() {
                    return Ok(None);
                }
                self.pos = start;
                if self.refill()? == 0 {
                    return Ok(None);
                }
                continue;
            }
            let after = self.buf[start + open.len()];
            if !(after.is_ascii_whitespace() || after == b'/' || after == b'>') {
                self.pos = start + open.len();
                continue;
            }

            // Need the opening tag's '>' in the buffer.
            let Some(gt) = find(&self.buf[start..self.filled], b">") else {
                if self.at_source_end() {
                    return Ok(None);
                }
                self.pos = start;
                if self.refill()? == 0 {
                    return Ok(None);
                }
                continue;
            };
            let tag_end = start + gt; // index of '>'
            if self.buf[tag_end - 1] == b'/' {
                self.pos = tag_end + 1;
                return Ok(Some(Element { attr: start..tag_end, body: None }));
            }

            // Need the matching close tag so the caller can read the body.
            let Some(rel_close) = find(&self.buf[tag_end..self.filled], close) else {
                if self.at_source_end() {
                    return Ok(None);
                }
                self.pos = start;
                if self.refill()? == 0 {
                    return Ok(None);
                }
                continue;
            };
            self.pos = tag_end + rel_close + close.len();
            return Ok(Some(Element { attr: start..tag_end, body: Some(tag_end + 1..tag_end + rel_close) }));
        }
    }
}

/// A forward-only scanner over a GPX byte source's track points. Call
/// [`next_point`](GpxScanner::next_point) until it returns `Ok(None)`.
pub struct GpxScanner<'a> {
    core: ScanCore<'a>,
}

impl<'a> GpxScanner<'a> {
    pub fn new(src: &'a dyn ByteSource) -> Self {
        GpxScanner { core: ScanCore::new(src) }
    }

    /// The next track point, or `None` once the source is exhausted. Malformed points
    /// (missing lat/lon) are skipped rather than erroring.
    pub fn next_point(&mut self) -> Result<Option<RawPoint>, Error> {
        loop {
            let Some(el) = self.core.next_element(b"<trkpt", b"</trkpt>")? else {
                return Ok(None);
            };
            let (lat, lon) = parse_latlon(&self.core.buf[el.attr]);
            let ele = el.body.and_then(|b| parse_ele(&self.core.buf[b]));
            if let (Some(lat), Some(lon)) = (lat, lon) {
                return Ok(Some(RawPoint { lon: micro(lon), lat: micro(lat), ele }));
            }
        }
    }
}

/// A forward-only scanner over a GPX byte source's `<wpt>` waypoints. Call
/// [`next_waypoint`](WptScanner::next_waypoint) until it returns `Ok(None)`.
///
/// A separate scanner (not a mode of [`GpxScanner`]) because GPX carries waypoints
/// file-level *before* the track: the converter runs this pass to completion first,
/// then streams the track — two sequential O(1)-RAM passes, never two live buffers.
pub struct WptScanner<'a> {
    core: ScanCore<'a>,
}

impl<'a> WptScanner<'a> {
    pub fn new(src: &'a dyn ByteSource) -> Self {
        WptScanner { core: ScanCore::new(src) }
    }

    /// The next waypoint, or `None` once the source is exhausted. Malformed waypoints
    /// (missing lat/lon) are skipped; a missing `<name>` yields an empty name, a missing
    /// `<sym>`/`<type>` an empty symbol.
    pub fn next_waypoint(&mut self) -> Result<Option<RawWaypoint>, Error> {
        loop {
            let Some(el) = self.core.next_element(b"<wpt", b"</wpt>")? else {
                return Ok(None);
            };
            let (lat, lon) = parse_latlon(&self.core.buf[el.attr]);
            let (ele, name, symbol) = match el.body {
                Some(b) => {
                    let body = &self.core.buf[b];
                    (parse_ele(body), parse_name(body), parse_symbol(body))
                }
                None => (None, String::new(), String::new()),
            };
            if let (Some(lat), Some(lon)) = (lat, lon) {
                return Ok(Some(RawWaypoint { lon: micro(lon), lat: micro(lat), ele, name, symbol }));
            }
        }
    }
}

/// Degrees → microdegrees, rounded (`libm::round`, no `std`).
fn micro(deg: f64) -> i32 {
    libm::round(deg * 1e6) as i32
}

/// First index of `needle` in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Parse `lat`/`lon` from an opening `<trkpt …>` / `<wpt …>` tag (order-independent).
fn parse_latlon(tag: &[u8]) -> (Option<f64>, Option<f64>) {
    let Ok(s) = core::str::from_utf8(tag) else {
        return (None, None);
    };
    (attr_f64(s, "lat"), attr_f64(s, "lon"))
}

/// Read a quoted attribute (`name="…"` / `name='…'`) as `f64`, matching `name` only as a
/// whole token.
fn attr_f64(tag: &str, name: &str) -> Option<f64> {
    let mut search = tag;
    loop {
        let idx = search.find(name)?;
        let before_ok = idx == 0 || search.as_bytes()[idx - 1].is_ascii_whitespace();
        let after = &search[idx + name.len()..];
        let after_trim = after.trim_start();
        if before_ok && after_trim.starts_with('=') {
            let after_eq = after_trim[1..].trim_start();
            let quote = after_eq.chars().next()?;
            if quote == '"' || quote == '\'' {
                let val = &after_eq[1..];
                let end = val.find(quote)?;
                return val[..end].trim().parse().ok();
            }
        }
        search = &search[idx + name.len()..];
    }
}

/// Pull `<ele>…</ele>` from an element body as `f32`, if present.
fn parse_ele(body: &[u8]) -> Option<f32> {
    let s = core::str::from_utf8(body).ok()?;
    let start = s.find("<ele>")? + "<ele>".len();
    let end = s[start..].find("</ele>")? + start;
    s[start..end].trim().parse().ok()
}

/// Pull `<name>…</name>` from a `<wpt>` body, trimmed and truncated to
/// [`WAYPOINT_NAME_CAP`] bytes on a char boundary. Missing name → empty string.
fn parse_name(body: &[u8]) -> String<WAYPOINT_NAME_CAP> {
    parse_text(body, "<name>", "</name>")
}

/// Pull the waypoint's symbol from a `<wpt>` body: `<sym>` if present and non-empty, else
/// `<type>`, else empty. Two tags for one idea — Garmin (and the planners that copy it) write
/// `<sym>`, RideWithGPS/Komoot write `<type>`, some exports carry both — so `<sym>` wins when it
/// says something and `<type>` is the fallback rather than a second, competing value.
fn parse_symbol(body: &[u8]) -> String<WAYPOINT_SYMBOL_CAP> {
    let sym = parse_text(body, "<sym>", "</sym>");
    if !sym.is_empty() {
        return sym;
    }
    parse_text(body, "<type>", "</type>")
}

/// Pull an `open`…`close` child element's text out of an element body, trimmed and truncated to
/// `N` bytes on a char boundary (the same bounded-buffer discipline for every child tag). Missing
/// tag, or an unterminated one → empty string.
fn parse_text<const N: usize>(body: &[u8], open: &str, close: &str) -> String<N> {
    let mut out = String::new();
    let Ok(s) = core::str::from_utf8(body) else {
        return out;
    };
    let Some(start) = s.find(open).map(|i| i + open.len()) else {
        return out;
    };
    let Some(end) = s[start..].find(close).map(|i| i + start) else {
        return out;
    };
    for ch in s[start..end].trim().chars() {
        if out.push(ch).is_err() {
            break;
        }
    }
    out
}
