# Simulator fixture assets

The committed `.obcm` maps the simulator (and several workspace tests) load.
They are **built artifacts** — their sources are not in the repo — so their
provenance is pinned here and in [`repack.sh`](repack.sh), the only supported
way to regenerate them.

| file | what / why | source | extract bbox (lon,lat,lon,lat) |
| :-- | :-- | :-- | :-- |
| `grimsel.obcm` | Grimsel Pass region — alpine showcase: switchbacks, sparse POIs, multi-component nav graph, and (since the 2026-08-03 repack) the **E3 contour lines** and real §8.3 ascent — it is packed **with** `grimsel.obcd`, the one fixture that exercises the whole terrain path | Geofabrik `europe/switzerland-latest.osm.pbf` | `8.15034,46.48261,8.46007,46.72070` |
| `grimsel-demo.obcm` | Landing-page live-demo map (epic #624 S4, #629) — a **tight corridor** around the climb, **shipped in the wasm only** (`include_bytes!` in `obc-web-demo`'s `demo.rs`). NOT a shared test fixture. Padded ~2 km around the `grimsel-climb.gpx` track so the demo tours have Lodging POIs + a routable nav graph. ~5× smaller than `grimsel.obcm`. | Geofabrik `europe/switzerland-latest.osm.pbf` | `8.26,46.54,8.37,46.67` |
| `monaco.obcm` | Central Monaco — dense urban: POI-rich (opening hours), tight street grid | Geofabrik `europe/monaco-latest.osm.pbf` | `7.39,43.71,7.47,43.77` |
| `grimsel-climb.gpx` / `.obcr` | Hand-made climb route inside the grimsel bbox (GPX + converted OBCR). The GPX is also the `ui-snapshots.sh` default replay track. The `.obcr` is pinned byte-for-byte against `gpx_to_obcr` on the `.gpx` beside it by `routes.rs`'s `committed_route_asset_matches_the_gpx_conversion`, so an OBCR format bump re-cuts it: `cargo test -p obc-sim regenerate_committed_route_asset -- --ignored` (not an `.obcm`, so the repack-provenance rules don't apply) | authored, not extracted | — |
| `grimsel-climb-demo.gpx` | Demo-only trimmed replay for the wasm demo (epic #624 S4, #629): the upper ~62 % of `grimsel-climb.gpx` (ele 1530→2151 m, past Grimsel Hospiz to the pass), every 5th point, **original timestamps kept** (rebased to a 08:00:00 start; Δt≤0 decimation artifacts dropped) so the on-screen speed/stats stay honest — a ~52 min ride segment, ≈17 min wall-clock per loop at the demo's `set_speed(3.0)` playback. `include_str!`'d in `obc-web-demo`'s `demo.rs`. Regenerate by re-decimating `grimsel-climb.gpx` — a hand-made GPX, not an `.obcm`, so the repack-provenance rules don't apply. | authored, decimated from `grimsel-climb.gpx` | — |
| `monaco-upahead.gpx` | The **"Up ahead" fixture route** (epic #946, U3): a hand-made ~2.7 km line across central Monaco, authored to sit inside `monaco.obcm` so the 300 m route corridor catches real Resupply / Pharmacy / Lodging POIs. Its waypoints cover five of the six categories via `<sym>`, two Generic ones, offsets on both sides of the line (two inside the 50 m side-hint threshold, three past it), and a name long enough to ellipsize — i.e. every case the merged timeline draws. Doubles as its own replay track (`<time>` one point per minute), so `--gpx ... --at SEC` rides along it. `ui-snapshots.sh` **imports it at run time** (`obc-sim --import`) rather than committing a second `.obcr`, so an OBCR format bump needs no re-cut here | authored, not extracted | — |
| `vector-loop-replay.gpx` | Synthetic replay tracing `specs/vectors/route-waypoints.obcr` ("Vector Loop") — 6 trackpoints at that route's own vertices (from `obc-vectors/src/route-source.gpx`), timestamped at a constant ~6 m/s, ending at vertex 6 (~1.40 km, ~300 m short of the "Pass Summit" waypoint). Lies *on* the route so the matcher locks and progress drives the waypoint chip/ticks; the `ui-snapshots.sh` waypoint frames use it (the Grimsel climb GPX is far off that 48°N route). Regenerate: re-derive from the route vertices — no packer step (a hand-made GPX, not an `.obcm`, so the repack-provenance rules don't apply) | authored, not extracted | — |
| `grimsel.obcd` | **Terrain sidecar** for `grimsel.obcm` (OBCT, epic #1068 / #1070) — the elevation the Climb screen, the profile, ride stats and GPX export read once terrain is mounted (EL7). Not an `.obcm`: a raster from Copernicus GLO-30, with its own artifact class and its own revision track ([`OBCT_Spec.md`](../../../specs/OBCT_Spec.md)). Baked at the **real v1 posting** (`2^9` µdeg) with a `2^16` cell, so the 4 × 6 cell rectangle is 786560 B instead of four 2 MiB v1 cells mostly outside the map. Regenerate with `./repack.sh terrain`; `obc-dem`'s `tests/assets.rs` checks it parses, covers the crop and reads surveyed pass elevations | Copernicus DEM GLO-30, AWS Open Data mirror | `46.48261,8.15034,46.72070,8.46007` (**lat first** — `obc-dem` order) |
| `TP1.OBT` | Fixture **trip** object (epic #526, TR2): "Alpen Traverse", stage ids `[0, 1, 99]` — the first two are the sorted-scan session ids of any two routes in the same folder, 99 a deliberate dangling ref (read-tolerance, spec §7.7). **Copy it into your `--routes-dir` beside two or more routes and rescan** to get a groupable menu: one folder (the first two routes filed) + the rest loose. TR3's snapshot harness stages exactly this file. Pinned byte-for-byte against `obc_route::write_trip` by `trips.rs`'s `committed_trip_asset_matches_the_production_writer`; regenerate with `write_trip("Alpen Traverse", &[0, 1, 99], …)` (not an `.obcm`, so the repack-provenance rules don't apply) | authored via `obc_route::write_trip` | — |

The terrain sidecar is not packed at all — it is baked from the DEM by
`obc-dem` and shares nothing with the OSM path. Its bbox is the same canonical
crop as `grimsel.obcm`, written latitude-first because that is `obc-dem`'s
argument order; the cell rectangle rounds outward to whole `2^16` cells, so the
raster already reaches a little past the crop on every side. **Attribution is a
licence obligation**: anything derived from these bytes must carry *"produced
using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space
GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all
rights reserved"* — the string lives once, in `obc_dem::COPERNICUS_ATTRIBUTION`.

All three maps are packed with `builder/presets/schema.json` via
`cargo run --release --bin obc-pack -- <source> <preset> <out> --bbox <bbox>` —
`repack.sh` runs the whole chain (download → pack). `grimsel.obcm` additionally
passes `--terrain grimsel.obcd` (the committed sidecar beside it), so its nav
graph carries real integrated ascent and its ladder carries the traced contours
the preset asks for — since #1104 that reaches the `coarse` band too, whose
LOD 2 carries the full contour set; `grimsel-demo.obcm` and `monaco.obcm` are packed
without terrain (the demo stays small for the wasm payload, and Monaco has no
sidecar), so the packer's "contours enabled but no terrain" warning is expected
for them. `grimsel-demo.obcm` shares the Switzerland snapshot with
`grimsel.obcm` (`./repack.sh grimsel-demo`).

The crop used to be a separate `osmium extract` step; since #910 the packer does
it during ingest, so regenerating a fixture needs no `osmium-tool`. The initial
implementation reproduced osmium's default *complete_ways* strategy byte for
byte. It now also completes renderable area relations, preventing a polygon
inside the bbox from disappearing when one member lies outside it. The current
simulator fixtures were not re-packed for that correction; expect polygon and
byte changes the next time their pinned snapshots are deliberately refreshed.

## wx10-rain-previews/

Rendered 240 x 320 review frames of the WX10 rain overlay (epic #1185), embedded in PR bodies so
look-tuning rounds have a stable surface to point at. **Not fixtures** -- nothing reads them; they
are regenerated (release sim, deterministic output) by exactly the `map-rain-*` commands in
`firmware/ui-snapshots.sh` plus the `demo:drizzle` variant and the rain-free baseline (the explicit `--weather-now
1800000000` anchor pins the first demo frame now that the sim's rain lease follows the live wall
clock — WX11 review F5; with `--clock` pinning the clock elsewhere the anchor keeps these frames
byte-identical):

```sh
cargo build --release -p obc-sim
S=target/release/obc-sim; M=apps/obc-sim/assets/grimsel.obcm; O=apps/obc-sim/assets/wx10-rain-previews
$S $M --weather demo:scattered --weather-now 1800000000 --clock "2025-06-29T14:40" --png $O/map-rain-scattered.png
$S $M --weather demo:frontal --heading 35 --zoom 4 --weather-now 1800000000 --clock "2025-06-29T14:40" --png $O/map-rain-frontal-heading.png
$S $M --weather demo:storm --weather-now 1800000000 --clock "2025-06-29T14:40" --png $O/map-rain-storm.png
$S $M --weather demo:drizzle --weather-now 1800000000 --clock "2025-06-29T14:40" --png $O/map-rain-drizzle.png
$S $M --clock "2025-06-29T14:40" --png $O/map-rain-none.png
```

Re-render after any edit to the rain tuning surface (`firmware/obc-render/src/rain.rs` -- the one
file a look round touches) and commit the refreshed frames with it; delete the directory whenever
the review era ends.

## The bbox-ratchet trap (read before re-packing)

**Never derive an extract bbox from an existing fixture's header.** The header
bbox is computed from the *packed content* and is always somewhat wider than
the extract bbox (a way crossing the boundary is kept whole, and stray
coastline/boundary features can stretch it far offshore). Extracting from a
header bbox therefore widens the fixture on every re-pack — that ratchet is
how monaco once ballooned to 14.5 MB and grimsel silently grew ~77 % in area
during the v9 bump. The bboxes in `repack.sh` are canonical; change them only
deliberately, in a reviewed commit.

Geofabrik `-latest` files roll daily, so re-packs still pick up OSM edits —
expect small content diffs between snapshots. When you commit a re-packed
fixture, update the log below.

## Pack log

| date (OSM snapshot) | file | note |
| :-- | :-- | :-- |
| 2026-07-07 | `grimsel.obcm` | v9; re-packed to the canonical bbox (the first v9 pack had self-sourced a wider bbox from the v8 header) |
| 2026-07-07 | `monaco.obcm` | v9; tight Monaco bbox established (PR #548 follow-up commit) |
| 2026-07-07 | `grimsel.obcm` | v10; line-style record (epic #556 #557). 3.80 MB → 3.80 MB (+84 B, the style table's 2-byte-per-record growth) |
| 2026-07-07 | `monaco.obcm` | v10; line-style record (epic #556 #557). Re-packed via `repack.sh` at the canonical bbox: 2.56 MB → 1.10 MB (the committed v9 file predated the repack.sh provenance pass and used a wider bbox; this is the tighter canonical size) |
| 2026-07-07 | `grimsel.obcm` | v10; rail/admin line styles (#558). `default.json` `railway.rail`/`light_rail` → dashed + white `color2` base (railway stripe), `admin_level.2` → dashed. Same snapshot, so size is unchanged (3.80 MB, 0 net bytes — records are already 8 bytes); only 7 style-table bytes differ (the two flag bits + `color2` per record). |
| 2026-07-07 | `monaco.obcm` | v10; rail/admin line styles (#558). Same preset change; unchanged size (1.10 MB, 0 net bytes), 7 style-table bytes differ. |
| 2026-07-08 | `grimsel.obcm` | v10; road casing (#559). `default.json` paved `highway` classes gained a darker `color2` (orange→`0xAAA0`, yellow→`0xAD40`, grey→`0x0000`) for the finest-LOD casing pass. Same snapshot, so size is unchanged (3.80 MB, 3797843 B → 3797843 B, 0 net bytes — records are already 8 bytes); only the paved-highway style-table records' `color2` bytes + flag bit differ. |
| 2026-07-08 | `monaco.obcm` | v10; road casing (#559). Same preset change; unchanged size (1.10 MB, 1098449 B → 1098449 B, 0 net bytes), paved-highway `color2` bytes + flag bit differ. |
| 2026-07-08 | `grimsel.obcm` | v10; polygon outlines (#560). `default.json` `building.yes` gained a darker-grey `color2` (`0x52AA`) for the finest-LOD building-ring outline. Same snapshot, so size is unchanged (3.80 MB, 3797843 B → 3797843 B, 0 net bytes — records are already 8 bytes); only the `building.yes` style-table record's `color2` bytes + flag bit differ. |
| 2026-07-08 | `monaco.obcm` | v10; polygon outlines (#560). Same preset change; unchanged size (1.10 MB, 1098449 B → 1098449 B, 0 net bytes), `building.yes` `color2` bytes + flag bit differ. |
| 2026-07-09 | `grimsel-demo.obcm` | v10; **new** demo-only map (epic #624 S4, #629). Canonical corridor bbox `8.26,46.54,8.37,46.67` from the `europe/switzerland-latest.osm.pbf` 2026-06-20 snapshot. 751518 B (734 KB) raw / 241 KB gzip vs `grimsel.obcm` 3797843 B (3.62 MB) / 1.20 MB gzip — ~5× smaller. 10 Lodging POIs, single 236 km / 1007-node routable nav component. |
| 2026-07-30 | all three | v11; tight chunks + compact feature header (#1009), on top of the ring-cap split fix (#1007). `grimsel.obcm` 3797843 B → 2618777 B, `monaco.obcm` 1098449 B → 683532 B, `grimsel-demo.obcm` 751518 B → 639262 B. **These deltas are three changes, not one**: the committed files predate `default.json`'s preset v4 (#984, 2026-07-29), so the re-pack carries that restyle, and it also carries #1007's ring cap, which forces extra splits in the two Switzerland cuts (grimsel 2614924 → 2618777 B, grimsel-demo 635216 → 639262 B when #1007 merged in; monaco is byte-identical either side, being too small to hold an over-cap polygon). The **format** alone, measured by packing one extract with both packers: monaco 1597945 B → 683532 B (2.34×), grimsel 6189979 B → 2614924 B (2.37×), `grimsel-demo` 1567286 B → 635216 B (2.47×) — `obcm_diff --dump` identical across the bump in all three. Switzerland snapshot 2026-06-21, Monaco 2026-06-16. |
| 2026-08-02 | all three | v12; directional ascent in the nav graph + profile climb weight (#1073, elevation epic #1068). `grimsel.obcm` 2618777 B → 2856199 B, `monaco.obcm` 683532 B → 708626 B, `grimsel-demo.obcm` 639262 B → 656408 B. **Most of those deltas are snapshot drift, not the format** — the committed files came from the 2026-06-21 Switzerland / 2026-06-16 Monaco snapshots and these come from 2026-08-01/02. The **format** alone, measured by packing the same extract with both packers: monaco 684738 → 708626 B (+23 888, +3.49 %), grimsel 2832391 → 2856199 B (+23 808, +0.84 %), grimsel-demo 651496 → 656408 B (+4 912, +0.75 %). Every byte is accounted for: `2 × adjacency entries + 4 × profiles` (monaco +23 730 over 11 857 entries), realised as whole 512-byte node chunks by §8.2's bin packing (+45 / +45 / +9 chunks). Monaco is the outlier because it is a small file with a dense graph — its §8 section is 61 % of it. The graphs themselves are identical across the bump (same node, edge and adjacency counts); `Ascent M` is `0` everywhere because these are packed without `--terrain`. Switzerland snapshot 2026-08-01, Monaco 2026-08-01. |
| 2026-08-02 | `grimsel.obcd` | **new** (epic #1068 / #1070) — the first OBCT terrain sidecar. Copernicus GLO-30 tile `N46_00_E008_00`, bilinearly point-sampled onto the `2^9` µdeg lattice, `2^16` cells: 24 cells (4 × 6), 786560 B, 100 % covered, no voids. Surveyed check through `obc_elevation::TerrainReader`: Grimsel Pass 2161 m (surveyed 2164), Furka 2429 (2429), Nufenen 2486 (2478). |
| 2026-08-03 | `grimsel.obcm` | still v12; first pack **with `--terrain grimsel.obcd`** (EL10a–c, #1094/#1095/#1096) — `repack.sh`'s grimsel target now passes the committed sidecar permanently. 2856199 B → 3545176 B. **The accounting is exact**: a same-snapshot control bake *without* terrain is 2856215 B — the old size **+16 B, which is precisely the two E3 contour style records** the v5 preset appends (ids 51/52: `0xAD55` weight 1, dashed major / solid index, `fixed_width` + `terrain_layer` flag bits) — so two days of OSM drift in this box nets to **zero bytes** and the whole **+688 961 B (+24.1 %)** is traced contours (100 m, index every 5, 15 m clamp; the §8.3 `Ascent M` field already existed and only its *values* changed). The nav graph now carries **real integrated ascent**: 4380 of 11 586 neighbor entries nonzero (the zeros are genuinely flat edges — the v12 entry's "`Ascent M` is 0 everywhere" era ends here). Reproduction verified byte-for-byte from the cached snapshot. `grimsel-demo.obcm` and `monaco.obcm` deliberately stay terrain-less (wasm payload size; no Monaco sidecar) — the packer's "contours enabled but no terrain" warning is expected for them. Switzerland snapshot 2026-08-03. |
| 2026-08-03 | `grimsel.obcm` | still v12; **contours reach LOD 2** (CL1, #1104 / epic #1103) — preset v6 moves **both** contour classes to `min_lod: 2`, one tier above the planning tier (LOD 3), so the terrain layer survives the zoom-out step where it used to vanish whole. LODs 0–1 stay contour-free. The first cut of this change gave LOD 2 to `index` alone; that was rejected on glass — solid grey lines with no dashes around them read as paths, because emphasis-by-continuity only means anything while the dashes are present — so the two classes travel together. 3545176 B → 3637616 B, **+92 440 B (+2.61 %)**. **The accounting is exact and it is all one tier**: a control bake of the *previous* preset from the same day's snapshot is byte-identical to the committed fixture, so there is zero OSM drift in this delta, and `obcm_diff` reports LODs 0, 1 and 3–6 unchanged down to the feature multiset and the node/chunk counts — the whole change is LOD 2 (61 → 133 index nodes, 46 → 100 chunks, 1172 → 3663 features). Of those +2491 features, **2297 are contours** (1834 major + 463 index); the other +194 net is re-splitting, not new content — the added bytes push leaves past the 4096 B chunk budget, they split, and straddling landuse polygons get clipped into more records (92 out, 286 in, same ground). Contour census per tier, from `obcm_diff --dump`: LOD 0–1 none, **LOD 2 = 1834 major / 463 index**, LOD 3 = 2447 major / 612 index, and up — LOD 2 holds about three quarters of LOD 3's count, the 40 m ladder simplify and the line-merge stitch being what separates them. It stays affordable because it is one tier at the ladder's coarsest useful tolerance: a seventh of what #1094's +24.1 % bought, by bytes (92 440 vs 688 961). `grimsel-demo.obcm` and `monaco.obcm` are unaffected (packed without terrain, so no contours at any tier). Switzerland snapshot 2026-08-03. |
| 2026-08-03 | all three | **v13**; the contour level on the wire + the `CONTOUR_INDEX` style bit (CL2, #1105 / epic #1103). `grimsel.obcm` 3637616 B → 3673466 B, **+35 850 B (+0.99 %)**; `monaco.obcm` 708626 B → 708642 B and `grimsel-demo.obcm` 656408 B → 656424 B, **+16 B each**. **The accounting is exact, and the two deltas are different changes.** Grimsel carries 17 925 contour features (14 340 major + 3 585 index, over LODs 2–6) and the map grew by 2 × 17 925 = 35 850 B — the v13 `int16` level, and *nothing else*: not one leaf re-split, and `obcm_diff --dump` is character-identical across the bump once the new `level=` annotations are stripped, on all three maps. The +16 B on the two terrain-less maps is **not** v13 at all (a map with no terrain packs no contours, so v13 costs it zero bytes): it is the two contour style records (ids 51/52, 8 B each) finally landing in fixtures last packed before #1094 added them — `obcm_diff` reports every LOD identical down to the feature multiset. Levels spot-check as multiples of the 100 m interval, and the index class as multiples of 500 m (`index_every: 5`): 1000, 1500, 2000, 2500, 3000, 3500, 4000. Switzerland + Monaco snapshots 2026-08-03; a control bake of the *previous* packer from the same snapshot reproduces the committed CL1 `grimsel.obcm` byte for byte, so there is zero OSM drift in this delta. |
| 2026-08-04 | all three | **back to v12**; the contour level and the `CONTOUR_INDEX` bit are removed (CL4, #1114 / epic #1103) after the elevation labels they existed for were cancelled (#1106 — too distracting at this panel resolution) and the ~1 % they cost was judged not worth carrying for a feature nothing was going to read. v13 was never published, so it is unwound rather than deprecated. `grimsel.obcm` 3673466 B → **3637616 B, byte-identical to the pre-#1105 fixture** — that identity is the proof the removal is complete, since the same snapshot and the same preset now produce the same file again. `monaco.obcm` (708642 B) and `grimsel-demo.obcm` (656424 B) are **unchanged in size** and differ from their v13 bytes in exactly **two bytes**: the header version (`0x0D` → `0x0C`) and style 52's flags, which loses bit 6 — a precise measure of what v13 was in a map with no contours. All three keep the +16 B of #1094 contour style records they picked up in the #1105 repack; those styles are real and stay. Switzerland + Monaco snapshots 2026-08-03, i.e. the same ones #1105 used, which is what makes the byte-identity meaningful. |
| 2026-08-08 | all three | **v13**; sparse lookup-only anchors for exact start/end projection onto §8.4 road geometry. Same-source v12 controls isolate the format cost: `grimsel.obcm` 3638272 B → **3664384 B** (+26112 B, +0.72 %), `grimsel-demo.obcm` 656896 B → **664064 B** (+7168 B, +1.09 %), `monaco.obcm` 709120 B → **710144 B** (+1024 B, +0.14 %); combined, +34304 B over 5004288 B (**+0.69 %**). The small-map percentages vary because the 12-byte records, quadtree, alignment, and final data occupy whole 512-byte chunks. Anchors are at most 300 m apart along long edges, are never graph nodes, and only discover candidate edge ids; both displayed route endpoints are still exact projections onto the full road polyline. Switzerland + Monaco snapshots 2026-08-08. |
