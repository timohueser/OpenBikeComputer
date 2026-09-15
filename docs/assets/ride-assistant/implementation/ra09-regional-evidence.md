# RA09 regional offline evidence

## West Cork

The normal baker processed the captured West Cork PBF and the reviewed small
landmark source output with all network access denied. The source is the recorded
RA01 crop, not a new synthetic road map. Bounds are -9.90,51.43 to -9.50,51.65.
The source-derived Dunlough Castle text and photo were compiled by the corrected
RA08 compiler. Ten map cells completed with no failed cut plan.

The native assembler verified each map cell and its captured DEM input by SHA-256.
Its normal network-denied assembly and verification pass produced:

- Map: 4,746,240 bytes, SHA-256
  `a48ebe53b9a545492b94ef4d59cdd2f371e70705683f092d9370112cccc29b23`.
- Ten map cells, one native terrain cell, four terrain-directory slots.
- Terrain: 2,097,200 bytes. The native DEM bake covers all 1,048,576 samples
  from the captured Copernicus N51W010 tile.
- Navigation: 7,986 nodes, 8,607 edges, no degree truncation or dropped nodes.
- Places: 55 records, no duplicate or dropped records.

The crop cells are marked partial. Assembly uses the normal explicit
`--accept-partial` option. Missing surrounding map coverage is not claimed complete.
The initial assembly with surface-enhanced terrain failed the existing Peak View
10% size gate: 4,745,216 to 5,799,936 bytes. The gate was not changed. The native
terrain path uses `obc-dem bake` and the normal assembler with the same captured
DEM. Peak View surface acceptance is not claimed for this small crop.

Local artifacts are under the landmark-map worktree's
`.artifacts/ra13-west-cork/`: source, DEM, region recipe, baker tree, native terrain,
assembler input manifests, and `assembled/west-cork.obcm`. Source, compile, bake,
and assembly steps used `sandbox-exec` with network access denied. No source
archive was published. The normal simulator imported the assembled map to a persistent flat card,
reopened it, and rendered the Dunlough area offline at 240 × 320. The card uses
map object 1, revision 1. The captured map frame is in the photo-phase worktree
at `.artifacts/ra13-west-cork/card-map.png`. This validates card import, reload,
and map rendering; final Landmarks/Visit interaction remains RA10/RA12 work.

The assembled landmark directory contains Dunlough Castle Q5315471 with two
text pages, an 8,096-byte compressed photo, 5,874 bytes of photo attribution,
and explicit OSM approach metadata for all four profiles. It is source-derived
content, not the study fixture.

## Grimsel and current mainline

The existing v16 Grimsel package passes the complete external route-navigation
suite: 52 tests, including real ascent and GPX export/re-import parity. It already
contains the required complete-DEM flags. No v16 fixture repack is needed.

Commit `40f62ee8` includes current develop explicit route cleanup, the revised
country-source storage policy, and the corrected v4 route pins. Country raw input
is retained outside the fixture cache for remaining validation. The owner removed
its public development-fixture object; no full-country raw archive will be
republished. Processed maps use the normal map publication path.

Regional Swiss assembly, full source-to-mapped-approach measurements, actual
Landmarks/photo/Visit UI acceptance, final resource/layout review, and hardware
acceptance remain pending. No local resource image or UI sweep was added.

## Catalog fixture and allocation corrections

Commit `bc8202c6` removes stale byte counts from the desktop launch fixture and
updates the browser checks to the current v16 producer output. The hash-failure
case now changes whitespace, so it remains valid JSON with different bytes.
The normal three WASM bridges were built before the complete frontend suite:
`npm run build:wasm --prefix builder/app` and `npm test --prefix builder/app`
passed (65 files, 980 tests). `./tools/obc suites check` and `git diff --check`
passed. The Linux desktop launch suite remains a CI gate on this macOS host;
its prior captured page error identified the stale band total (994 versus 1010).
No public documentation changed.

CI run 34941991009, board job 104292653311, measured the data head `40f62ee8`:
App 48,432 bytes; linked resident 303,760 bytes; `.uninit` 132,096 bytes including
the 131,072-byte arena; flash 1,530,000 bytes. The largest guarded poll frame is
9,784 bytes, residual main stack is 55,664 bytes, and the recorded deep-ride
high-water margin is 18,648 bytes against the unchanged 8,704-byte floor.
The App initialization frame is 64 bytes against 4,096. Commit `ef448514`
records the measured App size; it does not change any capacity or stack limit.
No local shipping image, hardware measurement, or snapshot sweep was run.
