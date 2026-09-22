# Ride Assistant source captures

The real inputs behind the Ride Assistant scenarios. Scenario inputs live in the immutable fixture
store; country-scale raw landmark captures stay in a local map-baker source cache, which
[CAPTURE.md](CAPTURE.md) covers.

## Acquire and verify

Run from a clean worktree. Needs Python 3.11 or newer, `osmium-tool`, and the normal map build
dependencies.

```sh
tools/obc fixtures sync assistant-inputs
tools/obc fixtures verify assistant-inputs
python3 fixtures/verify-assistant-inputs.py
```

Verification checks archive and member hashes, the tracked manifest copies, the raw source hashes,
all 11 recorded OSM identities, and the four Wiki identities and article revisions. A changed
source needs a new immutable package. No acquisition command runs during map generation.

`fixtures/verify-assistant-places.py` runs the `fixtures.assistant-places` suite against the
pinned Swiss PBF; see [place query validation](PLACES-VALIDATION.md).

## Build the maps offline

Build the shipping tools once while Cargo dependencies are available, then work from cached
inputs:

```sh
cargo build --locked --release -p obc-bake -p obc-dem -p obcm-assemble
tools/obc fixtures sync assistant-inputs peak-articles
python3 fixtures/build-assistant-package.py west-cork
python3 fixtures/build-assistant-package.py meiringen
```

The recipe verifies the region's input packages first. Cork runs the offline landmark compiler;
Switzerland loads its compiled input package. Both then run `obc-bake` with the normal cut stage,
a native `obc-dem bake`, and `obcm-assemble`. Assembly uses the normal catalog selection, verifies
every cell hash, and accepts partial cells at the authored crop boundary only — never a missing
cell, and never a skipped final verification.

Outputs go to `fixtures/build/maps`; `OBC_FIXTURE_BUILD_DIR` selects another directory.
**An existing work or package directory is refused.** `--bin-dir` uses existing shipping binaries,
in which case the recorded `recipe_commit` says nothing about their source revision. No Cargo
build and no external request runs implicitly.

`--landmarks PATH/content.json` and `--peaks PATH/peaks.json` select another compiled catalogue.
A landmark package must include its referenced photos.

To package completed work without a re-bake:

```sh
python3 fixtures/build-assistant-package.py west-cork \
  --assembled-map PATH/west-cork.obcm \
  --provenance fixtures/sources/ride-assistant/west-cork-v18.json
```

The Swiss recipe gives the full pinned national PBF to the baker and selects the cells that
intersect longitude 8.1–8.4 and latitude 46.5–46.8. **It does not pre-extract the PBF**, because a
pre-extract would change the data in the boundary cells. These map-selection bounds are separate
from the wider acquisition and replay bounds in `regions.geojson`. The catalog region ID is
`europe/switzerland`; the assembled package is `sim-assistant-meiringen`.

## Sources and licences

Each `sources[]` entry in the JSON records below pins `path`, `url`, `retrieved_at`, `bytes` and
`sha256`, plus the provider's own revision and date where it exposes them.

| Record | What it pins | Licence |
| --- | --- | --- |
| [`assistant-osm.json`](assistant-osm.json) | The full Switzerland and Monaco PBF files, and the West Cork complete-relation extract of the Ireland snapshot with its exact `osmium extract` command. All native OSM IDs, versions, timestamps, tags and geometry are retained. `switzerland.poly` records the provider's own country boundary, which is separate from the authored regional view bounds. | ODbL-1.0, © OpenStreetMap contributors |
| [`assistant-terrain.json`](assistant-terrain.json) | Untouched Copernicus GLO-30 TIFF tiles `N46_00_E008_00` and `N51_00_W010_00`, with HTTP Last-Modified, ETag, source URL, licence URL and SHA-256. | Produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all rights reserved |
| [`assistant-wiki.json`](assistant-wiki.json) | Captured Wikidata entity JSON, Wikipedia HTML at exact `oldid` revisions, raw page revisions, Commons metadata and original image bytes, for four review sites in 11 article languages, with the P279 ancestor closure for their P31 types. | Per article and per image; the notices travel in the capture |
| [`switzerland-content.json`](switzerland-content.json) | The `assistant-switzerland-content` archive: schema 2, the source capture, the compiler, the policy, the counts and the coverage limits. Each article variant and photo keeps its own source and licence notice. | Per article and per image |

The Swiss capture has no Spanish article capture and no locale entities, so every default language
falls back through the compiler's UI-language order. Its "country-complete" field is about
geographic and category acquisition; it does not claim complete language coverage.

`examples.json` records exact OSM objects with all their real tags. A way's coordinate is its
first node, not a rideable approach. QIDs link the review objects to the Wiki captures.

## Replay contract

`replays.json` describes five authored motion cases. The Meiringen traces are coordinates and UTC
timestamps from vertices of the pinned local road network; Dunlough follows the real OSM walking
approach. **No replay contains route cost, a selected route, arrival, opening status or any
application event.** The rider must still select a place and accept a computed route.

| Trace | Case | UTC anchor | Local offset |
| --- | --- | --- | --- |
| `meiringen-out-and-back` | Outbound motion and return on the same way | 2026-09-14 10:00 | +02:00 |
| `meiringen-forward-rejoin` | West-to-east motion through the local network | 2026-09-14 10:00 | +02:00 |
| `meiringen-loop-crossing` | Two loops with a shared crossing | 2026-09-14 10:00 | +02:00 |
| `monaco-dense` | Dense POI list and stable selection | 2026-09-14 10:00 | +02:00 |
| `dunlough-access` | Castle approach and return | 2026-09-14 10:00 | +01:00 |

The scenario clock is an explicit validation clock, separate from the GPX timestamps. All five
scenarios use `--clock` with `--utc-offset-min` so that both GUI and headless runs have trusted
UTC and a declared local offset; the GUI then disables ambient GPS time unless the user turns it
back on. No host timezone is inferred.

Monaco reuses the authored dense-list GPX byte for byte. Its eight named category waypoints and
eleven elevation values are authored test inputs, not captured terrain: `--gpx` motion ignores the
waypoints but feeds those elevations to the simulated barometer, and route import can consume the
waypoints. The other traces use distance-derived sampling times.

**No data was edited to produce a closed place, a bicycle prohibition or an easier alternative.**
The Dunlough paths carry `sac_scale=mountain_hiking` and mud or ground surfaces and do not set
`bicycle=no`, so the planner must use real access and profile suitability and keep unknown access
unknown. Hours are absent on the selected landmark objects. Dunlough Castle is Wikidata `Q5315471`
through OSM way `300189816`, with real article and photo Sources; a truthful unavailable Visit is
a valid result there.

## Run the scenarios

```sh
tools/obc sim assistant-dunlough-access
tools/obc sim assistant-monaco-dense
tools/obc sim assistant-out-and-back
tools/obc sim assistant-forward-rejoin
tools/obc sim assistant-loop-crossing
```

These use ordinary map loading and authored GPS replay. Once the packages and Cargo dependencies
are cached they need no live source API.

For a persistent card, start with a path that does not exist:

```sh
tools/obc sim assistant-dunlough-access -- --create-card .artifacts/west-cork.obc
cargo run --release -p obc-sim -- --card .artifacts/west-cork.obc --physical \
  --clock 2026-09-14T10:00 --utc-offset-min 60
```

The second command reopens saved routes and recordings without importing the fixture again.
**The card is user state**: fixture sync does not replace it, so create a new card to test a new
package. See the [simulator README](../../../apps/obc-sim/README.md) for imports and recording
recovery.
