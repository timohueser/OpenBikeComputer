# Ride Assistant source captures

These files describe the initial real inputs for Ride Assistant. Large bytes live in the
immutable fixture store. This input revision does not contain the later landmark map format.
It does not prove that an Assistant feature or a hardware test passed.

## Acquire and verify

Run from a clean worktree. Python 3.11+, `osmium-tool`, and the normal map build dependencies
are required. Acquisition uses the existing fixture registry:

```sh
tools/obc fixtures sync assistant-inputs
tools/obc fixtures verify assistant-inputs
python3 fixtures/verify-assistant-inputs.py
```

`OBC_FIXTURE_CACHE` selects the cache. Verification checks archive and member hashes, tracked
manifest copies, raw source hashes, all 11 recorded OSM identities, and the four Wiki identities
and article revisions. Source changes require a new immutable package. No acquisition command
runs during map generation.

## Offline maps

Build the tools once while Cargo dependencies are available. Then build from cached inputs:

```sh
cargo build --release -p obc-pack -p obc-dem
bash fixtures/build-map-package.sh assistant all
```

The script uses `cargo --offline`, the pinned OSM PBF, the pinned Copernicus tiles, the current
preset, `obc-dem bake`, and `obc-pack --bbox --terrain`. It writes initial OBCM/OBCT maps and GPX
motion to `fixtures/build/maps/sim-assistant-{meiringen,west-cork}`. An existing output is refused.
Set `OBC_FIXTURE_BUILD_DIR` to an empty directory for a second build. Compare the output hashes
at the same source commit and preset. This is the initial map revision; RA09 and RA13 must use
the normal pack/cut/assemble path to add landmark map content and publish new revisions.

The existing `monaco-upahead` scenario keeps its registered 2026-08-17 map. The new raw Monaco
source is 2026-09-13, so its later rebuild is a deliberate source refresh. The old upstream
2026-08-17 download returned HTTP 404 at capture time.

## Source boundaries and provenance

- `assistant-osm.json` pins the full 2026-09-13 Switzerland and Monaco PBF files. West Cork is
  a regional complete-relation extract of the same day's Ireland snapshot. Its source digest
  and exact `osmium extract` command are recorded. All native OSM IDs, object versions,
  timestamps, tags, and geometry are retained. The Swiss national extract is available for
  the later country count. `switzerland.poly` and its GeoJSON conversion record the provider's
  actual country extract boundary; they are separate from the authored regional view bounds.
- `assistant-terrain.json` pins untouched Copernicus GLO-30 TIFF tiles `N46_00_E008_00` and
  `N51_00_W010_00`. HTTP Last-Modified, ETag, source URL, license URL and SHA-256 are recorded.
  The Swiss tile was reused from the shared DEM cache and checked against the upstream MD5
  ETag. The Cork tile was downloaded. A shared cache is only an acquisition optimization;
  every clean worktree obtains both tiles from the immutable package.
- `assistant-wiki.json` describes captured Wikidata entity JSON, rendered Wikipedia HTML at
  exact `oldid` revisions, raw page revisions, Commons metadata and original image bytes.
  It contains four review sites, 11 article languages, P18 and article-lead image candidates,
  and the 155-class P279 ancestor closure for their P31 types. Complete article templates and
  footers retain imported text notices and license information. The content compiler must
  interpret those notices and omit unsupported credits. Capture does not assert publication
  eligibility. Full-country candidate/content capture and the production count remain RA08/RA13 work.

Each `sources[]` entry pins `path`, `url`, `retrieved_at`, `bytes`, and `sha256`. Source revisions
and dates are also recorded where the provider exposes them. Package members pin derived
`classes.json` and boundary GeoJSON. `examples.json` records exact OSM objects and all their
actual tags. Coordinates of ways are explicitly their first node, not a rideable approach.
QIDs link the OSM review objects to the Wiki captures. Meiringen station and a real water tap
also provide service cases without manufactured hours or accessibility facts.

## Replay contract

`replays.json` describes five authored motion cases. The three Meiringen traces contain
coordinates and UTC timestamps from vertices of the pinned local road network. The Dunlough
trace has the same fields and follows the actual OSM walking approach. No replay contains
route cost, selected route, arrival, opening status, or application events.

Monaco reuses the existing authored dense-list GPX byte for byte. It also contains eight named
category waypoints and eleven authored elevation values. For `--gpx` motion, the simulator
ignores the waypoints but uses those elevation values for the simulated barometer. Route import
can consume the waypoints. These elevations and waypoint annotations are authored test inputs,
not captured terrain or field measurements. The GPX timestamps start at 2025-06-29 09:00 UTC;
replay uses their relative times. The planned 2026-09-14 clock anchor below is separate from
those timestamps. The other traces use distance-derived sampling times to control motion.

| Trace | Case | UTC anchor | Configured offset |
| --- | --- | --- | --- |
| `meiringen-out-and-back` | Outbound motion and return on the same way | 2026-09-14 10:00 | +02:00 |
| `meiringen-forward-rejoin` | West-to-east motion through the local network | 2026-09-14 10:00 | +02:00 |
| `meiringen-loop-crossing` | Two loops with a shared crossing | 2026-09-14 10:00 | +02:00 |
| `monaco-dense` | Dense POI list and stable selection | 2026-09-14 10:00 | +02:00 |
| `dunlough-access` | Castle approach and return | 2026-09-14 10:00 | +01:00 |

These are motion inputs for interactive visits, not preselected navigation plans. The rider
must still select a place and accept a computed route. RA13 must apply the declared clock and
offset through normal simulator ports: the initial simulator supports headless `--clock`, but
has no deterministic GUI clock/offset argument. Do not present that missing wiring as complete.
Dunlough paths have `sac_scale=mountain_hiking` and mud/ground surfaces. They do not explicitly
set `bicycle=no`. The planner must use actual access and profile suitability; unknown access
must remain unknown. Hours are absent on the selected landmark objects. No data was edited to
produce a closed place, a bicycle prohibition, or an easier alternative.

## Initial simulator scenarios

```sh
tools/obc fixtures sync assistant
tools/obc fixtures verify assistant
tools/obc sim assistant-out-and-back
tools/obc sim assistant-forward-rejoin
tools/obc sim assistant-loop-crossing
tools/obc sim assistant-monaco-dense
tools/obc sim assistant-dunlough-access
```

These commands open the real maps and motion with the current simulator. They do not enable a
mock Assistant. The regional maps are built at source commit `3b7bf18b`; their `build.json`
records each source package digest. The map and terrain files are ordinary fixture members,
so subsequent format changes must produce new package hashes. RA13 adds the integrated
Assistant navigation and deterministic clock/offset behavior before simulator acceptance.
