# Route catalog

The route catalog lists the signed routes of a region: OSM route relations for hiking, foot, bicycle
and mountain-bike routes. Each route record also holds the plan that reproduces the route. A
relation with a member outside the extract leaves the catalog.

- Version: `format` `1`. A new field does not change the format. A reader ignores a field that it
  does not know.
- Writers: the region prepare step, `planner/router-build` (`src/catalog/mod.rs`,
  `src/catalog/connector.rs`).
- Readers: the planner search, online and offline: the web planner
  (`builder/web/src/lib/planner/signed-routes.ts`, `route-catalog.ts`) and its Explore page
  (`builder/web/src/pages/Explore.svelte`), the companion app (`OBCPlanner/SignedRoutes.swift`).
- Test vectors: [`vectors/signed-routes.json`](vectors/signed-routes.json).
- Related: [route API](route-api.md), [planner release](planner-release.md), [offline
  bundle](planner-offline.md).

## Files

| Release | Path | Content |
| --- | --- | --- |
| Regional | `routes/REGION.json` | Every record of the region |
| Grid | `routes/tiles/9-X-Y.json` | One file for each grid cell |

A file is UTF-8 JSON: `{"format": 1, "routes": [...]}`. The records are in ascending `id` order. A
grid release has no region file.

A grid cell is a Web Mercator tile at zoom 9 whose box overlaps the release bounds with a positive
area. Each grid cell has a file. A cell with no routes has an empty `routes` list. Equal files share
one object in the pool. A record is in the file of each grid cell in its `cells`, with all of its
fields.

The tile API serves a cell file at `/releases/ID/routes/tiles/9-X-Y.json`. An offline selection
holds the files of its `offline.cells`. A client reads only the files of covered cells: the grid
cells online, and `offline.cells` offline. A cell outside this set is not downloaded. A covered cell
without a file is an error, not a cell with no routes. In a regional release, the region file covers
every cell.

## Selection

The builder selects a relation by its tags: `type` is `route` or `superroute`, `route` is
`hiking`, `foot`, `bicycle` or `mtb`, it has a `name` or a `ref`, `network:type` is not
`node_network` or `basic_network`, and `state` is not `proposed`, `planned`, `disused` or
`abandoned`.

A selected relation that is a connector leg leaves the catalog. A connector leg is a signed piece
of the footpath network between two signposts, with a name such as "Rotkreuz - Auleten". It
leaves also when it is a stage of a long route. A relation is a connector leg when each row is
true:

| Test | A connector leg has |
| --- | --- |
| `route` | `hiking` or `foot` |
| `osmc:symbol` | absent, `yellow::yellow_diamond`, `red:white:red_bar` or `blue:white:blue_bar` (the Swiss trail grades) |
| `ref` | absent or empty |
| `roundtrip` | not `yes` |
| Child routes | none: no member is a route relation |
| `name` | empty, or a dash in it |
| Theme word | outside round brackets, no word ends in `weg`, `wäg`, `pfad`, `trail`, `loop` or `tour`, in any letter case |

A dash is `-`, `–` or `—`. A word is a run of letters and digits. Any dash in the name counts,
also inside round brackets or at the start or the end, as in "- Niederrickenbach". A name with no
dash, such as "Pilatus", is a route.

"Zwergenweg Käserstatt-Lischen" stays a route. "Emmenbrücke-Rotenburg", "Sonnhalde - Linden -
Meiestoss - Moos" and "Wädenswil - Richterswil (Seeweg)" leave. The symbols are the Swiss trail
grades, so elsewhere a local route with one of these symbols, no `ref` and a name "A - B" leaves
too.

A route record has a main line of at least 1 km. With this length, short named walks are in; the
connector rule, not the length, keeps out the pieces between signposts.

## Records

A record is a route record or a long route. A field with no value is absent. Coordinates are integer
microdegrees, longitude before latitude. Lengths and heights are integer metres.

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
| `profile_m` | Route record only: the height of the plan route in metres at each 100 m from its start, and at its end |
| `via` | Route record only: ascending indices of the shaping vertices in `line_udeg` |
| `turnarounds` | Route record only: the entries of `via` where the plan route turns back, and `0` for a loop that turns back at its start |
| `parent` | Stage only: relation ID of its long route |
| `stage` | Stage only: its number in the stages of `parent`, from `1` |
| `stages` | Long route only: the relation IDs of its stages, in order |
| `start_udeg` | Long route only: `[longitude, latitude]` of its start |

Each record has a `name` or a `ref`, or both. The builder cuts a longer `description` at a word
boundary. `symbol` is absent when France is the only country of the region; the client then draws
the `ref`. A region with France and another country has no catalog: the builder has no country
lookup.

### Plan

The plan is the start, the shaping points and the finish. The start is vertex 0 of `line_udeg`. The
finish is its last vertex. The shaping points are the vertices in `via`; vertex 0 and the last
vertex are never in `via`. Every route record has `via`, and it can be empty. `via` has at most 62
entries; a route that needs more leaves the catalog.

`turnarounds` has the meaning of `turnarounds` in the [route API request](route-api.md#request): at
each of these shaping points the plan route turns back on purpose. In the request, the points are
the start, the shaping points and the finish, so the request index of an entry is its position in
`via` plus 1. For a loop, `turnarounds` can hold 0: the plan route turns back at its start. It is
absent when there are none.

The plan reproduces the route with a profile of each activity that lists the kind: `hiking/alpine`
for `hiking` and `foot`, `mtb/technical` for `mtb`, and `road`, `gravel` and `touring` for
`bicycle`. A client plans a `hiking` or `foot` route with `hardest` above 1 (T2) with
`hiking/alpine`, an `mtb` route with `hardest` above 2 (S2) with `mtb/technical`, and every other
route with the Balanced profile. Both profiles give the same route when no grade on it is above the
ordinary limit. A hiking route can have `hardest` 4 or 5 (T5 or T6).
The main line is the member ways with an empty, `main`, `forward` or `backward`
role, ordered as the Waymarked Trails route builder orders them, with the forward branch where the
route splits by direction. When that order has more than one piece, the builder joins the pieces at
their nearest ends, from the first piece on, into one chain that uses each piece once, with each
join at most 500 m. When no such chain exists, for example because a piece ends inside another
piece, the member order stays if each of its gaps is at most 500 m; otherwise the route leaves the
catalog. The router patches each gap. To reproduce the route, the router's route for the plan with
each profile has a deviation of at most 2 % of the main-line length. The deviation is the length of
the routed line that is more than 30 m from the main line, plus the length of the main line that is
more than 30 m from the routed line. The plan route is the route that the router gives for the plan
with one of these profiles: `touring` for `bicycle`, else the only one. The plan runs in the member
order of the relation, also for a loop. A route is a loop when its main line is a closed ring, or
when it has `roundtrip=yes` and its ends are within 200 m. A loop plan ends at its start, so the
last vertex equals vertex 0.

`length_m`, `ascent_m` and `descent_m` are the totals of the route engine for the plan route. The
router smooths the heights of each leg first. Each height becomes the mean, over 100 m of the leg
centred on its point, of the straight line between the points. Near an end of the leg, or next to a
point without a height, the window gets shorter so that it stays centred. So those points keep
their own height. The ascent of a leg is the sum of the smoothed height rises between consecutive
points, and the descent is the sum of the drops. A rise or a drop next to a point without a height
does not count. The climb of each leg is rounded to whole metres before the legs are added.

### Line

`line_udeg` holds two integers for each vertex: longitude, then latitude. The first pair is
absolute. Each pair after it is the difference from the previous vertex. This is the encoding of
`coordinates_udeg` in the [route API](route-api.md#deltas). The vertices are points of the plan
route, in its order. No point of the plan route is more than 10 m from the line. Each shaping point
is a vertex of the line.

### Profile

Entry `i` of `profile_m` is the height at `i × 100` m along the plan route. The last entry is the
height at the end of the plan route, at `length_m`. It is there also when `length_m` is a multiple
of 100, so a route of 300 m has 5 entries.

The distance along the plan route is the sum of the distances between its points, scaled so that
the whole route has `length_m`. The lengths of `grades_m` use the same scale. A height is the
elevation of the plan route at that distance, rounded to the metre. When the distance is that of a
point, the height is the height of that point, and the entry is `null` when that point has no
height. Otherwise the height is on the straight line between the point before it and the point after
it, and the entry is `null` when one of these two points has no height. `profile_m` is absent when
no height of the plan route is known.

The profile samples the smoothed heights that `ascent_m` and `descent_m` come from. Its climb can
differ from `ascent_m` because it has only one height for each 100 m. A reader that needs the height
at a vertex of `line_udeg` takes the profile at the share of the line's length that the vertex has.
The line is simplified within 10 m of the plan route (see [Line](#line)), so on switchbacks it is
shorter than the plan route. The share of a vertex then points to another place along the plan
route. This offset grows with the switchbacks before the vertex and has no fixed limit, and the
height is wrong by the climb of the route over that stretch. The profile is for a picture and for
the pick of a route in TrailView, not for a figure.

### Grades

The grade of a road comes from its way. For `hiking` and `foot`, `sac_scale` gives the grade:

| `sac_scale` | Grade |
| --- | --- |
| `strolling`, `hiking` | T1 |
| `mountain_hiking` | T2 |
| `demanding_mountain_hiking` | T3 |
| `alpine_hiking` | T4 |
| `demanding_alpine_hiking` | T5 |
| `difficult_alpine_hiking` | T6 |

For `mtb`, the first digit of `mtb:scale` gives S0 to S5; S6 counts as S5. A road without a valid
value has no explicit grade and counts as T1 or S0.

`grades_m[i]` is the length of the plan route with grade T(i + 1), or with grade S(i) for `mtb`.
Each length is rounded, so the sum can differ from `length_m` by up to 3 m. `hardest` is the largest
`i` with an explicit grade on the plan route. It is absent when the plan route has no explicit
grade.

### Long routes

A long route is a relation that holds route relations. Its stages are its child routes with an empty
or `main` role and the same `network`, in member order. A long route is in the catalog only when
each of its stages is in it. A long route with a child relation that holds route relations leaves
the catalog.

A long route has no `line_udeg`, `via`, `turnarounds` or `profile_m`. Its line and its plan are
those of its stages in order. A reader that needs its profile joins the profiles of its stages, and
places the entries of each stage by the `length_m` of the stage. The client loads the stages by
their IDs from the files of the long route's `cells`. The joined plan has the points of the stage
plans in order. A stage finish and the next stage start are one point when they are the same vertex,
and two points otherwise. Each stage turnaround moves to its position in the joined plan. A client
plans a whole long route only when its joined plan has at most 64 points; otherwise it offers its
stages only. `start_udeg` is the start of its first stage. `length_m`, `ascent_m`, `descent_m` and
`grades_m` are the sums of the values of its stages. `hardest` is the largest `hardest` of its
stages. Its `cells` are the union of the `cells` of its stages. Thus each file that holds a stage
also holds its long route.

A stage has `parent` and `stage` only when its long route is in the catalog. When a route is a stage
of two long routes, `parent` is the one with the lower relation ID.

### Coverage

A segment of the plan route touches each zoom 9 cell whose box intersects the bounding box of the
segment, edges included. `cells` can name a cell outside the grid; that cell has no file. Offline, a
route lies wholly inside the download when each of its `cells` is the `id` of a cell in
`offline.cells`.

### Map view

A map view is a box of longitude and latitude. Its cells are the zoom 9 cells whose box intersects
the view, edges included. A client loads no more than 12 cell files for one view. When the view has
more cells, the client loads none and shows no routes. A route record is in the view when its
`kind` fits the activity and a vertex of its `line_udeg` is in the box, edges included. A long
route is never in the view, but its stages can be. Each record is in the view once, also when
several of its cells are loaded. A cell outside the covered set adds no record. Test vectors:
[`vectors/route-boxes.json`](vectors/route-boxes.json).

## Examples

A one-way stage of 3.7 km in cell `9-267-178`, with one shaping point:

```json
{"ascent_m":184,"cells":["9-267-178"],"descent_m":251,"grades_m":[2460,1250,0,0,0,0],
 "hardest":1,"id":1234567,"kind":"hiking","length_m":3710,
 "line_udeg":[8004512,47873120,9210,-6400,8800,-11500,6500,-4200,5510,-2100],
 "loop":false,"name":"Mühlbach-Weg, Etappe 1","operator":"Schwarzwaldverein",
 "parent":2345678,
 "profile_m":[620,618,625,631,640,652,661,668,676,684,690,699,708,716,725,733,741,748,752,
              760,768,773,779,784,788,780,768,751,733,712,694,671,650,628,610,590,571,555,553],
 "rank":1,"ref":"MW","stage":1,"symbol":"yellow:white:yellow_diamond","via":[2]}
```

Its long route, with two stages:

```json
{"ascent_m":420,"cells":["9-267-178"],"descent_m":449,"grades_m":[6830,2000,0,0,0,0],
 "hardest":1,"id":2345678,"kind":"hiking","length_m":8830,"loop":false,
 "name":"Mühlbach-Weg","operator":"Schwarzwaldverein","rank":1,"ref":"MW",
 "stages":[1234567,1234568],"start_udeg":[8004512,47873120],
 "symbol":"yellow:white:yellow_diamond"}
```
