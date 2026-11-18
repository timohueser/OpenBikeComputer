# Route catalog

The route catalog lists the signed routes of a region: OSM route relations for hiking, foot, bicycle
and mountain-bike routes. Each route record also holds the plan that reproduces the route. A
relation with a member outside the extract leaves the catalog.

- Version: `format` `1`. A new field does not change the format. A reader ignores a field that it
  does not know.
- Writers: the region prepare step, `planner/router-build` (`src/catalog/mod.rs`,
  `src/catalog/connector.rs`).
- Readers: the planner search, online and offline: the web planner
  (`builder/web/src/lib/planner/route-catalog.ts`) and its Explore page
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
fields; a long route is there without `parts` and `trail_profile_m`, which only the region file and
the routing package hold.

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
| Child routes | none: no member is a route relation, or the relation is a joined trail (see Long routes) |
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
| `id` | OSM relation ID; for a section, a negative number (see [Sections](#sections)). A negative `id` is no OSM relation ID: no reader builds an OSM link from it |
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
| `parent` | Stage of a complete long route only: relation ID of that long route |
| `stage` | Stage of a complete long route only: its number in the stages of `parent`, from `1` |
| `trail` | Part of a long route only: relation ID of the long route that lists it at the lowest level; for a section, relation ID of its cut line |
| `trail_stage` | Part of a long route only, not a section: its stage number in `trail` |
| `section` | Section only: its number in `trail`, from `1` |
| `stages` | Complete long route only: the relation IDs of its stages, in order |
| `start_udeg` | Complete long route only: `[longitude, latitude]` of its start |
| `parts` | Long route only: each of its parts, in order (see [Long routes](#long-routes)) |
| `stage_count` | Long route only: the number of different stage numbers in `parts` |
| `trail_profile_m` | Long route only: 201 heights of the whole trail (see [Long routes](#long-routes)) |

Each record has a `name` or a `ref`, or both. The builder cuts a longer `description` at a word
boundary. `symbol` is absent when a vertex of the route's full planned line, before the
simplification of `line_udeg`, is in France, and on a long route when one of its kept parts is in
France: the marks of the French networks belong to their federation. The client then draws the
`ref`. The rule is per route in each region, so a route of a French region that stays outside
France, such as in Monaco, keeps its `symbol`.

### Plan

The plan is the start, the shaping points and the finish. The start is vertex 0 of `line_udeg`. The
finish is its last vertex. The shaping points are the vertices in `via`; vertex 0 and the last
vertex are never in `via`. Every route record has `via`, and it can be empty. `via` has at most 62
entries; a route that needs more leaves the catalog, or the builder cuts it (see
[Sections](#sections)).

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
piece, the member order stays if each of its gaps is at most 500 m. Otherwise the builder takes the
pieces as a network and removes its dead-end side branches, the shortest first, while all removed
branches together are at most 10 % of the rest. If the network still forks, the route leaves the
catalog. The builder joins the parts that remain into one chain as above, and puts each side branch
of more than 30 m back as an out-and-back side trip at the point where it leaves; its tip is a
turnaround. A removed branch whose tip is the first point of the member order starts the line at
that tip when the line starts where the branch leaves; a branch at the last point ends the line in
the same way. A main member way that the import does not hold, such as a park or a way with no road
tag, is skipped when a held main way comes before it and after it in member order; a route skips
at most 2 such ways, and the line joins across them as across any gap. The router patches each gap.
The route's own line is its pieces joined straight across their gaps. A route leaves the catalog
when more than 500 m of a routed patch is farther than 30 m from its own line. To reproduce the
route, the router's route for the plan with each profile has a deviation of at most 2 % of the main-line length. The deviation is the length of
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

The child relations of a relation are its relation members with an empty, `main` or `part` role. A
piece is a child relation that is a route in the data, has no route relation as a member, and is not
a route of its own: it has no `name` and no `ref`, or it is a connector leg. A selected relation is a
joined trail when it has child routes, each of them is a piece, and no child relation is absent. A
joined trail is one route, not a long route. Its member ways are its own and those of each piece in
the place of the piece, in member order. Its record is an ordinary route record with its own `id`
and tags, and the pieces have no records. The connector test of a joined trail counts no child
routes.

A long route is a selected relation with a child relation that is absent from the data, or that is a
route and not a piece. Its parts are its child relations in member order:

| Child relation | Part |
| --- | --- |
| Absent from the data | One part, not in the catalog |
| A connector leg | None: it is no stage, so it takes no stage number |
| A long route by these rules, selected or not | Its own parts in its place, at most 4 levels of long routes deep |
| Another route relation | One part; a joined trail is one part |

A relation that comes twice counts once, at its first place. The stage number of a part is its
position in this list, from `1`, parts not in the catalog included. The sections of a cut part take
its place in `parts`, each with its stage number (see [Sections](#sections)). A long route inside another one
is also a long route of its own. A child route with no `name` and no `ref` that is a part gets a
route record with the `name` and the `ref` of the long route that has it as a child relation; of
two, the one with the lower relation ID. A long route is in the catalog when at least one of its
parts is.

The ends of a part in the catalog are the first and the last vertex of its `line_udeg`. A part that
left the catalog has the first and the last point of its main line as its ends, when the builder
made one: the pieces of the main line in their order, chained when they chain, before the router
patches the gaps (see [Plan](#plan)). Other parts have no ends. The distance from a point to a part
is the distance to the nearer end of the part. The trail start and the trail end of a part are its
first and its last end, swapped when the part runs backwards. The known end before a part is the
trail end of the part right before it, when that part has ends.

- A part whose nearer end is at most 500 m from the known end before it runs backwards when its last
  end is nearer to the known end than its first end.
- Each other part with ends starts a run. It runs backwards when its first end is nearer to the next
  part than its last end, and at most 500 m from it. It has a jump when a part in the catalog comes
  before it.
- A loop part never runs backwards. A part without ends has no direction and no jump.

So a run is a stretch of parts that join each other, such as the stages of a trail inside the region,
or one of several trails that one relation lists one after another.

A long route is complete when each part is in the catalog, and no part is cut, runs backwards or has
a jump.
A long route with a connector leg among its child relations, or among those of a long route in its
place, is never complete, because the leg leaves a gap between its parts. Only a complete long
route has `stages` and `start_udeg`, and only its stages have `parent` and `stage`: `stages` lists
its parts, and `start_udeg` is the start of its first part. When a route is a stage of two complete
long routes, `parent` is the one that lists it at the lower level (see `trail`); of two at the same
level, the one with the lower relation ID. A reader that knows only
these fields shows the parts of every other long route as single routes.

A long route has no `line_udeg`, `via`, `turnarounds` or `profile_m`. `length_m`, `ascent_m`,
`descent_m` and `grades_m` are the sums of the values of its parts in the catalog. `hardest` is the
largest `hardest` of these parts, and `cells` the union of their `cells`. So each file that holds a
part in the catalog also holds its long route. A long route is a loop when its first and last parts
are in the catalog and the trail start of the first part is the trail end of the last part, or when
it has `roundtrip=yes` and these two points are within 200 m.

Each part in the catalog has `trail` and `trail_stage`; a section has `trail` only. `trail` is the
long route that lists it at the lowest level of long routes inside each other, 1 for a child
relation of the long route itself; of two at the same level, the one with the lower relation ID.

An object of `parts` has these fields. A field with no value is absent.

| Field | Value |
| --- | --- |
| `id` | Relation ID of the part, or the `id` of a section |
| `stage` | Stage number of the part; absent for a section of a cut line that is no stage |
| `name` | The OSM `name` of the part |
| `length_m` | The `length_m` of the part's record; for a part not in the catalog, the length of its main line |
| `ascent_m` | The `ascent_m` of the part's record |
| `end_m` | The sum of `length_m` of this part and each part before it; a part without `length_m` adds 0 |
| `cells` | The `cells` of the part's record |
| `missing` | `true` when the part is not in the catalog |
| `cut` | `true` when the builder cut the line at the end of the part |
| `start_udeg`, `end_udeg` | Part not in the catalog only: its first and its last end |
| `backwards` | `true` when the part runs backwards |
| `jump` | `true` when the part has a jump |

Entry `k` of `trail_profile_m` is the height at `k / 200` of the whole trail, from its start (entry
0) to the last `end_m` (entry 200). The place along the trail lies on the first part whose `end_m`
reaches it and whose `length_m` is positive. The entry is the entry of that part's `profile_m`
nearest to the place, counted from the part's trail start, so a part that runs backwards counts from
its last vertex. The entry is `null` on a part not in the catalog, and where the height is not known.

A client loads the parts in the catalog by their IDs from the files of their `cells`. It joins the
plans of a stretch of parts in order, each part that runs backwards with its plan points in reverse
order. A stage finish and the next stage start are one point when they are the same vertex, and two
points otherwise. Each stage turnaround moves to its position in the joined plan. A stretch starts
at the trail start of a part in the catalog, ends at the trail end of a part in the catalog, and has
no jump after its first part. A part not in the catalog, or a run of such parts, inside a stretch is
an ordinary leg from the trail end of the part before to the trail start of the part after. A trip
can end a day at the trail end of each part without `cut`. A client plans a whole complete long
route only when its joined plan has at most 64 points; otherwise it offers its stages only. A reader
that needs the profile of a complete long route joins the profiles of its stages, and places the
entries of each stage by the `length_m` of the stage.

### Sections

When the plan of a route fails and its main line is longer than 60 km, the builder cuts its line, the
main line with the routed patches of its gaps, into ⌈length / 60 km⌉ sections. Each cut is at the
line point nearest to an equal share of the length, so each section ends at the point where the
next section starts. The builder plans each section as a route, by the rules of [Plan](#plan). A
section is never a loop. A line that needs more than 99 sections is not cut. A route whose plan
passes stays one route record, also when it is longer than 60 km.

A section is a route record with the tag fields of its relation, `section` and `trail`, and no
`parent`, `stage` or `trail_stage`. Its `id` is −(relation ID × 100 + `section`). So each section
has a key of its own, and the cut relation keeps its relation ID.

When no section passes, or the line is not cut, the relation leaves the catalog as a whole, with the
reason of its whole line. Else the relation becomes a cut long route: its parts are its sections in
order, with no `stage`, and each part but the last has `cut`. A section that fails is a part not in
the catalog, with the first and the last point of its line as its ends. When the cut relation is a
stage of a trail, its cut long route has `trail` and `trail_stage`, and in the `parts` of the trail
its sections take its place, each with the stage number and the name of the stage. A cut is no
place, so a trip ends no day there.

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
 "rank":1,"ref":"MW","stage":1,"symbol":"yellow:white:yellow_diamond","trail":2345678,
 "trail_stage":1,"via":[2]}
```

Its long route, with two stages, as in a cell file:

```json
{"ascent_m":420,"cells":["9-267-178"],"descent_m":449,"grades_m":[6830,2000,0,0,0,0],
 "hardest":1,"id":2345678,"kind":"hiking","length_m":8830,"loop":false,
 "name":"Mühlbach-Weg","operator":"Schwarzwaldverein","rank":1,"ref":"MW","stage_count":2,
 "stages":[1234567,1234568],"start_udeg":[8004512,47873120],
 "symbol":"yellow:white:yellow_diamond"}
```

The same long route with four stages, as in the region file, when stage 3 left the catalog after
the builder made its main line. The long route is not complete, so it has no `stages` and no
`start_udeg`, and its stages have no `parent` and no `stage`. `length_m` is the sum over the parts in
the catalog; the whole trail is the last `end_m`. `trail_profile_m` is shortened here:

```json
{"ascent_m":610,"cells":["9-267-178"],"descent_m":655,"grades_m":[9930,3200,0,0,0,0],
 "hardest":1,"id":2345678,"kind":"hiking","length_m":13130,"loop":false,"name":"Mühlbach-Weg",
 "operator":"Schwarzwaldverein","parts":[
  {"ascent_m":184,"cells":["9-267-178"],"end_m":3710,"id":1234567,"length_m":3710,
   "name":"Mühlbach-Weg, Etappe 1","stage":1},
  {"ascent_m":236,"cells":["9-267-178"],"end_m":8830,"id":1234568,"length_m":5120,
   "name":"Mühlbach-Weg, Etappe 2","stage":2},
  {"end_m":13650,"end_udeg":[8051210,47840730],"id":1234569,"length_m":4820,"missing":true,
   "name":"Mühlbach-Weg, Etappe 3","stage":3,"start_udeg":[8043980,47851020]},
  {"ascent_m":190,"cells":["9-267-178"],"end_m":17950,"id":1234570,"length_m":4300,
   "name":"Mühlbach-Weg, Etappe 4","stage":4}],
 "rank":1,"ref":"MW","stage_count":4,"symbol":"yellow:white:yellow_diamond",
 "trail_profile_m":[620,624,629,"…",655,null,null,"…",null,604,"…",612]}
```

A section, the second of a relation of 131 km, and its cut long route as in the region file
(shortened):

```json
{"id":-345678902,"kind":"hiking","length_m":43610,"line_udeg":["…"],"loop":false,
 "name":"Grat-Höhenweg","rank":2,"section":2,"trail":3456789,"via":["…"],"…":"…"}
{"id":3456789,"kind":"hiking","length_m":130920,"loop":false,"name":"Grat-Höhenweg","parts":[
  {"cut":true,"end_m":43650,"id":-345678901,"length_m":43650,"name":"Grat-Höhenweg","…":"…"},
  {"cut":true,"end_m":87260,"id":-345678902,"length_m":43610,"name":"Grat-Höhenweg","…":"…"},
  {"end_m":130920,"id":-345678903,"length_m":43660,"name":"Grat-Höhenweg","…":"…"}],
 "rank":2,"stage_count":0,"…":"…"}
```
