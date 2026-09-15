use super::*;
use obc_ports::Fix;
use obcm_testkit::scratch::scratch_dir;
use std::path::PathBuf;

const MAP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../obc-sim/assets/grimsel-demo.obcm");
const OBCR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
const GPX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx");

/// A fresh scratch card with the demo map imported into it, and the directory it lives in.
fn card(tag: &str) -> (PathBuf, PathBuf) {
    let directory = scratch_dir("obc-ios-host", tag);
    let card = directory.join("card.obc");
    import_map(&card, Path::new(MAP)).expect("the demo map imports into a fresh card");
    (card, directory)
}

fn open(card: &Path, directory: &Path) -> Box<Host> {
    Host::open(card, &directory.join("settings"), &directory.join("exports")).expect("the card opens")
}

/// A fix in the middle of the demo map's extract.
const GRIMSEL: Fix = Fix { lat: 46_560_000, lon: 8_340_000, course: Some(90.0), speed_mps: Some(4.0) };

/// The card is the whole device's memory: what [`import_map`] creates, [`Host::open`] mounts, and a
/// second open finds the same map revision and the same route ids behind. [`card_state`] is what
/// the shell asks before any of it.
#[test]
fn an_imported_card_boots_on_the_map_and_reopens_with_the_same_map_and_routes() {
    let directory = scratch_dir("obc-ios-host", "card");
    let card = directory.join("card.obc");
    assert_eq!(card_state(&card), Ok(CardState::Missing), "nothing is on the phone yet");
    import_map(&card, Path::new(MAP)).expect("the demo map imports into a fresh card");
    assert_eq!(card_state(&card), Ok(CardState::Ready));
    // A card is not a map: one created without an import has nothing to show.
    let empty = directory.join("empty.obc");
    drop(HostStore::create_file(&empty).expect("a bare card is created"));
    assert_eq!(card_state(&empty), Ok(CardState::NoMap));

    let mut host = open(&card, &directory);
    assert!(!host.ready);
    assert!(host.tick(0.0), "the first tick always renders");
    assert_eq!(host.screen(), "Map");
    assert_eq!(host.frame().len(), (FRAME_W * FRAME_H * 4) as usize);
    assert!(host.frame().iter().skip(3).step_by(4).all(|&a| a == 0xFF), "opaque alpha for the CGImage");

    // The render gate closes: a parked device stops redrawing, and the shell blits on that signal.
    let idle: Vec<bool> = (1..=64).map(|frame| host.tick(f64::from(frame) * 16.0)).collect();
    assert!(idle[32..].iter().all(|changed| !changed), "an idle host stops reporting frame changes");

    let id = host.import_route(Path::new(OBCR)).expect("the authored route imports");
    assert_eq!(host.routes.ids(), &[id]);
    // The catalog re-reads the card by itself, a pass or two after the import.
    for frame in 65..81 {
        host.tick(f64::from(frame) * 16.0);
        if host.app.route_ids().contains(&id) {
            break;
        }
    }
    assert!(host.app.route_ids().contains(&id), "the catalog re-reads the card within 16 ticks");

    let identity = {
        let source = host.map.source();
        (source.store_id(), source.id(), source.revision())
    };
    drop(host);
    let host = open(&card, &directory);
    let source = host.map.source();
    assert_eq!((source.store_id(), source.id(), source.revision()), identity, "the same map, not a re-import");
    assert_eq!(host.routes.ids(), &[id], "and the same route ids");
    drop(source);
    drop(host);
    std::fs::remove_dir_all(directory).unwrap();
}

/// The fresh-sample contract: a pushed value is `Some` exactly once, and a fix reaches the app.
#[test]
fn each_pushed_sensor_value_polls_once_and_a_fix_moves_the_app() {
    let mut sensors = PhoneSensors::default();
    sensors.push_fix(GRIMSEL, Some(1_700_000_123));
    sensors.push_heading(370.0);
    sensors.push_altitude(2_005.0);
    sensors.push_battery(140);
    {
        let mut ports = sensors.ports();
        assert_eq!(ports.loc.poll(), Some(GRIMSEL));
        let stamp = ports.clock.as_mut().unwrap().poll().expect("the fix stamps the wall clock");
        assert_eq!(stamp.second, 23);
        assert_eq!(stamp.utc.to_unix(), 1_700_000_100, "minute resolution, seconds beside it");
        assert_eq!(ports.compass.as_mut().unwrap().poll(), Some(10.0), "a heading wraps into 0..360");
        assert_eq!(ports.altimeter.as_mut().unwrap().poll(), Some(2_005.0));
        assert_eq!(ports.fuel.as_mut().unwrap().poll(), Some(100), "charge cannot exceed a full battery");
    }
    {
        let mut ports = sensors.ports();
        assert_eq!(ports.loc.poll(), None, "a taken sample is not fresh again");
        assert_eq!(ports.clock.as_mut().unwrap().poll(), None);
        assert_eq!(ports.compass.as_mut().unwrap().poll(), None);
        assert_eq!(ports.altimeter.as_mut().unwrap().poll(), None);
        assert_eq!(ports.fuel.as_mut().unwrap().poll(), None);
    }
    // CoreLocation reports an unusable heading as a negative value; it is no direction at all.
    sensors.push_heading(-1.0);
    sensors.push_heading(f32::NAN);
    assert_eq!(sensors.ports().compass.as_mut().unwrap().poll(), None, "an invalid heading is dropped");

    let (card, directory) = card("sensors");
    let mut host = open(&card, &directory);
    host.tick(0.0);
    assert!(host.app.state.user_fix.is_none(), "the device has no position until the phone gives one");
    host.sensors().push_fix(GRIMSEL, Some(1_700_000_123));
    host.tick(16.0);
    assert_eq!(host.app.state.user_fix.map(|fix| (fix.lat, fix.lon)), Some((GRIMSEL.lat, GRIMSEL.lon)));
    drop(host);
    std::fs::remove_dir_all(directory).unwrap();
}

/// The button edges go through the app's own recognizer, so the phone's Select is the device's: a
/// release inside the tap window is a `Press`, and a held Select fires `Hold` from plain ticks.
#[test]
fn a_select_tap_presses_and_a_held_select_holds() {
    let (card, directory) = card("input");
    let mut host = open(&card, &directory);
    host.tick(0.0);
    assert_eq!(host.screen(), "Map");

    host.push_button(Button::Select, true);
    host.tick(16.0);
    host.push_button(Button::Select, false);
    host.tick(100.0);
    assert_eq!(host.screen(), "RideStart", "a release inside the tap window is a Press");
    host.push_button(Button::Back, true);
    host.tick(1_000.0);
    host.push_button(Button::Back, false);
    host.tick(1_100.0);
    assert_eq!(host.screen(), "Map");

    // Select-hold on the Map enters Pan mode; the stack does not move, so the pan is the evidence.
    assert!(host.app.state.pan.is_none());
    host.push_button(Button::Select, true);
    host.tick(1_500.0);
    host.tick(1_900.0);
    assert!(host.app.state.pan.is_none(), "before the threshold a held Select has fired nothing");
    host.tick(2_100.0);
    assert!(host.app.state.pan.is_some(), "a held Select holds on a tick that carries no edge");
    drop(host);
    std::fs::remove_dir_all(directory).unwrap();
}

/// A GPX route converts through the one shared conversion, attributed against the card's map.
#[test]
fn a_gpx_route_imports_as_the_shared_conversion_attributed_to_the_card_map() {
    use obc_formats::io::ByteSource;
    let (card, directory) = card("gpx");
    let mut host = open(&card, &directory);
    let id = host.import_route(Path::new(GPX)).expect("the GPX converts and imports");
    let expected = convert_gpx(Path::new(GPX), Some((&host.map.reader(), host.attribution_key())))
        .expect("the same conversion outside the host")
        .0;
    let source = host.routes.source(id).unwrap();
    let mut stored = vec![0; source.len() as usize];
    source.read_at(0, &mut stored).unwrap();
    assert_eq!(stored, expected);
    assert_ne!(expected, convert_gpx(Path::new(GPX), None).unwrap().0, "the map's attribution is in the bytes");
    drop(source);
    drop(host);
    std::fs::remove_dir_all(directory).unwrap();
}
