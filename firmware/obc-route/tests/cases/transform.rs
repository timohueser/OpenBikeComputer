//! Real chunk boundaries and I/O budgets for the paced route transforms.

use crate::common::{build_obcr, convert, ChunkIn, RouteSpec, VecSink};
use core::cell::{Cell, RefCell};
use obc_formats::io::{ByteSink, ByteSource, Error, SliceSource};
use obc_route::{Leg, RouteIndex, RouteReader, SpliceStep, Splicer, TrimOutcome, TrimStep, Trimmer};

type Point = (i32, i32, i16);

fn encoded(points: &[Point], seam: usize, waypoints: &[crate::common::WpRec<'_>]) -> Vec<u8> {
    let distance = |points: &[Point]| -> u32 {
        points.windows(2).map(|p| obc_map_scene::ground_dist_m((p[0].0, p[0].1), (p[1].0, p[1].1)) as f64).sum::<f64>()
            as u32
    };
    let chunks = [
        ChunkIn { points: points[..=seam].to_vec(), cum_distance_m: 0, cum_ascent_m: 0 },
        ChunkIn { points: points[seam..].to_vec(), cum_distance_m: distance(&points[..=seam]), cum_ascent_m: 0 },
    ];
    build_obcr(&RouteSpec {
        name: "Paced",
        chunks: &chunks,
        totals: (distance(points), 0, 0),
        seam_shared: true,
        waypoints: Some(waypoints),
        ..RouteSpec::default()
    })
    .0
}

fn gpx(points: &[Point], elevation: bool) -> Vec<u8> {
    let mut gpx = String::from("<gpx><trk><trkseg>");
    for &(x, y, z) in points {
        gpx.push_str(&format!("<trkpt lon=\"{:.6}\" lat=\"{:.6}\">", x as f64 / 1e6, y as f64 / 1e6));
        if elevation {
            gpx.push_str(&format!("<ele>{z}</ele>"));
        }
        gpx.push_str("</trkpt>");
    }
    gpx.push_str("</trkseg></trk></gpx>");
    convert("Paced", &gpx)
}

struct Source<'a> {
    bytes: &'a [u8],
    reads: RefCell<Vec<(u64, usize)>>,
    calls: Cell<usize>,
    fail_at: Cell<Option<usize>>,
}
impl<'a> Source<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, reads: RefCell::new(Vec::new()), calls: Cell::new(0), fail_at: Cell::new(None) }
    }
}
impl ByteSource for Source<'_> {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.calls.set(self.calls.get() + 1);
        self.reads.borrow_mut().push((offset, out.len()));
        if self.fail_at.get() == Some(self.calls.get()) {
            return Err(Error::Io);
        }
        SliceSource(self.bytes).read_at(offset, out)
    }
}

#[derive(Default)]
struct Sink {
    inner: VecSink,
    bytes: usize,
    fail_write: bool,
    fail_patch: bool,
}
impl ByteSink for Sink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.bytes += bytes.len();
        if self.fail_write {
            return Err(Error::Io);
        }
        self.inner.write(bytes)
    }
    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
        self.bytes += bytes.len();
        if self.fail_patch {
            return Err(Error::Io);
        }
        self.inner.patch_at(offset, bytes)
    }
}

fn budget(orig: &Source, detour: &Source, sink: &mut Sink, trim: bool) {
    let reads: Vec<_> = orig.reads.take().into_iter().chain(detour.reads.take()).collect();
    if trim || reads.len() != 2 {
        assert!(reads.len() <= 1, "one chunk or waypoint per step: {reads:?}");
    } else {
        assert_eq!(
            reads,
            [(0, obc_formats::obcr::HEADER_FULL_LEN), (112, obc_formats::obcr::HEADER_FULL_LEN - 112)],
            "only the fixed waypoint header needs two reads"
        );
    }
    assert!(reads.iter().map(|(_, len)| len).sum::<usize>() <= 255 * obc_formats::obcr::POINT_RECORD_LEN);
    assert!(sink.bytes <= 16 * 1024, "one board output stage, including final index: {}", sink.bytes);
    sink.bytes = 0;
}

fn trim(orig: &Source, detour: &Source, target: u32, elevation: bool, sink: &mut Sink) -> TrimStep {
    let oi = RouteIndex::read(&SliceSource(orig.bytes)).unwrap();
    let di = RouteIndex::read(&SliceSource(detour.bytes)).unwrap();
    let (o, d) = (RouteReader::new(&oi, orig), RouteReader::new(&di, detour));
    let mut state = Trimmer::new(Leg::Detour, target, elevation);
    for _ in 0..oi.chunks().len() + 2 * di.chunks().len() + 8 {
        let step = state.step(&o, &d, sink);
        budget(orig, detour, sink, true);
        if step != TrimStep::Running {
            assert_eq!(state.step(&o, &d, sink), step);
            assert!(orig.reads.borrow().is_empty() && detour.reads.borrow().is_empty());
            assert_eq!(sink.bytes, 0, "terminal steps do no I/O");
            return step;
        }
    }
    panic!("trim did not terminate within its chunk budget")
}

#[test]
fn trim_chunk_seams_match_kept_geometry_bytes_and_elevation_metrics() {
    let original = encoded(&[(0, 0, 0), (10_000, 0, 0), (20_000, 0, 0)], 1, &[]);
    let mut points: Vec<Point> = (0..254).map(|i| (i * 30, 1_000 + (i % 2) * 300, (i % 30) as i16)).collect();
    points.extend([(10_000, 0, 40), (10_300, 0, 45), (10_600, 0, 50)]);
    for seam in [253, 254, 255] {
        let detour = encoded(&points, seam, &[]);
        for elevation in [false, true] {
            let mut sink = Sink::default();
            let TrimStep::Done(Some(result)) =
                trim(&Source::new(&original), &Source::new(&detour), 500, elevation, &mut sink)
            else {
                panic!("sustained contact")
            };
            let expected = gpx(&points[..=254], true);
            assert_eq!(sink.inner.buf, expected, "contact before/on/after a source chunk seam");
            let idx = RouteIndex::read(&SliceSource(&expected)).unwrap();
            assert_eq!(
                result,
                TrimOutcome { rejoin_m: 1113, detour_len_m: idx.total_distance_m, ascent_m: idx.total_ascent_m }
            );
        }
    }
    eprintln!(
        "transform sizes: Trimmer={} Splicer={}",
        core::mem::size_of::<Trimmer>(),
        core::mem::size_of::<Splicer>()
    );
}

#[test]
fn final_contact_pair_and_empty_tail_do_not_write() {
    let original = encoded(&[(0, 0, 0), (10_000, 0, 0), (20_000, 0, 0)], 1, &[]);
    let points = [(0, 1000, 0), (9000, 1000, 0), (10_000, 0, 0), (10_200, 0, 0)];
    for seam in [1, 2] {
        let detour = encoded(&points, seam, &[]);
        for target in [1113, 2226, u32::MAX] {
            let mut sink = Sink::default();
            assert_eq!(
                trim(&Source::new(&original), &Source::new(&detour), target, false, &mut sink),
                TrimStep::Done(None)
            );
            assert!(sink.inner.buf.is_empty());
        }
    }
}

#[test]
fn trim_failures_are_terminal_and_a_fresh_attempt_can_retry() {
    let original = encoded(&[(0, 0, 0), (10_000, 0, 0), (20_000, 0, 0)], 1, &[]);
    let detour = gpx(&[(0, 1000, 0), (9000, 1000, 0), (10_000, 0, 0), (10_500, 0, 0), (11_000, 0, 0)], false);
    for fault in 0..5 {
        let (orig, det) = (Source::new(&original), Source::new(&detour));
        let mut sink = Sink::default();
        match fault {
            0 => orig.fail_at.set(Some(1)), // tail geometry
            1 => det.fail_at.set(Some(1)),  // contact scan
            2 => det.fail_at.set(Some(2)),  // re-emission after header write
            3 => sink.fail_write = true,
            _ => sink.fail_patch = true,
        }
        assert_eq!(trim(&orig, &det, 500, false, &mut sink), TrimStep::Failed(Error::Io));
        let mut retry = Sink::default();
        assert!(matches!(
            trim(&Source::new(&original), &Source::new(&detour), 500, false, &mut retry),
            TrimStep::Done(Some(_))
        ));
    }
}

#[test]
fn splice_paces_all_waypoints_and_refuses_source_failure_after_seams() {
    let points = [(0, 0, 0), (5000, 0, 10), (10_000, 0, 20), (15_000, 0, 30), (20_000, 0, 40)];
    let waypoints: Vec<_> = (0..64).map(|i| (i * 3, 0, 0, 0, 1, 1, 0, &b"W"[..])).collect();
    let original = encoded(&points, 2, &waypoints);
    let detour = gpx(&[(2000, 0, 0), (7000, 3000, 10), (12_000, 0, 20)], true);
    let oi = RouteIndex::read(&SliceSource(&original)).unwrap();
    let di = RouteIndex::read(&SliceSource(&detour)).unwrap();
    let (orig, det) = (Source::new(&original), Source::new(&detour));
    let (o, d) = (RouteReader::new(&oi, &orig), RouteReader::new(&di, &det));
    let mut state = Splicer::new(Leg::Detour, 250, 1300, di.total_distance_m, true, "Paced");
    let mut sink = Sink::default();
    let mut completed = None;
    for _ in 0..90 {
        let step = state.step(&o, &d, &mut sink);
        budget(&orig, &det, &mut sink, false);
        if let SpliceStep::Done(stats) = step {
            completed = Some(stats);
            break;
        }
        assert_eq!(step, SpliceStep::Running);
    }
    assert_eq!(
        completed.unwrap().waypoint_count,
        32,
        "all records are read but the existing retained waypoint cap stays fixed"
    );
    assert!(matches!(state.step(&o, &d, &mut sink), SpliceStep::Done(_)));
    assert_eq!(sink.bytes, 0);
    for fault in [3, 4, 70] {
        let source = Source::new(&original);
        source.fail_at.set(Some(fault));
        let orig = RouteReader::new(&oi, &source);
        let mut state = Splicer::new(Leg::Detour, 250, 1300, di.total_distance_m, true, "Paced");
        let mut sink = Sink::default();
        let failed = loop {
            match state.step(&orig, &d, &mut sink) {
                SpliceStep::Running => {}
                terminal => break terminal,
            }
        };
        assert_eq!(failed, SpliceStep::Failed(Error::Io), "read failure cannot remove route geometry");
    }
}

#[test]
fn splice_waypoints_follow_retained_geometry_and_end_at_measured_total() {
    let points: Vec<Point> = (0..401).map(|i| (8_123_457 + i * 10, 47_123_457 + (i % 2) * 5, 0)).collect();
    let distance = |end: usize| -> u32 {
        points[..=end]
            .windows(2)
            .map(|p| obc_map_scene::ground_dist_m((p[0].0, p[0].1), (p[1].0, p[1].1)) as f64)
            .sum::<f64>() as u32
    };
    let waypoints: Vec<_> =
        [40, 300, 400].into_iter().map(|i| (distance(i), points[i].0, points[i].1, 0, 1, 1, 0, &b"W"[..])).collect();
    let original = encoded(&points, 200, &waypoints);
    let detour_points: Vec<_> = (0..101).map(|i| (8_124_357 + i * 10, 47_123_657 + (i % 2) * 5, 0)).collect();
    let detour = encoded(&detour_points, 50, &[]);
    let (os, ds) = (SliceSource(&original), SliceSource(&detour));
    let (oi, di) = (RouteIndex::read(&os).unwrap(), RouteIndex::read(&ds).unwrap());
    let (o, d) = (RouteReader::new(&oi, &os), RouteReader::new(&di, &ds));
    let mut sink = VecSink::default();
    let result = obc_route::splice_detour(Leg::Detour, &o, &d, 100, 200, di.total_distance_m, true, &mut sink).unwrap();
    let output = crate::common::route_points(&sink.buf);
    let mut measured = 0.0;
    let mut distances = Vec::new();
    for (i, p) in output.iter().enumerate() {
        if i > 0 {
            let prev = output[i - 1];
            measured += obc_map_scene::ground_dist_m((prev.lon, prev.lat), (p.lon, p.lat)) as f64;
        }
        distances.push(measured as u32);
    }
    assert_eq!(result.total_distance_m, measured as u32);
    let mut actual = Vec::new();
    obc_route::reader::for_each_waypoint(&SliceSource(&sink.buf), |w| actual.push(w.clone())).unwrap();
    assert_eq!(actual.len(), 3);
    for waypoint in &actual[..2] {
        let at = output
            .iter()
            .position(|p| (p.lon, p.lat) == (waypoint.lon, waypoint.lat))
            .expect("original vertex retained");
        assert!(waypoint.dist_along_m.abs_diff(distances[at]) <= 1, "waypoint must use the stored metre axis");
    }
    assert_eq!(actual[2].dist_along_m, result.total_distance_m);
}

#[test]
fn incomplete_segment_seeks_and_splice_boundaries_keep_elevation_unknown() {
    let mut original = encoded(&[(0, 0, 10), (10_000, 0, 110), (20_000, 0, 210)], 1, &[]);
    let index = RouteIndex::read(&SliceSource(&original)).unwrap();
    original[index.chunks()[0].byte_offset as usize + 6] = 8;
    let os = SliceSource(&original);
    let oi = RouteIndex::read(&os).unwrap();
    let o = RouteReader::new(&oi, &os);
    assert_eq!(o.elevation_at(0), Some(10));
    assert_eq!(o.elevation_at(556), None, "valid endpoint samples do not fill an incomplete interior");
    assert_eq!(o.elevation_at(oi.chunks()[1].cum_distance_m), Some(110));
    let detour = gpx(&[(5_000, 0, 40), (7_500, 1_000, 50), (10_000, 0, 60)], true);
    let ds = SliceSource(&detour);
    let di = RouteIndex::read(&ds).unwrap();
    let d = RouteReader::new(&di, &ds);
    let mut sink = VecSink::default();
    obc_route::splice_detour(
        Leg::Detour,
        &o,
        &d,
        556,
        oi.chunks()[1].cum_distance_m,
        di.total_distance_m,
        true,
        &mut sink,
    )
    .unwrap();
    let output = crate::common::route_points(&sink.buf);
    assert_eq!(output[0].elevation(), Some(10));
    assert_eq!(output[1].elevation(), None, "the clipped splice seam has no measured height");
    let source = SliceSource(&sink.buf);
    let index = RouteIndex::read(&source).unwrap();
    let reader = RouteReader::new(&index, &source);
    assert_eq!(reader.elevation_at(250), None);
    let facts = reader.interval_facts(0, 500).unwrap();
    assert!(!facts.complete_elevation());
    assert_eq!(facts.ascent_m, 0);
}
