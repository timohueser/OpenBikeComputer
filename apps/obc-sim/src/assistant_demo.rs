//! Synthetic shop visits over the shipped Grimsel map, using real route objects and app screens.

use obc_app::{
    assistant_demo::{Fixture, Stop},
    App,
};
use obc_formats::io::SliceSource;
use obc_host_core::{FlatRouteStore, RouteRepository, VecSink};
use obc_replay::{Track, TrackPoint};
use obc_route::{gpx_to_obcr, RouteStats};
use std::fmt::Write;

const TRACK: &str = include_str!("../../../fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx");

fn route(
    store: &mut FlatRouteStore,
    name: &str,
    points: &[TrackPoint],
    waypoint: Option<(&str, TrackPoint)>,
) -> Result<(usize, RouteStats), String> {
    let mut xml = String::from("<gpx version=\"1.1\">");
    if let Some((label, p)) = waypoint {
        let _ = write!(
            xml,
            "<wpt lon=\"{}\" lat=\"{}\"><name>{label}</name></wpt>",
            p.lon as f64 / 1e6,
            p.lat as f64 / 1e6
        );
    }
    xml.push_str("<trk><trkseg>");
    for p in points {
        let _ = write!(
            xml,
            "<trkpt lon=\"{}\" lat=\"{}\"><ele>{}</ele></trkpt>",
            p.lon as f64 / 1e6,
            p.lat as f64 / 1e6,
            p.ele.unwrap_or(0.0)
        );
    }
    xml.push_str("</trkseg></trk></gpx>");
    let mut sink = VecSink::default();
    let stats = gpx_to_obcr(&SliceSource(xml.as_bytes()), name, &mut sink).map_err(|e| format!("demo route: {e:?}"))?;
    let id = store.import(sink.bytes()).map_err(|e| e.to_string())?;
    let index = store.ids().iter().position(|&candidate| candidate == id).ok_or("demo route missing after import")?;
    Ok((index, stats))
}

pub fn install(app: &mut App, store: &mut FlatRouteStore) -> Result<(), String> {
    let track = Track::parse(TRACK)?;
    let (original, _) = route(store, "Grimselpass", &track.points, None)?;
    let mut stops = Vec::new();
    for (name, join, dx, dy, climb) in [("Village shop", 300, 1_200, 600, 3.0), ("Farm shop", 39, -4_272, 3_278, 150.0)]
    {
        let anchor = track.points[join];
        let mid = TrackPoint {
            lon: anchor.lon + dx / 2,
            lat: anchor.lat + dy / 2,
            ele: anchor.ele.map(|e| e + climb * 0.7),
            t: 0.0,
        };
        let stop =
            TrackPoint { lon: anchor.lon + dx, lat: anchor.lat + dy, ele: anchor.ele.map(|e| e + climb), t: 0.0 };
        let mut outbound = track.points[..=join].to_vec();
        outbound.extend([mid, stop]);
        let (outbound_id, cost) = route(store, name, &outbound, Some((name, stop)))?;
        let mut returning = vec![stop, mid];
        returning.extend_from_slice(&track.points[join..]);
        let (continuation, _) = route(store, "Grimselpass", &returning, Some(("Rejoin route", anchor)))?;
        // Measure the authored spur with the same GPX conversion used for the actual route objects.
        let mut spur_sink = VecSink::default();
        let spur_xml = format!("<gpx><trk><trkseg><trkpt lon=\"{}\" lat=\"{}\"><ele>0</ele></trkpt><trkpt lon=\"{}\" lat=\"{}\"><ele>{climb}</ele></trkpt></trkseg></trk></gpx>", anchor.lon as f64 / 1e6, anchor.lat as f64 / 1e6, stop.lon as f64 / 1e6, stop.lat as f64 / 1e6);
        let spur = gpx_to_obcr(&SliceSource(spur_xml.as_bytes()), "Access", &mut spur_sink)
            .map_err(|e| format!("demo access: {e:?}"))?;
        let approach = Box::leak(Box::new([(anchor.lon, anchor.lat), (mid.lon, mid.lat), (stop.lon, stop.lat)]));
        stops.push(Stop {
            name,
            approach,
            distance_m: cost.total_distance_m,
            climb_m: cost.total_ascent_m,
            extra_m: spur.total_distance_m * 2,
            extra_climb_m: spur.total_ascent_m,
            return_m: spur.total_distance_m,
            return_climb_m: 0,
            outbound: outbound_id,
            continuation,
        });
    }
    let fixture = Box::leak(Box::new(Fixture {
        original,
        destination: "Grimselpass",
        start: (track.points[0].lon, track.points[0].lat),
        stops: stops.try_into().map_err(|_| "expected two demo stops")?,
    }));
    app.set_routes_with_ids(store.catalog(), store.ids());
    let mut settings = *app.settings();
    settings.climb_mode = obc_app::ClimbMode::Manual;
    app.set_settings(settings);
    app.enable_assistant_demo(fixture);
    Ok(())
}
