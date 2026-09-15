mod common;
use common::{build_obcr, ChunkIn, RouteSpec, VecSink, WpRec};
use obc_formats::io::SliceSource;
use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
use obc_formats::obcr::RouteSourceKey;
use obc_route::visit::{forward_rejoin, VisitBuilder, VisitChoice, VisitCosts, VisitTarget};
use obc_route::{for_each_waypoint, RouteIndex, RouteReader};

fn key(id: u64) -> RouteSourceKey {
    RouteSourceKey { store: [1; 16], object: id, revision: 1 }
}
fn route(points: Vec<(i32, i32, i16)>, waypoints: &[WpRec<'_>], distance: u32) -> Vec<u8> {
    build_obcr(&RouteSpec {
        chunks: &[ChunkIn { points, cum_distance_m: 0, cum_ascent_m: 0 }],
        waypoints: Some(waypoints),
        totals: (distance, 20, 20),
        ..RouteSpec::default()
    })
    .0
}
fn append(builder: &mut VisitBuilder, bytes: &[u8], sink: &mut VecSink) {
    let source = SliceSource(bytes);
    let index = RouteIndex::read(&source).unwrap();
    let reader = RouteReader::new(&index, &source);
    for _ in 0..100 {
        if builder.append_leg_step(&reader, sink).unwrap() {
            return;
        }
    }
    panic!("leg did not finish");
}
#[test]
fn composition_preserves_all_waypoints_and_measures_both_directions() {
    let wps: Vec<WpRec<'_>> =
        (0..48).map(|i| (10 + i * 4, 100 + i as i32 * 40, 100, 30, 1, 4, 11, b"same" as &[u8])).collect();
    let original = route(vec![(0, 0, 10), (2000, 0, 30)], &wps, 222);
    let outbound = route(vec![(0, 0, 10), (0, 1000, 30)], &[], 111);
    let returning = route(vec![(0, 1000, 30), (0, 0, 10)], &[], 111);
    let mut builder = VisitBuilder::new(key(2), key(3), 0, 0, SourceId::osm(1, 99), (0, 1000)).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    append(&mut builder, &outbound, &mut sink);
    append(&mut builder, &returning, &mut sink);
    let source = SliceSource(&original);
    let index = RouteIndex::read(&source).unwrap();
    let reader = RouteReader::new(&index, &source);
    let stats = loop {
        if let Some(stats) = builder.finish_step(&reader, &mut sink).unwrap() {
            break stats;
        }
    };
    assert_eq!(stats.waypoint_count, 48);
    assert!(stats.total_distance_m >= 443 && stats.total_distance_m <= 445);
    assert_eq!((stats.total_ascent_m, stats.total_descent_m), (40, 20));
    let emitted = SliceSource(&sink.buf);
    let index = RouteIndex::read(&emitted).unwrap();
    let visit = RouteReader::new(&index, &emitted).visit_descriptor().unwrap().unwrap();
    assert_eq!(visit.accepted_anchors_m, [0, 111, 222]);
    let mut seen = 0;
    for_each_waypoint(&emitted, |w| {
        assert_eq!(w.name.as_str(), "same");
        assert_eq!(w.provenance.unwrap().source, key(2));
        assert_eq!(w.provenance.unwrap().ordinal, seen);
        assert_eq!(w.lateral_offset_m, 11);
        assert_eq!(w.dist_along_m, 222 + 10 + u32::from(seen) * 4);
        seen += 1;
    })
    .unwrap();
    assert_eq!(seen, 48);
    let facts = RouteReader::new(&index, &emitted).interval_facts(0, stats.total_distance_m).unwrap();
    assert_eq!((facts.ascent_m, facts.descent_m), (stats.total_ascent_m, stats.total_descent_m));
    let costs = VisitCosts::read(&emitted, [0, 111]).unwrap();
    assert_eq!(costs.arrival_ascent_m, 20);
    assert!(costs.arrival_elevation_complete && costs.complete_elevation);
    let mut corrupt = sink.buf.clone();
    corrupt[40..44].copy_from_slice(&999u32.to_le_bytes());
    assert!(VisitCosts::read(&SliceSource(&corrupt), [0, 111]).is_err());
}
#[test]
fn forward_join_is_clipped_to_first_stored_access_and_searches_are_bounded() {
    let bytes = route(vec![(0, 0, 0), (30_000, 0, 0)], &[(300, 1000, 2000, 0, 1, 1, 200, b"A")], 3339);
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    assert_eq!(forward_rejoin(&RouteReader::new(&index, &source), 0).unwrap(), Some(300));
    let mut choice = VisitChoice::new(0, Some(300));
    choice.remember_out_and_back(4000);
    assert!(choice.prefer_forward(3999));
    assert!(!choice.prefer_forward(4000));
    for _ in 0..6 {
        choice.search().unwrap();
    }
    assert!(choice.search().is_err());
}
#[test]
fn missing_access_wrong_map_and_profile_are_unavailable() {
    let mut target = VisitTarget {
        map: key(1),
        display: (500, 500),
        metadata: PoiMetadata { source: SourceId::osm(1, 2), approach: None },
    };
    assert!(target.approach(key(1), 0).is_none());
    target.metadata.approach = Some(PoiApproach { source: SourceId::osm(1, 3), lon: 0, lat: 0, profile_mask: 1 });
    assert_eq!(target.approach(key(1), 0), Some((0, 0)));
    assert!(target.approach(key(2), 0).is_none());
    assert!(target.approach(key(1), 1).is_none());
}
#[test]
fn disconnected_return_is_not_joined_with_a_straight_segment() {
    let outbound = route(vec![(0, 0, 0), (0, 1000, 0)], &[], 111);
    let returning = route(vec![(1000, 1000, 0), (0, 0, 0)], &[], 157);
    let mut builder = VisitBuilder::new(key(1), key(2), 0, 0, SourceId::osm(1, 2), (0, 1000)).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    append(&mut builder, &outbound, &mut sink);
    let source = SliceSource(&returning);
    let index = RouteIndex::read(&source).unwrap();
    assert!(builder.append_leg_step(&RouteReader::new(&index, &source), &mut sink).is_err());
}

#[test]
fn cancellation_connector_keeps_tail_and_removes_visit() {
    let original = route(vec![(0, 0, 10), (2000, 0, 30)], &[(150, 1400, 100, 20, 1, 4, 11, b"Tail")], 222);
    let source = SliceSource(&original);
    let index = RouteIndex::read(&source).unwrap();
    let reader = RouteReader::new(&index, &source);
    let at = reader.position_at(111).unwrap();
    let connector = route(vec![(at.lon, 1000, 10), (at.lon, at.lat, 20)], &[], 111);
    let mut slot = Box::<VisitBuilder>::new_uninit();
    let mut builder = unsafe {
        VisitBuilder::init_return_in_place(slot.as_mut_ptr(), key(2), key(3), 111).unwrap();
        slot.assume_init()
    };
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    append(&mut builder, &connector, &mut sink);
    let stats = loop {
        if let Some(stats) = builder.finish_step(&reader, &mut sink).unwrap() {
            break stats;
        }
    };
    let emitted = SliceSource(&sink.buf);
    let info = obc_route::RouteObjectInfo::read(&emitted).unwrap();
    assert!(info.visit.is_none() && info.assistant_candidate);
    assert_eq!(stats.waypoint_count, 1);
    let index = RouteIndex::read(&emitted).unwrap();
    let reader = RouteReader::new(&index, &emitted);
    assert!(reader.total_distance_m >= 221 && reader.total_distance_m <= 223);
    for_each_waypoint(&emitted, |w| {
        assert_eq!(w.name.as_str(), "Tail");
        assert_eq!(w.provenance.unwrap().source, key(2));
        assert_eq!(w.lateral_offset_m, 11);
    })
    .unwrap();
}
