# Ride Assistant source captures

These files describe the initial real inputs for Ride Assistant. Scenario inputs live in the
immutable fixture store. Country-scale raw landmark captures stay in a local map-baker source
cache; see [acquisition and recount commands](CAPTURE.md). The West Cork simulator output now
uses OBCM v16 with compiled landmark content. The initial Swiss regional output remains pending.
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

Build the shipping tools once while Cargo dependencies are available, then use cached inputs:

```sh
cargo build --locked --release -p obc-bake -p obc-dem -p obcm-assemble
python3 fixtures/build-assistant-package.py west-cork
```

The recipe verifies the four input packages before work. It runs the offline landmark compiler,
`obc-bake` with the normal cut stage, a native `obc-dem bake`, and `obcm-assemble`. Assembly uses
the normal catalog selection, verifies every cell hash, and explicitly accepts partial cells at
the authored crop boundary. It does not accept missing cells or skip the final map verification.
The map embeds terrain, services, hours, graph, landmark text, compressed photos, and Sources.

The default compiler input has four review sites. `--landmarks PATH/content.json` can use an
existing production compiler output with its own source coverage declaration. It must include
its referenced photos. This does not turn the regional crop into full-country map coverage.
No country raw archive is needed or published by the scenario.

Outputs go to `fixtures/build/maps`. An existing work or package directory is refused. Use
`OBC_FIXTURE_BUILD_DIR` for another build and `--bin-dir` for existing shipping tool binaries.
No Cargo build or external request runs implicitly. To package completed work without a rebake:

```sh
python3 fixtures/build-assistant-package.py west-cork \
  --assembled-map PATH/west-cork.obcm \
  --provenance fixtures/sources/ride-assistant/west-cork-v16.json
```

The published Cork map is 4,746,240 bytes, SHA-256
`a48ebe53b9a545492b94ef4d59cdd2f371e70705683f092d9370112cccc29b23`.
[The retained record](west-cork-v16.json) has source and executable hashes, the regional boundary,
compiler coverage, and normal assembly counts. Source commit attribution uses retained build times
and Git reflog; it is not an embedded binary build stamp. The new metadata adapter reproduced the
completed map byte for byte from the retained tree and native terrain; the bake was not repeated.
Monaco already uses its pinned 2026-09-13 v16 output. The Swiss regional v14 package is unchanged
until its actual v16 crop is complete and published.

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
must still select a place and accept a computed route. The scenario clock is an explicit UTC
validation clock, separate from GPX timestamps. The integrated `--clock` establishes trusted UTC
with a known zero offset. The intended +02:00/+01:00 offsets in this source table are not yet
applied by a CLI argument. Regional opening-hours acceptance remains pending that normal port
wiring; do not infer the offset from location or the host timezone.
Dunlough paths have `sac_scale=mountain_hiking` and mud/ground surfaces. They do not explicitly
set `bicycle=no`. The planner must use actual access and profile suitability; unknown access
must remain unknown. Hours are absent on the selected landmark objects. No data was edited to
produce a closed place, a bicycle prohibition, or an easier alternative.

## Cached production scenarios

```sh
tools/obc fixtures sync assistant-dunlough-access
tools/obc fixtures verify assistant-dunlough-access
tools/obc sim assistant-dunlough-access
tools/obc sim assistant-monaco-dense
```

These commands use ordinary map loading and authored GPS replay. They do not select a place,
accept a plan, inject arrival, or enable a study model. After the packages and Cargo dependencies
are cached, they require no live source API. The Cork fixture was also synced, verified and
resolved with network requests disabled after a fresh-cache download.

Dunlough Castle is Wikidata `Q5315471`, linked by OSM way `300189816`. It has real article and photo
Sources. Its captured way has no opening-hours or bicycle/access tags. The walking approach has
mountain-hiking and mud/ground tags; this does not prove a rideable approach. A truthful unavailable
Visit is a valid result. Read the stored Sources and verify the selected graph approach in the
integrated simulator before claiming navigation or hardware acceptance.

For a persistent Unix card, start with a path that does not exist:

```sh
tools/obc sim assistant-dunlough-access -- --create-card .artifacts/west-cork.obc
cargo run --release -p obc-sim -- --card .artifacts/west-cork.obc --physical
```

The second command reopens saved routes and recordings without importing the fixture again.
See the [simulator README](../../../apps/obc-sim/README.md) for imports and recording recovery.
The card is user state; fixture sync does not replace it. Create a new card to test a new package.

The Swiss commands below remain registered with their initial v14 regional package. They are
not ready for the v16 application until the ongoing crop build is published:

```sh
tools/obc sim assistant-out-and-back
tools/obc sim assistant-forward-rejoin
tools/obc sim assistant-loop-crossing
```

The runtime menu, regional hours offsets, real route acceptance, recorder traces, final resource
and pixel checks, and physical-device acceptance remain integrated work. A package or replay is
not evidence that those checks passed.
