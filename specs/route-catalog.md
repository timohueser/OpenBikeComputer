# Route catalog

The route catalog lists the signed routes of a region: OSM route relations for
hiking, foot, bicycle and mountain-bike routes. The planner search reads it
online and offline. Each route record also holds the plan that reproduces the
route. The region prepare step bakes the catalog. A relation with a member
outside the extract leaves the catalog.

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

A record is a route record or a long route. A field with no value is absent.
Coordinates are integer microdegrees, longitude before latitude. Lengths and
heights are integer metres.

| Field | Value |
| --- | --- |
| `id` | OSM relation ID |
| `kind` | The OSM `route` value: `hiking`, `foot`, `bicycle` or `mtb` |
| `name`, `ref`, `operator` | The OSM tag text |
| `description` | The OSM tag text, at most 200 characters |
| `website` | The OSM `website` text, else `contact:website`, else `url` |
| `symbol` | The OSM `osmc:symbol` text |
| `rank` | Network level from `network`: `4` for `iwn` and `icn`, `3` for `nwn` and `ncn`, `2` for `rwn` and `rcn`, `1` for `lwn` and `lcn`, else `0` |
| `loop` | `true` for a loop, `false` for a one-way route |
| `length_m` | Length of the plan route |
| `ascent_m`, `descent_m` | Climb and descent of the plan route |
| `grades_m` | `hiking`, `foot` and `mtb` only: 6 lengths, one for each grade |
| `hardest` | Index in `grades_m` of the hardest explicit grade |
| `cells` | Sorted IDs `9-X-Y` of the zoom 9 cells that the plan route touches |
| `line_udeg` | Route record only: the simplified plan route, flat |
| `via` | Route record only: ascending indices of the shaping vertices in `line_udeg` |
| `turnarounds` | Route record only: the entries of `via` where the plan route turns back, and `0` for a loop that turns back at its start |
| `parent` | Stage only: relation ID of its long route |
| `stage` | Stage only: its number in the stages of `parent`, from `1` |
| `stages` | Long route only: the relation IDs of its stages, in order |
| `start_udeg` | Long route only: `[longitude, latitude]` of its start |

Each record has a `name` or a `ref`, or both. The builder cuts a longer
`description` at a word boundary. `symbol` is absent when France is the only
country of the region; the client then draws the `ref`. A region with France and
another country has no catalog yet: the builder has no country lookup.

### Plan

The plan is the start, the shaping points and the finish. The start is vertex 0
of `line_udeg`. The finish is its last vertex. The shaping points are the
vertices in `via`; vertex 0 and the last vertex are never in `via`. Every route
record has `via`, and it can be empty. `via` has at most 62 entries; a route that
needs more leaves the catalog.

`turnarounds` has the meaning of `turnarounds` in the
[route API request](route-api.md#request): at each of these shaping points the
plan route turns back on purpose. In the request, the points are the start, the
shaping points and the finish, so the request index of an entry is its position
in `via` plus 1. For a loop, `turnarounds` can hold 0: the plan route turns
back at its start. It is absent when there are none.

The plan reproduces the route with the Balanced profile of each activity that
lists the kind: `hiking` for `hiking` and `foot`, `mtb` for `mtb`, and `road`,
`gravel` and `touring` for `bicycle`. The main line is the member ways with an
empty, `main`, `forward` or `backward` role, ordered as the Waymarked Trails route
builder orders them, with the forward branch where the route splits by
direction. When that order has more than one piece, the builder joins the
pieces at their nearest ends, from the first piece on, into one chain that uses
each piece once, with each join at most 500 m. When no such chain exists, for
example because a piece ends inside another piece, the member order stays if
each of its gaps is at most 500 m; otherwise the route leaves the catalog. The
router patches each gap. To reproduce the route, the router's route for the plan
with each profile has a deviation of at most 2 % of the main-line length. The
deviation is the length of the routed line that is more than 30 m from the main
line, plus the length of the main line that is more than 30 m from the routed
line. The plan route is the route that the
router gives for the plan with one of these profiles: `touring` for `bicycle`,
else the only one. The plan runs in the member order of the relation, also for a
loop. A route is a loop when its main line is a closed ring, or when it has
`roundtrip=yes` and its ends are within 200 m. A loop plan ends at its start, so
the last vertex equals vertex 0.

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
gives the grade:

| `sac_scale` | Grade |
| --- | --- |
| `strolling`, `hiking` | T1 |
| `mountain_hiking` | T2 |
| `demanding_mountain_hiking` | T3 |
| `alpine_hiking` | T4 |
| `demanding_alpine_hiking` | T5 |
| `difficult_alpine_hiking` | T6 |

For `mtb`, the first digit of `mtb:scale` gives S0 to S5; S6 counts as S5. A
road without a valid value has no explicit grade and counts as T1 or S0.

`grades_m[i]` is the length of the plan route with grade T(i + 1), or with grade
S(i) for `mtb`. Each length is rounded, so the sum can differ from `length_m` by
up to 3 m. `hardest` is the largest `i` with an explicit grade on the plan
route. It is absent when the plan route has no explicit grade.

### Long routes

A long route is a relation that holds route relations. Its stages are its child
routes with an empty or `main` role and the same `network`, in member order. A
long route is in the catalog only when each of its stages is in it. A long route
with a child relation that holds route relations leaves the catalog.

A long route has no `line_udeg`, `via` or `turnarounds`. Its line and its plan
are those of its stages in order. The client loads the stages by their IDs from
the files of the long route's `cells`. The joined plan has the points of the
stage plans in order. A stage finish and the next stage start are one point when
they are the same vertex, and two points otherwise. Each stage turnaround moves
to its position in the joined plan. A client
plans a whole long route only when its joined plan has at most 64 points;
otherwise it offers its stages only. `start_udeg` is the start of its first
stage. `length_m`, `ascent_m`, `descent_m` and `grades_m` are the sums of the
values of its stages. `hardest` is the largest `hardest` of its stages. Its
`cells` are the union of the `cells` of its stages. Thus each file that holds a
stage also holds its long route.

A stage has `parent` and `stage` only when its long route is in the catalog.
When a route is a stage of two long routes, `parent` is the one with the lower
relation ID.

### Coverage

A segment of the plan route touches each zoom 9 cell whose box intersects the
bounding box of the segment, edges included. `cells` can name a cell outside the
grid; that cell has no file. Offline, a route lies wholly inside the download
when each of its `cells` is the `id` of a cell in `offline.cells`.

## Examples

A one-way stage of 3.7 km in cell `9-267-178`, with one shaping point:

```json
{"ascent_m":184,"cells":["9-267-178"],"descent_m":251,"grades_m":[2460,1250,0,0,0,0],
 "hardest":1,"id":1234567,"kind":"hiking","length_m":3710,
 "line_udeg":[8004512,47873120,9210,-6400,8800,-11500,6500,-4200,5510,-2100],
 "loop":false,"name":"Mühlbach-Weg, Etappe 1","operator":"Schwarzwaldverein",
 "parent":2345678,"rank":1,"ref":"MW","stage":1,
 "symbol":"yellow:white:yellow_diamond","via":[2]}
```

Its long route, with two stages:

```json
{"ascent_m":420,"cells":["9-267-178"],"descent_m":449,"grades_m":[6830,2000,0,0,0,0],
 "hardest":1,"id":2345678,"kind":"hiking","length_m":8830,"loop":false,
 "name":"Mühlbach-Weg","operator":"Schwarzwaldverein","rank":1,"ref":"MW",
 "stages":[1234567,1234568],"start_udeg":[8004512,47873120],
 "symbol":"yellow:white:yellow_diamond"}
```
