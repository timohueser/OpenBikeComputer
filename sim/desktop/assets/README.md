# Simulator-owned assets

`grimsel-demo.obcm` is the map that the wasm landing-page demo ships. This directory is not the
developer-fixture store: [`catalog.toml`](../../../fixtures/catalog.toml) declares large maps,
terrain, captured provider data and runnable scenarios, and
[the fixture registry](../../../fixtures/README.md) describes them. Use `obc fixtures list` and
`obc sim SCENARIO`. Shared authored tracks and routes live under `fixtures/sources/`.

**Do not add generated review screenshots or realistic binary fixtures here.**

## Build

`grimsel-demo.obcm` is an OBCM v19 file holding the Grimsel ride corridor, named OSM summits,
routable roads, contours, three settlements, two landmark sites and embedded OBCT v3 surface
terrain. The terrain covers 46.30–46.95° N, 7.90–8.75° E, wider than the ride corridor, at
2^9-microdegree postings (about 57 m north–south) in 2^16-microdegree cells. Regenerate from the
repository root:

```sh
fixtures/build-map-package.sh grimsel-demo /path/to/switzerland.osm.pbf
```

The script packs roads and summits, samples ascent, traces contours and embeds the surface. It
bakes no panorama. `OBC_DEMO_DEM_DIR` reuses downloaded Copernicus
tiles. Set `OBC_DEMO_LANDMARKS` to the pinned `assistant-switzerland-content` `content.json`, and
`OBC_DEMO_PEAKS` to the pinned `peak-content/peaks.json` for Mönch's separate peak article.
[The build record](../../../fixtures/sources/ride-assistant/grimsel-demo-v19.json) pins the source
packages, producer, terrain and output identities.

Check with `obc test -p obc-web-demo`, and build the landing page with
`trunk build --release --config docs/Trunk.toml`.

## Attribution

OSM data carries the `osm-planet` credit and terrain the `copernicus-glo-30` credit. Both live
once, in [`data/sources.toml`](../../../data/sources.toml).
