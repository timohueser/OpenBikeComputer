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

The current OBCM v18 map uses the pinned `assistant-osm` Switzerland extract
(source timestamp 2026-09-13T20:21:20Z, SHA-256
`e6ae53a3cfeb8fbefbab291073e61f0906b03576e773b7403ab9b6d6232a8e88`).
The packer used the canonical demo box above and the existing embedded surface
terrain (SHA-256 `2c46061b61a444df3b24350089551a38451a01a9729de062520458f0749ad9ab`)
for contours and ascent. This terrain comes from Copernicus DEM GLO-30 tiles
N46 E007 and N46 E008. The map is 10 093 056 bytes and its SHA-256 is
`6a59043ab2d4de002c6e9fd5e7dcc88259ef6f7e9548f0c1ba91a8ae183de47f`. It holds three settlement
records: the village Guttannen and the hamlets Boden and Gletsch.
The map contains the same two landmark sites, Q114357029 and Q666668, from the pinned
`assistant-switzerland-content` schema 2 package. They now contain four article variants
in total and no photos. Set `OBC_DEMO_LANDMARKS` to its `content.json`.
Set `OBC_DEMO_PEAKS` to the pinned `peak-content/peaks.json` to include Mönch's separate
peak article, four language variants, photo and original Sources. The association uses
OSM node 1372219824. The canonical crop and embedded terrain are unchanged.
[The build record](../../../fixtures/sources/ride-assistant/grimsel-demo-v18.json) pins
source packages, producer, terrain and output identities.

OSM data is under ODbL-1.0.
Terrain is produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and
© Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the
European Union and ESA; all rights reserved.

Check with `obc test -p obc-web-demo` and build the landing page with
`trunk build --release --config docs/Trunk.toml`.
