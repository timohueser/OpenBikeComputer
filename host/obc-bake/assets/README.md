# Skin-preview source

`teningen-preview.obcm` is the canonical geometry behind the catalog's square
skin previews. `obc-bake` embeds it, stamps each current skin onto its style
table, and renders the fixed Teningen camera through the production map renderer.
It is never published itself.

Provenance: Geofabrik `europe/germany/baden-wuerttemberg/freiburg-regbez`,
snapshot `2026-06-18`, packed as OBCM v11 with
`builder/presets/schema.json` (Bikepacking v4) and this padded crop:

```text
obc pack freiburg-regbez-260618.osm.pbf builder/presets/schema.json \
  /tmp/teningen-preview.obcm -- \
  --bbox 7.798,48.119,7.830,48.141
```

The published 240×240 image is centred at `7.814,48.130` and uses `5 m/px`.
The live skin editor starts at that camera, allows pan/zoom, and treats the bbox
above as its dense camera coverage. OSM complete-way retention can expand the
OBCM header beyond the requested bbox; those overhanging coordinates are not a
licence to pan into sparse space. At wide scales the camera remains centred in
the requested crop while the full viewport stays within the file header.

The crop is wider than the initial frame so land-cover polygons whose vertices
lie just outside the view survive the packer's ingest-time crop. It is already
large enough for the interactive preview to select all seven LODs, so no second
browser fixture is needed.

Refresh this fixture whenever the schema's style-id assignment or OBCM version
changes. `obc-bake` checks the assignment before starting a region bake and
fails with this path rather than publishing stale previews.
