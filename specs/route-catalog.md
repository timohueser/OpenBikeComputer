# Route catalog

The route catalog lists the signed routes of a region: OSM route relations for hiking, foot, bicycle
and mountain-bike routes, and, when the target has the part `route-sources`, the routes of the
second sources of the region's countries, such as the SchweizMobil routes in Switzerland (see
[Second sources](#second-sources)). Each route record
also holds the plan that reproduces the route. A relation with a member outside the extract leaves
the catalog.

- Version: `format` `1`. A new field does not change the format. A reader ignores a field that it
  does not know.
- Writers: the region prepare step, `planner/router-build` (`src/catalog/mod.rs`,
  `src/catalog/connector.rs`, `src/catalog/sources.rs`, `src/catalog/merge.rs`,
  `src/catalog/trails.rs`, `src/grid.rs`); the facts files: `src/facts/`. The routes of the second
  sources come from `route-sources.json` of the step `planner/route-sources`
  ([obc-data.md](obc-data.md#route-sources)).
- Readers: the planner search, online and offline: the web planner
  (`builder/web/src/lib/planner/route-catalog.ts`) and its Explore page
  (`builder/web/src/pages/Explore.svelte`), the companion app (`OBCPlanner/SignedRoutes.swift`).
  A reader that draws or lists a record with `credit` shows the credit with it: on its card, and
  among the credits of a map, a 3D view or a list that shows it.
- Test vectors: [`vectors/signed-routes.json`](vectors/signed-routes.json).
- Related: [route API](route-api.md), [planner release](planner-release.md), [offline
  bundle](planner-offline.md).

## Files

| Release | Path | Content |
| --- | --- | --- |
| Regional | `routes/REGION.json` | Every record of the region |
| Grid | `routes/tiles/9-X-Y.json` | One file for each grid cell |
| Grid | `routes/tiles/trails/7-X-Y.json` | Far file: the trails of one zoom 7 cell (see [Trail files](#trail-files)) |
| Grid | `routes/tiles/long/ID.json` | Trail record file: the whole record of one trail (see [Trail files](#trail-files)) |
| Grid | `routes/tiles/facts/9-X-Y.json` | Facts file: the facts of the routes of one grid cell, only with the layer `route-facts` (see [Facts files](#facts-files)) |

A region file, a cell file and a trail record file are UTF-8 JSON: `{"format": 1, "routes": [...]}`.
The records are in ascending `id` order. A far file has its own form (see [Trail files](#trail-files)). A
grid release has no region file. A regional release has no trail files: its region file holds
every record whole.

A grid cell is a Web Mercator tile at zoom 9 whose box overlaps the release bounds with a positive
area. Each grid cell has a file. A cell with no routes has an empty `routes` list. Equal files share
one object in the pool. A record is in the file of each grid cell in its `cells`, with all of its
fields; a long route is there without `parts` and `trail_profile_m`, which only the region file, the
trail record files and the routing package hold.

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
| `id` | The key of the origin of the record's line: OSM relation ID; for a section, −(relation ID × 100 + `section`); for a record of a second source, −(origin × 10¹² + key × 100 + stage) (see [Ids](#ids)). A negative `id` is no OSM relation ID: no reader builds an OSM link from it |
| `kind` | The OSM `route` value: `hiking`, `foot`, `bicycle` or `mtb`; for a record of a second source, the kind of its route: `hiking`, `bicycle` or `mtb` |
| `name`, `ref` | The OSM tag text; for a record of a second source, the text of the source |
| `operator` | The OSM tag text; for a record of a second source, the text of the source, else by [Words](#words) |
| `description` | The OSM tag text, at most 200 characters; for a record of a second source, the text of the source |
| `website` | The OSM `website` text, else `contact:website`, else `url`; for a record of a second source, the text of the source, else by [Words](#words) |
| `symbol` | The OSM `osmc:symbol` text; for a record of a second source, by [Words](#words) |
| `rank` | Network level from `network`: `4` for `iwn` and `icn`, `3` for `nwn` and `ncn`, `2` for `rwn` and `rcn`, `1` for `lwn` and `lcn`, else `0`; for a record of a second source, the level of the source, else `0` |
| `loop` | `true` for a loop, `false` for a one-way route |
| `length_m` | Length of the plan route |
| `ascent_m`, `descent_m` | Climb and descent of the plan route |
| `moving_s` | Route record: the moving time of the plan route at the default pace of its plan profile, in whole seconds; long route: the sum over its parts in the catalog |
| `grades_m` | `hiking`, `foot` and `mtb` only: 6 lengths, one for each grade |
| `hardest` | Index in `grades_m` of the hardest explicit grade |
| `cells` | Sorted IDs `9-X-Y` of the zoom 9 cells that the plan route touches |
| `line_udeg` | Route record only: the simplified plan route, flat |
| `profile_m` | Route record only: the height of the plan route in metres at each 100 m from its start, and at its end |
| `via` | Route record only: ascending indices of the shaping vertices in `line_udeg` |
| `turnarounds` | Route record only: the entries of `via` where the plan route turns back, and `0` for a loop that turns back at its start |
| `parent` | Stage of a complete long route only: relation ID of that long route |
| `stage` | Stage of a complete long route only: the stage number of its part in `parent` |
| `trail` | Part of a long route only: relation ID of the long route that lists it at the lowest level; for a section, relation ID of its cut line |
| `trail_stage` | Part of a long route only, not a section: the stage number of its part in `trail` |
| `section` | Section only: its number in `trail`, from `1` |
| `stages` | Complete long route only: the `id` of each of its parts, in order; two parts can have one stage number |
| `start_udeg` | Complete long route only: `[longitude, latitude]` of its start |
| `parts` | Long route only: each of its parts, in order (see [Long routes](#long-routes)) |
| `stage_count` | Long route only: the number of its stages, the parts of its relation by the table of [Long routes](#long-routes) before any part leaves for a second source; `0` for a cut line |
| `trail_profile_m` | Long route only: 201 heights of the whole trail (see [Long routes](#long-routes)) |
| `credit` | The `attribution` in `data/sources.toml` of the second route source of the record's line; on a long route, the distinct credits of its own line and of its parts, joined by `; `; absent otherwise |

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
route with the Balanced profile. When no grade on the route is above the ordinary limit, both
profiles give a route of the same cost. Cost is not time: on a tie, the route and its time can
differ slightly. A hiking route can have `hardest` 4 or 5 (T5 or T6).
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
more than 30 m from the routed line. The shaping points are points of the main line. The builder
first adds a point to the main line at least every 60 m, so that a shaping point can go near the
middle of each stretch that a route leaves, also on a long straight segment. The builder routes each
leg of the plan alone and adds shaping points while they make the deviation of the legs smaller.
When 8 points in a row for each profile do not make it smaller by 2 %, or when no new point is
left, it removes each point that does not help, and the route of the whole plan decides; when it
fails, the point of its worst leg is added, up to 6 times. The plan route is the route that the
router gives for the plan with one of these profiles: `touring` for `bicycle`, else the only one.
The plan runs in the member order of the relation, also for a loop. A route is a loop when its main
line is a closed ring, or when it has `roundtrip=yes` and its ends are within 200 m. A loop plan
ends at its start, so the last vertex equals vertex 0.

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
position in this list, from `1`, parts not in the catalog included. A child route that left for the
records of a second source has these records as its parts (see [Parts](#parts)). The sections of a cut part take
its place in `parts`, each with its stage number (see [Sections](#sections)). A long route inside another one
is also a long route of its own. A child route with no `name` and no `ref` that is a part gets a
route record with the `name` and the `ref` of the long route that has it as a child relation and
is in the catalog; of two, the one with the lower relation ID. When no such long route has it as a
child relation, it takes the `name` and the `ref` of its `trail`, and with no `trail` it is not in
the catalog, as any route with no `name` and no `ref`. A long route is in the catalog when at least
one of its parts is, unless its parts are separate routes (see below).

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

A long route whose parts are separate routes, such as day loops from one village, is not in the
catalog, and no part has it as `trail` or `parent`. The member order of its parts does not matter.
Two open parts join when one has an end at most 60 m from an end of the other; 60 m leaves room for
the slack of two stage ends at one place. A loop part joins no part: it comes back to its start and
takes the line no further. A part whose plan failed is a loop when the two ends of its main line are
at most 60 m apart. The parts are separate routes when at least two parts have ends, loops included,
and the largest group of open parts that join holds fewer than a third of all parts with ends. With
no loop part, a line with one or two holes in the map data still keeps a piece of at least a third.
A part with no ends counts for nothing. Each long route is judged on its own parts, so a long route
inside another one can leave while the outer one keeps the same parts as its stages.

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
`descent_m`, `moving_s` and `grades_m` are the sums of the values of its parts in the catalog. `hardest` is the
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
| `id` | The `id` of the part's record: a relation ID, the `id` of a section or of a record of a second source |
| `stage` | Stage number of the part; absent for a section of a cut line that is no stage |
| `name` | The OSM `name` of the part's relation; for a record of a second source, its `name` |
| `length_m` | The `length_m` of the part's record; for a part not in the catalog, the length of its main line |
| `ascent_m` | The `ascent_m` of the part's record |
| `moving_s` | The `moving_s` of the part's record |
| `hardest` | The `hardest` of the part's record |
| `end_m` | The sum of `length_m` of this part and each part before it; a part without `length_m` adds 0 |
| `cells` | The `cells` of the part's record |
| `missing` | `true` when the part is not in the catalog |
| `cut` | `true` when the builder cut the line at the end of the part |
| `start_udeg`, `end_udeg` | Part not in the catalog only: its first and its last end |
| `backwards` | `true` when the part runs backwards |
| `jump` | `true` when the part has a jump |

`ascent_m` and `moving_s` of a part are in the direction of its record, also for a part that runs
backwards.

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
can end a day at the trail end of each part without `cut`. A client that sends a plan of more than
64 points in several route requests plans a whole complete long route or a stretch with any number
of points. A client that sends one request for a plan offers the stages only when the joined plan
has more than 64 points. A reader that needs the profile of a complete long route joins the
profiles of its stages, and places the entries of each stage by the `length_m` of the stage.

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

### Trail files

A trail is a record with no `trail` field that has a `stage_count` of 2 or more, or that takes more
than 12 hours of moving time: its `moving_s`. So a long route inside another one can be a trail, and a cut stage is none.

A trail record file is `{"format": 1, "routes": [record]}` with the whole record of one trail, as
in the region file, `parts` and `trail_profile_m` included. So a reader of cell files reads it too.
`ID` is the `id` of the trail.

A far file is one for each zoom 7 cell whose box overlaps the release bounds with a positive area:
`{"format": 1, "tolerance_m": T, "trails": [...]}`. `trails` holds the trails whose far line
touches the cell, in ascending `id` order. A cell with no trails has an empty list. An entry:

| Field | Value |
| --- | --- |
| `id`, `kind`, `name`, `ref`, `symbol`, `rank`, `loop`, `length_m`, `ascent_m`, `hardest` | As in the record |
| `stage_count` | As in the record, when it is positive; absent for a route record |
| `far_udeg` | A list of pieces of the far line in this cell, each encoded as `line_udeg` |

The far line of a trail is the `line_udeg` of each part in the catalog, in order and in the trail
direction (a part that runs backwards reversed), or the `line_udeg` of the route itself. It breaks
at a part not in the catalog, and where a part starts more than 100 m from the end of the part
before it.

The pieces of a far line in a cell are its runs of segments that touch the cell, by the rule of
[Coverage](#coverage) at zoom 7. So the segment that crosses a cell edge is the last segment of a
piece in one cell and the first segment of a piece in the other. The builder simplifies each
piece, with its first and its last segment fixed, within `tolerance_m`: 100 m, doubled while the
file is larger than 400,000 bytes. When the file is larger with each piece reduced to these
segments, the build fails. So no far file is larger than 400,000 bytes, and a far line is within
`tolerance_m` + 10 m of the plan route. A reader draws each piece as a line of its own; the pieces
of one `id` meet on their shared segments.

### Facts files

A facts file holds the facts of the route records of one grid cell for the search of the Explore
page: the car parks, the lifts and the places near each route, the shares of its length, and its
snow window. The optional layer `route-facts` writes them; a release without it has no facts files.
The facts come from the OSM extract of the region bounds with their margin, the routing source,
GLO-30 and, when the release has them, the [canopy archive](trailview-tiles.md#canopy) and
[`snow-seasons`](planner-snow-tiles.md#snow-seasons). They do not change a record.

Each grid cell has a facts file `{"format": 1, "places": {...}, "routes": {...}}`, also a cell with
no routes:

- `routes` maps the `id` of each record of the cell file that has `line_udeg`, as a decimal string,
  to its entry. A negative `id` is a key too. A long route has no entry.
- `places` maps a place id to its place, for each place that an entry of the file names and for the
  car park of each such lift. A place can lie in another cell.

A place id names the OSM element: `n`, `w` or `r` for a node, a way or a relation, and its ID, such
as `n123`. A place has the same id in each file. Place ids sort by element (nodes, ways, then
relations), then by ID. An area is a closed way or a multipolygon relation. A place:

| Field | Value |
| --- | --- |
| `kind` | `parking`, `lift`, `peak`, `hut` (a hut with food), `lake` or `waterfall` |
| `name` | The OSM `name` |
| `ele_m` | The OSM `ele`, a number with or without `m`, rounded to whole metres |
| `at_udeg` | `[longitude, latitude]` in microdegrees: the node; the top station of a lift; for an area, the centroid of its largest outer ring when it lies inside that ring, else the vertex of that ring nearest to the centroid |
| `size` | Car park only: `large` or `small` |
| `valley_udeg` | Lift only: `[longitude, latitude]` of the valley station |
| `parking` | Lift only: the place id of the nearest car park within 300 m of the valley station |

| Kind | OSM object; an object that fits more than one row takes the first |
| --- | --- |
| `parking` | A node or an area with `amenity=parking`, without `access` `private` or `customers`. It is `large` when `capacity` is a whole number of 50 or more, or, when `capacity` is no whole number, when the area has 1,250 m² or more; else `small` |
| `lift` | A way with `aerialway` `cable_car`, `gondola`, `chair_lift` or `mixed_lift`, without `access` `private` or `no`. Its ends are the stations. The top station is the end with the greater height: the `ele` of a node with `aerialway=station` at the end, else the GLO-30 height. A lift without a height at an end, or with two equal heights, is no place |
| `peak` | A node with `natural=peak` |
| `hut` | A node or an area with `tourism=alpine_hut`, or with `amenity` `restaurant`, `cafe` or `biergarten` whose `at_udeg` is not inside an area with `landuse` `residential`, `commercial` or `retail` |
| `lake` | An area with `natural=water` and `water` `lake` or `pond`, with a `name` or with 10,000 m² or more |
| `waterfall` | A node with `waterway=waterfall` |

An entry:

| Field | Value |
| --- | --- |
| `parking` | The 5 nearest `large` and the 5 nearest `small` car parks within 3,000 m of the route |
| `lifts` | The 5 nearest lifts whose top station is within 3,000 m of the route, by the distance of the top station |
| `places` | Each peak, hut, lake and waterfall within 150 m of the route |
| `paved_pct`, `big_road_pct`, `water_pct` | The shares of the samples that are paved, near a big road and by water |
| `canopy_pct` | The share of the samples with a canopy value that are in forest. Absent when fewer than 90 % of the samples have a canopy value, or without a canopy archive |
| `snow_free` | `[first, last]`: the first and the last day step of the snow window (see below); `[]` when no day step is free. Absent when more than 20 % of the samples have fewer than 5 seasons with data, or without `snow-seasons` |
| `lift_ends` | Route that is not a loop only: `[place id, "start"]` or `[place id, "end"]` (see below) |
| `back_s` | Route that is not a loop only: the moving time in the other direction, in whole seconds (see below) |

An item of a list is `[place id, metres]`: the straight-line distance between the place and the
nearest point of `line_udeg`, in whole metres. The distance of an area is to its nearest edge, and 0
when the line enters the area. A list is in the order of the distance, then of the place id. An
empty list is absent. A share is a whole percent of the samples, rounded to the nearest.

The samples of a route are the points of `line_udeg` at each entry of `profile_m`: at `i × 100` m,
with the line scaled to `length_m`, and at its end. A sample is:

- Paved when the nearest way of the routing source within 15 m that has a `highway` class of the
  router and no `area=yes` has a `surface` that the router counts as paved (`asphalt`, `paved`,
  `concrete`, `concrete:plates`, `concrete:lanes`, `paving_stones`), or has no `surface` and is a
  `motorway`, `trunk`, `primary`, `secondary`, `tertiary` (each also `_link`), `unclassified` or
  `residential` way.
- Near a big road when a `motorway`, `trunk` or `primary` way of the routing source without
  `tunnel` (or with `tunnel=no`) is within 200 m. The routing source holds no node outside the
  region bounds.
- By water within 100 m of the edge of an area with `natural=water` and `water` `lake`, `pond`,
  `reservoir` or `river`, or of a way with `waterway` `river` or `canal`.
- In forest when the cell of the canopy archive that holds it is a forest cell with a cover of 5 or
  more (50 %). A cell with the code 65535, or a cell of a tile that the archive does not hold, has no
  canopy value.

The snow window is the longest run of free day steps. A day step is the step of
[`snow-seasons`](planner-snow-tiles.md#seasons), from 0 on 1 September to 182. Each sample reads its
pixel in the tile of the deepest zoom that `snow-seasons` holds there. A sample with fewer than 5
seasons with data is left out. A day step is free when no sample that is not left out had
[snow](planner-snow-tiles.md#byte-values) on it in a quarter or more of its seasons with data. Steps 182 and 0 are neighbours, so the window can cross
1 September; then `first` is greater than `last`. A route that is free on every step has
`[0, 182]`. Of two runs of the same length, the window is the run with the smaller first step.

A route that is not a loop has `lift_ends` when one end of `line_udeg` lies within 300 m of the
valley station of a lift and the other end within 300 m of its top station. The second value names
the end at the valley. Of several lifts, the entry takes the one with the smallest sum of the two
distances, then the smallest place id.

`back_s` is `moving_s` × T(backward) ÷ T(forward), rounded to whole seconds. T(forward) is the sum
over the steps between the entries of `profile_m` of the moving time of the pace model of `moving_s`
at its default pace for the kind: walking for `hiking` and `foot`, cycling for `bicycle` and
`mtb`, with the pace of `mtb` for `mtb`. A step is 100 m, the last step the rest of `length_m`; its
rise is the difference of its two entries, and 0 when one of them is `null`. T(backward) uses the
negative rises. A flat route has `back_s` = `moving_s`, and so has a route without `profile_m`.

### Map view

A map view is a box of longitude and latitude. Its cells are the zoom 9 cells whose box intersects
the view, edges included. A client loads no more than 12 cell files for one view. When the view has
more cells, the client loads none and shows no routes. A route record is in the view when its
`kind` fits the activity and a vertex of its `line_udeg` is in the box, edges included. A long
route is never in the view, but its stages can be. Each record is in the view once, also when
several of its cells are loaded. A cell outside the covered set adds no record. Test vectors:
[`vectors/route-boxes.json`](vectors/route-boxes.json).

A far view is a view with more than 12 zoom 9 cells and at most 12 zoom 7 cells. It reads the far
files of its zoom 7 cells, and a trail record file when the rider selects a trail. A trail is in the
far view when a segment of a piece of its far line meets the box, edges included: a segment of a
simplified far line can be longer than the view. Each trail is in the
view once, with the pieces of all far files of the view.

## Second sources

A second source is an official source of signed routes, such as SchweizMobil, with a description
in `data/routes/`. The step `planner/route-sources` reads the sources of the region into
`route-sources.json` ([obc-data.md](obc-data.md#route-sources)). The builder makes a record of a
source route as it makes one of an OSM relation, and merges the routes of all origins so that each
route shows once. The official route takes the place of the OSM relation of the same route. A
region with no second source, or a target without the part `route-sources`, has the catalog of its
OSM relations only.

### Origins

OSM is origin 0. A second source has the `origin` of its description. The rank of the origins is
each second source in the order of `route-sources.json`, which is the order of `data/sources.toml`,
and then OSM. "Earlier" means earlier in this rank.

### Ids

A section has the `id` −(relation ID × 100 + section), so −10¹² < `id` < 0 while relation IDs are
below 10¹⁰. A record of a second source has the `id` −(origin × 10¹² + key × 100 + stage), with the
origin from 1 to 9006, the key of its route below 10¹⁰ and the stage from 0 (a route) to 99. So
−(9007 × 10¹²) < `id` ≤ −10¹², and the two ranges do not meet. A reader that needs the number of a
section reads `-id % 100` of an `id` in the first range only. A record has the `id` of the origin
of its line, so it keeps its `id` when an OSM relation of its route comes, goes or splits.

### Jobs

A job is one OSM relation, as the builder plans it today; one source route without `stages`; or
one stage of a source route. A stage job takes the `ref`, the `rank` and the `kind` of its route,
its own `name` and `ascent_m`, and the `description`, `operator` and `website` of its route when it
has none of its own. A stage with no members is no job.

### Line of a source job

Each point of a member is a node, and points at the same microdegree point are one node. The
builder joins the members by the rules of [Plan](#plan): the member order, the nearest-end chain
with joins of at most 500 m, and the side branches. A job with a point outside the bounds of the
release leaves as `ExtractEdge`. A source job is a loop when the two ends of this main line are
within 200 m.

The builder puts points into each run of the main line so that no two points are more than 10 m
apart. A cut is a stretch of these points that are more than 30 m from every road that the first
plan profile of the kind may use: `hiking/alpine` for `hiking`, `mtb/technical` for `mtb`,
`touring` for `bicycle`. Such a road is a way of the routing source with a road of the package that
the profile may use, in either direction. Points within 30 m of such a road for less than 100 m
between two cuts belong to the cut, so a lift or a boat line that crosses a path is one cut. The
length of a cut is the length of the line from the last point before it to the first point after
it, or to the end of the run.

- A cut longer than 500 m drops the job as `Gap`, also at an end.
- A shorter cut takes its points out of the line. Inside the line, the router patches the gap as
  for an OSM route. The patch bound measures against the main line before the cut: a patch with
  more than 500 m farther than 30 m from it drops the job as `Gap`. At the start or the end, the
  line gets shorter.
- For a loop, the start and the end are one place: a cut at the start and a cut at the end that are
  longer than 500 m together drop the job.

So a stretch of up to 500 m on a path that the map closes to the bike type of the route is cut, the
router goes around it, and the official route stays.

The length of a source job is the length of its runs after the cut; a job under 1 km leaves as
`Short`. The shaping budget is 2 % of that length, as for every route. A loop plan ends at its
start: when the cut made an end shorter, the router patches the gap of at most 500 m between the
two ends. A source line is never cut into sections, so a source line of more than 60 km whose plan
fails leaves with its reason.

### Match lines

The match line of a job is a set of pieces, with no line between two pieces:

- for an OSM job, each main member way that the import holds, as a piece in its node order, also
  for a job that leaves as `ExtractEdge` or whose plan fails;
- for a source job, the runs of its main line before the cut.

The share of a match line along a set of match lines is the share of its points, at each 10 m of
each piece, that lie within 30 m of a piece of the set.

### Pairs

Two jobs of different origins agree when they have the same kind family (`hiking` and `foot` are
one), the same `rank` when both are above 0, and either both have a `ref` and the refs agree, or
one has no `ref`, both have a name, and the names agree. Two refs agree when they share a number (a
run of digits and dots, read as the whole numbers between its dots: `SM 561` and `561`, `1;EV17`
and `1`; `4.1` agrees with neither `4` nor `1`), or, when neither has a digit, when their letters
and digits are equal in lower case. Two names agree when their letters and
digits are equal in lower case.

For an OSM relation that is a part of a long route, the `ref` is its own or else that of its long
route, and the name is that of its long route. Its long route is the one that lists it at the
lowest level, of two the one with the lower relation ID. For a source stage, the `ref` and the name
are those of its route.

Two jobs are a pair when (a) they agree and the match line of one lies at least a fifth along the
other, or (b) they have the same kind family and each match line lies at least 80 % along the
other, whatever their refs, names and ranks.

### Who stays

A job of a later origin leaves (`Replaced`) when the kept records of earlier origins that are its
pairs together cover at least four fifths of its match line. For a part of a long route, only its
pairs by (a) count: a pair by (b) is another route on the same path, which cannot take its place.
A kept record never leaves for a job of a later origin, so an official record never leaves for an
OSM job.

So no route leaves the catalog for another one unless the match lines of the records that stay cover
four fifths of its match line: the rule counts only kept records of earlier origins, and nothing
removes such a record later, as `named` drops only unnamed OSM stages and the builder cuts no source
line into sections. The line that a record shows can differ from its match line where its plan goes
around a cut of [Line of a source job](#line-of-a-source-job). A job that fails removes nothing. The
builder plans all source jobs first. Then it applies the rule to the jobs of the second and later
sources, in their order, and to the OSM jobs, so an OSM job that leaves needs no search. Then it
plans the OSM jobs that stay.

A route can show twice: an OSM job that its pairs cover by less than four fifths stays beside them.
Examples are two stagings of one route with other stage ends, such as an official stage and an OSM
stage of one trail that lie three quarters along each other, and a long OSM route with the number
of a short official route. To remove one could remove a stretch that shows nowhere else.

A name can show nowhere: a job that is no part of a long route can leave for an official record of
another name on the same path, a pair by (b), such as a EuroVelo route that is a relation of its
own, listed by no long route of the extract, on the line of a national route with another number.

### Successors

The successors of a job that left are its pairs by (a). A pair by (b) is another route on the same
path, so it is no successor. So a regional OSM stage on a path that a national route shares has the
regional official stage as its successor, not the national one, and a regional official stage never
takes the place of a national stage.

### Words

A record that stays takes each of `operator`, `website` and `symbol` that it lacks from one job
whose successor it is (not `description`: an OSM description often tells how the relation is
mapped, such as "the route is not complete"): the job whose match line covers the largest share of the
record's match line; of two equal shares, the job with the smaller absolute `id`. An OSM job gives
its tags by the rules of [Records](#records). A record whose plan route has a vertex in France takes
no `symbol`.

### Parts

A part of an OSM long route whose job left points to its successors, in the order of
their starts along it: the place of the point of its match line nearest to the start of each
successor, by the member order of its pieces, then along the piece. Each successor is a part with
the stage number of the part that left. A record appears once in a long route, at its first place,
and a part whose successors all stand at earlier places gives no part. So two parts can have one
stage number (one OSM stage over two official stages), and a number can have no part (one official
route over three OSM stages takes the number of the first). A part with no successor is a part not
in the catalog, without ends. Then the rules of [Long routes](#long-routes) apply as to any part,
so an official stage that runs against the trail is `backwards`, and the long route is not
complete.

A reader shows "Stage n of N" with n the stage number of the part and N the `stage_count` of the
long route, and "K of N stages", with K = N less the stage numbers that only parts not in the
catalog have, or 0 when no part is in the catalog.

### Record

A record of a second source has the fields of a route record from its plan, `moving_s`, `hardest`
and `profile_m` included; its `id` by [Ids](#ids); its `kind`; `rank`, `name` and `ref` from the
source; `description` from the source; `operator` and `website` from the source, else by
[Words](#words); `symbol` by [Words](#words); and `credit`. It has `trail` and `trail_stage` only from an OSM long route that
lists it, and `parent` and `stage` only from a complete one. A long route with a part that has
`credit` has the distinct credits of its parts in the catalog, in part order, joined by `; `.
`planner/route-facts` gives a record of a second source its facts entry as any record, under its
`id`.

## Examples

A one-way stage of 3.7 km in cell `9-267-178`, with one shaping point:

```json
{"ascent_m":184,"cells":["9-267-178"],"descent_m":251,"grades_m":[2460,1250,0,0,0,0],
 "hardest":1,"id":1234567,"kind":"hiking","length_m":3710,
 "line_udeg":[8004512,47873120,9210,-6400,8800,-11500,6500,-4200,5510,-2100],
 "loop":false,"moving_s":4380,"name":"Mühlbach-Weg, Etappe 1","operator":"Schwarzwaldverein",
 "parent":2345678,
 "profile_m":[620,618,625,631,640,652,661,668,676,684,690,699,708,716,725,733,741,748,752,
              760,768,773,779,784,788,780,768,751,733,712,694,671,650,628,610,590,571,555,553],
 "rank":1,"ref":"MW","stage":1,"symbol":"yellow:white:yellow_diamond","trail":2345678,
 "trail_stage":1,"via":[2]}
```

Its long route, with two stages, as in a cell file:

```json
{"ascent_m":420,"cells":["9-267-178"],"descent_m":449,"grades_m":[6830,2000,0,0,0,0],
 "hardest":1,"id":2345678,"kind":"hiking","length_m":8830,"loop":false,"moving_s":10500,
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
 "hardest":1,"id":2345678,"kind":"hiking","length_m":13130,"loop":false,"moving_s":15240,
 "name":"Mühlbach-Weg","operator":"Schwarzwaldverein","parts":[
  {"ascent_m":184,"cells":["9-267-178"],"end_m":3710,"hardest":1,"id":1234567,"length_m":3710,
   "moving_s":4380,"name":"Mühlbach-Weg, Etappe 1","stage":1},
  {"ascent_m":236,"cells":["9-267-178"],"end_m":8830,"hardest":1,"id":1234568,"length_m":5120,
   "moving_s":6120,"name":"Mühlbach-Weg, Etappe 2","stage":2},
  {"end_m":13650,"end_udeg":[8051210,47840730],"id":1234569,"length_m":4820,"missing":true,
   "name":"Mühlbach-Weg, Etappe 3","stage":3,"start_udeg":[8043980,47851020]},
  {"ascent_m":190,"cells":["9-267-178"],"end_m":17950,"hardest":1,"id":1234570,"length_m":4300,
   "moving_s":4740,"name":"Mühlbach-Weg, Etappe 4","stage":4}],
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
