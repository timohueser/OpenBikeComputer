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
    let nearby = route(vec![(0, 1000, 0), (100, 0, 0)], &[], 111);
    assert!(target.validate_destination(&SliceSource(&nearby), 0).is_err());
    let exact = route(vec![(0, 1000, 0), (0, 0, 0)], &[], 111);
    assert!(target.validate_destination(&SliceSource(&exact), 0).is_ok());
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
    assert!(builder.rejected_geometry());
    struct FailedSource;
    impl obc_formats::io::ByteSource for FailedSource {
        fn read_at(&self, _: u64, _: &mut [u8]) -> Result<(), obc_formats::io::Error> {
            Err(obc_formats::io::Error::Io)
        }
        fn len(&self) -> u64 {
            u64::MAX
        }
    }
    let mut builder = VisitBuilder::new(key(1), key(2), 0, 0, SourceId::osm(1, 2), (0, 1000)).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    assert_eq!(
        builder.append_leg_step(&RouteReader::new(&index, &FailedSource), &mut sink),
        Err(obc_formats::io::Error::Io)
    );
    assert!(!builder.rejected_geometry());
}

#[test]
fn quantized_return_seam_is_coalesced_but_a_disconnected_tail_is_rejected() {
    let end = (8_337_028, 46_576_671);
    let stop = (end.0, end.1 + 1000);
    let outbound = route(vec![(end.0, end.1, 10), (stop.0, stop.1, 10)], &[], 111);
    let returning = route(vec![(stop.0, stop.1, 10), (end.0, end.1, 10)], &[], 111);
    for (head, accepted) in [((8_337_021, 46_576_670), true), ((8_337_008, 46_576_670), false)] {
        let original = route(vec![(head.0, head.1, 10), (head.0 + 2000, head.1, 10)], &[], 153);
        let source = SliceSource(&original);
        let index = RouteIndex::read(&source).unwrap();
        let reader = RouteReader::new(&index, &source);
        let mut builder = VisitBuilder::new(key(1), key(2), 0, 0, SourceId::osm(1, 2), stop).unwrap();
        let mut sink = VecSink::default();
        builder.begin(&mut sink).unwrap();
        append(&mut builder, &outbound, &mut sink);
        append(&mut builder, &returning, &mut sink);
        let first = builder.finish_step(&reader, &mut sink);
        if !accepted {
            assert_eq!(first, Err(obc_formats::io::Error::BadOffset));
            assert!(builder.rejected_geometry());
            continue;
        }
        assert!(first.unwrap().is_none());
        while builder.finish_step(&reader, &mut sink).unwrap().is_none() {}
        let emitted = SliceSource(&sink.buf);
        let index = RouteIndex::read(&emitted).unwrap();
        let reader = RouteReader::new(&index, &emitted);
        let points = reader.preview_polyline::<8>();
        assert_eq!(points.as_slice(), &[end, stop, end, (head.0 + 2000, head.1)]);
        let visit = reader.visit_descriptor().unwrap().unwrap();
        assert_eq!(visit.original_anchors_m, [0; 3]);
        assert_eq!(visit.accepted_anchors_m, [0, 111, 222]);
        let costs = VisitCosts::read(&emitted, [0, 111]).unwrap();
        assert!(costs.arrival_elevation_complete);
        assert!(!costs.complete_elevation);
    }
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

#[test]
fn easier_composition_preserves_access_order_full_metadata_and_clean_provenance() {
    let wps = [
        (55, 450, 500, 17, 4, 4, -50, b"Cafe" as &[u8]),
        (55, 451, 501, 18, 2, 4, 50, b"Camp" as &[u8]),
        (166, 1400, 400, 19, 1, 4, 40, b"Shop" as &[u8]),
    ];
    let bytes = route(vec![(0, 0, 10), (2000, 0, 30)], &wps, 222);
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    let original = RouteReader::new(&index, &source);
    let mut slot = Box::<VisitBuilder>::new_uninit();
    let mut builder = unsafe {
        VisitBuilder::init_easier_in_place(slot.as_mut_ptr(), key(1), key(2), 0).unwrap();
        slot.assume_init()
    };
    builder.prepare_easier(&original).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    let mut targets = vec![];
    while let Some((from, to)) = builder.easier_leg(&original, (0, 0)).unwrap() {
        targets.push(to);
        // A bent candidate leg still ends at the exact on-route access point, never the annotation.
        let leg = route(vec![(from.0, from.1, 10), ((from.0 + to.0) / 2, 200, 20), (to.0, to.1, 30)], &[], 100);
        append(&mut builder, &leg, &mut sink);
        builder.finish_easier_leg(&original).unwrap();
    }
    assert_eq!(targets.len(), 3);
    assert!(targets.iter().all(|p| p.1 == 0));
    let stats = loop {
        if let Some(stats) = builder.finish_step(&original, &mut sink).unwrap() {
            break stats;
        }
    };
    assert_eq!(stats.waypoint_count, 3);
    let out = SliceSource(&sink.buf);
    let info = obc_route::RouteObjectInfo::read(&out).unwrap();
    assert!(info.assistant_candidate && !info.unresolved_avoidance && info.visit.is_none());
    let mut seen = vec![];
    for_each_waypoint(&out, |w| seen.push(w.clone())).unwrap();
    for (i, w) in seen.iter().enumerate() {
        assert_eq!(
            (w.lon, w.lat, w.ele, w.category_id, w.lateral_offset_m),
            (wps[i].1, wps[i].2, wps[i].3, wps[i].4, wps[i].6)
        );
        assert_eq!(w.name.as_bytes(), wps[i].7);
        assert_eq!(w.provenance.unwrap().ordinal, i as u16);
        assert_eq!(w.provenance.unwrap().source, key(1));
    }
    assert_eq!(seen[0].dist_along_m, seen[1].dist_along_m);
    assert!(seen[2].dist_along_m > seen[1].dist_along_m);
    let index = RouteIndex::read(&out).unwrap();
    let accepted = RouteReader::new(&index, &out);
    builder.prepare_easier(&accepted).unwrap(); // A clean accepted route permits another comparison.
}

#[test]
fn easier_refuses_constraint_overflow_and_preserves_later_loop_anchors() {
    let wps: Vec<_> = (0..33).map(|i| (20 + i, 100, 100, 10, 1, 1, 10, b"A" as &[u8])).collect();
    let bytes = route(vec![(0, 0, 10), (2000, 0, 30)], &wps, 222);
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    let original = RouteReader::new(&index, &source);
    let mut slot = Box::<VisitBuilder>::new_uninit();
    let mut builder = unsafe {
        VisitBuilder::init_easier_in_place(slot.as_mut_ptr(), key(1), key(2), 0).unwrap();
        slot.assume_init()
    };
    assert_eq!(builder.prepare_easier(&original), Err(obc_formats::io::Error::TooLarge));
    let bytes = route(
        vec![(0, 0, 10), (1000, 0, 10), (0, 0, 10), (2000, 0, 10)],
        &[(111, 1000, 100, 10, 1, 1, 10, b"A"), (222, 0, 100, 10, 1, 1, 10, b"B")],
        444,
    );
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    let original = RouteReader::new(&index, &source);
    builder.prepare_easier(&original).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    let mut targets = vec![];
    while let Some((from, to)) = builder.easier_leg(&original, (0, 0)).unwrap() {
        targets.push(to);
        append(&mut builder, &route(vec![(from.0, from.1, 10), (to.0, to.1, 10)], &[], 111), &mut sink);
        builder.finish_easier_leg(&original).unwrap();
    }
    assert_eq!(targets.len(), 3);
    assert!(targets[1].0 < targets[0].0 && targets[2].0 > targets[0].0);
}
