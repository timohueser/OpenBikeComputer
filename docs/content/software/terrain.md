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
<svg viewBox="0 0 720 420" role="img" aria-label="The terrain pipeline in three bands. Top band, left to right: Copernicus GLO-30 float32 GeoTIFF tiles are resampled by the host tool obc-dem into terrain cells of 2 to the 19 microdegrees, published as .obcd objects on their own revision track; a selection's cells are then placed by the assembler into one terrain region, spliced into the map file's own tail. Middle band, spanning the full width: obc-elevation, the single implementation of the OBCT section 5 sampling rules — integer bilinear over a four-slot 512-byte tile cache. Bottom band, three consumers fed from that one sampler: the packer integrating per-edge ascent into the OBCM section 8.3 nav graph at bake time; the device's route emit filling each OBCR point's height when it plans a route; and the live altimeter fusion that turns the barometer's relative reading into an absolute elevation. A footer states that in a map with no terrain every one of those three answers no height here, and nothing else changes.">
  <defs>
    <marker id="aT1" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#3c6b39" /></marker>
    <marker id="aT2" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10 z" fill="#cf6a2a" /></marker>
  </defs>
  <text class="d-tag" x="20" y="22">one surface, baked once — then read by everything</text>

  <!-- band 1: bake -->
  <text class="d-sub" x="24" y="42" style="font-size:9px;fill:#6b7758">① bake — on a host, once per dataset release</text>
  <rect class="d-panel-2" x="24" y="50" width="136" height="56" rx="9" />
  <text class="d-label" x="38" y="70" style="font-size:10.5px">Copernicus GLO-30</text>
  <text class="d-sub" x="38" y="86" style="font-size:8.5px">float32 GeoTIFF, 1&#8243;</text>
  <text class="d-sub" x="38" y="98" style="font-size:8.5px">tiled · DEFLATE</text>

  <line class="d-flow" x1="162" y1="78" x2="190" y2="78" marker-end="url(#aT1)" />
  <rect x="194" y="50" width="132" height="56" rx="9" style="fill:#f8efe4;stroke:#cf6a2a;stroke-width:2" />
  <text class="d-label" x="208" y="70" style="fill:#a9501c;font-size:10.5px">obc-dem</text>
  <text class="d-sub" x="208" y="86" style="font-size:8.5px">resample · flip rows</text>
  <text class="d-sub" x="208" y="98" style="font-size:8.5px">round half away from 0</text>

  <line class="d-flow" x1="328" y1="78" x2="356" y2="78" marker-end="url(#aT1)" />
  <rect class="d-water" x="360" y="50" width="150" height="56" rx="9" stroke="#3c6b39" stroke-width="1.2" />
  <text class="d-label" x="435" y="70" text-anchor="middle" style="fill:#fff;font-size:10.5px">terrain cells</text>
  <text class="d-sub" x="435" y="86" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">2&#185;&#8313; &#181;deg · 1024&#178; samples</text>
  <text class="d-sub" x="435" y="98" text-anchor="middle" style="fill:#dfe6e0;font-size:8.5px">one .obcd per square</text>

  <line class="d-flow" x1="512" y1="78" x2="540" y2="78" marker-end="url(#aT1)" />
  <rect class="d-panel" x="544" y="50" width="152" height="56" rx="9" />
  <text class="d-label" x="620" y="70" text-anchor="middle" style="font-size:10.5px">terrain region</text>
  <text class="d-sub" x="620" y="86" text-anchor="middle" style="font-size:8.5px">spliced into the map</text>
  <text class="d-sub" x="620" y="98" text-anchor="middle" style="font-size:8.5px">OBCM §1.3, at its tail</text>

  <text class="d-sub" x="24" y="133" style="font-size:8.5px;fill:#a9501c">own revision track — an OBCM bump republishes none of it</text>

  <!-- band 2: the sampler -->
  <line class="d-flow" x1="435" y1="108" x2="435" y2="152" marker-end="url(#aT1)" />
  <line class="d-flow" x1="620" y1="108" x2="620" y2="152" marker-end="url(#aT1)" />
  <rect x="24" y="156" width="672" height="58" rx="11" style="fill:#f8efe4;stroke:#cf6a2a;stroke-width:2.4" />
  <text class="d-tag" x="40" y="176" style="fill:#a9501c">② sample — obc-elevation, the only implementation of OBCT §5</text>
  <text class="d-sub" x="40" y="196" style="font-size:9.5px">integer bilinear · half-open cell ownership, cross-cell fetch at a seam, clamp at the coverage edge</text>
  <text class="d-sub" x="40" y="208" style="font-size:9.5px">4 × 512 B tile cache &#183; a <tspan style="font-weight:700">NODATA</tspan> corner voids the whole query — never a guessed height</text>

  <!-- band 3: consumers -->
  <line class="d-flow" x1="130" y1="216" x2="130" y2="256" marker-end="url(#aT2)" stroke="#cf6a2a" />
  <line class="d-flow" x1="360" y1="216" x2="360" y2="256" marker-end="url(#aT2)" stroke="#cf6a2a" />
  <line class="d-flow" x1="590" y1="216" x2="590" y2="256" marker-end="url(#aT2)" stroke="#cf6a2a" />
  <text class="d-sub" x="24" y="248" style="font-size:9px;fill:#6b7758">③ three consumers</text>

  <rect class="d-panel" x="24" y="260" width="212" height="96" rx="10" />
  <text class="d-label" x="40" y="280" style="font-size:10.5px">pack time — obc-pack</text>
  <text class="d-sub" x="40" y="298" style="font-size:9px">walks each edge's polyline,</text>
  <text class="d-sub" x="40" y="311" style="font-size:9px">samples at most 50 m apart,</text>
  <text class="d-sub" x="40" y="324" style="font-size:9px">integrates through the dead-band</text>
  <text class="d-sub" x="40" y="343" style="font-size:8.5px;fill:#a9501c">→ 2 B per direction, OBCM §8.3</text>

  <rect class="d-panel" x="254" y="260" width="212" height="96" rx="10" />
  <text class="d-label" x="270" y="280" style="font-size:10.5px">route emit — on device</text>
  <text class="d-sub" x="270" y="298" style="font-size:9px">fills every OBCR point's height,</text>
  <text class="d-sub" x="270" y="311" style="font-size:9px">densifying to 250 m so a crest</text>
  <text class="d-sub" x="270" y="324" style="font-size:9px">between two vertices can't hide</text>
  <text class="d-sub" x="270" y="343" style="font-size:8.5px;fill:#a9501c">→ profile · climbs · stats · GPX</text>

  <rect class="d-panel" x="484" y="260" width="212" height="96" rx="10" />
  <text class="d-label" x="500" y="280" style="font-size:10.5px">live — altimeter fusion</text>
  <text class="d-sub" x="500" y="298" style="font-size:9px">map − barometer at each fix,</text>
  <text class="d-sub" x="500" y="311" style="font-size:9px">low-passed over ~5 minutes:</text>
  <text class="d-sub" x="500" y="324" style="font-size:9px">that difference is the offset</text>
  <text class="d-sub" x="500" y="343" style="font-size:8.5px;fill:#a9501c">→ absolute Current Elevation</text>

  <!-- footer -->
  <rect class="d-panel-2" x="24" y="374" width="672" height="32" rx="8" />
  <text class="d-sub" x="360" y="394" text-anchor="middle" style="font-size:9.5px">a map with no terrain → all three answer <tspan style="font-weight:700">&#8220;no height here&#8221;</tspan>, and nothing else in the system changes</text>
</svg>
<figcaption>The bakery publishes OBCT cells. The assembler puts the selected cells in the map. One sampler supplies all elevation consumers.</figcaption>
</figure>

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
| Device-planned route | Before the first valid sample, the route uses zero heights. After that sample, it carries the last valid height across gaps. |
| Detour | The splice interpolates between the seam elevations. |
| Ride recording | The recorder stores the barometer measurement. |
| Current elevation | The UI uses the raw barometric estimate. |
| Time estimate | The model uses zero remaining ascent. |

## Coverage and `NODATA`

A bilinear query needs four sample corners.
If one corner is `NODATA`, the query returns `None`.
The sampler does not estimate a missing corner.

Route elevation parity requires terrain coverage for the complete route.
OBCR has no per-point unknown-height value.
Points before coverage starts use zero height.
The route integrator does not use these placeholder points as an ascent anchor.
The packer applies the pause rule at coverage boundaries and `NODATA` gaps.
Device route filling carries the last valid height after coverage starts.
A resumed sample can add ascent from that carried height.

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
