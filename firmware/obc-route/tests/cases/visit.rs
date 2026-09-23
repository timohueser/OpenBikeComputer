use crate::common::{build_obcr, ChunkIn, RouteSpec, VecSink, WpRec};
use obc_formats::io::SliceSource;
use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
use obc_formats::obcr::RouteSourceKey;
use obc_route::visit::{visit_anchor, VisitBuilder, VisitChoice, VisitCosts, VisitTarget};
use obc_route::{for_each_waypoint, BikeType, RouteIndex, RouteReader};

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
    let mut original = route(vec![(0, 0, 10), (2000, 0, 30)], &wps, 222);
    original[obc_formats::obcr::BIKE_TYPE_OFF] = BikeType::Mtb as u8;
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
    assert_eq!(index.bike_type(), BikeType::Mtb, "the visit keeps the original route's type");
    let visit = RouteReader::new(&index, &emitted).visit_descriptor().unwrap().unwrap();
    assert_eq!(visit.accepted_anchors_m, [0, 111, 222]);
    let reader = RouteReader::new(&index, &emitted);
    let preview = reader.assistant_preview_polyline::<64>().unwrap();
    assert_eq!(preview.first(), Some(&(0, 0)));
    assert!(preview.iter().any(|&(lon, lat)| lon == 0 && lat == 1000), "the stop remains visible");
    assert!(preview.iter().all(|&(lon, _)| lon == 0), "the unrelated original tail is outside the preview");
    assert!(preview.last().unwrap().1.abs() < 10, "the preview ends at rejoin");
    assert_eq!(reader.preview_polyline::<64>().last(), Some(&(2000, 0)), "ordinary overview keeps the tail");
    assert!(reader.assistant_preview_polyline::<0>().unwrap().is_empty());
    assert_eq!(reader.assistant_preview_polyline::<1>().unwrap().as_slice(), &[(0, 0)]);
    let original_preview =
        RouteReader::new(&RouteIndex::read(&source).unwrap(), &source).assistant_preview_polyline::<64>().unwrap();
    assert_eq!(original_preview.as_slice(), &[(0, 0), (2000, 0)], "non-Visit review keeps the full shape");
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
    let costs = VisitCosts::read(&emitted, [0, 111], None).unwrap();
    assert_eq!(costs.arrival_ascent_m, 20);
    assert!(costs.arrival_elevation_complete && costs.complete_elevation);
    let mut corrupt = sink.buf.clone();
    corrupt[40..44].copy_from_slice(&999u32.to_le_bytes());
    assert!(VisitCosts::read(&SliceSource(&corrupt), [0, 111], None).is_err());
}
#[test]
fn near_place_anchor_keeps_occurrence_prefix_waypoints_and_two_search_limit() {
    let wps: Vec<WpRec<'_>> =
        [10, 50, 150, 300, 500].into_iter().map(|at| (at, 0, 0, 0, 1, 1, 0, b"same" as &[u8])).collect();
    let bytes = route(vec![(0, 0, 0), (2000, 0, 0), (0, 0, 0), (0, 2000, 0)], &wps, 666);
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    let original = RouteReader::new(&index, &source);
    let target = (1000, 100);
    let anchor = visit_anchor(&original, 20, target).unwrap();
    assert!((110..=112).contains(&anchor), "first matching forward occurrence");
    assert!((332..=335).contains(&visit_anchor(&original, 250, target).unwrap()), "never return to the past crossing");
    let at = original.position_at(anchor).unwrap();
    let outbound = route(vec![(at.lon, at.lat, 0), (target.0, target.1, 0)], &[], 11);
    let returning = route(vec![(target.0, target.1, 0), (at.lon, at.lat, 0)], &[], 11);
    let mut builder = VisitBuilder::new(key(2), key(3), 20, anchor, SourceId::osm(1, 99), target).unwrap();
    builder.keep_prefix(anchor).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    while !builder.append_prefix_step(&original, &mut sink).unwrap() {}
    append(&mut builder, &outbound, &mut sink);
    append(&mut builder, &returning, &mut sink);
    while builder.finish_step(&original, &mut sink).unwrap().is_none() {}
    let source = SliceSource(&sink.buf);
    let index = RouteIndex::read(&source).unwrap();
    let derived = RouteReader::new(&index, &source);
    let descriptor = derived.visit_descriptor().unwrap().unwrap();
    assert_eq!(descriptor.original_anchors_m, [20, anchor, anchor]);
    assert_eq!(descriptor.accepted_anchors_m[0], 0);
    assert!((100..=104).contains(&descriptor.accepted_anchors_m[1]), "arrival includes the original prefix");
    assert!((666..=670).contains(&derived.total_distance_m), "retain both original loops and add only the excursion");
    let mut seen = 0;
    for_each_waypoint(&source, |w| {
        assert_eq!(w.provenance.unwrap().source, key(2));
        assert_eq!(w.provenance.unwrap().ordinal, seen + 1);
        if seen == 0 {
            assert_eq!(w.dist_along_m, 30);
        } else {
            assert!((wps[seen as usize + 1].0 + 1..=wps[seen as usize + 1].0 + 4).contains(&w.dist_along_m));
        }
        seen += 1;
    })
    .unwrap();
    assert_eq!(seen, 4);
    let mut choice = VisitChoice::new();
    choice.search().unwrap();
    choice.search().unwrap();
    assert!(choice.search().is_err());
}
#[test]
fn preview_keeps_a_short_excursion_after_a_dense_prefix() {
    let original = route((0..=200).map(|i| (i * 100, 0, 0)).collect(), &[], 2223);
    let source = SliceSource(&original);
    let index = RouteIndex::read(&source).unwrap();
    let reader = RouteReader::new(&index, &source);
    let anchor = 1500;
    let at = reader.position_at(anchor).unwrap();
    let stop = (at.lon, 10);
    let outbound = route(vec![(at.lon, at.lat, 0), (stop.0, stop.1, 0)], &[], 1);
    let returning = route(vec![(stop.0, stop.1, 0), (at.lon, at.lat, 0)], &[], 1);
    let mut builder = VisitBuilder::new(key(2), key(3), 0, anchor, SourceId::osm(1, 99), stop).unwrap();
    builder.keep_prefix(anchor).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    while !builder.append_prefix_step(&reader, &mut sink).unwrap() {}
    append(&mut builder, &outbound, &mut sink);
    append(&mut builder, &returning, &mut sink);
    while builder.finish_step(&reader, &mut sink).unwrap().is_none() {}
    let source = SliceSource(&sink.buf);
    let index = RouteIndex::read(&source).unwrap();
    let reader = RouteReader::new(&index, &source);
    let anchors = reader.visit_descriptor().unwrap().unwrap().accepted_anchors_m;
    let target = reader.position_at(anchors[1]).unwrap();
    let rejoin = reader.position_at(anchors[2]).unwrap();
    let preview = reader.assistant_preview_polyline::<64>().unwrap();
    assert!(preview.len() <= 64);
    assert_eq!(preview.first(), Some(&(0, 0)));
    assert!(preview.contains(&(target.lon, target.lat)), "sampling must retain the stop occurrence");
    assert_eq!(preview.last(), Some(&(rejoin.lon, rejoin.lat)));
    let three = reader.assistant_preview_polyline::<3>().unwrap();
    assert_eq!(three.as_slice(), &[(0, 0), (target.lon, target.lat), (rejoin.lon, rejoin.lat)]);
}

#[test]
fn on_route_stop_needs_no_artificial_excursion() {
    let bytes = route(vec![(0, 0, 0), (2000, 0, 0)], &[], 222);
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    let original = RouteReader::new(&index, &source);
    for anchor in [0, 100] {
        let at = original.position_at(anchor).unwrap();
        let zero = route(vec![(at.lon, at.lat, 0)], &[], 0);
        let mut builder = VisitBuilder::new(key(2), key(3), 0, anchor, SourceId::osm(1, 99), (at.lon, at.lat)).unwrap();
        builder.keep_prefix(anchor).unwrap();
        let mut sink = VecSink::default();
        builder.begin(&mut sink).unwrap();
        while !builder.append_prefix_step(&original, &mut sink).unwrap() {}
        append(&mut builder, &zero, &mut sink);
        append(&mut builder, &zero, &mut sink);
        while builder.finish_step(&original, &mut sink).unwrap().is_none() {}
        let source = SliceSource(&sink.buf);
        let info = obc_route::RouteObjectInfo::read(&source).unwrap();
        assert!((221..=223).contains(&info.distance_m));
        let anchors = info.visit.unwrap().accepted_anchors_m;
        assert_eq!(anchors[0], 0);
        assert_eq!(anchors[1], anchors[2]);
        assert!((anchor.saturating_sub(1)..=anchor).contains(&anchors[1]));
    }
}
#[test]
fn coordinate_destinations_use_normal_snap_but_mapped_approaches_remain_exact() {
    let mut target = VisitTarget {
        map: key(1),
        display: (500, 500),
        metadata: PoiMetadata { source: SourceId::osm(1, 2), approach: None },
    };
    assert_eq!(target.approach(key(1), BikeType::Road), Some(target.display));
    assert!(target.approach(key(2), BikeType::Road).is_none());
    let snapped = route(vec![(0, 0, 0), (0, 500, 0)], &[], 55);
    assert_eq!(target.destination(&SliceSource(&snapped), BikeType::Road).unwrap(), (0, 500));
    let distant = route(vec![(0, 0, 0), (0, 2000, 0)], &[], 222);
    assert!(target.validate_destination(&SliceSource(&distant), BikeType::Road).is_err());
    target.metadata.approach = Some(PoiApproach { source: SourceId::osm(1, 3), lon: 0, lat: 0, profile_mask: 1 });
    assert_eq!(target.approach(key(1), BikeType::Road), Some((0, 0)));
    assert!(target.approach(key(2), BikeType::Road).is_none());
    assert!(target.approach(key(1), BikeType::Gravel).is_none());
    let nearby = route(vec![(0, 1000, 0), (100, 0, 0)], &[], 111);
    assert!(target.validate_destination(&SliceSource(&nearby), BikeType::Road).is_err());
    let exact = route(vec![(0, 1000, 0), (0, 0, 0)], &[], 111);
    assert!(target.validate_destination(&SliceSource(&exact), BikeType::Road).is_ok());
}
#[test]
fn coordinate_visit_records_the_real_stop_and_keeps_its_return_connected() {
    let target = VisitTarget {
        map: key(2),
        display: (500, 1000),
        metadata: PoiMetadata { source: SourceId::osm(1, 9), approach: None },
    };
    let outbound = route(vec![(0, 0, 0), (0, 1000, 0)], &[], 111);
    let returning = route(vec![(0, 1000, 0), (0, 0, 0)], &[], 111);
    let original = route(vec![(0, 0, 0), (2000, 0, 0)], &[], 222);
    let mut builder = VisitBuilder::new(key(1), target.map, 0, 0, target.metadata.source, target.display).unwrap();
    let mut sink = VecSink::default();
    builder.begin(&mut sink).unwrap();
    let wrong = VisitTarget { metadata: PoiMetadata { source: SourceId::osm(1, 10), approach: None }, ..target };
    assert!(builder.resolve_destination(wrong, &SliceSource(&outbound), BikeType::Road).is_err());
    builder.resolve_destination(target, &SliceSource(&outbound), BikeType::Road).unwrap();
    assert_eq!(builder.destination(), Some((0, 1000)));
    append(&mut builder, &outbound, &mut sink);
    append(&mut builder, &returning, &mut sink);
    let source = SliceSource(&original);
    let index = RouteIndex::read(&source).unwrap();
    while builder.finish_step(&RouteReader::new(&index, &source), &mut sink).unwrap().is_none() {}
    let source = SliceSource(&sink.buf);
    let index = RouteIndex::read(&source).unwrap();
    let descriptor = RouteReader::new(&index, &source).visit_descriptor().unwrap().unwrap();
    assert_eq!((descriptor.target_lon, descriptor.target_lat), (0, 1000));
    assert_eq!(descriptor.target_id, 9);
    assert_eq!(descriptor.accepted_anchors_m, [0, 111, 222]);
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
    for (head, accepted) in [((8_337_021, 46_576_670), true), ((8_335_008, 46_576_670), false)] {
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
        let costs = VisitCosts::read(&emitted, [0, 111], None).unwrap();
        assert!(costs.arrival_elevation_complete);
        assert!(!costs.complete_elevation);
    }
}

#[test]
fn imported_route_connections_are_retained_measured_and_bounded() {
    let bytes = route(vec![(0, 0, 10), (3000, 0, 10)], &[], 333);
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    let original = RouteReader::new(&index, &source);
    for anchor in [0, 111] {
        let at = original.position_at(anchor).unwrap();
        for gap in [20, 1000] {
            let graph = (at.lon, at.lat + gap);
            let stop = (at.lon, at.lat + 2000);
            let outbound = route(vec![(graph.0, graph.1, 10), (stop.0, stop.1, 10)], &[], 222);
            let returning = route(vec![(stop.0, stop.1, 10), (graph.0, graph.1, 10)], &[], 222);
            let mut builder = VisitBuilder::new(key(1), key(2), 0, anchor, SourceId::osm(1, 9), stop).unwrap();
            builder.keep_prefix(anchor).unwrap();
            let mut sink = VecSink::default();
            builder.begin(&mut sink).unwrap();
            while !builder.append_prefix_step(&original, &mut sink).unwrap() {}
            if gap == 1000 {
                let source = SliceSource(&outbound);
                let index = RouteIndex::read(&source).unwrap();
                assert!(builder.append_leg_step(&RouteReader::new(&index, &source), &mut sink).is_err());
                assert!(builder.rejected_geometry());
                continue;
            }
            append(&mut builder, &outbound, &mut sink);
            append(&mut builder, &returning, &mut sink);
            while builder.finish_step(&original, &mut sink).unwrap().is_none() {}
            let source = SliceSource(&sink.buf);
            let index = RouteIndex::read(&source).unwrap();
            let composed = RouteReader::new(&index, &source);
            let shape = composed.preview_polyline::<16>();
            assert!(shape.windows(2).any(|p| p == [(at.lon, at.lat), graph]));
            assert!(shape.windows(2).any(|p| p == [graph, (at.lon, at.lat)]));
            let descriptor = composed.visit_descriptor().unwrap().unwrap();
            assert_eq!(descriptor.original_anchors_m, [0, anchor, anchor]);
            assert!((anchor + 443..=anchor + 445).contains(&descriptor.accepted_anchors_m[2]));
            assert!((777..=779).contains(&composed.total_distance_m), "composed {} m", composed.total_distance_m);
            let costs = VisitCosts::read(&source, [0, descriptor.accepted_anchors_m[1]], None).unwrap();
            assert!(!costs.arrival_elevation_complete && !costs.complete_elevation);
        }
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
