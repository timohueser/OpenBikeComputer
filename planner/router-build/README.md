# Regional route builder

`obc data` builds the `planner/routing` layer of a region with this crate, in four steps
(`step.rs`). The [package contract](../../specs/route-package.md) defines the output.

## Build

The step reads the OSM PBF files of the region and clips the graph at the region bounds. It does
not fetch missing data. The nodes of the input must come in ID order, as `osmium sort` writes
them; else the import stops. `data/planner.toml` lists the profiles. To build the layer again after
a code change, run:

```sh
obc data update local --only planner/routing
```

The step writes a new layer directory. Do not edit an installed package in place.

The roads step prints the counts of the roads and turns, and stops when the package cannot number
them.

## Layers

The store keeps only the work layer of each step and options that a run used last (`cache`).
`planner/routing` and `planner/route-facts` (part `route-facts`) are client layers.

| Step | Runs | Layer |
| --- | --- | --- |
| `planner/routing-source` | Pass 1, the node join, the way facts, the turn rules | `source/` |
| `planner/routing-roads` | The walks, the partition into cells, the order with the terrain, the turn restrictions, the topology | `roads/` |
| `planner/routing-costs` | The way bases, the road pass, the snap bits | `costs/` |
| `planner/routing` | The package, the overlay index, the route catalog and the routes of each grid cell | `routing/`, `cells/`, `routes/` |
| `planner/route-facts` | The places of the OSM extract, the shares and the facts of each route | `facts/` |

| Change | Steps that run |
| --- | --- |
| A stop or a crash in a step | That step and the steps after it |
| The profile list, or the revision of `planner/routing-costs` | Costs, package, facts |
| `region`, `attribution`, `download_level` | Package, facts |
| The GLO-30 tiles or the reference, `cell_sizes`, or the revision of `planner/routing-roads` | Roads, costs, package, facts |
| The OSM cut, the bounds, the format of the work layers, or the revision of `planner/routing-source` | All five |
| The canopy archive, `snow-seasons`, or the revision of `planner/route-facts` | Facts |

A change of the bytes of a work layer changes its format, of its content its revision. A column
is one file of little-endian values; `layout.json` gives its type and count (`column::Layout`).
The columns take, without the tags and the closure sets:

| Layer | Bytes |
| --- | --- |
| `planner/routing-source` | 48 for each way, 24 for each node reference |
| `planner/routing-roads` | 56 for each road, 12 for each shape point, 4 for each turn, 27 for each junction, 4 for each leaving border road of each level |
| `planner/routing-costs` | For each profile, 8.125 for each road and 4 for each turn; and for each cell of each level, a cost table: 16, and 1, 2 or 4 for each pair of an entering and a leaving road, twice with the copy by column |

```sh
obc test -p planner-router-build
cargo clippy -p planner-router-build --all-targets --all-features -- -D warnings
```

```sh
# The package on the roads/ and costs/ folders of a bake: the bytes of each part and column, the
# objects, the long roads and the boxes; then the install of all cells and the check of each object.
cargo run --release -p planner-router-build --example cells -- write ROADS COSTS OUT [ZSTD_LEVEL]
cargo run --release -p planner-router-build --example cells -- install OUT TARGET [THREADS]
```

The builder also writes `overlays.sqlite` into `routing/`: the route networks and access
restrictions of the source data, bound to the identity of the package. The [planner release contract](../../specs/planner-release.md) gives its layout. The
package does not keep the source OSM objects. The index omits proposed routes. Access markings are
snapshot data, not live closures.

| Overlay feature | Minimum zoom |
| --- | --- |
| National or international route | 6 |
| Regional route | 8 |
| Local route | 11 |
| Construction or conditional access | 10 |
| Other access restriction | 13 |
| Directional rule | 14 |
| Pushing section | 15 |

## Work files and memory

The bake keeps its large tables in memory-mapped work files and sorts them on disk.

| Setting | Value |
| --- | --- |
| Work folder | `OBC_DATA_WORK`, else `TMPDIR`, else the temporary folder of the system; `obc data` sets `TMPDIR` to a folder beside the output |
| Bake folder | `obc-router-build-<process ID>-<n>` in the work folder, with the first free `n`; a step removes it at the end, also after an error. A step moves the columns that it keeps into its layer by a rename |
| Page budget | The smallest of the free memory of the bake's cgroup v2 or of a parent (`memory.max` less `anon`), `MemAvailable`, and `OBC_DATA_MEMORY_BYTES` less the bake's own `RssAnon`; read once at the start, and printed with its source; a value that is not a number of bytes stops the bake |
| Sort run | A quarter of the page budget, from 1 MiB to 4 GiB |
| Order buckets | The roads of one bucket, with their shapes before and after the terrain, take about one sort run; 1 to 256 buckets |
| Threads | All cores, or `RAYON_NUM_THREADS`; the bytes of the package are the same for each thread count |
| Road pass | The profiles of one group at a time; the way bases of a group take at most a quarter of the page budget |
| Profile jobs | The snap bits of one profile in each job; the page budget sets how many jobs run at a time |
| Cost tables | One table in each task; a batch of tables holds at most 64 MiB of values at 8 B each, or one table for each thread when that is more |
| Own memory | 4 GiB; the step samples `RssAnon` every 100 ms and stops at the first sample above it, with the name of the stage; `catalog` is exempt |
| Partition | 48 B of RAM for each junction and 6 B for each arc, about 64 B for each junction at about 2.67 arcs for each junction, outside the 4 GiB: the stage `partition` has the budget of this need plus 256 MiB. A need above the page budget stops the roads step |
| Disk need | The stored layers above, and the work files of the step that runs; the roads step needs the most. The route catalog installs the package of its bake in the bake folder: the installed bytes of the package, about 1.7 times its objects |

Put the work folder on a disk, not on a tmpfs, whose files take RAM. A killed bake
leaves its bake folder; remove it. A bake that the memory guard stops also leaves it.

The costs layer holds the columns and the cost tables of each profile, with the bytes above.
The page budget changes only the speed of the bake, never its bytes. Each step prints the time and
the peak own memory of each stage: `import` (source and roads), `partition` and `emit` (roads),
`open`, `way-bases`, `road-pass`, `snap` and `tables` (costs), `open`, `cells`, `overlays` and
`catalog` (package).

## Route catalog

The builder also writes `route-catalog.json`, the signed routes in the format of
[the catalog contract](../../specs/route-catalog.md). The step metrics are its report: the counts,
the dropped relations by reason and the shaping points. The builder leaves out connector legs
([selection](../../specs/route-catalog.md#selection)). The catalog needs the `touring`, `road`,
`gravel`, `mtb/technical` and `hiking/alpine` profiles. A route in France has no route mark. The
builder checks the profiles before the import. The catalog routes on an install of all cells of the
package of its bake, with one router for each CPU; each router holds only its search.

The step also writes the [trail files](../../specs/route-catalog.md#trail-files) into `routes/trails/`
and `routes/long/`; the metric `trail_files` gives their sizes.

## Terrain

The import samples the terrain while it puts the roads into their final order (`layout::Buckets`).
`terrain::sample` takes a height callback. The step reads heights from the Copernicus GLO-30 tiles
of the bounds through `obc-dem`. Terrain samples are at most 20 m apart. A symmetric 20 m filter
leaves junction heights and missing samples intact. Bridges and tunnels interpolate between their
endpoints. Without a height, climb and slope remain unknown.

## Supported import policy

| Input | Policy |
| --- | --- |
| Country defaults | Worldwide defaults, with the differences in the [OSM wiki country tables](https://wiki.openstreetmap.org/wiki/OSM_tags_for_routing/Access_restrictions) (`planner_router_build::country`); a way takes the country of its first known node |
| Side of the road | A cycle lane without a direction runs with the traffic of the country |
| One-way | Binds riding only; `oneway:bicycle=no` or a contraflow lane opens it; roundabouts imply it; circular junctions need an explicit tag; `alternating` is two-way; `reversible` keeps both directions and the route reports a possible closure |
| Access and direction | Separate cycling, walking and pushing; pushing follows foot access unless `bicycle:pushing` restricts it |
| Node identity | Shared OSM node IDs create junctions; geometry crossings do not |
| Via-node turn restrictions | Mode-specific forbidden transitions |
| Via-way restrictions | Exclude affected member roads for the restricted mode |
| `yes`, `designated`, `official`, `permissive`, `mtb`, `optional_sidepath` | Keep the mode |
| `no`; `dismount` for riding | Exclude the mode |
| Any other access value, on a way or a node | Keep the mode at three times the road cost, or 300 more at a node; the route reports a possible closure (`planner_router_build::source::classify`) |
| Conditional access | Keep the mode; the route reports a possible closure |
| Conditional turns | Ignored; a turn restriction never closes its member roads |
| Barrier nodes | Pass gates, bollards, toll booths, cattle grids, kerbs and blocks; stiles admit walkers; unknown barriers admit walking and pushing, and the route reports them |
| Bridge and tunnel | Preserve structure flag for terrain |
| Source data | All tags, way node IDs, relation members and roles feed costs, closures, overlays and the catalog |
| Road suitability | Highway, surface, smoothness, tracktype and difficulty remain distinct |

The importer does not model all OSM semantics. It has no ferry schedule, opening-time evaluator
or general via-way automaton. Defaults stop at the country: state and local rules need explicit
tags.

## Profiles

The `profiles` option names the profile IDs, or `all` for all 10 metrics.

| Road profile | Preference |
| --- | --- |
| `road` | BRouter fastbike surfaces, traffic stress, turns and downhill costs; trunk roads cost 10 times more |
| `road/less-climbing` | Additional uphill cost above the slope threshold |

Road weights derive from the MIT-licensed BRouter
[fastbike](https://github.com/abrensch/brouter/blob/29898106b555e342ff3ade7ae3e9c1ae6644a43b/misc/profiles2/fastbike.brf),
[trekking](https://github.com/abrensch/brouter/blob/29898106b555e342ff3ade7ae3e9c1ae6644a43b/misc/profiles2/trekking.brf)
and [gravel](https://github.com/abrensch/brouter/blob/29898106b555e342ff3ade7ae3e9c1ae6644a43b/misc/profiles2/gravel.brf)
profiles.

| Adaptation | Behavior |
| --- | --- |
| Missing surface | Use highway and track-grade context; no universal unknown-surface penalty |
| Explicit poor surface | Apply a minimum cost even on a paved road class |
| Smoothness | Apply gravel-profile dry-surface multipliers |
| Traffic | Speed limit, lane count and cycle tracks or lanes in the travel direction; no measured traffic |
| Elevation | Filtered fractional heights; 1.5% slope threshold; additive costs |
| BRouter elevation buffer | Not reproduced; no path-dependent state in the graph |
| Estimated traffic, forest and noise | Not inferred from BRouter's derived data |
| Access | Importer rules; restricted access is not a soft penalty |
| Pushing | Allowed where permitted, with distance and entry costs; no cycling permission implied |
| Steps and via ferrata | Cycling profiles never use steps; no profile uses a via ferrata |

Touring, gravel, MTB and hiking use separate surface and road-class tables, and add smoothness,
track grade, trail grade and traffic. Hiking also adds trail visibility. Touring and gravel have a
less-climbing variant with triple climb cost. A signed cycle route discounts only the preference above the physical and
traffic cost, in the direction that its member role gives. Access rules apply to every variant.
MTB permits S2 at most and `mtb/technical` every grade; hiking permits T2 at most and
`hiking/alpine` every known grade up to T6.

The package holds the roads, their geometry and possible closures, the turns, the costs and snap
bits of each profile, and the cost tables of each cell, separate from map tiles. A profile's snap
bits mark the roads in a large strongly connected part of its graph (`connectivity`).

## Route bench

```sh
# The route measures of a region with the cost code of this checkout: bench.json and bench.md in OUT.
obc exec cargo run --release -p planner-router-build --example route_bench -- run \
  --pbf region.osm.pbf --dem DEM --bounds W,S,E,N --out OUT

# Two checkouts: run each with the same pairs file, for the same panels and for the page of the
# blind comparison.
route_bench run … --pairs ~/bench/pairs.json --out ~/bench/A    # in the first checkout
route_bench run … --pairs ~/bench/pairs.json --out ~/bench/B    # in the second checkout
route_bench compare ~/bench/A/routes.json ~/bench/B/routes.json --names old,new --out compare.html

# Several extracts: one file with its nodes in ID order.
osmium sort first.osm.pbf second.osm.pbf -o region.osm.pbf
```

| Input of `run` | Value |
| --- | --- |
| `--pbf FILE` | One OSM extract with its nodes in ID order |
| `--dem DIR` | The folder with the Copernicus GLO-30 tiles of the bounds |
| `--bounds W,S,E,N` | The region, in degrees |
| `--out DIR` | The folder of the output files |
| `--profiles NAME,…` | The profiles, or `all`. The default is `touring` |
| `--town W,S,E,N` | A draw has the set `town`: both ends of its pairs lie inside this box |
| `--pairs FILE` | The file with the sets of pairs, as coordinates. The run draws the sets into a missing file. With an existing file it draws nothing: it reads each set, so each run with the file routes the same pairs. Adds `routes.json` |
| `--pairs-box W,S,E,N` | Both ends of a drawn pair of the set `compare` lie inside this box. The default is the bounds |

| Output | Content |
| --- | --- |
| `bench.json` | Each number of the run |
| `bench.md` | The same numbers as tables: for each profile the signed routes and the panels, then the price list |
| `routes.json` | For each pair of the set `compare` and each profile the line, the km, the ascent and the measures of the route, or `null` for no route. For each profile the overlay: the lines of the ways of each overlay kind in the box of those pairs, with a margin of about 1 km |
| The page of `compare` | The two routes of a pair on a map of live tiles, in a random order that stays the same. Under them it draws the overlay of the first run, with one checkbox for each kind. It takes one answer for each pair and gives the answers as text. It leaves out a pair with no route in a run, and a pair with the same routes |

| Measure | Definition |
| --- | --- |
| Route | The cheapest roads from a junction to a junction, by an exact search of the bench over the road and turn costs of the bake. The production router snaps a point to a place on a road, so a short route can differ at its two ends |
| Price list | The factor of `cost::way_basis` for a fixed list of tags, with the worldwide access defaults: the cost of 1 m of flat road. A cycling profile also has the factor on a regional and a national cycle route |
| Signed route | A relation with `route=bicycle` for a cycling profile, `route=hiking` or `route=foot` for a walking profile, `type=route` or `superroute`, not `state=proposed` |
| Signed line | The main line of a signed route with at least 5 km in the bounds: the runs of `assemble::main_line`, in the order of `assemble::chain` with gaps of at most 500 m, or in the order of the members where the runs do not chain. Unlike the route catalog, the bench does not prune forks or place side trips. The line lies on the directed roads of its member ways. A gap, a part with no road that the profile may use, and a part against a one-way for cycling cut the line. A piece under 1 km is not measured |
| Leg | The signed line between two points at junctions about every 5 km. One search runs for each leg |
| `near %` | The share of the signed length of the legs within 30 m of the router's line |
| `same road %` | The share on a directed road that the router's route also takes |
| Split | From the first road of a leg that differs to the next road that both lines take. The same two stretches count once |
| Set `panel` | A panel of 100 pairs each within 10 % of 5, 15 and 40 km in a straight line. SplitMix64 with a fixed seed draws junctions where a road with the snap bit of the profile arrives. A drawn pair has a route |
| Set `town` | A panel of 100 pairs of 3 to 8 km |
| Set `compare` | 40 pairs, a third each within 10 % of 6, 12 and 22 km, for the blind comparison |
| Draw | Without `--pairs`, each profile draws its panels. With `--pairs` and no file, the first profile draws each set |
| Pair of a file | Each end snaps to the nearest junction where a road with the snap bit of the profile arrives. A panel counts a pair with no route in `pairs`, not in `routes` |
| `length / shortest` | The length of a route against the shortest route on the roads and turns that the profile may take |
| Turn | A change of heading over 45° at a junction of three or more road ends. The headings are taken over 15 m |
| Leave-and-rejoin event | The next road shares no `name`, `ref` or signed route with the road before, and a road within 2 km of route length shares one again |
| Signal node | A signal at a junction inside the route, not at its two ends |
| Fast main road | Class 2 with a limit over 50 km/h or no tagged limit, and no cycle track by `traffic::protection` |
| Slow main road | Class 2 with a tagged limit of 50 km/h or less |
| `signed route %` | The length on a way of a signed route |
| Overlay kind | `cycle path`: a cycleway, or a `highway=path` or `highway=footway` with `bicycle=designated`. `cycle lane or track`: `traffic::protection` gives a lane or a track in one of the two directions. `signed route`: a way of a signed route. A walking profile has only the last kind |
| Same routes | Each route has at most 10 % of its length farther than 30 m from the other |

## Pitfalls

- Rebuild packages after you change the profile code. Rider validation is required.
- Read the warnings of `package.json` before publishing.
- Preserve the Copernicus source credits with the package, OpenStreetMap attribution and ODbL
  notices with the data, and [the BRouter licence notice](LICENSE.brouter) when you redistribute
  the BRouter weights.
