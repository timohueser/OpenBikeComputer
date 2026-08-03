# Skin-preview source

`teningen-preview.obcm` is the canonical geometry behind the catalog's square
skin previews. `obc-bake` embeds it, stamps each current skin onto its style
table, and renders the fixed Teningen camera through the production map renderer.
It is never published itself.

Provenance: Geofabrik `europe/germany/baden-wuerttemberg/freiburg-regbez`,
snapshot `2026-08-03`, packed as OBCM v12 with
`builder/presets/schema.json` (Bikepacking v6) and this padded crop:

```text
obc pack freiburg-regbez-260803.osm.pbf builder/presets/schema.json \
  /tmp/teningen-preview.obcm -- \
  --bbox 7.798,48.119,7.830,48.141
```

No `--terrain`, as before: the Rhine plain has nothing to show in a 1.2 km frame,
and a preview whose subject is the skin should not double as a contour test. So
this file carries no contours at any tier — which is why it took the two contour
style records (below) and no contour geometry.

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

481 517 B → **481 533 B**, still v12 (#1105 re-packed it, #1114 returned the
format). The **+16 B is not a format change at all**: it is the two contour style
records (ids 51/52, 8 B each) that #1094 appended to the schema, landing here for
the first time — this file was last packed before them. Those styles are real and
stay. Content is otherwise untouched: `obcm_diff --dump` is identical feature for
feature, and the two new records take the next free ids so nothing this file
already carried was renumbered.

The version byte went 12 → 13 → 12 in between, which cost this file nothing in
either direction: v13's only substance was a feature field for contours and a
style bit for index ones, and the Rhine plain has neither. The map is byte-identical
across that round trip apart from the header's version byte.

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

One case deliberately does **not** owe a refresh: feature types *appended* to the
schema take the next free ids and leave every id in this file meaning exactly
what it meant, so the check requires the fixture's table to be a leading run of
the schema's assignment rather than all of it, and the trailing styles are not
stamped (`previews.rs`). A schema that stops covering an id this file carries
still fails. `features.contour.*` (#1094) was the first type to ride that rule,
and rode it until the #1105 re-pack, so the fixture now carries ids 51/52 like any
other.
