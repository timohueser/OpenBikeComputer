# Route catalog

The route catalog lists the signed routes of a region: OSM route relations for
hiking, foot, bicycle and mountain-bike routes. The planner search reads it
online and offline. Each record also holds the plan that reproduces the route.
The region prepare step bakes the catalog.

## Files

| Release | Path | Content |
| --- | --- | --- |
| Regional | `routes/REGION.json` | Every record of the region |
| Grid | `routes/tiles/9-X-Y.json` | One file for each grid cell |

A file is UTF-8 JSON: `{"format": 1, "routes": [...]}`. The records are in
ascending `id` order. A grid release has no region file.

A grid cell is a Web Mercator tile at zoom 9 whose box overlaps the release
bounds with a positive area. `routing/layers.json` lists the same cells. Each
grid cell has a file. A cell with no routes has an empty `routes` list. Equal
files share one object in the pool. A record is in the file of each grid cell
in its `cells`, with all of its fields.

The tile API serves a cell file at `/releases/ID/routes/tiles/9-X-Y.json`. An
offline selection holds the files of its `offline.cells`. A client reads only
the files of covered cells: the grid cells online, and `offline.cells` offline.
A cell outside this set is not downloaded. A covered cell without a file is an
error, not a cell with no routes. In a regional release, the region file covers
every cell.

## Records

A field with no value is absent. Coordinates are integer microdegrees, longitude
before latitude. Lengths and heights are integer metres.

| Field | Value |
| --- | --- |
| `id` | OSM relation ID |
| `kind` | The OSM `route` value: `hiking`, `foot`, `bicycle` or `mtb` |
| `name`, `ref`, `operator`, `description` | The OSM tag text |
| `website` | The OSM `website` text, else `contact:website` |
| `symbol` | The OSM `osmc:symbol` text; absent when the mark must not show, and the client then draws the `ref` |
| `rank` | Network level from `network`: `4` for `iwn` and `icn`, `3` for `nwn` and `ncn`, `2` for `rwn` and `rcn`, `1` for `lwn` and `lcn`, else `0` |
| `loop` | `true` for a loop, `false` for a one-way route |
| `parent` | Stage only: relation ID of its long route |
| `stage` | Stage only: its number in the stages of `parent`, from `1` |
| `stages` | Long route only: the number of its stages |
| `length_m` | Length of the plan route |
| `ascent_m`, `descent_m` | Climb and descent of the plan route |
| `grades_m` | `hiking`, `foot` and `mtb` only: 6 lengths, one for each grade |
| `hardest` | Index in `grades_m` of the hardest explicit grade |
| `line_udeg` | The simplified plan route, flat |
| `via` | Ascending indices of the shaping vertices in `line_udeg` |
| `cells` | Sorted IDs `9-X-Y` of the zoom 9 cells that the plan route touches |

Each record has a `name` or a `ref`, or both.

### Plan

The plan is the start, the shaping points and the finish. The start is vertex 0
of `line_udeg`. The finish is its last vertex. The shaping points are the
vertices in `via`; vertex 0 and the last vertex are never in `via`. Every record
has `via`, and it can be empty.

The plan reproduces the route with the Balanced profile of each activity that
lists the kind: `hiking` for `hiking` and `foot`, `mtb` for `mtb`, and `road`,
`gravel` and `touring` for `bicycle`. The plan route is the route that the
router gives for the plan with one of these profiles: `touring` for `bicycle`,
else the only one. A one-way plan runs in the member order of the
relation. A route is a loop when its main line is a closed ring, or when it
has `roundtrip=yes` and its ends are within 200 m. A loop plan ends at its start,
so the last vertex equals vertex 0.

`length_m`, `ascent_m` and `descent_m` are the totals of the route engine for
the plan route. The ascent is the sum of the height rises between consecutive
points of the road geometry. The descent is the sum of the height drops.

### Line

`line_udeg` holds two integers for each vertex: longitude, then latitude. The
first pair is absolute. Each pair after it is the difference from the previous
vertex. This is the encoding of `coordinates_udeg` in the
[route API](route-api.md#deltas). The vertices are points of the plan route, in
its order. No point of the plan route is more than 50 m from the line. Each
shaping point is a vertex of the line.

### Grades

The grade of a road comes from its way. For `hiking` and `foot`, `sac_scale`
gives the grade T1 to T6: `strolling` and `hiking` are T1, `mountain_hiking` is
T2, through `difficult_alpine_hiking`, T6. For `mtb`, the first digit of
`mtb:scale` gives S0 to S5; S6 counts as S5. A road without a valid value has no
explicit grade and counts as T1 or S0.

`grades_m[i]` is the length of the plan route with grade T(i + 1), or with grade
S(i) for `mtb`. Each length is rounded, so the sum can differ from `length_m` by
up to 3 m. `hardest` is the largest `i` with an explicit grade on the plan
route. It is absent when the plan route has no explicit grade.

### Stages

A long route is a relation that holds route relations. Its stages are its child
routes with the same `network`, in member order. `stages` counts these members,
also members that are not in the catalog. A long route is a record of its own.

A stage has `parent` and `stage` only when its long route is in the catalog.
When a route is a stage of two long routes, `parent` is the one with the lower
relation ID. The `cells` of a long route include the `cells` of each of its
stages. Thus each file that holds a stage also holds its long route.

### Coverage

A segment of the plan route touches each zoom 9 cell that its bounding box
overlaps. `cells` can name a cell outside the grid; that cell has no file.
Offline, a route lies wholly inside the download when each of its `cells` is in
`offline.cells`.

## Example

A loop of 6.2 km in cell `9-267-178`, with one shaping point:

```json
{"ascent_m":312,"cells":["9-267-178"],"descent_m":312,"grades_m":[4990,1250,0,0,0,0],
 "hardest":1,"id":1234567,"kind":"hiking","length_m":6240,
 "line_udeg":[8004512,47873120,9210,-6400,8800,-11500,-12500,-4200,-5510,22100],
 "loop":true,"name":"Mühlbach-Runde","operator":"Schwarzwaldverein","rank":1,"ref":"MR",
 "symbol":"yellow:white:yellow_diamond","via":[2]}
```
