//! The captured Komoot export from import to the drawn waypoint chip.
//!
//! The waypoint tests elsewhere build their table by hand, so they can only prove the chip's own
//! rules. This one starts at `fixtures/sources/route-import/`: real planner bytes through
//! `gpx_to_obcr`, the resident table the Map draws from, and the names on the panel.

use embedded_graphics::pixelcolor::Rgb888;
use obc_formats::io::SliceSource;
use obc_ports::{Fix, RideClock, Sensors};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader};
use obc_render::text::{text_width, Font};
use obc_route::{RouteIndex, RouteReader, RouteSummary, MAX_POINTS_PER_CHUNK};

use crate::harness::support::{build_min_obcm, wpts_from_obcr, Buf, OnceFix, VecSink};
use crate::screen::map::chip_band_box;
use crate::screen::palette::PARCHMENT;
use crate::settings::WaypointMode;
use crate::{App, AppState, Settings};

const KOMOOT: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/sources/route-import/komoot-schwarzwald.gpx"));

const PANEL: (i32, i32) = (240, 320);

fn obcr(gpx: &[u8]) -> Vec<u8> {
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx), "Schwarzwald", &mut sink).unwrap();
    sink.0
}

/// Ride the converted export from end to end, one fix per stored route point, and keep the Map
/// frame of the first step at which the chip names each waypoint, in the order they are reached.
///
/// The fixes are the route's own points, so the rider never goes off route and the chip never has
/// to be coaxed into showing.
fn chip_per_waypoint(obcr: &[u8]) -> Vec<(String, Buf)> {
    let src = SliceSource(obcr);
    let index = RouteIndex::read(&src).expect("the converted route parses");
    let route = RouteReader::new(&index, &src);

    let start = Fix::at(route.start_lat, route.start_lon);
    let mut app = App::new(AppState::new(start.lon, start.lat, 1.0));
    // Always, so a frame does not also depend on how far ahead the next waypoint happens to sit.
    app.set_settings(Settings { waypoint_mode: WaypointMode::Always, ..Settings::default() });
    app.set_routes_with_ids(&[RouteSummary::read(&src).unwrap()], &[1]);
    app.activate_route(0);

    let map_bytes = build_min_obcm(0);
    let map_src = SliceSource(&map_bytes);
    let tables = MapTables::parse(&map_src).expect("valid obcm");
    let cache = MapCache::new();
    let reader = Reader::new(&map_src, &tables, &cache);

    let mut seen: Vec<(String, Buf)> = Vec::new();
    let mut at = 0u32;
    for chunk in 0..index.chunks().len() {
        let mut points = heapless::Vec::<_, MAX_POINTS_PER_CHUNK>::new();
        route.decode_chunk(chunk, &mut points).expect("the chunk decodes");
        for point in &points {
            at += 1;
            app.tick(
                RideClock(at * 1_000),
                Sensors::new(&mut OnceFix(Some(Fix::at(point.lat, point.lon)))),
                Some(&route),
            );
            let Some(k) = app.navigator.route_state().next_waypoint else { continue };
            if k != seen.len() {
                continue;
            }
            let name = app.navigator.waypoints().as_slice()[k].name.as_str().to_string();
            let mut frame = Buf::new(PANEL.0, PANEL.1);
            let mut scratch = Box::new(obc_render::RenderScratch::new());
            app.render_frame(
                Some(&mut scratch),
                &mut frame,
                &reader,
                Some(&route),
                PANEL.0 as f32,
                PANEL.1 as f32,
                |c| {
                    let (r, g, b) = rgb565_to_rgb888(c);
                    Rgb888::new(r, g, b)
                },
            );
            seen.push((name, frame));
        }
    }
    seen
}

/// The chip pill's drawn width: the widest unbroken run of pill fill in the Map's bottom band. The
/// band also holds the scale bar, which is narrower, and the run is measured inside the pill's ink
/// outline, so the same two pixels are missing from every frame.
fn pill_width(frame: &Buf) -> i32 {
    let (r, g, b) = rgb565_to_rgb888(PARCHMENT);
    let fill = Rgb888::new(r, g, b);
    let band = chip_band_box(PANEL.0, PANEL.1);
    let mut widest = 0;
    for y in band.top_left.y..band.top_left.y + band.size.height as i32 {
        let mut run = 0;
        for x in 0..PANEL.0 {
            run = if frame.get(x, y) == fill { run + 1 } else { 0 };
            widest = widest.max(run);
        }
    }
    assert!(widest > 0, "the band holds no pill");
    widest
}

/// Every `<wpt>` the export carries becomes the chip's name at its own point in the ride, under the
/// name the record stores — the 24-byte cap and all. The generic one, whose symbol the curation
/// misses, is named like the rest: an unmapped symbol costs a category, never the waypoint.
#[test]
fn every_komoot_waypoint_reaches_the_map_chip_under_its_own_name() {
    let bytes = obcr(KOMOOT);
    let drawn: Vec<String> = chip_per_waypoint(&bytes).into_iter().map(|(name, _)| name).collect();
    let stored: Vec<String> = wpts_from_obcr(&bytes).as_slice().iter().map(|w| w.name.to_string()).collect();

    assert_eq!(drawn, stored, "the chip draws the resident table, in ride order");
    assert_eq!(
        stored,
        [
            "Steiler Abschnitt auf de",
            "Freudenstädter Wasserfo",
            "Feuerstelle Schmidsberge",
            "Blick auf die Landschaft",
            "Fuxxbau",
        ]
    );
}

/// The pixels, not just the model. Everything in the pill but the name is fixed width, so riding
/// the same export with one name cut to a letter widens it by exactly that name's glyphs.
///
/// The last waypoint is the subject because its name fits the pill outright; the longer ones are
/// ellipsised to the chip's own budget, which `screen::map` measures.
#[test]
fn the_pill_is_drawn_to_the_width_of_the_real_name() {
    const LAST: usize = 4;
    let full = chip_per_waypoint(&obcr(KOMOOT));
    let stub = chip_per_waypoint(&obcr(&rename_waypoint(KOMOOT, LAST, "W")));
    let (name, frame) = &full[LAST];
    assert_eq!((name.as_str(), stub[LAST].0.as_str()), ("Fuxxbau", "W"));
    assert_eq!(
        pill_width(frame) - pill_width(&stub[LAST].1),
        text_width(name, Font::Body) as i32 - text_width("W", Font::Body) as i32,
    );
}

/// Replace the text of the `k`-th `<wpt>`'s `<name>`, leaving every other byte alone. The route's
/// own `<name>` is inside `<metadata>`, ahead of every `<wpt>`, so the search starts at the k-th one.
fn rename_waypoint(gpx: &[u8], k: usize, to: &str) -> Vec<u8> {
    let find = |from: usize, needle: &[u8]| {
        gpx[from..].windows(needle.len()).position(|w| w == needle).map(|at| from + at).expect("the element is present")
    };
    let mut wpt = 0;
    for _ in 0..=k {
        wpt = find(wpt, b"<wpt ") + 1;
    }
    let open = find(wpt, b"<name>") + b"<name>".len();
    let close = find(open, b"</name>");
    let mut out = gpx[..open].to_vec();
    out.extend_from_slice(to.as_bytes());
    out.extend_from_slice(&gpx[close..]);
    out
}
