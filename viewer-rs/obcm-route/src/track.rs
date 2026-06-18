//! Recorded-track format: the fixed-record ride log + its GPX export (`no_std`).
//!
//! While riding, the device appends one [`TrackPoint`] per accepted GPS fix to an SD-card
//! file as a **fixed 16-byte record** — *no header*, so the file is just a record array
//! and truncating to a 16-byte boundary is always valid (the worst a power-loss can cost
//! is the in-flight record). On **Finish** the log is converted to a `.gpx`
//! ([`track_to_gpx`]) in one streaming pass and the temp log is dropped.
//!
//! This is deliberately *not* the [`OBCR`](crate) route format: a route is decimated for
//! compact drawing, whereas a recorded track wants full GPS fidelity. So the log keeps
//! every accepted point verbatim and only the on-screen breadcrumb (host-side, in RAM) is
//! decimated.
//!
//! The format + the GPX writer live here in the format crate so the firmware and the
//! simulator share one implementation, exactly like the GPX→OBCR [`convert`](crate::convert)
//! path. The byte I/O goes through the same [`ByteSource`]/[`ByteSink`] seam.

use core::fmt::Write;

use heapless::String;

use crate::byte_io::{ByteSink, ByteSource, Error};

/// One recorded fix: position (microdegrees), barometric elevation (m), a millisecond
/// timestamp, and whether it begins a new track segment (after a pause or a GPS gap).
///
/// `t_ms` is stored for a future wall-clock but **not yet emitted** into the GPX `<time>` —
/// the device has no date/time source, so writing one now would be a fabricated timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackPoint {
    pub lon: i32,
    pub lat: i32,
    pub ele: i16,
    pub t_ms: u32,
    /// `true` on the first point of a new `<trkseg>` (start of ride, or first fix after a
    /// pause / dropout). Drives segment splitting in [`track_to_gpx`].
    pub segment_start: bool,
}

/// On-disk size of one record. The whole log is `N × TRACK_RECORD_LEN` bytes, no header.
pub const TRACK_RECORD_LEN: usize = 16;

/// Layout: `lon`(i32) `lat`(i32) `ele`(i16) `flags`(u16, bit0 = segment_start) `t_ms`(u32).
const FLAG_SEGMENT_START: u16 = 0x0001;

/// Encode a point to its fixed 16-byte record (little-endian, matching the readers in
/// `reader.rs` / `convert.rs`).
pub fn encode_record(p: &TrackPoint) -> [u8; TRACK_RECORD_LEN] {
    let mut b = [0u8; TRACK_RECORD_LEN];
    b[0..4].copy_from_slice(&p.lon.to_le_bytes());
    b[4..8].copy_from_slice(&p.lat.to_le_bytes());
    b[8..10].copy_from_slice(&p.ele.to_le_bytes());
    let flags = if p.segment_start { FLAG_SEGMENT_START } else { 0 };
    b[10..12].copy_from_slice(&flags.to_le_bytes());
    b[12..16].copy_from_slice(&p.t_ms.to_le_bytes());
    b
}

/// Decode one fixed 16-byte record.
pub fn decode_record(b: &[u8; TRACK_RECORD_LEN]) -> TrackPoint {
    let lon = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let lat = i32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let ele = i16::from_le_bytes([b[8], b[9]]);
    let flags = u16::from_le_bytes([b[10], b[11]]);
    let t_ms = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
    TrackPoint { lon, lat, ele, t_ms, segment_start: flags & FLAG_SEGMENT_START != 0 }
}

/// Records read per [`ByteSource`] call — one SD read fills a block rather than a record,
/// keeping the one-shot Finish conversion fast on the device.
const BLOCK_RECORDS: usize = 64;

/// Convert a recorded `.obct` log (`src`, a flat array of [`TRACK_RECORD_LEN`]-byte records)
/// into a GPX 1.1 track written to `sink`, naming the track `name`.
///
/// One streaming pass: a fresh `<trkseg>` opens on each [`segment_start`](TrackPoint::segment_start)
/// (and on the first point), so pauses/gaps become honest segment breaks. `<time>` is
/// intentionally omitted until the device has a real clock. A trailing partial record (a
/// power-loss mid-write) is ignored — the log stays valid at any 16-byte boundary.
pub fn track_to_gpx(src: &dyn ByteSource, name: &str, sink: &mut dyn ByteSink) -> Result<(), Error> {
    let mut line: String<160> = String::new();

    put(sink, b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
    put(sink, b"<gpx version=\"1.1\" creator=\"obcm\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n")?;
    put(sink, b"<trk><name>")?;
    write_escaped(sink, name)?;
    put(sink, b"</name>\n")?;

    let total = (src.len() as usize) / TRACK_RECORD_LEN;
    let mut buf = [0u8; BLOCK_RECORDS * TRACK_RECORD_LEN];
    let mut seg_open = false;
    let mut done = 0usize;

    while done < total {
        let n = (total - done).min(BLOCK_RECORDS);
        let bytes = &mut buf[..n * TRACK_RECORD_LEN];
        src.read_at((done * TRACK_RECORD_LEN) as u32, bytes)?;
        for i in 0..n {
            let mut rec = [0u8; TRACK_RECORD_LEN];
            rec.copy_from_slice(&bytes[i * TRACK_RECORD_LEN..(i + 1) * TRACK_RECORD_LEN]);
            let p = decode_record(&rec);

            if p.segment_start || (done == 0 && i == 0) {
                if seg_open {
                    put(sink, b"</trkseg>\n")?;
                }
                put(sink, b"<trkseg>\n")?;
                seg_open = true;
            }

            line.clear();
            let _ = line.push_str("<trkpt lat=\"");
            write_deg(&mut line, p.lat);
            let _ = line.push_str("\" lon=\"");
            write_deg(&mut line, p.lon);
            let _ = writeln!(line, "\"><ele>{}</ele></trkpt>", p.ele);
            put(sink, line.as_bytes())?;
        }
        done += n;
    }

    if seg_open {
        put(sink, b"</trkseg>\n")?;
    }
    put(sink, b"</trk>\n</gpx>\n")?;
    Ok(())
}

/// Append raw bytes to the sink.
fn put(sink: &mut dyn ByteSink, b: &[u8]) -> Result<(), Error> {
    sink.write(b)
}

/// Write a microdegree coordinate as a fixed 6-decimal degree string (exact integer math,
/// no float formatting / rounding drift): e.g. `-7654321` → `-7.654321`.
fn write_deg<const N: usize>(s: &mut String<N>, ud: i32) {
    if ud < 0 {
        let _ = s.push('-');
    }
    let a = ud.unsigned_abs();
    let _ = write!(s, "{}.{:06}", a / 1_000_000, a % 1_000_000);
}

/// Append `text` to the sink with the minimal XML escaping a track name may need.
fn write_escaped(sink: &mut dyn ByteSink, text: &str) -> Result<(), Error> {
    for ch in text.chars() {
        let esc: &[u8] = match ch {
            '&' => b"&amp;",
            '<' => b"&lt;",
            '>' => b"&gt;",
            _ => {
                let mut tmp = [0u8; 4];
                put(sink, ch.encode_utf8(&mut tmp).as_bytes())?;
                continue;
            }
        };
        put(sink, esc)?;
    }
    Ok(())
}
