//! GPX → OBCR conversion tests: convert an in-memory GPX, read it back with the
//! reader, and check the geometry round-trips and the stats are exact.

use obc_route::{gpx_to_obcr, ByteSink, Error, RouteIndex, RoutePoint, RouteReader, SliceSource, MAX_POINTS_PER_CHUNK};

/// A `ByteSink` over a growable `Vec` — the host's "write the whole file to RAM"
/// backing (the device uses a FatFs-backed sink instead).
#[derive(Default)]
struct VecSink {
    buf: Vec<u8>,
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.buf[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

fn convert(name: &str, gpx: &str) -> Vec<u8> {
    let src = SliceSource(gpx.as_bytes());
    let mut sink = VecSink::default();
    gpx_to_obcr(&src, name, &mut sink).unwrap();
    sink.buf
}

fn decode(r: &RouteReader, k: usize) -> Vec<RoutePoint> {
    let mut out = heapless::Vec::<_, MAX_POINTS_PER_CHUNK>::new();
    r.decode_chunk(k, &mut out).unwrap();
    out.to_vec()
}

/// A straight, gently rolling eastward track. The four points are collinear, so the
/// geometry decimates to its two endpoints — but the stats come from every raw point.
const STRAIGHT: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"><ele>200.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8030"><ele>210.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8060"><ele>225.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8090"><ele>215.0</ele></trkpt>
</trkseg></trk></gpx>"#;

#[test]
fn straight_track_stats_and_decimation() {
    let bytes = convert("Rhine Path", STRAIGHT);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.name(), "Rhine Path");
    assert_eq!(r.start_lon, 7_800_000);
    assert_eq!(r.start_lat, 48_000_000);

    // ~223 m per 0.003° lon step at 48° lat × 3 steps ≈ 670 m.
    assert!((665..=675).contains(&r.total_distance_m), "dist {}", r.total_distance_m);
    // Ascent: +10 then +15 (both over the 3 m dead-band); the −10 is descent.
    assert_eq!(r.total_ascent_m, 25);
    assert_eq!(r.total_descent_m, 10);
    assert_eq!(r.min_ele_m, 200);
    assert_eq!(r.max_ele_m, 225);

    // Collinear → decimated to the two endpoints, one chunk.
    assert_eq!(r.point_count, 2);
    assert_eq!(r.chunks().len(), 1);
    let pts = decode(&r, 0);
    assert_eq!(
        pts,
        vec![
            RoutePoint { lon: 7_800_000, lat: 48_000_000, ele: 200 },
            RoutePoint { lon: 7_809_000, lat: 48_000_000, ele: 215 },
        ]
    );
}

/// An L-shaped track: north then east. The decimator must keep the corner.
const CORNER: &str = r#"<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"/>
  <trkpt lat="48.0100" lon="7.8000"/>
  <trkpt lat="48.0100" lon="7.8100"/>
</trkseg></trk></gpx>"#;

#[test]
fn corner_is_preserved() {
    let bytes = convert("Jura Heights", CORNER);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    // The corner vertex is kept: all three points survive decimation.
    assert_eq!(r.point_count, 3);
    let pts = decode(&r, 0);
    assert_eq!(pts.len(), 3);
    assert_eq!(pts[1], RoutePoint { lon: 7_800_000, lat: 48_010_000, ele: 0 });
}

#[test]
fn empty_gpx_is_an_error() {
    let src = SliceSource(b"<gpx></gpx>".as_slice());
    let mut sink = VecSink::default();
    assert_eq!(gpx_to_obcr(&src, "x", &mut sink), Err(Error::Empty));
}
