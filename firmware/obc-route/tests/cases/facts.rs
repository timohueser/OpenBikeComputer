use crate::common::VecSink;
use obc_formats::{io::SliceSource, obcr::RouteSourceKey};
use obc_route::{gpx_to_obcr_attributed, RouteIndex, RouteReader};
const MAP: RouteSourceKey = RouteSourceKey { store: [1; 16], object: 2, revision: 3 };
fn route(elevations: &[Option<i16>]) -> Vec<u8> {
    let mut gpx = String::from("<gpx><trk><trkseg>");
    for (i, ele) in elevations.iter().enumerate() {
        gpx.push_str(&format!("<trkpt lat=\"0\" lon=\"{}\">", i as f64 * 0.0005));
        if let Some(ele) = ele {
            gpx.push_str(&format!("<ele>{ele}</ele>"));
        }
        gpx.push_str("</trkpt>");
    }
    gpx.push_str("</trkseg></trk></gpx>");
    let mut sink = VecSink::default();
    gpx_to_obcr_attributed(
        &SliceSource(gpx.as_bytes()),
        "Facts",
        obc_route::BikeType::Road,
        &mut sink,
        Some(MAP),
        |_, b| Ok(if b.0 < 1000 { 1 } else { 3 }),
    )
    .unwrap();
    sink.buf
}

#[test]
fn flat_missing_and_partial_remain_distinct_after_reload() {
    for (elevations, has_elevation, complete, ascent) in [
        (vec![Some(0); 5], true, true, 0),
        (vec![None; 5], false, false, 0),
        (vec![None, Some(0), Some(10), None, Some(1000), Some(1010), None], true, false, 20),
    ] {
        let bytes = route(&elevations);
        let src = SliceSource(&bytes);
        let idx = RouteIndex::read(&src).unwrap();
        let r = RouteReader::new(&idx, &src);
        let facts = r.interval_facts(0, r.total_distance_m).unwrap();
        assert_eq!(r.has_elevation(), has_elevation);
        assert_eq!(facts.complete_elevation(), complete);
        assert_eq!(facts.ascent_m, ascent);
        assert_eq!(facts.ascent_m, r.total_ascent_m);
        assert_eq!(facts.surface_m.iter().sum::<u32>(), facts.distance_m());
        assert!(facts.surface_current_for(MAP));
        assert!(!facts.surface_current_for(RouteSourceKey { revision: 4, ..MAP }));
        if !complete {
            let profile = r.elevation_profile();
            assert!(profile.cols().iter().any(|c| c.0 > c.1));
            assert!(r.detect_climbs().is_empty());
        }
    }
}

#[test]
fn clipping_conserves_integer_facts_across_chunks_and_surface_boundaries() {
    let elevations: Vec<_> = (0..600).map(|i| Some((i % 70) as i16)).collect();
    let bytes = route(&elevations);
    let src = SliceSource(&bytes);
    let idx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&idx, &src);
    assert!(r.chunks().len() > 1);
    let whole = r.interval_facts(0, r.total_distance_m).unwrap();
    for split in [1, 55, 110, 1777, r.chunks()[1].cum_distance_m, r.total_distance_m - 1] {
        let a = r.interval_facts(0, split).unwrap();
        let b = r.interval_facts(split, r.total_distance_m).unwrap();
        assert_eq!(a.distance_m() + b.distance_m(), whole.distance_m());
        assert_eq!(a.ascent_m + b.ascent_m, whole.ascent_m);
        assert_eq!(a.descent_m + b.descent_m, whole.descent_m);
        for i in 0..8 {
            assert_eq!(a.surface_m[i] + b.surface_m[i], whole.surface_m[i]);
        }
    }
    assert_eq!(whole.ascent_m, r.total_ascent_m);
    assert_eq!(whole.descent_m, r.total_descent_m);
    // A walk that stops after its farthest end gives each end the facts of a whole walk.
    let start = 55;
    let ends = [110, r.chunks()[1].cum_distance_m, start, 1777];
    let each = r.interval_facts_to::<4>(start, &ends).unwrap();
    for (facts, end) in each.iter().zip(ends) {
        assert_eq!(*facts, r.interval_facts(start, end).unwrap());
    }
}

#[test]
fn grades_use_ordered_endpoints_and_do_not_cross_missing_spans() {
    let bytes = route(&[Some(0), Some(10), None, Some(100), Some(90)]);
    let src = SliceSource(&bytes);
    let idx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&idx, &src);
    let mut samples = Vec::new();
    r.interval_facts_with_grades(0, r.total_distance_m, |s| samples.push(s)).unwrap();
    assert!(samples[0].grade_percent.unwrap() > 0.0);
    assert!(samples[1].grade_percent.is_none());
    assert!(samples[2].grade_percent.is_none());
    assert!(samples[3].grade_percent.unwrap() < 0.0);
    assert_eq!(std::mem::size_of::<obc_route::RoutePoint>(), 12);
}

#[test]
fn malformed_optional_metadata_and_zero_length_steps_are_explicit() {
    use obc_formats::obcr::{VisitDescriptor, WaypointProvenance};
    let provenance = WaypointProvenance { source: MAP, ordinal: 7 };
    assert_eq!(WaypointProvenance::decode(&provenance.encode()).unwrap(), Some(provenance));
    let mut absent = [0; 36];
    absent[0] = 1;
    assert!(WaypointProvenance::decode(&absent).is_err());
    let descriptor = VisitDescriptor {
        original: MAP,
        original_anchors_m: [0, 2, 3],
        accepted_anchors_m: [0, 2, 3],
        target_id: 123,
        target_lon: 0,
        target_lat: 0,
        target_kind: 1,
    };
    assert_eq!(VisitDescriptor::decode(&descriptor.encode().unwrap()).unwrap(), descriptor);
    let mut invalid = descriptor.encode().unwrap();
    invalid[72] = 5;
    assert!(VisitDescriptor::decode(&invalid).is_err());
    let mut bytes = route(&[Some(0), Some(10)]);
    bytes[118] = 1;
    assert!(RouteIndex::read(&SliceSource(&bytes)).is_err());

    let gpx = br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"><ele>0</ele></trkpt><trkpt lat="0" lon="0"><ele>100</ele></trkpt><trkpt lat="0" lon="0.0005"><ele>101</ele></trkpt><trkpt lat="0" lon="0.0005"><ele>200</ele></trkpt></trkseg></trk></gpx>"#;
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx), "Same location", &mut sink).unwrap();
    let src = SliceSource(&sink.buf);
    let index = RouteIndex::read(&src).unwrap();
    let route = RouteReader::new(&index, &src);
    assert_eq!(route.interval_facts(0, route.total_distance_m).unwrap().ascent_m, route.total_ascent_m);
}

#[test]
fn ahead_window_keeps_the_complete_crossing_climb_and_rejects_another_parse() {
    use obc_route::{
        window::{AheadRange, RouteWindow},
        ClimbSeg, Climbs,
    };
    let bytes = route(&vec![Some(0); 300]);
    let src = SliceSource(&bytes);
    let idx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&idx, &src);
    let mut climbs = Climbs::new();
    let climb = ClimbSeg { start_m: 2000, end_m: 9000, base_ele_m: 100, top_ele_m: 500, gain_m: 400, avg_grade_pct: 6 };
    climbs.0.push(climb).unwrap();
    let next = RouteWindow::new(&r, 1000, AheadRange::FiveKm);
    assert_eq!(next.end_m, 6000);
    assert_eq!(next.climb(&climbs), Some(&climb));
    let active = RouteWindow::new(&r, 4000, AheadRange::FiveKm);
    assert_eq!(active.climb(&climbs), Some(&climb));
    let past = RouteWindow::new(&r, 9000, AheadRange::FiveKm);
    assert!(past.climb(&climbs).is_none());
    let other = RouteIndex::read(&src).unwrap();
    let other = RouteReader::new(&other, &src);
    assert!(next.facts(&other).is_err());
}

#[test]
fn next_authored_waypoint_keeps_its_name_beyond_the_window() {
    use obc_route::window::{AheadRange, RouteWindow};
    let gpx=b"<gpx><wpt lat=\"0\" lon=\"0.07\"><name>Village bridge</name></wpt><trk><trkseg><trkpt lat=\"0\" lon=\"0\"><ele>0</ele></trkpt><trkpt lat=\"0\" lon=\"0.1\"><ele>0</ele></trkpt></trkseg></trk></gpx>";
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx), "Late waypoint", &mut sink).unwrap();
    let source = SliceSource(&sink.buf);
    let index = RouteIndex::read(&source).unwrap();
    let route = RouteReader::new(&index, &source);
    let window = RouteWindow::new(&route, 0, AheadRange::FiveKm);
    let next = window.next_waypoint(&route).unwrap().unwrap();
    assert!(!window.contains(next.dist_along_m));
    assert_eq!(next.name.as_str(), "Village bridge");
}
