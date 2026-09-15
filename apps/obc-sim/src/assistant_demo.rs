//! Portable, synthetic place visits using real route objects and app screens.

use obc_app::{
    assistant_demo::{Fixture, Landmark, Stage, Stop},
    App,
};
use obc_formats::io::SliceSource;
use obc_host_core::{FlatRouteStore, RouteRepository, VecSink};
use obc_replay::{Track, TrackPoint};
use obc_route::{gpx_to_obcr, RouteStats};
use std::fmt::Write;

mod easier;
mod landmark_samples;

// Adapted from Wikipedia; article revisions and CC BY-SA 4.0 attribution are in
// docs/assets/ride-assistant/landmarks-study/README.md. Positions and access routes are synthetic.
static AARE: Landmark = Landmark {
    kind: "Gorge",
    article: "https://en.wikipedia.org/wiki/Aare_Gorge",
    photo: Some(&obc_app::assistant_demo::photos::AARE),
    pages: &[
        "The Aare cuts a narrow passage through limestone near Meiringen. In places, the rock walls stand about 50 metres high.",
        "Glacial meltwater carved the gorge. Paths and walkways have let visitors explore it since 1889.",
    ],
};
static FALLS: Landmark = Landmark {
    kind: "Waterfall",
    article: "https://en.wikipedia.org/wiki/Reichenbach_Falls",
    photo: Some(&obc_app::assistant_demo::photos::FALLS),
    pages: &[
        "These waterfalls tumble down a hillside near Meiringen. The highest single drop is about 110 metres.",
        "Conan Doyle set the fictional clash of Holmes and Moriarty here in his 1893 story The Final Problem.",
    ],
};
static GELMER: Landmark = Landmark {
    kind: "Funicular",
    article: "https://en.wikipedia.org/wiki/Gelmer_Funicular",
    photo: None,
    pages: &[
        "A cable railway from Handegg to the Gelmersee reservoir. The steepest section has a gradient of 106 percent.",
        "Built in 1926 to carry materials for reservoir construction, the railway opened to the public in 2001.",
    ],
};

static DUNLOUGH: Landmark = Landmark {
    kind: "Castle",
    article: "https://en.wikipedia.org/wiki/Dunlough_Castle",
    photo: Some(&obc_app::assistant_demo::photos::DUNLOUGH),
    pages: &[
        "Three ruined towers stand between a lake and the Atlantic cliffs at Three Castle Head in County Cork.",
        "A defensive wall links the towers. The castle was founded by Donagh O'Mahony in 1207.",
    ],
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scenario {
    #[default]
    Four,
    TwoAlong,
    UsefulDetour,
    WorseDetour,
    FourAlong,
    DetoursOnly,
    One,
    Empty,
}

impl Scenario {
    pub const ALL: [(Self, &'static str); 8] = [
        (Self::Four, "four"),
        (Self::TwoAlong, "two-along"),
        (Self::UsefulDetour, "useful-detour"),
        (Self::WorseDetour, "worse-detour"),
        (Self::FourAlong, "four-along"),
        (Self::DetoursOnly, "detours-only"),
        (Self::One, "one"),
        (Self::Empty, "empty"),
    ];

    pub fn candidates(self) -> &'static [usize] {
        match self {
            Self::Four => &[0, 1, 2, 3],
            Self::TwoAlong => &[0, 2],
            Self::UsefulDetour => &[0, 1, 2],
            Self::WorseDetour => &[0, 2, 4],
            Self::FourAlong => &[0, 2, 3, 5],
            Self::DetoursOnly => &[1, 4],
            Self::One => &[0],
            Self::Empty => &[],
        }
    }

    pub fn key(self) -> &'static str {
        Self::ALL.iter().find(|(s, _)| *s == self).unwrap().1
    }
}

#[derive(Clone, Debug)]
pub struct Seed {
    pub route: Option<String>,
    pub stage: Stage,
    pub scenario: Scenario,
    pub landmarks: Option<String>,
    pub option: usize,
}

impl Default for Seed {
    fn default() -> Self {
        Self { route: None, stage: Stage::Map, scenario: Scenario::default(), landmarks: None, option: 0 }
    }
}

fn local_track(min: (i32, i32), max: (i32, i32), center: (i32, i32)) -> Vec<TrackPoint> {
    // Leave room for shop access paths. Small maps scale down the whole study.
    let half_y = 27_000.min((center.1 - min.1).min(max.1 - center.1) * 3 / 4);
    let half_x = 4_000.min((center.0 - min.0).min(max.0 - center.0) / 4);
    (0..129)
        .map(|i| {
            let t = i as f64 / 128.0;
            TrackPoint {
                lon: center.0 + (half_x as f64 * (t * 6.0).sin()) as i32,
                lat: center.1 - half_y + (2.0 * half_y as f64 * t) as i32,
                ele: Some((400.0 + t * 180.0) as f32),
                t,
            }
        })
        .collect()
}

fn lengths(points: &[TrackPoint]) -> Vec<f64> {
    let mut result = vec![0.0];
    for pair in points.windows(2) {
        let aspect = ((pair[0].lat as f64 + pair[1].lat as f64) / 2e6).to_radians().cos();
        let dx = (pair[1].lon as f64 - pair[0].lon as f64) * aspect;
        let dy = pair[1].lat as f64 - pair[0].lat as f64;
        result.push(result.last().unwrap() + dx.hypot(dy) * 0.111_195);
    }
    result
}

fn access(anchor: TrackPoint, min: (i32, i32), max: (i32, i32), metres: f64, climb: f32) -> [TrackPoint; 3] {
    let aspect = (anchor.lat as f64 / 1e6).to_radians().cos().max(0.01);
    let east = (max.0 as f64 - anchor.lon as f64) * aspect;
    let west = (anchor.lon as f64 - min.0 as f64) * aspect;
    let north = max.1 as f64 - anchor.lat as f64;
    let south = anchor.lat as f64 - min.1 as f64;
    let (dx, dy, room) = if east.max(west) >= north.max(south) * 0.2 {
        (if east >= west { 1.0 / aspect } else { -1.0 / aspect }, 0.0, east.max(west))
    } else {
        (0.0, if north >= south { 1.0 } else { -1.0 }, north.max(south))
    };
    let offset = (metres / 0.111_195).min(room * 0.8);
    let point = |fraction: f64| TrackPoint {
        lon: anchor.lon + (dx * offset * fraction) as i32,
        lat: anchor.lat + (dy * offset * fraction) as i32,
        ele: Some(anchor.ele.unwrap_or(0.0) + climb * fraction as f32),
        t: 0.0,
    };
    [anchor, point(0.5), point(1.0)]
}

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

pub fn install(
    app: &mut App,
    store: &mut FlatRouteStore,
    tables: &obc_reader::MapTables,
    args: &crate::Args,
) -> Result<(), String> {
    let seed = &args.assistant;
    let bbox = tables.bbox;
    let min = (bbox.min_lon, bbox.min_lat);
    let max = (bbox.max_lon, bbox.max_lat);
    let center = args.center.unwrap_or((min.0 + (max.0 - min.0) / 2, min.1 + (max.1 - min.1) / 2));
    let contains = |(x, y)| x >= min.0 && x <= max.0 && y >= min.1 && y <= max.1;
    if !contains(center) {
        return Err("--center must be inside the loaded map".into());
    }
    let points = if let Some(path) = &seed.route {
        Track::load(std::path::Path::new(path))?.points
    } else {
        local_track(min, max, center)
    };
    if points.len() < 2 || !points.iter().all(|p| contains((p.lon, p.lat))) {
        return Err("the study GPX must have at least two points, all inside the loaded map".into());
    }
    let cumulative = lengths(&points);
    let length = *cumulative.last().unwrap();
    if length < 100.0 {
        return Err("the study needs at least 100 m of route; choose a larger area or move --center inward".into());
    }
    if store.ids().len() + 24 > obc_app::MAX_ROUTES {
        return Err("not enough route slots for the study".into());
    }
    let (original, _) = route(store, "Study route", &points, None)?;
    let mut stops = Vec::new();
    let mut places = vec![
        ("Village shop", 0.30, 2_000.0, 110.0, 3.0, None),
        ("Farm shop", 0.03, 300.0, 490.0, 150.0, None),
        ("Supermarket", 0.50, 3_500.0, 50.0, 2.0, None),
        ("General store", 0.68, 4_500.0, 130.0, 2.0, None),
        ("Ridge shop", 0.80, 5_500.0, 700.0, 100.0, None),
        ("Bakery", 0.16, 1_000.0, 40.0, 2.0, None),
        ("Aare Gorge", 0.05, 400.0, 250.0, 12.0, Some(&AARE)),
        ("Reichenbach Falls", 0.12, 900.0, 400.0, 50.0, Some(&FALLS)),
        ("Gelmerbahn", 0.25, 1_800.0, 350.0, 25.0, Some(&GELMER)),
        ("Dunlough Castle", 0.35, 2_400.0, 450.0, 30.0, Some(&DUNLOUGH)),
    ];
    if let Some(key) = seed.landmarks.as_deref() {
        let samples = match key {
            "glaciers" => &landmark_samples::GLACIERS,
            "passes" => &landmark_samples::PASSES,
            _ => return Err("unknown landmark sample set".into()),
        };
        places.truncate(6);
        places.extend(samples.iter().enumerate().map(|(i, &(name, landmark))| {
            (name, 0.05 + i as f64 * 0.1, 400.0 + i as f64 * 800.0, 250.0, 12.0, Some(landmark))
        }));
    }
    for (name, fraction, target_m, spur_m, climb, landmark) in places {
        let distance = (length * fraction).min(target_m);
        let join = cumulative.partition_point(|&d| d < distance).min(points.len() - 2).max(1);
        let approach = access(points[join], min, max, spur_m, climb);
        let [anchor, mid, stop] = approach;
        let mut outbound = points[..=join].to_vec();
        outbound.extend([mid, stop]);
        let (outbound_id, cost) = route(store, name, &outbound, Some((name, stop)))?;
        let mut returning = vec![stop, mid];
        returning.extend_from_slice(&points[join..]);
        let (continuation, _) = route(store, "Study route", &returning, Some(("Rejoin route", anchor)))?;
        // The displayed access costs use the same converter as the prepared route legs.
        let mut spur_sink = VecSink::default();
        let spur_xml = format!("<gpx><trk><trkseg><trkpt lon=\"{}\" lat=\"{}\"><ele>0</ele></trkpt><trkpt lon=\"{}\" lat=\"{}\"><ele>{climb}</ele></trkpt></trkseg></trk></gpx>", anchor.lon as f64 / 1e6, anchor.lat as f64 / 1e6, stop.lon as f64 / 1e6, stop.lat as f64 / 1e6);
        let spur = gpx_to_obcr(&SliceSource(spur_xml.as_bytes()), "Access", &mut spur_sink)
            .map_err(|e| format!("demo access: {e:?}"))?;
        stops.push(Stop {
            open_now: None,
            landmark,
            name,
            approach: Box::leak(Box::new(approach.map(|p| (p.lon, p.lat)))),
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
    let alternatives = easier::install(store, original, &points, min, max)?;
    let fixture = Box::leak(Box::new(Fixture {
        original,
        start: (points[0].lon, points[0].lat),
        stops: Box::leak(stops.into_boxed_slice()),
    }));
    app.set_routes_with_ids(store.catalog(), store.ids());
    let mut settings = *app.settings();
    settings.climb_mode = obc_app::ClimbMode::Manual;
    app.set_settings(settings);
    app.enable_assistant_demo(fixture);
    app.state.assistant_demo.as_mut().unwrap().easier = Some(alternatives);
    app.set_assistant_demo_candidates(seed.scenario.candidates());
    if !app.show_assistant_demo(seed.stage, seed.option) {
        return Err("that stage or option is unavailable in this candidate set".into());
    }
    if let Some(demo) = app.state.assistant_demo {
        eprintln!(
            "assistant candidates: {} available, {} suggested",
            seed.scenario.candidates().len(),
            demo.candidates.len
        );
        for (i, stop) in demo.stops().enumerate() {
            eprintln!(
                "  {} {}: {} m, {} m climb, {} m extra",
                i + 1,
                stop.name,
                stop.distance_m,
                stop.climb_m,
                stop.extra_m
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_routes_and_access_stay_inside_different_map_areas() {
        for (min, max) in [
            ((7_000_000, 45_000_000), (7_200_000, 45_200_000)),
            ((-73_001_000, -40_005_000), (-72_999_000, -39_995_000)),
        ] {
            let center = (min.0 + (max.0 - min.0) / 2, min.1 + (max.1 - min.1) / 2);
            let points = local_track(min, max, center);
            assert!(lengths(&points).last().unwrap() > &100.0);
            for anchor in points {
                for p in access(anchor, min, max, 700.0, 100.0) {
                    assert!((min.0..=max.0).contains(&p.lon));
                    assert!((min.1..=max.1).contains(&p.lat));
                }
            }
        }
    }
}
