//! Streaming GPX track-point scanner (`no_std`).
//!
//! Pulls `<trkpt lat=".." lon="..">…<ele>…</ele></trkpt>` points out of a
//! [`ByteSource`] one at a time, reading the file in fixed blocks with compaction so a
//! point that straddles a block boundary is handled transparently. RAM is O(1) (one
//! [`SCAN_BUF`]-sized buffer) regardless of route length, so converting a hundreds-of-km
//! GPX on-device is feasible.
//!
//! A deliberately small hand-rolled scan, not a full XML stack: GPX track points are a
//! regular shape and that is all the converter needs. Elevation is optional; timestamps
//! are ignored (a route has no time).

use crate::byte_io::{ByteSource, Error};

/// Scan buffer size. Must comfortably exceed one `<trkpt>…</trkpt>` element (a few
/// hundred bytes) so at least one whole point is always resident after a refill.
const SCAN_BUF: usize = 4096;

/// One raw track point straight from the GPX: microdegree position + optional
/// elevation in meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: Option<f32>,
}

/// A forward-only scanner over a GPX byte source. Call [`next_point`](GpxScanner::next_point)
/// until it returns `Ok(None)`.
pub struct GpxScanner<'a> {
    src: &'a dyn ByteSource,
    buf: [u8; SCAN_BUF],
    filled: usize,
    pos: usize,
    next_read: u32,
    src_len: u32,
}

impl<'a> GpxScanner<'a> {
    pub fn new(src: &'a dyn ByteSource) -> Self {
        let src_len = src.len();
        GpxScanner { src, buf: [0; SCAN_BUF], filled: 0, pos: 0, next_read: 0, src_len }
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

    /// The next track point, or `None` once the source is exhausted. Malformed points
    /// (missing lat/lon) are skipped rather than erroring.
    pub fn next_point(&mut self) -> Result<Option<RawPoint>, Error> {
        loop {
            let window = &self.buf[self.pos..self.filled];
            let Some(rel) = find(window, b"<trkpt") else {
                // No start tag here. Keep a short tail (a split "<trkpt") across the refill.
                if self.at_source_end() {
                    return Ok(None);
                }
                let keep = 5.min(self.filled - self.pos);
                self.pos = self.filled - keep;
                if self.refill()? == 0 {
                    return Ok(None);
                }
                continue;
            };
            let start = self.pos + rel;

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
            let self_closing = self.buf[tag_end - 1] == b'/';
            let (lat, lon) = parse_latlon(&self.buf[start..tag_end]);

            if self_closing {
                self.pos = tag_end + 1;
                if let (Some(lat), Some(lon)) = (lat, lon) {
                    return Ok(Some(RawPoint { lon: micro(lon), lat: micro(lat), ele: None }));
                }
                continue;
            }

            // Need the matching "</trkpt>" so we can read the body's <ele>.
            let Some(close) = find(&self.buf[tag_end..self.filled], b"</trkpt>") else {
                if self.at_source_end() {
                    return Ok(None);
                }
                self.pos = start;
                if self.refill()? == 0 {
                    return Ok(None);
                }
                continue;
            };
            let ele = parse_ele(&self.buf[tag_end + 1..tag_end + close]);
            self.pos = tag_end + close + "</trkpt>".len();
            if let (Some(lat), Some(lon)) = (lat, lon) {
                return Ok(Some(RawPoint { lon: micro(lon), lat: micro(lat), ele }));
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

/// Parse `lat`/`lon` from an opening `<trkpt …>` tag (order-independent).
fn parse_latlon(tag: &[u8]) -> (Option<f64>, Option<f64>) {
    let Ok(s) = core::str::from_utf8(tag) else {
        return (None, None);
    };
    (attr_f64(s, "lat"), attr_f64(s, "lon"))
}

/// Read a quoted attribute (`name="…"` / `name='…'`) as `f64`, matching `name` only as
/// a whole token. Ported from the simulator's GPX parser.
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

/// Pull `<ele>…</ele>` from a trkpt body as `f32`, if present.
fn parse_ele(body: &[u8]) -> Option<f32> {
    let s = core::str::from_utf8(body).ok()?;
    let start = s.find("<ele>")? + "<ele>".len();
    let end = s[start..].find("</ele>")? + start;
    s[start..end].trim().parse().ok()
}
