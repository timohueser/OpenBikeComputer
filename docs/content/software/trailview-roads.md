---
title: How TrailView draws roads
description: Why TrailView draws near roads as meshes, paints far roads, and fits each road's line in the bake.
---

# How TrailView draws roads

TrailView draws a road in two ways. Near the eye, a road is its own mesh, a corridor. Far from the
eye, a road is paint on the terrain. Both follow one line, which the feature bake fits. This page
says why. The bytes are in the [feature tile contract](src:specs/trailview-features.md).

## Why paint is not enough near the eye

Paint comes from a mask, and the terrain mesh carries it. Both have a fixed grid. Near the eye, that
grid shows: a road edge has stair steps, a surface has ripple, a curve has kinks, and a junction
looks broken. Better masks and stamps do not remove these faults, because the grid stays.

A corridor has no such grid. Its vertices lie on the road's own curve, so a kerb is a real edge and
a junction is a real shape. Paint stays correct far from the eye, where a road is thin on the screen
and a mesh costs more than it gives.

So each road, track, path and railway within about 600 m draws as a corridor
([`RANGE_M`](src:builder/web/src/lib/trailview/roads/roads.ts)), and paint draws the rest.

## One line, fitted in the bake

OSM gives a road as a chain of straight segments. A corridor needs a smooth curve with a tangent,
a grade, a bank and a widening at each point. The bake fits this curve in the same
[solve](src:host/obc-trailview-features/src/roads/align.rs) that sets the road's heights, and stores
the values at each kept vertex.

A fit in the browser is simpler to start, but it has three faults:

- Each tile fits alone, so two tiles do not agree on the line at their border.
- The metres along a road restart in each tile, so dashes jump at a tile border.
- Each tile fits again at each zoom, which costs time on each load.

With the fit in the bake, all tiles that hold a vertex hold the same values. The metres along a road
run through tiles and ways. The bake also says which way of a junction is the through road, so the
stamps, the paint and the corridors all agree. The stamp worker, the paint and the road worker only
read the line. They do not fit it again.

## The paint stays under the road

The terrain has no hole under a corridor. A hole would need the terrain to know which roads have
arrived, and a late road would then leave a gap in the ground. Without a hole, a road is never
missing: the [road worker](src:builder/web/src/lib/trailview/roads/road-worker.ts) builds meshes off
the main thread and off the stamps' worker, and terrain never waits for it. Until a mesh arrives,
the paint shows.

Corridors draw before the terrain, so the depth test hides the terrain under them.

The handover uses the same idea. The paint [fades in](src:builder/web/src/lib/trailview/terrain/features.wgsl)
with distance, up to what the corridors hold. A dither gives a share of the pixels to the paint, and
the corridors discard that share. Each pixel draws once, and the frames resolve the dither, so
the change is hard to see. What the corridors hold moves with the meshes in the pool, so
[a piece that does not draw](src:builder/web/src/lib/trailview/roads/roads.ts) gives its ground back
to the paint.

## How a corridor is built

A corridor starts from [stations](src:builder/web/src/lib/trailview/roads/stations.ts) on the curve.
At each station, a [cross-section](src:builder/web/src/lib/trailview/roads/section.ts) says what lies
across the road: carriageway, kerb, sidewalk, shoulder, then a band. The
[sweep](src:builder/web/src/lib/trailview/roads/sweep.ts) joins two stations into faces.

The terrain still takes the road's [stamps](src:builder/web/src/lib/trailview/terrain/stamp.wgsl),
which cut and fill the ground for it. The band is the rows of faces that lie on this stamped ground
beside the road. It makes the road meet the terrain with no visible step. Only the band reads the ground in the vertex stage, because only
the GPU knows the stamped height. Every other vertex has its height from the worker, which keeps the
vertex stage cheap. The worker also makes each decision once per station: a
[rail or a wall](src:builder/web/src/lib/trailview/roads/standing.ts) stands or does not stand, so
two vertices of a station cannot disagree.

## Junctions, marks and structures

The [junction plan](src:builder/web/src/lib/trailview/roads/junctions.ts) is a pure function of the
lines that a tile holds. This is what lets two tiles agree on a junction at their border. The through
pair runs on. Each other way is a side arm, whose first metres lie on the through road's banked
plane, so a mouth meets the edge of the road level. A kerb turns round each corner on an arc that fits
the curves of both ways.

A zebra comes from an OSM tag, never from a road's class. A give-way line and a lowered kerb
[follow the junction plan](src:builder/web/src/lib/trailview/roads/marks.ts). A [bridge deck](src:builder/web/src/lib/trailview/roads/decks.ts)
is a corridor too, so its markings run across with the same dash phase. The sides of a way
[stop at a river or a lake](src:builder/web/src/lib/trailview/roads/shore.ts). A stream passes under
a way on the ground: its [ribbon stops at the way's edge](src:builder/web/src/lib/trailview/terrain/water.wgsl),
and a short culvert is part of the stream.

## A region is rules, not code

Roads differ between countries: a Swiss zebra is yellow, most others are white, and a give-way line
has many forms. All of this is data. A look in `data/trailview` holds marking colours, zebra colours,
give-way forms and dash lengths as
[values by place](src:specs/trailview-tiles.md). One implementation serves all regions. A new
country needs a rule, not a branch.

## Looks

The looks of the road are [procedural](src:builder/web/src/lib/trailview/roads/looks.wgsl). Asphalt
noise and rule lookups cost little in a shader, so a baked texture atlas would add memory and bake
work and save almost no frame time. Asphalt stays plain, on purpose. Each detail fades to its mean
when a pixel spans it, so the far look keeps the colour of the paint.

A track or a trail frays into the ground beside it. The look says what share of the ground shows, and
[the corridor discards](src:builder/web/src/lib/trailview/roads/corridor.wgsl) that share of its
pixels by a dither. The terrain or the band draws those pixels, so no pixel shades the ground twice.

## Frame time and memory

Rendering speed comes first, and three choices give it.

**One mesh, three ranges.** Each [mesh](src:builder/web/src/lib/trailview/roads/mesh.ts) belongs to a
level 15 owner. Its pieces hold three index ranges over the same vertices: near, far and casters.
The far range has fewer faces. The CPU culls pieces by view and range. A test keeps the CPU's range
and the GPU's range equal.

**Own pipeline.** Corridors have their own vertex format and WGSL entry. A
[caster pipeline](src:builder/web/src/lib/trailview/roads/roads.ts) draws only what stands up: walls,
rails, posts, sleepers and steps. A flat road casts no useful shadow.

**Fixed pools.** The GPU pools have a fixed size. Near pieces come first, so far pieces go first under
pressure, and a stat counts what the pools hold back. A road that does not fit falls back to paint,
and never to a hole.

## Read next

- [Feature tiles](src:specs/trailview-features.md): the alignment, the fields and the solver.
- [TrailView tiles and rules](src:specs/trailview-tiles.md): the rule fields and the values by place.
- [TrailView README](src:builder/web/src/lib/trailview/README.md): build, shots, flights and tests.
- [Terrain and elevation](../terrain/): the terrain that the roads stand on.
