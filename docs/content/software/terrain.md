---
title: Terrain and elevation
description: How OpenBikeComputer bakes, assembles, samples, and uses terrain data.
copy: ai
---

# Terrain and elevation

One terrain raster serves every elevation function: the contours on the map, the climb on a route,
the ascent a router weighs, the altimeter, and Peak View. The source is
[Copernicus DEM GLO-30](https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM),
and [`obc-dem`](src:host/obc-dem) converts it to the OBCT format.

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

Terrain cells are baked and revised separately from map cells, and the assembler copies the
selected cells into the terrain region of the map. A map schema change therefore does not force a
new terrain bake.

## One sampling truth

The packer and the device use the same `no_std` sampler,
[`obc-elevation`](src:firmware/obc-elevation). This matters more than it sounds: the packer bakes
the ascent the router weighs, and the device measures the climb the rider sees. If the two sampled
differently, a route would promise one climb and report another.

[OBCT section 5](src:specs/OBCT_Spec.md) specifies the integer interpolation exactly, so an
independent implementation returns the same whole-meter height. Cells own their seam samples, so a
position on a cell boundary has one answer.

Only `obc-dem` decodes the source DEM. The packer never touches GeoTIFF data.

### Missing height is not zero

The sampler returns nothing when terrain is unavailable or a required corner has no data, and no
consumer may read that as zero, because zero meters is a valid height at the coast.

An unknown sample pauses ascent integration and the next valid run starts a new reference, so a
gap in coverage contributes no climb instead of a false one. A route records which of its segments
had complete coverage, and a profile leaves the gaps empty.

| Function | Behavior without terrain |
| :-- | :-- |
| Map rendering | Continues. Baked contours do not need the raster. |
| Routing | Continues, on the graph's baked ascent. |
| Imported route | Keeps the heights the source supplied. |
| Device-planned route | Heights stay unknown, and the route records incomplete coverage. |
| Ride recording | Stores the barometer measurement. |
| Current elevation | Uses the raw barometric estimate. |
| Time estimate | Treats the remaining ascent as zero. |

### Navigation coupling

The ascent stored on each navigation edge belongs to a terrain revision, and the catalog records
which revision the network was baked from. The bake refuses a network whose terrain revision does
not match, because routing costs and route profiles must describe one surface.

## Crest lifts

A 30 m grid cannot hold a rock tower. At Engelberg the Copernicus surface runs about a hundred
meters below the summit of the Hahnen, and a panorama drawn from it loses the shape that makes the
mountain recognisable.

The bakery corrects this with a **crest lift**. Where a finer national elevation model says the
ground inside a sample's own cell stands well above our surface, and the ground there is convex,
the baker raises that sample. The convexity test is what keeps a steep flat face and a mountain
pass unchanged: only a crest moves.

The sample rises by the gap between the two models, not to the highest ground in its cell. On a
slope most of that height is the cell's own fall, and lifting by it would raise the ground all
around a summit. The highest measured ground stays the ceiling, so a lift never invents height.

Copernicus stays the base model, and a national model can only lift: it measures bare ground, so it
sits below Copernicus over every forest and town. Coverage may stop at any sample, so a model that
ends at a border is not a problem, and a cell with no finer coverage is identical to a cell baked
without one.

The lift goes into the baked sample, so the contours, the ascent, the route profile, the altimeter,
and Peak View all keep reading one surface. A change of finer model is therefore a new terrain
revision, which also re-bakes the navigation graph. Only the cells the changed source reaches are
baked again.

Each finer model keeps its own attribution, and it must travel with the map. See
[Attribution](#attribution).

## Peak View

Peak View draws the horizon around the rider and names the summits on it. It uses the loaded map
and the current position: the named summits come from the map's own summit category, and their
visibility is tested against the terrain.

The bakery writes a surface index for this one purpose. Coarse height levels and conservative
height bounds let the renderer skip terrain that cannot be visible and draw large smooth areas
without visiting every sample. There are no stored viewpoints and no stored images: the panorama is
computed where the rider stands.

A visibility test uses the greater of the summit's recorded height and the sampled surface, so a
narrow peak is not rejected because the grid samples below its top. The four lattice corners under
the rider are clamped to the rider's own height, so the ground the rider stands on never blocks the
view.

Candidates are ranked by their angle above the horizon, and one place is reserved per sector for
the tallest summit there, so a single high massif cannot take every label.

The view is 60° wide and starts below the horizon. A summit above the window opens the frame
upward and widens it by the same ratio, because the horizontal and vertical scales must stay equal
or the drawn shape of a mountain is wrong. Lighting has a fixed world direction, so turning changes
neither the scale nor the shading.

Opening Peak View needs a recent fix, and it asks for one even when no ride is recording. The
renderer prepares the current view first and fills the rest of the circle in the background, and a
hatch shows what is not drawn yet. Turning toward an unfinished view gives it priority.

**Live** follows the compass. **Select** enters **Browse** on the most prominent visible peak, and
further steps walk the visible peaks, turning the view only when there is no next peak. Where the
map carries content for a summit, a mark appears. Content is text, a photo, or both: Select opens
the text pages and the photo, and a summit with a photo and no text opens on the photo. The photo
has to help the rider recognise the mountain, so the bakery prefers a picture taken at a distance
and puts a picture taken on the summit last.
**Down + Back** opens the sources, because credits must travel with the content. An article is
linked by the summit's OSM identity alone: a name and a coordinate cannot prove that an article
describes that summit.

Peak articles are reached only through Peak View. Other places are in Ride Assistant landmarks.

## Terrain in the map

The assembler verifies each selected cell against the catalog, copies the cells into one container,
and puts that container in the map's terrain region. The device reads terrain from there and from
nowhere else.

`obc-pack --terrain` is a different operation: it samples terrain input to trace
[contours](../packer-routing/#contours-traced-from-the-terrain) and to integrate
[edge ascent](../packer-routing/#weighting-the-climb). It does not copy that input into its output
map.

The posting and the cell size are header fields, so changing them is a re-bake and not a format
version. See the [OBCT specification](src:specs/OBCT_Spec.md) for the limits and the byte layout,
and [OBCC section 13](src:specs/OBCC_Spec.md) for the catalog contract.

## Attribution

The data requires this attribution:

> produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all rights reserved

The text is stored once, in
[`COPERNICUS_ATTRIBUTION`](src:host/obc-dem/src/lib.rs), and the bakery copies it into the catalog
terrain block, so every consumer reads it from the catalog and none of them hard-codes it. A map
with terrain-derived contours needs it too.

A map with [crest lifts](#crest-lifts) also carries the attribution of each finer model it used.
The catalog lists those models with their required credit and license, copied from the reference
archive that holds the wording. A consumer that shows one shows all of them. The map builder shows
them on the map summary card.

Map data remains © OpenStreetMap contributors.

## Implementation

- OBCT contract: [`OBCT_Spec.md`](src:specs/OBCT_Spec.md)
- Reader and sampler: [`obc-elevation`](src:firmware/obc-elevation)
- DEM converter: [`obc-dem`](src:host/obc-dem)
- Terrain publisher: [`terrain.rs`](src:host/obc-bake/src/terrain.rs)
- Map assembly: [`terrain.rs`](src:host/obcm-assemble/src/terrain.rs)
- Edge ascent: [`nav.rs`](src:host/obc-pack/src/nav.rs)
- Altimeter fusion: [`altitude.rs`](src:firmware/obc-app/src/altitude.rs)
