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

## `teningen-preview.obcd` — the terrain companion

`teningen-preview.obcd` is the OBCT terrain sidecar for the same square (epic
#1068 / #1070). `obc-bake` does not use it yet; it exists so the skin-preview
geometry has real elevation available when EL7 wires terrain into the shared
render path, and because a Rhine-plain fixture is the flat counterpart to the
simulator's alpine one.

Provenance: Copernicus DEM GLO-30, tile `N48_00_E007_00` from the AWS Open Data
mirror, over the same crop as the `.obcm` above, written latitude-first (which is
`obc-dem`'s argument order, the opposite of `obc-pack`'s):

```text
apps/obc-sim/assets/repack.sh terrain
# = obc-dem bake --sources <dem> --bbox 48.119,7.798,48.141,7.830 \
#     --cell-log2 16 --shard host/obc-bake/assets/teningen-preview.obcd
```

That script is the only supported way to regenerate it, like every other built
fixture in the repo. It is baked at the **real v1 posting** (`2^9` µdeg) with a
`2^16` cell, so the 1 × 2 cell rectangle is 65576 B rather than a 2 MiB v1 cell
mostly outside the crop — `OBCT_Spec.md` §1.3 makes both header data and §4.5
requires a reader to accept the pairing. `obc-dem`'s `tests/assets.rs` checks it
parses and covers the published camera centre.

**Attribution is a licence obligation.** Anything derived from these bytes must
carry *"produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus
Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union
and ESA; all rights reserved"*. The string lives once, in
`obc_dem::COPERNICUS_ATTRIBUTION`.

Refresh this fixture whenever the schema's style-id assignment or OBCM version
changes. `obc-bake` checks the assignment before starting a region bake and
fails with this path rather than publishing stale previews.
