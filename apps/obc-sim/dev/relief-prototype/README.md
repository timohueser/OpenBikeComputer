# Relief F visual prototype

This branch is a simulator experiment. It leaves the device renderer, OBCM contract, and normal packer unchanged. Do not publish the prototype map: its reserved fill colours appear pink or magenta in an ordinary simulator or on the device.

The cached OSM rock/scree/shingle polygons are intersected with a smoothed Copernicus DEM hillshade. `bake.py` writes an extra OSM PBF and a private preset. The existing packer combines that PBF with the original map. The terrain switch still hides the shading. Tiny shade components are removed before baking. The map still contains the normal roads, contours, peaks, POIs, and elevation data.

The simulator opts in with `OBC_SIM_RELIEF=1`. A small draw-target adapter interprets the prototype colour as a fixed 32×32 tile containing 64 dark pixels: pattern F. Its phase is anchored at a fixed map coordinate and follows camera panning and rotation. Dots remain one pixel wide; zoom recomputes the phase rather than making the dots grow with ground scale. It fills spans before paths, contours, and icons are drawn. No random sampling or elevation calculation occurs at runtime. Both GUI and headless paths use the adapter. In this mode the GUI opens directly on the map and honours `--center` and `--zoom` at startup.

This is not a performance implementation: it draws the pattern through the generic pixel path, and shadow polygons overlay the original rock fill. A production design would replace that overdraw with partitions and use an optimized span fill. Host timing is not evidence of device cost. Use this prototype to inspect appearance, zoom transitions, rotation, and motion.

## What changed in this round

Pattern F is unchanged. Three other things changed, each measured against the earlier settings.

### 1. Contour colour: khaki to dark teal

The contour colour was `0xAD4A`, which the panel shows as RGB222 (2,2,1). Grey rock is (2,2,2). The two differ in one channel by one step, so the luminance gap is 10 of 255. Over forest the gap is 25. The contours were close to invisible exactly where the shading needs them most.

The panel's four levels per channel place the common backgrounds at luminance 135 (forest), 170 (rock), 220 (grassland), and 245 (the cream land base). Every gap between them is at most 55 wide, so any mid-luminance contour lands within 25 of one background. A contour that reads on all four has to sit below about 110. The palette therefore forces a dark contour; restraint has to come from hue, from the dash, and from the one-pixel width, not from lightness.

The colour is now `0x02AA`, RGB222 (0,1,1), a dark teal. Luminance gaps: 110 to rock, 186 to the land base, 160 to grassland, 75 to forest. No road, trail, or water style uses it, and its cool hue separates it from the warm brown trail family, which is the group prominent contours were mistaken for. Weight, dash, and LOD range are untouched.

Rejected, on the same real frames: brick `0xAAAA` reads as a member of the trail family; maroon `0x5000` is readable but looks heavy where contours bunch on a headwall; slate `0x52B5` was a close second but sits near the water hue; plum `0x5015` reads well but is unconventional.

### 2. Terrain selection: light direction, generalisation, and where the boundary falls

Three separate problems.

**The light came from the wrong side.** The illumination term used `+0.5*dy` for the north component. The DEM rows run north to south, so that put the virtual light in the **northeast**, not the northwest this file claimed. Shading fell on the flanks a reader expects to be lit.

**The terrain was not generalised.** A 30 m Gaussian on a 25 m grid is close to the raw DEM. Inside the rock the median slope is then 34 degrees, and a binary threshold over it produces speckle rather than landform.

**The boundary fell mid-slope.** At 45 degrees altitude, flat ground has illumination 0.707. The old threshold of 0.56 shades any slope steeper than about 11 degrees that faces away from the light, so the shaded edge sits partway down a face and looks arbitrary. Setting the threshold at the flat-ground value instead makes the test one of aspect alone, and the edge lands on the ridge and valley lines.

The current settings are northwest light at 45 degrees, 200 m smoothing, and a threshold 0.10 below flat ground: enough below to leave gentle ground unshaded, close enough that the edge follows the ridges. That shades 43.1% of the rock, the same ink as the old 41.9%, and needs 755 shade polygons instead of 1,033.

`shade-variants.md` records the variants that were compared and the hillshade check that shows the old selection was uncorrelated with the valley system while the new one traces it.

### 3. Zoom cutoff restored at 16 m/pixel

Shading is `min_lod` 9 again: present at 16 m/pixel and closer, absent beyond. This is the existing LOD 9 boundary, inside the 15–20 m/pixel range where the shading stops being useful.

## Reproduce locally

From the repository root, provide the cached input paths. The defaults are the settings described above; the flags below reproduce the earlier F control.

```sh
python3 apps/obc-sim/dev/relief-prototype/bake.py \
  --source /path/to/source-august.geojson \
  --dem /path/to/Copernicus_DSM_COG_10_N46_00_E008_00_DEM.tif \
  --out .artifacts/relief-prototype
obc-pack /path/to/source-august.osm.pbf \
  .artifacts/relief-prototype/shadows.osm.pbf \
  .artifacts/relief-prototype/shadows-style.json \
  .artifacts/relief-prototype/engelberg-relief.obcm \
  --bbox 8.20,46.65,8.65,46.95 --no-land --terrain /path/to/terrain.obcd
cargo build --release -p obc-sim
OBC_SIM_RELIEF=1 target/release/obc-sim \
  .artifacts/relief-prototype/engelberg-relief.obcm \
  --center 8614300,46790000 --zoom 70 --scale 2
```

The earlier F control: `--azimuth 45 --smooth-m 30 --threshold 0.56 --min-lod 0 --contour-color 0xAD4A`.
The two-level exploration: add `--deep-fraction 0.18`.

`bake.py` keeps the prepared rock in `rock.wkb` beside its outputs, because parsing the cached extract costs about a minute. Delete that file to re-read the source.

The one-off script requires NumPy, SciPy, rasterio, Shapely, and the `osmium` CLI. These are host tools only. This local experiment uses the retained OSM snapshot from 2026-08-05; it does not fetch current data.

© OpenStreetMap contributors, ODbL: https://www.openstreetmap.org/copyright . Elevation: Copernicus DEM GLO-30, produced using Copernicus WorldDEM-30, © DLR e.V. 2010–2014 and © Airbus Defence and Space GmbH 2014–2018, provided under COPERNICUS by the European Union and ESA; all rights reserved.

## Dot density

`--density sparse|mid|dense` picks the dot density of the shade. Each level repeats pattern F at another tile offset, so dots stay one pixel wide and irregular and no second table is needed: 64, 126, and 224 dots per 1,024, or 6.25%, 12.3% and 21.9%.

Density costs nothing. The polygons, the file, and the render counters are identical; only the fill colour in the style table changes. On the test mountains 6.25% is hard to see at all, 12.3% reads as a distinct darker zone at a glance, and 21.9% approaches a solid tone and starts to compete with the trails, which is what the earlier solid dark-grey fill did.

`--deep-fraction` or `--deep-slope-above` adds a second, denser level for the most extreme share, cut out of the ordinary one so no pixel is filled twice. It is off by default: the step from 6.25% to 12.3% is not legible enough across a whole map to pay for the extra polygons.

## Selecting what to shade

`bake.py` offers three criteria. They cost about the same and they are not equally informative.

| Criterion | Flag | Ink | Shade polygons | What it says |
| --- | --- | --- | --- | --- |
| Light | `--aspect-drop` (default) | 43.1% | 755 | Which flank faces away from a northwest sun |
| Steepness | `--slope-above 35` | 20.0% | 481 | Where the ground is too steep to travel |
| Hollows | `--hollow-radius 250` | 28.0% | 977 | Which ground sits below its surroundings |

Steepness is the cheapest and the only one that states something a rider acts on. Hollows separate a gully from a spur, which contours alone cannot do, but they cost the most. The light criterion carries the least information: it depends on an arbitrary sun, and with a northwest light 52% of sloped rock is cross-lit and therefore sits near the threshold with no clear signal either way.

A hard limit applies to all three. The texture can only express a region wide enough to hold several dots, which is about 15 to 20 pixels, or 250 to 350 m on the ground at 16 m/pixel. Narrow features cannot be drawn with it. Shading hollows at a 150 m radius picks out the gully network cleanly as a signal, but the bands are too narrow to render as a tone, so that result is not usable with this pattern.

A reserved marker is only safe while nothing else draws that colour, and the panel's 64 colours make near misses collide. Two were rejected: `0xFA9F` quantises onto the shade marker `0xFABF`, and `0xF81F` **is** `palette::ROUTE`, so with it the 11 px magenta route line was itself drawn as dots — the route is a stroke, and strokes of 2 px and wider are laid down as spans through `fill_solid`. The deep marker is now `0xFAB5`. `the_markers_are_colours_the_device_never_draws` checks both markers against every device palette entry.

## Current validation

- `cargo build --release -p obc-sim`: passed.
- `cargo nextest run -p obc-sim --locked`: all 76 tests passed. This runs the complete package suites, including map-colour parity, camera-pan/rotation checks, and the two new marker tests. No doc-test command was run because the simulator is binary-only.
- `cargo clippy -p obc-sim --all-targets -- -D warnings`: passed.
- `tools/obc suites check`: passed.
- `cargo fmt --all`: completed.
- `git diff --check`: passed.
- Peak View is unaffected. The Gornergrat panorama renders pixel-identically with `OBC_SIM_RELIEF=1` and without it, and identically on the new map and the F control. Peak View draws white, grey, brown, black, amber, and khaki; it never fills a marker colour. Peak View from this map's own terrain reports "Terrain unavailable", because `terrain.obcd` carries no surface product; that is a property of the cached input, not of this change.

Zoom cutoff, measured on two maps that differ only in `min_lod`. The counters are the renderer's own per-frame figures; the percentages are the span, point, and ring budgets. All views dropped zero features.

| View | m/pixel | Features, no cutoff → 16 m/pixel | Chunks | Budgets, no cutoff → 16 m/pixel |
| --- | --- | --- | --- | --- |
| Close | 4.6 | 70 → 70 | 4 → 4 | 2/5/2% → 2/5/2% |
| Ride | 13 | 105 → 105 | 9 → 9 | 3/9/3% → 3/9/3% |
| Just inside | 14.8 | 136 → 136 | 11 → 11 | 4/11/4% → 4/11/4% |
| Just outside | 18.1 | 163 → 112 | 10 → 7 | 5/11/5% → 4/8/4% |
| Region | 81 | 941 → 373 | 28 → 16 | 31/37/32% → 12/18/13% |
| Farthest | 325 | 1,050 → 110 | 27 → 4 | 34/25/35% → 4/4/3% |

At every view coarser than 16 m/pixel the 16 m/pixel cutoff measures the same as the earlier, tighter 10 m/pixel cutoff, so the looser cutoff costs nothing at the overview scales and keeps the shading through the 10–16 m/pixel riding band. The map is 16,604,672 bytes, down from 17,159,680.

No device benchmark, resource build, full test graph, or UI snapshot sweep was run for this host-only visual prototype. No public conceptual documentation changed. The generic patterned pixel path and extra polygon overdraw must not be used as the final performance design.
