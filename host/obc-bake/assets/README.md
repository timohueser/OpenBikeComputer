# Skin-preview source

`teningen-preview.obcm` is the canonical geometry behind the catalog's square
skin previews. `obc-bake` embeds it, stamps each current skin onto its style
table, and renders the fixed Teningen camera through the production map renderer.
It is never published itself.

Provenance: Geofabrik `europe/germany/baden-wuerttemberg/freiburg-regbez`,
snapshot `2026-08-01`, packed as OBCM v12 with
`builder/presets/schema.json` (Bikepacking v4) and this padded crop:

```text
obc pack freiburg-regbez-260801.osm.pbf builder/presets/schema.json \
  /tmp/teningen-preview.obcm -- \
  --bbox 7.798,48.119,7.830,48.141
```

The published 240×240 image is centred at `7.814,48.130` and uses `5 m/px`.
The live skin editor starts at that camera, allows pan/zoom, and treats the bbox
above as its dense camera coverage. OSM complete-way retention can expand the
OBCM header beyond the requested bbox; those overhanging coordinates are not a
licence to pan into sparse space. At wide scales the camera remains centred in
the requested crop while the full viewport stays within the file header.

The crop is wider than the initial frame, while the packer's relation-complete
selection also pulls in every member of a land-cover multipolygon reached from
inside it. That keeps residential, forest, and farmland fills whole even when a
ring segment lies outside the crop. It is already large enough for the
interactive preview to select all seven LODs, so no second browser fixture is
needed.

Size log: 472 061 B at OBCM v11 → **481 517 B at v12** (#1073). The +9 456 B is
the v12 §8.3 ascent field — `2 × 4 700` adjacency entries plus 4 B of profile
table, realised as +18 whole 512-byte node chunks. The snapshot moved at the same
time and cost nothing: the v11 packer produces the identical 472 061 B from the
2026-08-01 extract.

Refresh this fixture whenever the schema's style-id assignment or OBCM version
changes. `obc-bake` checks the assignment before starting a region bake and
fails with this path rather than publishing stale previews.
