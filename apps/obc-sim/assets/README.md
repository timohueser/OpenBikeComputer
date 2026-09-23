# Simulator-owned assets

`grimsel-demo.obcm` is the map the wasm landing-page demo ships. This directory is not the
developer-fixture store: large maps, terrain, captured provider data and runnable scenarios are
declared in [`catalog.toml`](../../../fixtures/catalog.toml) and described in
[the fixture registry](../../../fixtures/README.md). Use `obc fixtures list` and `obc sim
SCENARIO`. Shared authored tracks and routes live under `fixtures/sources/`, which is what keeps
firmware, host, iOS and app tests out of one another's asset directories.

**Do not add generated review screenshots or realistic binary fixtures here.**

## The landing demo map

`grimsel-demo.obcm` is an OBCM v19 file holding the Grimsel ride corridor, named OSM summits,
routable roads, contours, three settlements, two landmark sites and embedded OBCT v3 surface
terrain. The terrain covers 46.30–46.95° N, 7.90–8.75° E, wider than the ride corridor, at
2^9-microdegree postings (about 57 m north–south) in 2^16-microdegree cells. Peak View computes
its panorama at the rider's position, and the firmware marks coverage that ends at the terrain
boundary.

Regenerate from the repository root:

```sh
fixtures/build-map-package.sh grimsel-demo /path/to/switzerland.osm.pbf
```

The script packs roads and summits, samples ascent, traces contours and embeds the surface. It
bakes no panorama and changes no screen rendering. `OBC_DEMO_DEM_DIR` reuses downloaded Copernicus
tiles. Set `OBC_DEMO_LANDMARKS` to the pinned `assistant-switzerland-content` `content.json`, and
`OBC_DEMO_PEAKS` to the pinned `peak-content/peaks.json` for Mönch's separate peak article.
[The build record](../../../fixtures/sources/ride-assistant/grimsel-demo-v19.json) pins the source
packages, producer, terrain and output identities.

Check with `obc test -p obc-web-demo`, and build the landing page with
`trunk build --release --config docs/Trunk.toml`.

## Attribution

OSM data is under ODbL-1.0, © OpenStreetMap contributors.

Terrain is produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and
Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all rights reserved.
