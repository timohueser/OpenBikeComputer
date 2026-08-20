//! Recorded-track format: the fixed-record ride log + its GPX export (`no_std`).
//!
//! While riding, the device appends one [`TrackPoint`](obc_ports::TrackPoint) per accepted GPS fix
//! as a fixed 20-byte record. Finalize appends the ride-v3 footer; it does not convert or rewrite
//! the samples. [`track_to_gpx`] accepts only a finished ride-v3 object and excludes the footer
//! from its streamed point walk; the retired headerless/partial-log path is not a compatibility
//! input.
//!
//! This is deliberately *not* the [`OBCR`](crate) route format: a route is decimated for
//! compact drawing, whereas a recorded track wants full GPS fidelity. The log keeps every
//! accepted point verbatim; only the on-screen breadcrumb (host-side, in RAM) is decimated.

use core::fmt::Write;

use heapless::String;
use obc_formats::io::{ByteSink, ByteSource, Error};
use obc_formats::track::{decode_record, RECORD_LEN as TRACK_RECORD_LEN};

/// Records read per [`ByteSource`] call, amortizing storage reads during a requested GPX export.
const BLOCK_RECORDS: usize = 64;

/// Convert a finished ride-v3 object into GPX 1.1.
///
/// One streaming pass: a fresh `<trkseg>` opens on each
/// [`segment_start`](obc_ports::TrackPoint::segment_start)
/// (and on the first point), so pauses/gaps become honest segment breaks. `<time>` is
/// intentionally omitted until the device has a real clock.
pub fn track_to_gpx(src: &dyn ByteSource, name: &str, sink: &mut dyn ByteSink) -> Result<(), Error> {
    // Widest point line = `<trkpt>` + negative lat/lon + `<ele>-32768</ele>` + the full sensor
    // extensions block (`gpxtpx:TrackPointExtension` hr+cad, a bare `<power>`) ≈ 224 chars. Sized
    // to 320 so that line — and a future `<time>` element — can never truncate (a clipped GPX line
    // is silent corruption).
    let mut line: String<320> = String::new();

    put(sink, b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
    put(
        sink,
        b"<gpx version=\"1.1\" creator=\"OpenBikeComputer\" xmlns=\"http://www.topografix.com/GPX/1/1\" xmlns:gpxtpx=\"http://www.garmin.com/xmlschemas/TrackPointExtension/v1\">\n",
    )?;
    put(sink, b"<trk><name>")?;
    write_escaped(sink, name)?;
    put(sink, b"</name>\n")?;

    let total = sample_count(src)?;
    let mut buf = [0u8; BLOCK_RECORDS * TRACK_RECORD_LEN];
    let mut seg_open = false;
    let mut done = 0usize;

    while done < total {
        let n = (total - done).min(BLOCK_RECORDS);
        let bytes = &mut buf[..n * TRACK_RECORD_LEN];
        src.read_at((done * TRACK_RECORD_LEN) as u64, bytes)?;
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
            let _ = write!(line, "\"><ele>{}</ele>", p.ele);
            // Sensor extensions (epic #707): `gpxtpx:hr`/`gpxtpx:cad` inside a TrackPointExtension
            // wrapper, plus a bare `<power>` (the de-facto Strava form). Each element is omitted
            // when its field is absent; the whole `<extensions>` block when all three are.
            if p.hr.is_some() || p.cadence.is_some() || p.power.is_some() {
                let _ = line.push_str("<extensions>");
                if p.hr.is_some() || p.cadence.is_some() {
                    let _ = line.push_str("<gpxtpx:TrackPointExtension>");
                    if let Some(hr) = p.hr {
                        let _ = write!(line, "<gpxtpx:hr>{hr}</gpxtpx:hr>");
                    }
                    if let Some(cad) = p.cadence {
                        let _ = write!(line, "<gpxtpx:cad>{cad}</gpxtpx:cad>");
                    }
                    let _ = line.push_str("</gpxtpx:TrackPointExtension>");
                }
                if let Some(power) = p.power {
                    let _ = write!(line, "<power>{power}</power>");
                }
                let _ = line.push_str("</extensions>");
            }
            let _ = line.push_str("</trkpt>\n");
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

/// Count only sample records after validating the one supported finished format.
fn sample_count(src: &dyn ByteSource) -> Result<usize, Error> {
    usize::try_from(crate::RideInfo::read(src)?.point_count).map_err(|_| Error::TooLarge)
}

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
