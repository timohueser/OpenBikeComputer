//! GPX **scanner** tests for the converter's front end ([`GpxScanner`]).
//!
//! These pin the streaming scanner's behaviour on *malformed* input. The scanner is deliberately
//! lenient — it powers the on-device GPX→OBCR conversion, where a single bad point in an
//! otherwise-good planner file shouldn't abort the whole route. See
//! [`scanner_skips_a_missing_coordinate`] for the divergence from `obc-replay`'s stricter
//! `Track::parse`.

use obc_formats::io::SliceSource;
use obc_route::{GpxScanner, RawPoint};

/// Collect every point the scanner yields from `gpx` (panicking on a read error, which an
/// in-memory `SliceSource` never returns).
fn scan(gpx: &str) -> Vec<RawPoint> {
    let src = SliceSource(gpx.as_bytes());
    let mut s = GpxScanner::new(&src);
    let mut out = Vec::new();
    while let Some(p) = s.next_point().unwrap() {
        out.push(p);
    }
    out
}

/// The obc-route scanner *skips* a `<trkpt>` missing a coordinate and reads on, rather than
/// erroring, so a lone bad point in a long planner GPX doesn't abort the conversion. Divergence:
/// `obc-replay`'s `Track::parse` errors on the same GPX.
#[test]
fn scanner_skips_a_missing_coordinate() {
    // First point has no lon; it's dropped, the valid second point is returned.
    let pts = scan(r#"<gpx><trkpt lat="48.0"/><trkpt lat="48.1" lon="7.9"/></gpx>"#);
    assert_eq!(pts.len(), 1, "the lon-less point is skipped, not errored");
    assert_eq!(pts[0], RawPoint { lon: 7_900_000, lat: 48_100_000, ele: None });

    // The skip also applies to a non-self-closing (body-bearing) bad point in the middle.
    let pts = scan(
        r#"<gpx><trkpt lat="48.0" lon="7.8"/><trkpt lat="48.1"><ele>5</ele></trkpt><trkpt lat="48.2" lon="7.9"/></gpx>"#,
    );
    assert_eq!(pts.len(), 2, "the middle lon-less point is skipped; the two valid ones survive");
    assert_eq!(pts[0].lon, 7_800_000);
    assert_eq!(pts[1].lon, 7_900_000);
}

/// An unterminated `<trkpt` opening tag (no `>` before end of source) ends the scan with
/// `Ok(None)` rather than erroring or looping — a truncated file stops at the last whole point.
#[test]
fn scanner_stops_on_unterminated_tag() {
    let src = SliceSource(br#"<gpx><trkpt lat="48.0" lon="7.8""#.as_slice());
    let mut s = GpxScanner::new(&src);
    assert_eq!(s.next_point(), Ok(None), "an unterminated opening tag ends the scan, not errors");
}
