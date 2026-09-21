//! The captured planner exports, read by the converter that has to survive them.
//!
//! `symbol.rs` is a curation read off real exports, so every other converter test writes its own
//! GPX and can only prove the table agrees with itself. These read `fixtures/sources/route-import/`
//! instead: whatever a planner actually emits is what arrives here.

use obc_formats::io::SliceSource;
use obc_reader::PoiCategory;
use obc_route::{gpx_to_obcr, RouteIndex, RouteReader};

use crate::common::VecSink;

const KOMOOT: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/sources/route-import/komoot-schwarzwald.gpx"));

/// The `OBCMock` duplicate SwiftPM needs inside its own target directory.
const KOMOOT_SWIFT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../companion-ios/Packages/OBCKit/Sources/OBCMock/Fixtures/sample-import.gpx"
));

/// `<wpt>` elements in the source, counted off the raw bytes rather than restated.
fn wpt_elements(gpx: &[u8]) -> usize {
    gpx.windows(5).filter(|w| *w == b"<wpt ").count()
}

#[test]
fn the_canonical_komoot_export_and_its_swift_duplicate_are_the_same_bytes() {
    assert_eq!(KOMOOT, KOMOOT_SWIFT, "the duplicate has drifted from fixtures/sources/route-import/");
}

/// Every `<wpt>` the export carries reaches the file, under its own name and category, in ride
/// order. Komoot writes Garmin symbol names: `Restaurant` has an honest home among the six and
/// `Fishing Hot Spot Facility` has none, so it stays generic rather than being dropped or forced.
#[test]
fn the_komoot_export_stores_every_waypoint_with_its_own_category() {
    let mut sink = VecSink::default();
    let stats = gpx_to_obcr(&SliceSource(KOMOOT), "Schwarzwald", &mut sink).unwrap();
    assert_eq!(usize::from(stats.waypoint_count), wpt_elements(KOMOOT));

    let src = SliceSource(&sink.buf);
    let index = RouteIndex::read(&src).unwrap();
    let wpts = RouteReader::new(&index, &src).load_waypoints(0);
    assert!(!wpts.truncated);
    // Stored names are capped at 24 bytes on a character boundary, which four of these reach.
    let stored: Vec<(&str, Option<PoiCategory>)> =
        wpts.as_slice().iter().map(|w| (w.name.as_str(), w.category)).collect();
    assert_eq!(
        stored,
        [
            ("Steiler Abschnitt auf de", None),
            ("Freudenstädter Wasserfo", None),
            ("Feuerstelle Schmidsberge", None), // <sym>Fishing Hot Spot Facility</sym>
            ("Blick auf die Landschaft", None),
            ("Fuxxbau", Some(PoiCategory::Resupply)), // <sym>Restaurant</sym>
        ]
    );
}
