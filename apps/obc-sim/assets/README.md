# Simulator-owned assets

This directory contains the shipped web-demo map and reviewable renderer
goldens. It is not the developer-fixture store.

- `grimsel-demo.obcm` is a shipped wasm demo payload.

Shared authored tracks/routes live under `fixtures/sources/`; keeping them
there prevents firmware, host, iOS, and app tests from reaching into one
another's asset directories.

Large maps, terrain, captured provider data, and coherent runnable scenarios
are declared in [`../../../fixtures/catalog.toml`](../../../fixtures/catalog.toml)
and documented in [`../../../fixtures/README.md`](../../../fixtures/README.md).
Use `obc fixtures list` and `obc sim SCENARIO`; do not add generated review
screenshots or realistic binary fixtures here.

## Landing demo map

`grimsel-demo.obcm` contains the Grimsel ride corridor, named OSM summits,
routable roads, contours, and embedded OBCT v3 surface terrain. The terrain
covers 46.30–46.95° N, 7.90–8.75° E, wider than the ride corridor. It uses
2^9-microdegree postings (about 57 m north–south) and 2^16-microdegree cells.
Peak View computes its panorama at the rider’s position. Coverage ends at
the terrain boundary; the firmware marks incomplete coverage.

Regenerate from the repository root:

```sh
fixtures/build-map-package.sh grimsel-demo /path/to/switzerland.osm.pbf
```

Set `OBC_DEMO_DEM_DIR` to reuse downloaded Copernicus tiles. The script packs
roads and summits, samples ascent, traces contours, and embeds the geographic
surface. It does not bake a panorama or change the software’s screen rendering.

The current map uses Geofabrik’s Switzerland extract downloaded on 2026-09-13
and Copernicus DEM GLO-30 tiles N46 E007 and N46 E008. OSM data is under ODbL-1.0.
Terrain is produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and
© Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the
European Union and ESA; all rights reserved.

Check with `obc test -p obc-web-demo` and build the landing page with
`trunk build --release --config docs/Trunk.toml`.
