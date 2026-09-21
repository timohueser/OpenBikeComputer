# Skin-preview source

`teningen-preview.obcm` is the canonical geometry behind the catalog's square skin previews.
`obc-bake` embeds it, stamps each current skin onto its style table, and renders the fixed
Teningen camera through the production map renderer. The file is never published itself.

`teningen-preview.obcd` is the OBCT terrain sidecar for the same square. `obc-bake` does not use
it yet; it exists so the preview geometry has real elevation available when terrain reaches the
shared render path.

## Regenerate

`fixtures/build-map-package.sh` is the only supported way, as it is for every built fixture here.
The map is packed from a Geofabrik `europe/germany/baden-wuerttemberg/freiburg-regbez` extract
with `builder/presets/schema.json`, on this canonical crop, with `--no-land` and no terrain:

```text
obc pack freiburg-regbez.osm.pbf builder/presets/schema.json \
  /tmp/teningen-preview.obcm -- \
  --bbox 7.798,48.119,7.830,48.141
```

The terrain sidecar comes from Copernicus DEM GLO-30 tile `N48_00_E007_00`, over the same crop
but written latitude-first, which is `obc-dem`'s argument order and the opposite of `obc-pack`'s:

```text
fixtures/build-map-package.sh terrain
# = obc-dem bake --sources <dem> --bbox 48.119,7.798,48.141,7.830 \
#     --cell-log2 16 --shard host/obc-bake/assets/teningen-preview.obcd
```

It is baked at the real v1 posting (`2^9` µdeg) with a `2^16` cell, so the 1 × 2 cell rectangle is
65 576 B rather than a 2 MiB v1 cell that is mostly outside the crop. `OBCT_Spec.md` §1.3 makes
both header data and §4.5 requires a reader to accept the pairing.

There is no terrain in the map, deliberately: the Rhine plain has nothing to show in a 1.2 km
frame, and a preview whose subject is the skin should not double as a contour test. There is no
golden preview PNG either — the tests assert that the skins render distinctly and
deterministically, and that residential fills cover the frame.

## When to refresh it

**Refresh whenever the schema's style-id assignment or the OBCM version changes.** `obc-bake`
checks the assignment before it starts a region bake and fails with this path rather than
publishing stale previews.

One case deliberately owes no refresh: feature types *appended* to the schema take the next free
ids and leave every id in this file meaning what it meant, so `check_source` in
`host/obc-bake/src/previews.rs` requires the fixture's table to be a leading run of the schema's
assignment rather than all of it. A schema that stops covering an id this file carries still
fails.

## The published camera

The 240 × 240 image is centred at `7.814,48.130` at 5 m/px. The live skin editor starts at that
camera, allows pan and zoom, and treats the crop above as its dense coverage. The crop is wider
than the frame, and the packer's relation-complete selection also pulls in every member of a
land-cover multipolygon reached from inside it, which keeps residential, forest and farmland fills
whole. That is already enough for the interactive preview to select every rung of the ladder, so
no second browser fixture is needed.

OSM complete-way retention can push the OBCM header beyond the requested bbox. **Those
overhanging coordinates are not a licence to pan into sparse space**: at wide scales the camera
stays centred in the requested crop while the viewport stays inside the file header.

## Attribution

Anything derived from `teningen-preview.obcd` must carry *"produced using Copernicus WorldDEM-30
© DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by
the European Union and ESA; all rights reserved"*. The string lives once, in
`obc_elevation::COPERNICUS_ATTRIBUTION`.

OSM data is under ODbL-1.0, © OpenStreetMap contributors.
