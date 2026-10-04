# Regional route builder

Build the package with Rust. The input is one or more local OSM PBF files.
Choose bounds inside the source extract, with room around the intended trip.
The importer clips the graph at those bounds. It does not fetch missing data.

```sh
cargo run --release -p route-build -- \
  /data/freiburg-regbez.osm.pbf --output /data/routes/freiburg \
  --region freiburg --country DE --bounds 7.5,47.7,8.5,48.4 --profiles all \
  --countries DE
```

The output directory must not exist. The builder writes objects to a temporary
directory and publishes the manifest with a directory rename. Keep old packages
until their consumers finish. Do not edit an installed package in place.

The builder also writes `overlays.sqlite` into the output directory: the route
networks and access restrictions of the source data, bound to the new manifest
identity. The [planner release contract](../../specs/planner-release.md) gives
its layout. The source OSM objects stay in memory only; the package does not
keep them.

## Route catalog

With `--countries`, the builder also writes `route-catalog.json`, the signed
routes in the format of [the catalog contract](../../specs/route-catalog.md). It
prints a JSON report: the counts, the dropped relations by reason and the
shaping points. The catalog needs the `touring`, `road`, `gravel`, `mtb` and
`hiking` profiles. Name the region's countries, so that the catalog can omit
route marks in France. When France is the only country, no record has a route
mark. A region with France and another country fails. The builder checks these
options before the import.

## Terrain

The base crate accepts a height callback through `terrain::apply`. It has no
terrain provider dependency. The optional `obc-terrain` adapter uses the OBC
bare-earth archive and local Copernicus GeoTIFFs:

```sh
cargo run --release -p route-build --features obc-terrain -- \
  /data/freiburg-regbez.osm.pbf --output /data/routes/freiburg \
  --region freiburg --country DE --bounds 7.5,47.7,8.5,48.4 --profiles all \
  --reference /data/obc-reference --dem /data/copernicus
```

The ground archive takes precedence. Preserve its source credits with the
package. Terrain samples are at most 20 m apart. A symmetric 20 m filter leaves
junction heights and missing samples intact. Bridges and tunnels interpolate
between their endpoints. Without terrain, climb and slope remain unknown.

## Supported import policy

| Input | Policy |
| --- | --- |
| Country defaults | Germany only; other country codes fail |
| Access and direction | Separate cycling, walking and pushing; pushing follows foot access unless `bicycle:pushing` restricts it |
| Node identity | Shared OSM node IDs create junctions; geometry crossings do not |
| Via-node turn restrictions | Mode-specific forbidden transitions |
| Via-way restrictions | Exclude affected member roads for the restricted mode |
| `yes`, `designated`, `official`, `permissive`, `mtb`, `optional_sidepath` | Keep the mode |
| `no`; `dismount` for riding | Exclude the mode |
| Any other access value, on a way or a node | Keep the mode at three times the road cost, or 300 more at a node; the route reports a possible closure (`route_build::source::classify`) |
| Conditional access | Keep the mode; the route reports a possible closure |
| Conditional turns | Ignored; a turn restriction never closes its member roads |
| Barrier nodes | Pass gates, bollards, toll booths, cattle grids, kerbs and blocks; stiles admit walkers; unknown barriers admit walking and pushing, and the route reports them |
| Bridge and tunnel | Preserve structure flag for terrain |
| Source data | All tags, way node IDs, relation members and roles feed costs, closures, overlays and the catalog |
| Road suitability | Highway, surface, smoothness, tracktype and difficulty remain distinct |

Read the manifest warnings before publishing. The importer does not yet model
all OSM semantics. It has no ferry schedule, opening-time evaluator, general
via-way automaton or country-default catalogue. Do not use
this German regional importer as a worldwide release pipeline.

## Profiles

Pass `--profiles all` to prepare all 21 metrics, or supply comma-separated IDs.
Rebuild packages after changing the profile code. Rider validation is required.

| Road profile | Preference |
| --- | --- |
| `road` | BRouter fastbike road classes, turns and downhill costs |
| `road/shorter` | Same road suitability; no terrain penalty; lower turn cost |
| `road/smoother` | Higher costs for rough surfaces and poor smoothness |
| `road/less-climbing` | Additional uphill cost above the slope threshold |
| `road/quieter` | Road class, signed speed, cycle lanes and bicycle routes |

Road weights derive from the MIT-licensed BRouter
[fastbike](https://github.com/abrensch/brouter/blob/29898106b555e342ff3ade7ae3e9c1ae6644a43b/misc/profiles2/fastbike.brf),
[trekking](https://github.com/abrensch/brouter/blob/29898106b555e342ff3ade7ae3e9c1ae6644a43b/misc/profiles2/trekking.brf)
and [gravel](https://github.com/abrensch/brouter/blob/29898106b555e342ff3ade7ae3e9c1ae6644a43b/misc/profiles2/gravel.brf)
profiles. Preserve [the licence notice](LICENSE.brouter) when redistributing them.

| Adaptation | Behavior |
| --- | --- |
| Missing surface | Use highway context; no universal unknown-surface penalty |
| Explicit poor surface | Apply a minimum cost even on a paved road class |
| Smoothness | Apply gravel-profile dry-surface multipliers |
| Elevation | Filtered fractional heights; 1.5% slope threshold; additive costs |
| BRouter elevation buffer | Not reproduced; no path-dependent state in the graph |
| Estimated traffic, forest and noise | Not inferred from BRouter's derived data |
| Access | German importer rules; restricted access is not a soft penalty |
| Pushing | Allowed where permitted, with distance and entry costs; no cycling permission implied |

Touring, gravel, MTB and hiking use separate surface and road-class tables.
Each has shorter, smoother and less-climbing variants. Shorter ignores road
class and climb preferences but keeps surface preferences. Smoother raises
surface costs. Access rules apply to every variant.

The package includes geometry, snap cells, a shared directed graph, profile
costs, and compressed landmark bounds. It is separate from map tiles. See [the package contract](../../specs/route-package.md).
Preserve OpenStreetMap attribution and ODbL notices when distributing the data.

```sh
obc test -p route-build
cargo clippy -p route-build --all-targets --all-features -- -D warnings
```
