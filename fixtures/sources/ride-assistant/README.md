# Ride Assistant source captures

These files describe the initial real inputs for Ride Assistant. Scenario inputs live in the
immutable fixture store. Country-scale raw landmark captures stay in a local map-baker source
cache; see [acquisition and recount commands](CAPTURE.md). The West Cork and Swiss regional simulator outputs use OBCM v18 with compiled landmark content.
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

The `fixtures.assistant-places` manual suite checks the place queries against these sources; see
[place query validation](PLACES-VALIDATION.md).

## Offline maps

Build the shipping tools once while Cargo dependencies are available, then use cached inputs:

```sh
cargo build --locked --release -p obc-bake -p obc-dem -p obcm-assemble
tools/obc fixtures sync assistant-inputs peak-articles
python3 fixtures/build-assistant-package.py west-cork
python3 fixtures/build-assistant-package.py meiringen
```

The recipe verifies the selected region's input packages before work. Cork runs the offline
landmark compiler; Switzerland loads its compiled input package. Both then run `obc-bake` with
the normal cut stage, a native `obc-dem bake`, and `obcm-assemble`. Assembly uses
the normal catalog selection, verifies every cell hash, and explicitly accepts partial cells at
the authored crop boundary. It does not accept missing cells or skip the final map verification.
The map embeds terrain, services, hours, graph, landmark text, compressed photos, and Sources.
Meiringen also uses the verified `peak-content` package. Its peak articles remain separate
from landmarks and join only by emitted OSM summit node ID. `--peaks PATH/peaks.json` selects
another compiled peak catalogue. The crop bounds and terrain are unchanged.

The Cork compiler input has four review sites. The Swiss recipe uses verified
`assistant-switzerland-content`: 1,478 sites, 2,391 article variants, 1,109 RGB222 photos, and their source notices.
`--landmarks PATH/content.json` can use another production compiler output with its own source coverage declaration. It must include
its referenced photos. This does not turn the regional crop into full-country map coverage.
No country raw archive is needed or published by the scenario.

Outputs go to `fixtures/build/maps`. An existing work or package directory is refused. Use
`OBC_FIXTURE_BUILD_DIR` for another build and `--bin-dir` for existing shipping tool binaries.
No Cargo build or external request runs implicitly. Build provenance records `recipe_commit`
for this checkout and separate SHA-256 hashes for `obc-bake`, `obc-dem`, and `obcm-assemble`.
A recipe commit does not assert the source revision of prebuilt binaries supplied by `--bin-dir`.
To package completed work without a rebake:

```sh
python3 fixtures/build-assistant-package.py west-cork \
  --assembled-map PATH/west-cork.obcm \
  --provenance fixtures/sources/ride-assistant/west-cork-v18.json
```

The Cork map is 4,783,296 bytes, SHA-256
`61ad73d12a06ae6224c2594659edd9b9d30e4d8948e4e7a759e836b875fa6105`.
[The build record](west-cork-v18.json) pins the source, producer and output identities.
The map retains its landmark content and byte-identical native terrain.

The Swiss map is 48,740,272 bytes, SHA-256
`232d44072991da3470ea29e48bf44a30fbd81de8d80d56465dcf9e10cee01e48`.
[Its build record](meiringen-v18.json) pins the full PBF, crop, compiled content, producer
executables, land polygons and output. Packaged `build.json` records each shipping command
and selected cell. The map contains 65 landmarks,
123 landmark article variants and 48 landmark photos.
The separate peak collection resolves Titlis (node 26864921), Eiger (node 31664302) and
Mönch (node 1372219824). Gross Wendenstock (node 1244930329) has no article.
The production reader checks these identities directly. No name or coordinate matching is used.
Terrain and replay bytes remain unchanged. These checks establish map content and reader behavior;
they do not establish Peak View user-interface or hardware acceptance.

## Swiss compiled input

`assistant-switzerland-content` is a 10,741,228-byte immutable archive. Its SHA-256 is
`42a2f4986a77641753c99a91df8a3b419dd20cbf4ac1e35512a608a936434ebd`.
[The source record](switzerland-content.json) pins schema 2, the source capture, compiler,
policy, counts and coverage limits. Its 1,110 content members are `content.json` and 1,109
RGB222 photos, plus the package manifest. Each article variant and photo keeps its original
source and license notice. Raw requests, article captures and original photos stay in the
local acquisition cache.

The retained capture supplies 669 usable English, 1,200 German and 522 French article variants
for 1,478 sites. It has no Spanish article capture or locale entities. All default languages
therefore use the compiler's UI-language order fallback. The source's country-complete field
refers to geographic and category acquisition; it does not establish complete coverage of the
current language and locale policy. The compiler omits the captured Italian articles.
Two compiler passes ran with network access denied. All 1,110 output files were byte-identical.

The Swiss map recipe gives the full pinned national PBF to the shipping baker. It selects cells
that intersect longitude 8.1–8.4 and latitude 46.5–46.8. It does not pre-extract the PBF: a pre-extract
would change data in the boundary cells. These map-selection bounds are separate from the wider
acquisition/replay bounds in `regions.geojson`. The catalog region ID is `europe/switzerland`, with
a validation-crop name; the assembled simulator package remains `sim-assistant-meiringen`.
This regional map is not a full-country map. The completed country source census is separate.

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
  eligibility. The completed country census is linked in the integrated handoff below.

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
validation clock, separate from GPX timestamps. All five scenarios use `--clock` with
`--utc-offset-min` to establish trusted UTC and the declared local offset in GUI and headless mode.
The GUI disables ambient GPS time for an explicit initial clock unless the user enables it.
No host timezone is inferred.
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
are cached, they require no live source API. The Cork and Swiss fixtures were also synced, verified and resolved with network requests disabled
after fresh-cache downloads.

Dunlough Castle is Wikidata `Q5315471`, linked by OSM way `300189816`. It has real article and photo
Sources. Its captured way has no opening-hours or bicycle/access tags. The walking approach has
mountain-hiking and mud/ground tags; this does not prove a rideable approach. A truthful unavailable
Visit is a valid result. Read the stored Sources and verify the selected graph approach in the
integrated simulator before claiming navigation or hardware acceptance.

For a persistent Unix card, start with a path that does not exist:

```sh
tools/obc sim assistant-dunlough-access -- --create-card .artifacts/west-cork.obc
cargo run --release -p obc-sim -- --card .artifacts/west-cork.obc --physical \
  --clock 2026-09-14T10:00 --utc-offset-min 60
```

The second command reopens saved routes and recordings without importing the fixture again.
See the [simulator README](../../../apps/obc-sim/README.md) for imports and recording recovery.
The card is user state; fixture sync does not replace it. Create a new card to test a new package.

The three Swiss scenarios use the same v18 crop and the +02:00 local clock offset:

```sh
tools/obc sim assistant-out-and-back
tools/obc sim assistant-forward-rejoin
tools/obc sim assistant-loop-crossing
```

[Issue #1734](https://github.com/timohueser/OpenBikeComputer/issues/1734) links normal-menu
acceptance, the complete Swiss Visit and saved recording, a real Easier alternative, resource
measurements, and the country census. Physical-device acceptance remains pending.
Package verification alone does not establish runtime acceptance.
