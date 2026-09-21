---
title: Terrain and elevation
description: How OpenBikeComputer bakes, assembles, samples, and uses terrain data.
copy: ai
---

# Terrain and elevation

OpenBikeComputer uses one terrain raster for all elevation functions.
The source is [Copernicus DEM GLO-30](https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM).
The `obc-dem` tool converts this source to the OBCT format.

Terrain cells have a separate catalog revision from map cells.
The assembler copies selected terrain cells into the terrain region of the `.obcm` file.
The packer and device use the same `no_std` sampler.
Thus, all consumers use the same elevation values.

## Data flow

<figure class="fig">
<div class="diagram-scroll" role="region" aria-label="Diagram; scroll horizontally to see all content" tabindex="0" style="--diagram-width: 720px">
<svg viewBox="0 0 720 483" role="img" aria-label="Terrain cells contain a route sample. A query at the center of heights 100, 120, 120 and 140 metres returns 120 metres by bilinear interpolation. Sampling along the route gives an elevation profile. Contours, integrated uphill ascent, visibility and altitude correction use the same terrain.">
<defs><marker id="r68arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker></defs>
<text class="d-tag" x="20" y="26" text-anchor="start">Terrain · one lattice, shared sampling, several geometric uses</text>
<text class="d-title" x="20" y="60" text-anchor="start">Baked terrain cells</text>
<text class="d-title" x="260" y="60" text-anchor="start">Sample between four heights</text>
<text class="d-title" x="560" y="60" text-anchor="start">Route profile</text>
<rect x="25" y="82" width="34" height="34" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="59" y="82" width="34" height="34" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="93" y="82" width="34" height="34" fill="#b8cba2" stroke="#3c6b39" stroke-width="1.2" />
<rect x="127" y="82" width="34" height="34" fill="#91b378" stroke="#3c6b39" stroke-width="1.2" />
<rect x="25" y="116" width="34" height="34" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="59" y="116" width="34" height="34" fill="#b8cba2" stroke="#3c6b39" stroke-width="1.2" />
<rect x="93" y="116" width="34" height="34" fill="#91b378" stroke="#3c6b39" stroke-width="1.2" />
<rect x="127" y="116" width="34" height="34" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="25" y="150" width="34" height="34" fill="#b8cba2" stroke="#3c6b39" stroke-width="1.2" />
<rect x="59" y="150" width="34" height="34" fill="#91b378" stroke="#3c6b39" stroke-width="1.2" />
<rect x="93" y="150" width="34" height="34" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="127" y="150" width="34" height="34" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="25" y="184" width="34" height="34" fill="#91b378" stroke="#3c6b39" stroke-width="1.2" />
<rect x="59" y="184" width="34" height="34" fill="#eae4cb" stroke="#3c6b39" stroke-width="1.2" />
<rect x="93" y="184" width="34" height="34" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<rect x="127" y="184" width="34" height="34" fill="#b8cba2" stroke="#3c6b39" stroke-width="1.2" />
<path d="M30 191 Q76 139 157 121" fill="none" stroke="#cf6a2a" stroke-width="3"/>
<circle cx="103" cy="140" r="4" fill="#cf6a2a"/>
<path d="M174 150 H231" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r68arrow)"/>
<rect x="269" y="86" width="178" height="132" fill="#d5dfc6" stroke="#3c6b39" stroke-width="1.2" />
<path d="M269 152 L447 152" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M358 86 L358 218" fill="none" stroke="#9aa884" stroke-width="1.3" />
<circle cx="269" cy="86" r="4" fill="#cf6a2a"/>
<circle cx="447" cy="86" r="4" fill="#cf6a2a"/>
<circle cx="269" cy="218" r="4" fill="#cf6a2a"/>
<circle cx="447" cy="218" r="4" fill="#cf6a2a"/>
<text class="d-sub" x="269" y="80" text-anchor="start">120 m</text>
<text class="d-sub" x="447" y="80" text-anchor="end">140 m</text>
<text class="d-sub" x="269" y="240" text-anchor="start">100 m</text>
<text class="d-sub" x="447" y="240" text-anchor="end">120 m</text>
<circle cx="358" cy="152" r="5" fill="#24331c"/>
<text class="d-sub" x="358" y="177" text-anchor="middle">120 m</text>
<path d="M458 152 H516" fill="none" stroke="#3c6b39" stroke-width="1.5" marker-end="url(#r68arrow)"/>
<path d="M540 218 L698 218" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M540 218 L540 88" fill="none" stroke="#9aa884" stroke-width="1.3" />
<path d="M544 200 L567 161 L592 171 L626 116 L655 149 L694 125" fill="none" stroke="#cf6a2a" stroke-width="3"/>
<text class="d-sub" x="540" y="240" text-anchor="start">distance</text>
<text class="d-sub" x="20" y="265" text-anchor="start">obc-dem → cells → map</text>
<text class="d-sub" x="260" y="265" text-anchor="start">Integer bilinear interpolation</text>
<text class="d-sub" x="545" y="265" text-anchor="start">Heights along the route</text>
<path d="M20 290 L700 290" fill="none" stroke="#9aa884" stroke-width="1.3" />
<text class="d-title" x="20" y="319" text-anchor="start">One surface supports several geometric queries</text>
<ellipse cx="95" cy="365" rx="57" ry="28" fill="none" stroke="#9aa884" stroke-width="1.5"/>
<ellipse cx="95" cy="365" rx="39" ry="19" fill="none" stroke="#9aa884" stroke-width="1.5"/>
<ellipse cx="95" cy="365" rx="19" ry="10" fill="none" stroke="#9aa884" stroke-width="1.5"/>
<text class="d-sub" x="95" y="417" text-anchor="middle">Contour lines</text>
<path d="M207 396 L232 352 L265 379 L297 330 L331 387" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<path d="M207 396 L232 352" fill="none" stroke="#cf6a2a" stroke-width="3"/>
<path d="M265 379 L297 330" fill="none" stroke="#cf6a2a" stroke-width="3"/>
<text class="d-sub" x="269" y="417" text-anchor="middle">Integrated ascent</text>
<path d="M397 394 L425 354 L445 373 L477 337 L512 394" fill="none" stroke="#3c6b39" stroke-width="1.5" />
<path d="M397 394 L477 337" fill="none" stroke="#cf6a2a" stroke-width="1.3" />
<circle cx="397" cy="394" r="4" fill="#cf6a2a"/>
<text class="d-sub" x="455" y="417" text-anchor="middle">Peak View / visibility</text>
<text class="d-sub" x="560" y="352" text-anchor="start">Barometer + map</text>
<text class="d-sub" x="560" y="375" text-anchor="start">altitude correction</text>
<text class="d-sub" x="560" y="417" text-anchor="start">Live elevation</text>
<text class="d-sub" x="20" y="459" text-anchor="start">Missing terrain stays explicit. Each consumer applies the fallback described below.</text>
</svg>
</div>
<div class="diagram-hint" aria-hidden="true">Scroll horizontally to see the full diagram.</div>
<figcaption>The center-sample example uses equal weights. The packer and device share elevation rules; Peak View additionally uses the baked surface index.</figcaption>
</figure>

## Peak View surface data

The production terrain baker writes a geographic surface index for Peak View.
Heights are stored once and coarser levels select posts from them. Conservative
maximum-height bounds let the renderer skip hidden terrain. Baked height and gradient error
bounds also let it draw large smooth patches without visiting each small source cell.
The extra coarse levels keep those patches' corner reads close together in storage.
The data contains no stored viewpoints or images.

Peak View uses the loaded map and the current GPS position. It reads nearby named summits from
that map, projects their geographic coordinates, and checks their visibility against the terrain.
The check uses the greater of the recorded summit height and the sampled height, so a narrow summit is not rejected
just because the terrain grid samples below its top. Without a recorded height, it uses the
sampled surface. Labels stay anchored to the sampled terrain in both cases.
The renderer tests up to 64 candidates at a time. It ranks them by elevation angle above the
observer and reserves one place per 22.5° sector for the tallest summit. When the picture is
complete, it removes hidden candidates and fills their places with untested names. It can make
two more visibility passes. These passes do not change the picture.

Opening Peak View uses a GPS fix from the last 30 seconds or requests a new fix, even when no
ride is recording. It waits for that fix before it generates terrain. A cached position without
an arrival time does not count as a current fix. With a recent fix, the compass and
pending-column hatch appear in the first frame, without a waiting screen.
After acquisition, GPS can sleep if no recording needs it. The compass stays
active while Peak View is open. Leaving the screen cancels its pending position request.
Completed terrain replaces the hatch as generation progresses. Progress redraws occur at most twice per second.
The renderer prepares the current field of view first, then fills the rest of the circle in the
background. Static dots show that work remains. Turning toward an unfinished view gives that
view priority. Completed views reuse the panorama in RAM. In Live, movement above 20 m starts another
panorama after the current job finishes. Browse keeps its observer position. Leaving Peak View cancels generation and releases the arena.

The normal view spans 60° and starts 12° below the horizon. Lower angles show only the ground at
the wheel. A summit above the window opens the frame upwards and makes the view wider by the same
ratio, because the horizontal and the vertical scale must stay equal. Equal scales keep the drawn
shape of a summit correct. Live follows the compass and leaves the peak ledger empty. Select enters Browse on the visible
peak with the highest elevation angle. A step to the right enters Browse on the leftmost visible
peak; a step to the left starts on the rightmost. Each further step selects the next visible peak
in that direction without moving the view. If there is no next peak, the view turns by 15° and
selects the first peak that enters. If no peak enters, the selected peak stays selected until it
leaves the view. Further steps continue through empty areas. Reversing direction steps back
through the visible peaks. Back returns from Browse to Live; another Back leaves Peak View.
Browse keeps its selected summit identity when new names become available.

A small information mark appears in the selected peak's bottom panel when its installed article
has readable text. Select opens that article. Without readable content, Select returns to Live.
Up/Down moves through text pages and an optional photo. Select or Back leaves the photo for the
article; Back from the article restores the selected summit and Browse heading. **Down + Back →
Sources** opens the text and photo credits. There is no Visit action or landmark list in this path.
Peak articles are available only through Peak View. Ride Assistant Landmarks contains other places.

The map links an article by its original OSM summit identity. Names and coordinates do not make
article links. Text uses the UI language, then English, then the default language stored for the
place. A missing or unreadable photo leaves the text available. All content comes from the installed
map; the device does not request online content. A map change invalidates the open article and photo.

Article text and Sources retain the panorama, so Back restores Browse without rebuilding it.
The panorama and photo decoder use the same scratch arena. Opening a photo releases the panorama.
The selected summit identity, observer position and heading stay outside that arena. After a photo,
returning to Browse regenerates the panorama at that observer position.
All visible names are considered for chart labels. There is no fixed label-count limit. Higher
elevation angles take priority where names would overlap; labels keep at least 15 pixels of
horizontal space. A chart name that does not fit above its summit is shortened with `..`. The selected peak's name
appears in the ledger; a name wider than the ledger scrolls there.

The window is chosen once per observer from the catalogue elevation angles. Its bottom stays 12°
below the horizon. Its top grows upward until the highest summit and its label fit, and the
horizontal field grows with it at the chart's aspect, so one degree is the same number of pixels on
both axes. Missing height metadata keeps the base window, and a summit below the observer never
moves the bottom of the frame. Terrain and labels share the projection, while bearings, elevations,
distances and visibility stay geographic. Turning changes neither the scale nor the horizon
position. Lighting has a fixed northwest world direction, so turning does not change the shading.

The eye is 2 m above the ground the observer stands on. That ground is the map-referenced altitude
when the altimeter has settled and the value agrees with the observer's terrain cell, or the highest
corner of that cell if it does not. The four lattice nodes of the cell the rider stands in are
clamped to the rider's own height, so the ground under the rider never blocks the view and the
surface stays continuous.

The renderer requests terrain out to 100 km. Missing distant coverage is marked with dashed bearing
segments. Missing terrain at the observer or a storage read failure makes the
view unavailable. Absent geographic cells are skipped without traversing their individual samples.

At the standard posting and cell size, an indexed cell occupies 3,149,824 bytes instead of
2,097,152 bytes. Terrain itself grows about 50.2%, so a terrain-heavy selection makes the
complete map measurably larger. Crest lifts add no bytes at all.

The normal map bakery writes indexed terrain and named summit records directly. Re-bake all
published terrain and geometry cells before publication. Catalog generation rejects mixed native
and indexed terrain blocks. Standalone DEM baking produces native terrain; its surface conversion
command adds the index and changes no height.

The builder uses 57 m postings to 25 km, 114 m postings from 25 to 50 km, and 228 m postings
from 50 to 100 km. Coarse levels use exact vertices from the
native grid. A narrow summit can therefore lose apex height; its label anchor follows the same
sampled surface. Smooth open terrain can merge into large patches. Rough mountain faces require more
individual cells. Performance must therefore be checked at varied observer positions on the device.
See [OBCT section 8](src:specs/OBCT_Spec.md) for the byte layout.

## Crest lifts

A 57 m posting cannot hold a rock tower. At Engelberg the surface through Copernicus GLO-30
runs 100 m below the summit of the Hahnen, and a panorama drawn from it loses the shape that
makes the mountain recognisable.

The bakery corrects this with a **crest lift**. Where a finer national elevation model says the
ground inside a sample's own cell stands more than 10 m above our surface, **and** the ground
there is convex, the baker raises that sample to the finer model's height. The convexity test is
what keeps a steep flat face and a mountain pass unchanged: only a crest moves. The lift goes
into the baked sample, so contours, ascent, the route profile, the altimeter and Peak View all
read one surface.

Copernicus stays the base model. A national model gives lifts only, because it measures bare
ground and therefore sits below Copernicus over every forest and town.

The baker reads one archive rather than each national service. The archive holds the **highest**
ground in each 7 m square of a fixed lattice, so a rock tower one metre wide survives, and the
baker streams the squares of one cell at a time.

Coverage can stop at any sample, so a national model that stops at a border is not a problem.
A cell with no finer coverage is identical to a cell baked without one.

Each finer model keeps its own attribution, which must travel with the map. A change of finer
model changes the baked heights, so it is a new terrain revision.

## One sampling truth

The packer samples OBCT tiles to calculate navigation-edge ascent.
It also uses them to [trace contour lines](../packer-routing/#contours-traced-from-the-terrain).
The device samples the embedded raster for route heights and altimeter fusion.
Route profiles, climb detection, and elevation statistics use these route heights.

The shared [`obc-elevation`](src:firmware/obc-elevation) crate implements all sampling.
The packer does not decode GeoTIFF data.
Only `obc-dem` decodes the source DEM.

[OBCT section 5](src:specs/OBCT_Spec.md) specifies integer bilinear interpolation.
The calculation uses 64-bit integers and half-away-from-zero rounding.
Independent implementations must return the same whole-meter height.

The sampler uses half-open cell ownership at seams.
It reads a sample from the cell that owns that sample.
It clamps the last sample at the outer coverage edge.

## Terrain artifacts and map assembly

Published terrain cells use this identity tuple:

- `dataset_version`
- `posting_log2`
- `cell_log2`
- `terrain_revision`

The tuple does not contain an OBCM schema revision.
Thus, a map schema change does not require a new terrain bake.
See [OBCC section 13](src:specs/OBCC_Spec.md) for the catalog contract.

The assembler verifies each selected cell against the catalog.
It copies the cells into one OBCT container.
The assembler puts this container in the map terrain region.
The device reads terrain only from this region.

`obc-pack --terrain` has a different function.
It samples OBCT input for contours and navigation-edge ascent.
It does not put that input in its output map.

### Navigation coupling

Navigation-edge ascent depends on a terrain revision.
The catalog records this revision as `network_terrain_revision`.
The bake guard rejects a network that uses a different terrain revision.
This check keeps routing costs and route profiles on the same terrain surface.

## Raster layout and resource use

The current published raster uses these values:

| Item | Value |
| :-- | :-- |
| Posting | `2^9` microdegrees |
| Terrain cell side | `2^19` microdegrees |
| Samples per cell | 1024 × 1024 |
| Tile size | 16 × 16 samples, 512 bytes |
| Sample type | Little-endian signed 16-bit meters |
| `NODATA` value | `-32768` |
| Default tile cache | Four tiles, approximately 2.1 KiB |

The posting and cell size are OBCT header fields.
A change to either value requires a terrain bake, not an OBCT version change.
See the [OBCT specification](src:specs/OBCT_Spec.md) for all limits and byte layouts.

## Routing ascent

Each navigation adjacency stores directional `ascent_m`.
The value is the accumulated ascent along the edge polyline.
It is not the elevation difference between the endpoints.

The packer samples each edge at intervals of at most 50 m.
It applies the shared 3 m elevation dead band.
The reverse adjacency stores the ascent for the reverse direction.

The router uses this cost:

```text
cost = weighted_distance + ascent_m × climb_weight
```

A descent does not reduce the cost.
See [Weighting the climb](../packer-routing/#weighting-the-climb) for profile behavior.

## Missing terrain

The sampler returns `None` when terrain is unavailable.
It also returns `None` if a required sample is `NODATA`.
Consumers must not replace `None` with zero elevation.
Zero meters is a valid height.

For packer edge-ascent integration, `None` pauses the shared 3 m dead band.
The next valid sample starts a new segment.
Thus, a coverage gap contributes no ascent.
Valid samples on each side can still contribute ascent in their segments.

| Function | Behavior without terrain |
| :-- | :-- |
| Map rendering | Rendering continues. Baked contour geometry does not require the raster. |
| Routing | Routing continues with the graph's baked ascent values. |
| Imported GPX route | The route keeps its supplied heights. |
| Device-planned route | Missing heights stay unknown. The route records incomplete segment coverage. |
| Detour | Known seam heights can adjust sampled detour heights. Missing heights stay unknown. |
| Ride recording | The recorder stores the barometer measurement. |
| Current elevation | The UI uses the raw barometric estimate. |
| Time estimate | The model uses zero remaining ascent. |

## Coverage and `NODATA`

A bilinear query needs four sample corners.
If one corner is `NODATA`, the query returns `None`.
The sampler does not estimate a missing corner.

OBCR v4 stores missing elevation explicitly. Zero metres remains valid elevation.
An unknown point pauses ascent integration; the next valid run starts a new reference.
The graph also records whether every terrain integration sample was present.
A missing interior sample keeps an emitted segment incomplete even if its endpoints resolve.
Profiles leave these gaps empty. Interval facts report the measured coverage with the totals.

## Attribution

The data requires this attribution:

> produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all rights reserved

The source code stores this text in [`COPERNICUS_ATTRIBUTION`](src:host/obc-dem/src/lib.rs).
The bakery copies it to the catalog terrain block.
Consumers read the text from the catalog.
A map with derived contour geometry also requires this attribution.

Map data remains © OpenStreetMap contributors.

## Implementation

- OBCT contract: [`OBCT_Spec.md`](src:specs/OBCT_Spec.md)
- OBCT constants: [`obct.rs`](src:firmware/obc-formats/src/obct.rs)
- Reader and sampler: [`obc-elevation`](src:firmware/obc-elevation)
- DEM converter: [`obc-dem`](src:host/obc-dem)
- Terrain publisher: [`terrain.rs`](src:host/obc-bake/src/terrain.rs)
- Map assembly: [`terrain.rs`](src:host/obcm-assemble/src/terrain.rs)
- Edge ascent: [`nav.rs`](src:host/obc-pack/src/nav.rs)
- Route planning: [`nav.rs`](src:firmware/obc-route/src/nav.rs)
- Altimeter fusion: [`altitude.rs`](src:firmware/obc-app/src/altitude.rs)
