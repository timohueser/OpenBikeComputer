# Captured route-import exports

Real GPX files from the route planners riders use. `firmware/obc-route/src/symbol.rs` maps a
planner's freeform `<sym>` or `<type>` text onto one of six POI categories. That table is a
curation read off real exports, not a standard, so it is held by real exports and not by GPX
written for a test.

Each file is the canonical copy. A consumer that cannot read from here keeps a duplicate, and a
test holds the two byte-identical.

| File | Provenance | Licence | Duplicate |
| --- | --- | --- | --- |
| `komoot-schwarzwald.gpx` | komoot export, `creator="https://www.komoot.de"`, "Schwarzwald Tour · Tag 2": 150 track points and five `<wpt>`. The symbols are Garmin names komoot copies through — `Flag, Blue` three times, `Restaurant`, and `Fishing Hot Spot Facility`, which no category fits. Who exported it is not recorded anywhere in the tree; it arrived as the iOS companion's import fixture. | ODbL-1.0 for the track, which is komoot's routing output over OSM ways. **The authored part — the chosen route and the five waypoint names — is unconfirmed**, pending the owner. | `companion-ios/Packages/OBCKit/Sources/OBCMock/Fixtures/sample-import.gpx` |

SwiftPM processes `Sources/OBCMock/Fixtures` as the `OBCMock` target's bundled resources, and a
resource must live inside its own target directory. The iOS copy therefore stays where it is.
