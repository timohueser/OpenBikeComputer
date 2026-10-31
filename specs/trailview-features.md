# TrailView feature tiles

The archive of the layer `planner/trailview-features`: the OSM features that TrailView draws, with
their heights. The kind table [`data/trailview/kinds.json`](../data/trailview/kinds.json) selects
the features and the tags that the tiles keep.

- Version: `features_format` 3. A reader reads only the format that it knows.
- Writers: the bake `host/obc-trailview-features`. Its step reads the OSM of `planner/osm` and the
  master terrain of the grid of `planner/terrain` in the same release, and the water polygons of
  the source `water-polygons`.
- Readers: the TrailView engine (`builder/web/src/lib/trailview/stream/features.ts`),
  `tools/planner_verify.py`.
- Test vectors: [`vectors/trailview-features.json`](vectors/trailview-features.json) and
  [`vectors/trailview-features.bin`](vectors/trailview-features.bin): one tile in its decoded form
  and its bytes. The encoder of the bake and the two decoders of the readers read both.
- Related: [obc-data.md](obc-data.md), [TrailView tiles](trailview-tiles.md).

## Archive

The archive is PMTiles v3 with tile type 0 (unknown), gzip tile compression and XYZ tile
addresses at zooms 8 to 13. The grid step packs each tile into the archive of its zoom 11
parent, or of its own tile below zoom 11 ([`obc-data.md`](obc-data.md)).

| Metadata | Value |
| --- | --- |
| `features_format` | 3 |
| `extent` | 16384: tile units across a tile, about 0.2 m at zoom 13 in the Alps |
| `buffer_m` | 40: the buffer on the ground, in metres |
| `attribution` | The credit of OpenStreetMap |
| `bounds` | West, south, east and north of the region |

Zoom 13 holds every active kind. Zooms 8 to 12 hold the kinds whose `min_zoom` is that zoom or
lower. A tile that meets `bounds` and that the archive does not hold has no features.

### Buffer

A tile holds each feature that comes within the buffer of the tile square, cut to the square
grown by the buffer. In tile units, the buffer is `buffer_m` on the ground at the tile edge
nearer the pole, so it is at least `buffer_m` at every point of the tile. 40 m holds the cut
and fill cone of a road up to 30 m wide, which reaches 25 m past the road edge.

The cut makes no vertex. A piece of a line keeps the segments that meet the buffered square whole,
and one more vertex at each end, so that both tiles know the bend at each end of such a segment.
A ring keeps each edge that crosses a side of the square whole, and joins the vertices outside
the side directly. Such a join lies outside the square. Thus inside the buffered square, the
tiles that hold a feature hold the same vertices and edges, and a stamp near a tile edge gets
the same answer in both tiles.

A tile owns the points from 0 to less than 16384 on each axis: a
reader that draws or places features uses only the part of a feature in that square, so a point
on a shared edge or in the buffer of two tiles is drawn once.

### Generalization

Zoom 13 keeps each vertex, but on a way that the road solver aligns (see Alignment). Below zoom
13, the bake simplifies each line and ring with Douglas-Peucker to `extent / 2048` tile units, and
the features have no alignment: a reader draws the chords, which lie on the curve at each vertex. A
vertex that two features share stays, and so do the ends of a line. Then each edge of an area that
crosses another edge of the same feature gets back the vertex at which Douglas-Peucker would
split it next, until no two edges cross. Thus a ring that does not cross itself in OSM does not
cross itself after the simplification, and an island stays inside its lake. The bake simplifies a
feature once for each zoom, before it cuts the feature into tiles, so neighbouring tiles hold the
same line.

The cut of a ring can leave edges outside the buffered square, between vertices of the feature.
After the cut, the bake rounds each vertex to tile units, which moves it by at most half a unit,
and removes repeated vertices. A line part with fewer than 2 vertices goes. A ring with
fewer than 3 vertices or with no area goes; when an outer ring goes, its holes go too.

## Tile

A tile, after gzip decompression, is a sequence of varints and strings. A varint is an unsigned
LEB128 number. A signed value `n` is stored as the varint of its zigzag code: `2n` for `n ≥ 0`,
`−2n − 1` for `n < 0`.

```
tile    = varint string_count, string × string_count, varint feature_count,
          heads, tags, parts, vertices, along, tangent, grade, bank, words
string  = varint byte_length, UTF-8 bytes
heads   = (varint kind, varint header) per feature
tags    = (varint tag_count, (varint key, varint value) × tag_count) per feature
parts   = varint (vertex_count × 2 + outer) per part
vertices = (zigzag dx, zigzag dy, then varint z_code when has_z is 1) per vertex
along   = (varint 0 for a part with no alignment, or varint along_code) per part of a feature with
          has_plan 1
tangent, grade, bank = zigzag value per vertex of a part with an alignment
words   = varint word per vertex of a part with an alignment
header  = part_count × 16 + has_plan × 8 + has_z × 4 + geometry
```

Each stream holds one field in the order of the features, their parts and their vertices, so gzip
finds the runs of that field. A stream has no length: the streams before it give its count.

`kind`, `key` and `value` are indices from 0 into the strings of the tile. The tags of a feature are
sorted by key. `geometry` is 0 for a point, 1 for a line and 2 for an area. The features of a tile
come in a fixed order: first the kinds that no solver sets, in the order of the OSM file, the points
and the lines before the areas; then the buildings, the lifts, the rivers and lakes, the roads,
paths and tracks, the bridge piers, the catenary masts, the platforms and the sea. So the bytes of a
tile do not depend on how much of the file lies outside the region.

### Coordinates

Coordinates are Web Mercator tile units with the origin at the north-west corner of the tile, x
to the east and y to the south. The tile square is 0 to 16384 on both axes. A coordinate in the
buffer is below 0 or above 16384. For tile column `c` and row `r` at zoom `z`, a point with world
coordinates `wx`, `wy` in [0, 1] has `x = round(wx × 2^z × 16384) − 16384c` and
`y = round(wy × 2^z × 16384) − 16384r`, so neighbouring tiles round a point the same way. A feature
with `has_plan` 1 counts in quarters of a tile unit: `x = round(wx × 2^z × 65536) − 65536c`, and the
same for `y`. A tile unit would move a smooth curve by up to 10 cm at each vertex. A vertex that such
a feature shares with a river or a stream lies on a whole tile unit, so the two hold one point.

`dx` and `dy` are the difference from the vertex before, across the parts of the feature. The
first vertex of a feature is the difference from (0, 0).

### Parts

- A point has one part. Each vertex of the part is a point.
- A line part is a polyline of at least 2 vertices. A cut line has a part for each piece.
- An area part is a ring of at least 3 vertices. The ring does not repeat its first vertex.
  `outer` 1 starts a polygon, and the rings after it up to the next outer ring are its holes.
  The winding of a ring has no meaning. `outer` is 0 for points and lines.

### Heights

The solvers of the bake set the heights of a feature. A feature with no height has `has_z` 0.

Each vertex of a `main-road`, `road`, `path`, `railway`, `tunnel` or `gallery` has the height of the
road surface or of the rails, except a `via_ferrata`, which has none. The class table
[`data/trailview/roads.json`](../data/trailview/roads.json) gives the rules per class: the steepest
grade and a smoothing length. A track is a way with a `railway` of `rail`, `narrow_gauge`,
`light_rail` or `funicular` and no `highway`. The class of a track is its `railway`, or `rack` when
it has a `rack` tag other than `no`; the class of another way is its `highway`. The table lists the
tracks first, so at a level crossing the road meets the track. The options of the step hold the
grade, the smoothing length and the fields of the alignment (see Alignment) of each class, in the
order of the table. The other fields of the table are the stamp profile of the engine. They are not
options of the step, so a change to them does not bake the features again. The road solver:

- samples the master terrain every 2 m along each way,
- fits a smooth height line to the samples, with weights that let a spike or a gully of the terrain
  go, so that it does not move the road,
- holds each segment within the grade of its class on the line of OSM, but not the approach of a
  side road, so that a side road does not move the through road. The fit of the plan can shorten the
  tip of a hairpin, and the heights stay, so the curve can pass the grade there,
- gives ways that share a vertex one height there, and puts the first 4 m of a side road on the
  surface of the through road of a junction (1.5 m where paths meet), but not past the middle of a
  way between two junctions. The through road is the first class of the table at the junction, and
  of two ways of that class, the pair that runs most nearly straight on,
- puts each vertex of a track on the ground at the height of the nearest track on the ground that
  runs the same way within 6 m beside it, so that the tracks of a double line lie at one height. A
  track follows only a track after it in the bake's order. A vertex keeps its own height where the
  terrain under the two tracks differs by more than 0.5 m, there or within 100 m along its way: a
  ramp or a flyover,
- ignores the terrain under a bridge and in a tunnel: these run between their ends. A tunnel runs in
  a straight line between the two ends of its chain of tunnel ways that meet end to end, so a side
  tunnel or a shaft that joins it follows it and does not lift it. A tunnel chain with an end that
  meets no way runs level from its other end, or on the grade of the road that runs on into it
  there. A bridge never sags under the straight line between the two ends of its chain of bridge
  ways,
- raises a bridge that is not a culvert where it crosses a waterway, or a road, a path or a track on
  the ground that it does not meet: its road lies at least 1.5 m (water), 4.5 m (road), 2.5 m (path)
  or 6 m (track) over it, plus a deck of 1 m, or 0.4 m for a path. The way below may sink to give
  the room. A bridge whose end lies on the road that it crosses, within that road's half-width,
  meets it there and does not rise,
- lets a bridge rise from where it lands at a height that it cannot move no more steeply than 1:1:
  an end that meets no other way stands on the terrain, and an end that leaves a junction as a side
  lies on the through road. A crossing that asks more lies where the bridge lands and does not raise
  it,
- sinks a tunnel where a road, a path or a track crosses over it without meeting it: the tunnel's
  road lies at least its bore and 1 m under that road's middle and both its edges. A road on the
  ground over a tunnel does not move,
- passes one way over the other where two bridges cross that no bridge joins or that both tag a
  different `layer`, and where two ways on the ground with a different `layer` cross, without
  meeting: the one of the higher `layer`, at an equal layer of the first class of the table, and of
  one class the later one in the OSM data, lies over the other by the clearance and the deck above.
  Both ways may move to give the room. A way on the ground of a negative `layer` without a `tunnel`
  tag, such as a corridor of a station, runs under something that the solver leaves out: no way on
  the ground passes over or under it,
- puts a vertex that a road shares with a river or a stream, and the crossing of a way tagged `ford`
  with a waterway, 0.2 m below the water.

The road solver works by area: an area is a tile of zoom 10. The solve of an area holds each way
within 500 m of the area, along the ways from the nearest vertex of OSM of a way in a straight line.
A bridge or a tunnel has no length here, so the solve holds it whole, and the ways that it crosses
join at their distance from it in a straight line. Each vertex of OSM in an area, and each vertex
that the solver adds after it up to the next vertex of OSM, takes its height from the solve of that
area. A network of bridges and tunnels that share a point joins the solve whole, also where it runs
past the region, with each way on the ground that meets one of its ends there. In the order of the
OSM data that the rules above use, each way past the region comes after each way of the region. The
bake reads OSM to 20 km and one zoom-13 tile past the region. A network with an end past that which
meets no way has no heights: the input cuts it, so it has no ends to run between. A bridge is a
culvert, and a tunnel passes under a road, by the solve of the area of its first vertex. A solve of
an area does not depend on the other solves, so the heights do not depend on how many solves run at
once.

A bridge shorter than 8 m whose deck lies no more than 1 m above the terrain is a culvert: the tiles
keep it without its `bridge` tag, as a road. A tunnel whose terrain lies less
than its bore (5 m, 2.6 m for a path, 6.5 m for a track) over it everywhere runs under something
that the terrain leaves out, such as a building: the tiles keep it without its `tunnel` tag, as a
way of its class. A `building_passage` and a tunnel under a road, a path or a track stay tunnels.
The tiles leave out a part of a tunnel that rises more than 5 m over the terrain anywhere farther
than 20 m from its ends.

### Alignment

The road solver also fits the plan of each way that it sets the heights of. The fit runs in each
solve of an area, on the ways of that solve, and each vertex takes its alignment from the solve of
the area that gives it its height. A solve holds the ways within 500 m of its area, and the fit of a
way reaches only some tens of metres along it, so that two areas agree to 2 cm and 0.5° where they
meet.

A stroke is a chain of ways that run on from each other. Where ways end at one point, two ends
within 60° of straight on join: of the pairs, the one whose lower class comes first in the table,
then the one that runs most nearly straight on. The two ways can differ in class, and another way
can pass through the point. The two ways share the tangent at their join, and the sample there has
the class of the two that comes first in the table. The fit does not depend on the direction of a
stroke: the penalty on three steps takes the mean of the classes of its two middle samples.

The fit takes a sample of each stroke about every 2 m, and minimises the square of the distance to
the line of OSM plus a penalty on the third difference of the samples. The smoothing length is 25
`plan_tol_m` of the class table, and the fit holds a sample harder while it leaves the line by more
than `plan_tol_m`. These vertices do not move:

- a junction, other than a point where only the two ways of a stroke meet,
- a vertex of OSM of a bridge or a tunnel,
- the end of the approach of a side road,
- a vertex that a way shares with a river or a stream, and each vertex of a ford's crossing,
- the vertices about a place where a clearance or a pass holds, so that it holds on the curve,
- the two ends of a stroke.

A class with a `design_kmh` banks and widens in a bend of signed curvature `k`, from the fitted line:

- The bank is `V² |k| / 254`, at most `bank` of the class, to the inside of the bend.
- The widening is `widen_m` of the class in a radius of 10 m or less, and zero from 25 m, linear
  between.
- Both run in along the stroke: a triangle filter of ±22 m for the bank and ±12 m for the widening.
  Both are zero within 14 m of a bridge, a tunnel, or a junction with a road of its class or of a
  class before it, and reach their full value 18 m past that. A path or a track at a junction does
  not count, so a road banks through a level crossing.
- The widening moves the curve to the inside of the bend by half of it, so that the outer edge keeps
  its line, but not within 14 m of a vertex that does not move. The rest is the field `widen`.

The tiles keep the vertices that do not move, each end of a run of known heights, and the first
vertex of OSM in each area along a way, so that the solve that owns a span has tested it. After a kept
vertex, the next one is the farthest point of the fit for which the curve below holds the fit and
the solved heights within `keep_m` of the class table, with its tangents and grades as this format
rounds them, and its chord holds the fit within 2 m and between its ends. A tile cuts a way by its
chords, so this leaves 38 m of the buffer. Other vertices of OSM do not stay.

Between two vertices of a part, with the positions `p0`, `p1`, the unit tangents `t0`, `t1`, the
heights `z0`, `z1`, the grades `g0`, `g1` and the chord `c` in metres, the curve at `u` from 0 to 1
is the cubic Hermite curve

```
h00 = 2u³ − 3u² + 1,  h10 = u³ − 2u² + u,  h01 = −2u³ + 3u²,  h11 = u³ − u²
p   = h00 p0 + h10 c t0 + h01 p1 + h11 c t1
z   = h00 z0 + h10 c g0 + h01 z1 + h11 c g1
```

The fields follow the direction of the part:

| Field | Value |
| --- | --- |
| `along_code` | 1 + 2 × (decimetres along its stroke at the first vertex) + 1 when the metres fall along the part: it runs against its stroke. A stroke starts at 0 m at the end of the way that comes first in the bake's order of the chain. The metres are the length of the curve |
| `tangent` | The difference from the vertex before, 0 before the first vertex of a part, in 1/1024 of a turn from +x to +y, wrapped to −512 to 511. A reader takes the sum modulo 1024 |
| `grade` | The difference from the vertex before, 0 before the first vertex of a part, in 1/250. The rise per metre: from the solved heights, not fitted a second time |
| `bank` | In 1/1000. The rise per metre to the left of the direction |
| `word` | 4 × `widen` + `side` |
| `widen` | In centimetres. What the half-width of the road gains at the vertex |
| `side` | The ways out of the vertex that leave its junction as a side: 1 for the way back along the part, 2 for the way on, 3 for both |

Thus the tiles that hold a vertex hold one alignment there, and the metres along run on through a
stroke across tiles and ways.

A way out is a way of a road or a track that leaves a point, toward its point 8 m along OSM. Where
three or more ways out leave a point, `side` marks each one but the through pair of the point. Of
the pairs that open by at least the angle of cosine −0.3, this is the pair that holds the first
class of the table, then the first class beside that, then that runs most nearly straight on. So
a road runs on into a lesser class, and a road that ends on a straight road is a side of it. This
rule serves the drawing. The heights follow the through road of the solver (see Heights), which can
differ. A point with no such pair has no `side`, two ways out both run through, and a path has
`side` 0 and is never a way out. Where a track crosses three or more ways out of roads, the tracks and
the roads each take this rule alone, so each keeps a pair. The tiles keep each vertex that has a `side`. Below zoom 13 a tile
has no `side`, so each way there runs through.

### Structures

The bake adds these to the features of the road solver:

| Added | On | Value |
| --- | --- | --- |
| tag `deck` | each way of a bridge | The depth of its deck in metres, one decimal: 1/30 of its longest span within 0.5 to 2.5 m, for a path 1/40 within 0.3 to 1 m |
| tag `bridge:structure` | each way of a bridge that hangs | `simple-suspension`, where OSM does not tag a cable bridge: a path bridge longer than 40 m that runs more than 25 m over the terrain |
| tag `uphill` | each `gallery` | `left` or `right` of its way: the side where the terrain 10 m from it lies higher on average |
| point `bridge-pier` | each pier of a bridge | Its height is the bridge's road there, on the curve of its deck (see Alignment). Its kind has the `min_zoom` of its bridge |
| tag `stations` | each `aerialway` with an end that has no station of its own | `start`, `end` or `none`: the ends that have one |
| point `catenary-mast` | each mast of the contact wire of a `railway` tagged `electrified` `contact_line` or `yes`, but not a funicular | Its height is its track's rail there. Tag `on` `deck` on a bridge |

The masts of a track stand evenly along each of its ways, at most 50 m apart, 3 m beside its middle,
or 2.5 m for a narrow gauge (`gauge` below 1200 mm, else `narrow_gauge` and `funicular`). On a
bridge a mast stands against the deck's edge: 2.02 m beside the middle, or 1.62 m for a narrow
gauge. A mast stands on the side away from another track that lies 1 to 8 m beside it; a track with
such tracks on both sides has no masts. The bake leaves out a mast that lies less than 8 m from a
mast before it. A track in a tunnel has no masts.

Each vertex of a `platform` or a `platform-line` has the height of its top: 0.55 m, or 0.35 m for a
narrow gauge, over the rail of the nearest track within 15 m that is not in a tunnel. A vertex with
no track near takes the median of the other vertices; a platform with no track near has no heights.

A bridge is the chain of bridge ways whose ends meet two by two. Its piers divide it evenly into
spans of at most 45 m, or 30 m for a path. A pier that falls in water which the bridge crosses for
less than 30 m moves 2 m past the nearer bank of it, and one that falls on a road, a path or a
tunnel moves 2 m past its nearer side; one that comes within 6 m of another pier or of an end goes.
Water is a waterway within its half-width and 1 m, a river area or a lake. A road is its line within
1 m and its half-width: half its `width` (at least 0.4 m), else 6 m for a main road, 3 m for a road
or a `pedestrian` way, 1.5 m for a `cycleway`, 0.6 m for a `via_ferrata` and 1 m for another path. A
cable bridge has no piers.

Each vertex of an `aerialway` between its ends is a pylon. A way splits at each vertex where another
`aerialway` ends: there is a mid-station. Where a span is longer than its rig allows, or where a
carrier would come nearer the terrain than its clearance, the bake adds a pylon: in the middle of a
long span, else at the point of the span at least 25 m from both of its ends where the carrier comes
nearest. The rigs are those of `host/obc-trailview-features/src/lifts.rs`. A station at an end that
lies inside a mapped building is that building; of the ways that end at one point, only the first in
the OSM data has the station there. The tag `stations` names the ends that have a station of their
own.

A `building` in an area `landuse=industrial` or `landuse=farmyard` has the tag `landuse` of that
area: of the smallest such area that holds the mean of the vertices of its first outer ring. The
tiles do not hold these areas.

A `building` has the tag `use`, `pharmacy`, `hospital` or `shop`, when it, a node that lies inside
or on its first outer ring, or an area that holds the mean of that ring's vertices has that use.
`amenity` or `healthcare` `pharmacy` is a pharmacy, `amenity` or `healthcare` `hospital` a hospital,
and any `shop` a shop. Of several uses, the first in that order wins. The tiles do not hold these
nodes and areas.

Each vertex of a `river` or `stream` that is not in a tunnel has a water level, never higher than a
vertex before it: the vertices run in the direction of the flow. OSM draws a waterway that way; the
bake turns a way whose terrain rises by more than 5 m from end to end. The level is the fit that
never rises (pool-adjacent violators) to the lowest terrain within ±(w/2 + 2 m) across the line,
every 5 m, with a fall of at least 0.05 % in a pool, smoothed over ±25 m. The smoothing does not
cross a `waterfall`, a `dam` or a `dam-point`. Waterways that share a vertex are one network: a
level never rises through a junction, and the waterways at a junction have one level there. In a
loop of waterways, the waterways go from the highest start down, and one that flows back into a
junction before it ends below that junction's level. A point in a lake, or within 2 m of its shore,
holds the lake's level.

Each vertex of a `lake` has one level. It is the median of the terrain inside the lake when 80 % of
that terrain lies within 0.3 m of it; else the 15th percentile of the terrain 2 m inside the shore.
A pond (`water=pond`) takes the lowest terrain 2 m inside its shore, a reservoir (`water=reservoir`
or `landuse=reservoir`) the median of the terrain on its outline: its full level. An `ele` tag
within 1.5 m of the level wins. Of the other kinds, only the `sea` has a height (see Sea).

The water solver solves each network apart, with each lake whose bounding box comes within 120 m of
the bounding box of one of its waterways, and each other lake alone. A lake near two networks joins
them into one solve. A level depends only on its network and on the lakes near it, so the levels are
those of one solve of all waterways and lakes.

With `has_z` 1, each vertex has a `z_code`. `z_code` 0 means not solved: the height is unknown,
never 0 m. Otherwise `z_code − 1` is the zigzag difference in centimetres from the last solved
height of the feature, which starts at 0. A height is in metres in the vertical datum of
`planner/terrain`.

### Sea

The bake adds the kind `sea`, `min_zoom` 8: each polygon of the source `water-polygons`
(`water-polygons-split-3857.zip` of osmdata.openstreetmap.de) whose bounding box comes within one
zoom-8 tile of the region. It is one area with its islands as holes, and each of its vertices has
the height 0 m, sea level in the vertical datum of `planner/terrain`. The source splits the sea into
pieces that overlap by 100 m at the split lines, so no gap opens between them.

## Kind table

The table is a JSON list of entries. An OSM object goes to the first entry that has its geometry
and a selector that matches its tags. An object that no entry takes is not in the tiles.

| Field | Value |
| --- | --- |
| `kind` | The name: lowercase kebab-case, unique. A tile names its kinds; the engine module of the same name draws it |
| `geometry` | `point`: a node. `line`: a way, open or closed. `area`: a closed way, or a multipolygon relation with its holes. A closed way with `area=yes` is an area only, and one with `area=no` is a line only. Another closed way is a line and an area, and can give one feature of each, except when one tag selects it for a line entry and for an area entry: then it is an area only, as a closed `waterway=dam` is |
| `match` | A list of selectors. One selector must match. A selector maps OSM keys to `"*"`, any value other than `no`, or to a list of values. Each key of the selector must match |
| `keep` | The OSM keys whose values the tiles keep, when the object has them |
| `height` | `surface`: it stands on the drawn surface and has no height of its own. `pad`: one height for the whole feature, such as a house floor or a lake level. `profile`: a height for each vertex, such as a road or a river level. `none`: no height, such as an outline |
| `min_zoom` | 8 to 13, 13 when absent: the lowest zoom that holds the kind |
| `active` | `false`: the entry takes its objects and the bake drops them. `true` when absent |

The bake validates the table before it reads OSM, and a test of `host/obc-trailview-features`
validates the committed table. The table is part of the options of the step, so a change to it
bakes the layer again.

### Add a kind

Add one entry at the position where it must take its objects before the entries after it. For
example, wayside shrines:

```json
{
  "kind": "shrine",
  "geometry": "point",
  "match": [{"historic": ["wayside_shrine"]}],
  "keep": ["height"],
  "height": "surface"
}
```

Then add the engine module of the kind, `builder/web/src/lib/trailview/kinds/shrine.ts`, and the
model that it names, `models/shrine.ts`. The engine finds the module by its file name. The tile
format, the bake, the decoder and the other engine files do not change.
