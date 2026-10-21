# TrailView feature tiles

The layer `planner/trailview-features` holds the OSM features that TrailView draws. Its step
reads the OSM of `planner/osm` and the master terrain of the grid of `planner/terrain` in the same
release.
The kind table
[`data/trailview/kinds.json`](../data/trailview/kinds.json) selects the features and the tags
that the tiles keep. A new kind is one entry in that table. The format does not change.

## Archive

The archive is PMTiles v3 with tile type 0 (unknown), gzip tile compression and XYZ tile
addresses at zooms 8 to 14. The grid step packs each tile into the archive of its zoom 11
parent, or of its own tile below zoom 11 ([`obc-data.md`](obc-data.md)).

| Metadata | Value |
| --- | --- |
| `features_format` | 1 |
| `extent` | 8192: tile units across a tile |
| `buffer_m` | 40: the buffer on the ground, in metres |
| `attribution` | The credit of OpenStreetMap |
| `bounds` | West, south, east and north of the region |

Zoom 14 holds every active kind. Zooms 8 to 13 hold the kinds whose `min_zoom` is that zoom or
lower. A tile that meets `bounds` and that the archive does not hold has no features.

### Buffer

A tile holds each feature that comes within the buffer of the tile square, cut to the square
grown by the buffer. In tile units, the buffer is `buffer_m` on the ground at the tile edge
nearer the pole, so it is at least `buffer_m` at every point of the tile. The widest stamp, the
cut and fill cone of a road, reaches 25 m past the road edge. 40 m holds it for a road up to
30 m wide, so neighbouring terrain nodes stamp the same apron.

The cut makes no vertex. A piece of a line keeps the segments that meet the buffered square whole,
and one more vertex at each end, so that both tiles know the bend at each end of such a segment.
A ring keeps each edge that crosses a side of the square whole, and joins the vertices outside
the side directly. Such a join lies outside the square. Thus inside the buffered square, the
tiles that hold a feature hold the same vertices and edges, and a stamp near a tile edge gets
the same answer in both tiles.

The buffer is for the stamps. A tile owns the points from 0 to less than 8192 on each axis: a
reader that draws or places features uses only the part of a feature in that square, so a point
on a shared edge or in the buffer of two tiles is drawn once.

### Generalization

Zoom 14 keeps each vertex, and the vertices that the road solver adds (see Heights). Below zoom 14, the bake simplifies each line and ring with
Douglas-Peucker to `extent / 2048` tile units, a quarter texel of a 512-texel terrain node. A
vertex that two features share stays, and so do the ends of a line. Thus road junctions stay
joined, and a river keeps its end on the shore of its lake. Then each edge of an area that
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
tile    = varint string_count, string × string_count, varint feature_count, feature × feature_count
string  = varint byte_length, UTF-8 bytes
feature = varint kind, varint tag_count, (varint key, varint value) × tag_count,
          varint header, part × part_count
header  = part_count × 8 + has_z × 4 + geometry
part    = varint (vertex_count × 2 + outer), vertex × vertex_count
vertex  = zigzag dx, zigzag dy, then varint z_code when has_z is 1
```

`kind`, `key` and `value` are indices from 0 into the strings of the tile. The tags of a feature
are sorted by key. `geometry` is 0 for a point, 1 for a line and 2 for an area.

### Coordinates

Coordinates are Web Mercator tile units with the origin at the north-west corner of the tile, x
to the east and y to the south. The tile square is 0 to 8192 on both axes. A coordinate in the
buffer is below 0 or above 8192. For tile column `c` and row `r` at zoom `z`, a point with world
coordinates `wx`, `wy` in [0, 1] has `x = round(wx × 2^z × 8192) − 8192c` and
`y = round(wy × 2^z × 8192) − 8192r`, so neighbouring tiles round a point the same way.

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

Each vertex of a `main-road`, `road`, `path`, `tunnel` or `gallery` has the height of the road surface,
except a `via_ferrata`, which has none. The class table
[`data/trailview/roads.json`](../data/trailview/roads.json) gives the rules per `highway` class: the
steepest grade and a smoothing length. It is part of the options of the step. The road solver:

- samples the master terrain every 2 m along each way,
- fits a smooth height line to the samples, with weights that let a spike or a gully of the terrain
  go, so that it does not move the road,
- holds each segment within the grade of its class, but not the approach of a side road, so that a
  side road does not move the through road,
- gives ways that share a vertex one height there, and puts the first 4 m of a side road on the
  surface of the through road of a junction (1.5 m where paths meet), but not past the middle of a
  way between two junctions. The through road is the first class of the table at the junction, and
  of two ways of that class, the pair that runs most nearly straight on,
- ignores the terrain under a bridge and in a tunnel: these run between their ends,
- puts a vertex that a road shares with a river or a stream, and the crossing of a way tagged `ford`
  with a waterway, 0.2 m below the water.

The tiles keep the vertices of OSM and the vertex at the end of each approach of a side road, and add
a vertex between two of them where the solved height leaves the straight line between them by more
than 20 cm. A bridge shorter than 8 m whose deck lies
no more than 1 m above the terrain is a culvert: the tiles keep it without its `bridge` tag, as a road.

Each vertex of a `river` or `stream` that is not in a tunnel has the height of a straight line fitted
to the terrain, sampled every 2 m, over ±12 m of the line; near an end the window holds the part of
it on the line. It is never higher than a vertex before it: OSM draws a waterway in the direction of
its flow. Each vertex of a `lake` has the median of the terrain inside it. Other kinds have no height yet.
With `has_z` 1, each vertex has a `z_code`. `z_code` 0 means not solved: the height is unknown,
never 0 m. Otherwise `z_code − 1` is the zigzag difference in centimetres from the last solved
height of the feature, which starts at 0. A height is in metres in the vertical datum of
`planner/terrain`.

The shared vectors [`vectors/trailview-features.json`](vectors/trailview-features.json) and
[`vectors/trailview-features.bin`](vectors/trailview-features.bin) are one tile in its decoded
form and its bytes. The Python encoder and the TypeScript decoder of the engine read both.

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
| `min_zoom` | 8 to 14, 14 when absent: the lowest zoom that holds the kind |
| `active` | `false`: the entry takes its objects and the bake drops them. `true` when absent |

The bake validates the table before it reads OSM, and `tools/tests/test_planner_trailview_features.py`
validates the committed table. The table is part of the options of the step, so a change to it
bakes the layer again.

### Add a kind

Add one entry at the position where it must take its objects before the entries after it. For
example, summit crosses:

```json
{
  "kind": "summit-cross",
  "geometry": "point",
  "match": [{"summit:cross": ["yes"]}],
  "keep": ["material"],
  "height": "surface"
}
```

Then add the engine module of the kind. The tile format, the bake and the decoder do not change.
