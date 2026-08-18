---
title: Terrain & elevation
description: How ground height reaches the device — the Copernicus GLO-30 bake, the OBCT raster carried beside the map, and the three consumers (routing cost, route profiles, altimeter reference) that all read one surface.
---

# Terrain & elevation

For most of this project's life the map knew where a road went and not how much it
climbed. That is a strange gap on a device built for bikepacking: routing picked the
cheapest line under your bike profile without noticing it crossed a 400 m col;
a route the device planned itself exported with `<ele>0</ele>` on every point, so its
Climb screen was dead and its profile a flat line; and the barometer, having no
absolute reference anywhere in the system, could only ever be trusted for
*differences*.

Closing that gap needed one new thing and exactly one: **a raster of ground heights,
carried beside the map**. This page is the argument for its shape — why it is its own
file rather than a section of the map, why every consumer samples the same bytes, and
what still works when a card carries no terrain at all.

## The pipeline, end to end

<figure class="fig">
<svg viewBox="0 0 720 420" role="img" aria-label="The terrain pipeline in three bands. Top band, left to right: Copernicus GLO-30 float32 GeoTIFF tiles are resampled by the host tool obc-dem into terrain cells of 2 to the 19 microdegrees, published as .obcd objects on their own revision track; a selection's cells are then placed by the assembler into one terrain shard carried on the card, spliced into the map file's own terrain region since OBCM v14 (previously MS-id.OBD inside a volume set, or an .obcd sidecar beside a single-file map). Middle band, spanning the full width: obc-elevation, the single implementation of the OBCT section 5 sampling rules — integer bilinear over a four-slot 512-byte tile cache. Bottom band, three consumers fed from that one sampler: the packer integrating per-edge ascent into the OBCM section 8.3 nav graph at bake time; the device's route emit filling each OBCR point's height when it plans a route; and the live altimeter fusion that turns the barometer's relative reading into an absolute elevation. A footer states that with no terrain file every one of those three answers no height here, and nothing else changes.">
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
  <text class="d-label" x="620" y="70" text-anchor="middle" style="font-size:10.5px">terrain shard</text>
  <text class="d-sub" x="620" y="86" text-anchor="middle" style="font-size:8.5px">MS7.OBD in a set</text>
  <text class="d-sub" x="620" y="98" text-anchor="middle" style="font-size:8.5px">or a .obcd sidecar</text>

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
  <text class="d-sub" x="360" y="394" text-anchor="middle" style="font-size:9.5px">no terrain file → all three answer <tspan style="font-weight:700">&#8220;no height here&#8221;</tspan>, and nothing else in the system changes</text>
</svg>
<figcaption>Four stages and one hinge. <b>obc-dem</b> turns the public DEM into cells on the map grid; the bakery publishes them under their <b>own revision</b>; the assembler <b>places</b> the selected squares — copying blocks, grafting nothing — into one raster on the card. Everything below that hinge — the packer's baked ascent, the device's emitted route heights, the live altimeter reference — goes through <b>one</b> sampler over <b>one</b> artifact. The arrows never run the other way: nothing about the map is an input to a terrain bake.</figcaption>
</figure>

## One sampling truth

The property worth designing for is not "elevation exists" but **that every number
derived from it agrees**. A rider plans a route, reads `▲394 m` on the overview,
watches the profile band draw a pass, and — without ever being told — assumes the
router priced that climb with the same metres. If the router had integrated a DEM at
one resolution while the profile drew from another, those numbers would differ by
tens of metres and nobody could say which was wrong.

So the rule is stated once and obeyed everywhere: **the terrain cells are baked
first, and everything downstream reads them.**

- `obc-pack` samples **baked OBCT tiles** when it integrates per-edge ascent, and
  again when it [traces contour lines](../packer-routing/#contours-traced-from-the-terrain)
  out of them. It does *not* read a GeoTIFF; it has no DEM decoder at all and gains
  no native dependency (libGEOS stays the last one).
- The device samples the same tiles at route emit and at every GPS fix.
- The host tools, the browser and the firmware run the *same* `no_std` crate —
  [`obc-elevation`](src:firmware/obc-elevation) — over the *same* bytes.

That reduces agreement from a testing problem to an arithmetic one, which is why
[`OBCT_Spec.md` §5](src:specs/OBCT_Spec.md) is written as integer arithmetic with no
floating point anywhere: the interpolation is a 64-bit weighted sum divided by `P²`
with half-away-from-zero rounding, so two independent implementations must produce
**bit-identical** heights or one of them is wrong. It is the raster analogue of the
seam rule that makes two neighbouring map cells agree about a road.

The one seam that could still have drifted is a *cut* edge: the packer slices a way
at a cell border, and each stub books its own climb. Because the lattice is global —
not per-cell — both stubs sample the same surface at the same points, and their
booked climbs sum to the uncut way's within the dead-band's re-anchoring cost. That
is a test, not a hope.

## Why terrain is not a section of the map

The obvious design is a raster section inside OBCM, next to POIs and the nav graph.
It was rejected on four counts, and all four are properties of the *data* rather than
preferences.

**Terrain is static; OpenStreetMap churns.** The cell store re-bakes on every OBCM
version or schema-revision bump — that lockstep is what makes
[assembly a byte copy](../formats/#the-alignment-trick). The DEM underneath terrain
is re-released every few *years*. Fold the raster into that lockstep and a routine
schema tweak republishes hundreds of megabytes of byte-identical height data, and
every rider re-downloads it. Outside it, each store moves when its own inputs move
and never otherwise.

**Blast radius.** As a separate artifact class, `obc-reader`, `obcm_diff`,
`obcm-testkit` and the assembler's graft path never learn a raster exists. The format
surface every existing consumer parses is unchanged.

**Splittability.** A raster splits by bounding box trivially, so it never spent the
[core file's headroom](../formats/#one-map-several-files) — the scarcest resource in a
volume set, because the nav graph was the one component that *could not* be split by box. (Volume
sets are superseded by OBCM v14,
[#1420](https://github.com/timohueser/OpenBikeComputer/issues/1420): a map is one file, and an
assembly's raster is spliced **into** it rather than carried beside it. The property still holds; it
is no longer paying for anything.)

**Independence.** A terrain re-bake at a new posting does not touch the map; a map
re-bake does not touch the raster.

The catalog carries that independence as an explicit contract: a terrain block naming
`(dataset_version, posting_log2, cell_log2, terrain_revision)`, and **that tuple is
the terrain store's entire lockstep**. The pinned terrain index deliberately carries
no `schema_revision` at all — a terrain cell does not know which map schema it will be
used beside, and a field naming one would make an OBCM bump rewrite the document. The
normative rules are [`OBCC_Spec.md` §13](src:specs/OBCC_Spec.md); the shape of the
published objects is on the [data formats](../formats/#the-catalog-the-map-builders-source-of-truth)
page.

### The one coupling, and its guard

Independence runs in one direction only, and the place it does not is worth naming.
The routing band's cells are baked **sampling** the raster: each edge's `Ascent M` is
an integral over that surface. Those bytes are therefore a function of a particular
terrain revision, so the catalog root records which one (`network_terrain_revision`),
and the bake guard **fails a publish** whose cells name an older revision than the
terrain beside them.

Without that check, a terrain re-bake would leave the router costing climbs from one
surface while the device drew its profile from another — with every file still
parsing perfectly, every digest still verifying, and no symptom a rider could
describe. It is exactly the class of bug a lockstep exists to make impossible.

## What it costs

| | |
| :-- | --: |
| Terrain at the shipped posting | ≈ 0.90 MiB per 1000 km² |
| …as a share of a whole map | **+4.4–6.7 %**, in its own file |
| Per-edge ascent in the nav graph | 24–130 KB per 1000 km² (alpine → dense) |
| …as a share of the core file | ≤ +1.9 % (≤ +0.65 % of a whole map) |
| **Total** | **≈ +5–7 %** against an agreed 20 % ceiling |
| DACH (~482 000 km²) | ≈ 430 MiB of terrain objects, baked once per dataset version |
| Device RAM | < 4 KB resident — a 32-byte header, a 4 × 512 B tile cache, one memoized directory entry |
| Emit I/O | ~120–150 tile reads per 100 km of route, with strong locality |

A finer `2^8` posting (≈ 28 m) was measured and rejected: it costs +17–26 % of a whole
map for gradient detail below the ~100 m scale a cyclist can act on. The remaining
headroom under the ceiling is deliberately reserved rather than spent.

The one number that moved a *hard* limit is the nav graph's: two bytes per adjacency
entry pull the graph-alone 4 GiB ceiling from roughly 640–700 thousand km² down to
630–690 thousand — still comfortably past DACH, and the documented escape hatch
(sharding the nav graph) is unchanged and still unused.

> **That ceiling is gone.** [OBCM v14](src:specs/OBCM_Spec.md)
> ([#1420](https://github.com/timohueser/OpenBikeComputer/issues/1420)) scales every global offset
> to 16-byte units and re-addresses the edge pool by `(chunk, ordinal)`, so a map — nav graph
> included — reaches **64 GiB** rather than 4 GiB, and the flat store replaced the FAT32 file cap
> underneath it. The two bytes per adjacency entry still cost what they cost; what they no longer
> eat into is a limit at country scale. Sharding the nav graph was the escape hatch from a 4 GiB
> file and is now unnecessary rather than merely unused: geometry, POIs and the graph share one
> 64 GiB interior with no sub-region ceiling under it.

## What the router does with it

The nav graph learned exactly two fields, and the interesting one is a design
constraint disguised as a data type. `Ascent M` is **directional** — the entry
`a→b` carries the climb of riding toward `b`, and `b→a` carries what is the first
direction's descent — and it is an **integral along the polyline, never an endpoint
difference**: a pass road between two junctions at the same height has hundreds of
metres of climbing in it and no net change at all.

It lives in the adjacency entry rather than in the edge pool because relaxation reads
exactly that record and nothing else, so climb-awareness costs the router **zero
extra reads**. The full argument — the cost formula, why a descent may never buy a
discount, and what ε now bounds — is on the
[packer & routing](../packer-routing/#weighting-the-climb) page; the byte layout is on
the [data formats](../formats/#the-navigation-graph-a-routable-network) page.

## The degrade ladder

Terrain is an enhancement, and "removable" is a property that has to be *stated per
feature* or it is just a hope. The sampler sits behind a one-method seam whose null
implementation answers `None` for everything, and that implementation is pinned to
reproduce the pre-terrain behaviour byte for byte — a device-planned route emitted
with it has the same OBCR digest it had before any of this existed.

| Without a terrain file | What happens |
| :-- | :-- |
| Map rendering | Unchanged. Nothing in the render path reads the raster. |
| Routing | Works. The map's baked ascents are `0`, so the climb term vanishes and the router costs exactly as it did before — the same result a climb weight of `0` gives. |
| Imported GPX routes | Unchanged: they carry their own heights, so profile, climbs, stats and export were never affected. |
| Device-planned routes | Plan and ride normally, with a **flat** profile: heights store as `0`, the Climb screen finds no climb, the ascent stat is `0`. |
| [Detours](../ui/#detouring-around-a-blocked-stretch) | Plan and splice normally. The spliced detour's heights fall back to a straight interpolation between the two seam elevations — byte-identically to what the splice wrote before terrain existed — and the preview's climb figure reads `--` instead of inventing one. |
| Ride recording | Unchanged. The recorded track's elevations are the *barometer's* own measurement and are deliberately never fused. |
| Current Elevation tile | Reads exactly what it read before — the raw barometric estimate. The fusion never settles, and the tile never claims a precision it does not have. |
| Estimated time / time-to-go | Still shown. The ascent-to-go is zero, so the model collapses to distance ÷ speed rather than special-casing anything. |

There is no "is there elevation?" branch anywhere downstream. Every one of those rows
is what falls out of the sampler answering `None`, which is what makes the claim
checkable instead of aspirational.

## Coverage edges, honestly

Two boundaries deserve stating rather than glossing.

**A hole is silence, never a guess.** If any of the four corners a query interpolates
between is `NODATA`, the whole sample is `None` — no partial interpolation over the
survivors, no nearest-neighbour substitution. A `NODATA` region is typically water or
radar shadow, exactly where a fabricated height would be most confidently wrong.
Consumers must treat `None` as "no height here" and must never substitute `0`; zero
metres is a real elevation. The one place a neighbouring sample *is* used is the outer
edge of coverage, where the surface flattens over the last half posting instead of
dropping to nothing — the same thing every texture sampler does, and what stops a
route grazing the boundary from developing a one-posting notch.

**Route parity holds inside coverage.** A device-planned route's climb totals are
computed with the same dead-band integrator, at the same ±3 m threshold, that the GPX
converter runs over an imported track — so planning a route, exporting it and
re-importing it agrees. The exception is real and worth naming: OBCR has no per-point
"unknown", so a route whose *opening* points fall outside coverage stores `0` for
them. The route's own stats are right (the integrator does not run until coverage
begins — pushing that placeholder would anchor the band at sea level and book the
entire first real height as ascent, a bug that shipped +1 412 m of phantom climb in
testing before it was caught). But an export of such a route re-imports with a
`0 → first-real-height` step that the converter's dead-band *will* book. Parity
therefore holds for a route lying **wholly inside coverage**, which is every route on
a map whose terrain was baked for it. The honest fix for the exception is a terrain
file that covers the map's graph — never a fabricated height.

## Attribution

The elevation data is Copernicus DEM GLO-30, and its licence requires a credit
wherever the data have been adapted — which a resample to a different lattice
certainly is:

> produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and
> Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all
> rights reserved

That string has exactly one home in the code
([`obc_dem::COPERNICUS_ATTRIBUTION`](src:host/obc-dem/src/lib.rs)), from which the
bakery stamps it into the catalog's terrain block. Every consumer that shows it —
the map builder's download summary, beside the raster's own size — reads it **from
the catalog** rather than hard-coding it, so a change of dataset carries its own
notice with it instead of leaving a stale credit behind. The bake tool also prints it
at the end of every run, so nobody producing cells can fail to have seen it.

The obligation follows the *derivation*, not the file type. A map with
[traced contours](../packer-routing/#contours-traced-from-the-terrain) carries
GLO-30-derived geometry in its own bytes, so the packer states the same credit, from
the same `const`, whenever a run packs any.

Map data remains © OpenStreetMap contributors.

---

## Where this lives

- The normative byte contract and the exhaustive sampling rules: [`OBCT_Spec.md`](src:specs/OBCT_Spec.md); its code authority for magic, offsets and sentinels: [`obc-formats/src/obct.rs`](src:firmware/obc-formats/src/obct.rs)
- The reader, the sampler, the tile cache, the shared dead-band and the `ElevationSource` seam: [`obc-elevation`](src:firmware/obc-elevation)
- The rasteriser — GeoTIFF decode, the mosaic, the deterministic bake, and the one OBCT container writer: [`obc-dem`](src:host/obc-dem)
- The bakery stage that publishes terrain cells: [`obc-bake/src/terrain.rs`](src:host/obc-bake/src/terrain.rs); the catalog contract it fills: [`OBCC_Spec.md`](src:specs/OBCC_Spec.md) §13
- The assembler's placement pass and its verify-before-write gate: [`obcm-assemble/src/terrain.rs`](src:host/obcm-assemble/src/terrain.rs)
- Per-edge ascent at pack time: [`obc-pack/src/nav.rs`](src:host/obc-pack/src/nav.rs); the climb-aware relaxation: [`obc-route/src/nav.rs`](src:firmware/obc-route/src/nav.rs)
- The altimeter fusion filter: [`obc-app/src/altitude.rs`](src:firmware/obc-app/src/altitude.rs); the gradient-aware time model: [`obc-route/src/eta.rs`](src:firmware/obc-route/src/eta.rs)

For the raster's bytes in the same guided-tour form the map and route get, see
[data formats](../formats/#obct-the-terrain-raster). For what the climb weight does to
a plan, see [packer & routing](../packer-routing/#weighting-the-climb). For the screens
this lit up, see [the UI system](../ui/#climbs-get-their-own-panel).
