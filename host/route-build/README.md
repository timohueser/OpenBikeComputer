# Regional route builder

Build the package with Rust. The input is one or more local OSM PBF files.
Choose bounds inside the source extract, with room around the intended trip.
The importer clips the graph at those bounds. It does not fetch missing data.

```sh
cargo run --release -p route-build -- \
  /data/freiburg-regbez.osm.pbf --output /data/routes/freiburg \
  --region freiburg --country DE --bounds 7.5,47.7,8.5,48.4 --profiles all
```

The output directory must not exist. The builder writes objects to a temporary
directory and publishes the manifest with a directory rename. Keep old packages
until their consumers finish. Do not edit an installed package in place.

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
| Access and direction | Separate cycling, walking and pushing permissions |
| Node identity | Shared OSM node IDs create junctions; geometry crossings do not |
| Via-node turn restrictions | Mode-specific forbidden transitions |
| Via-way restrictions | Exclude affected member roads for the restricted mode |
| Conditional access or turns | Exclude affected modes; no date evaluation |
| Destination, private or unsupported access | Conservative exclusion |
| Bridge and tunnel | Preserve structure flag for terrain |
| Surface, MTB scale, SAC scale | Separate fields; missing values stay unknown |

Read the manifest warnings before publishing. The importer does not yet model
all OSM semantics. It has no ferry schedule, opening-time evaluator, general
via-way automaton, smoothness model, or country-default catalogue. Do not use
this German regional importer as a worldwide release pipeline.

The profile catalogue has touring, road, gravel, MTB and hiking. Each has
shorter, smoother and less-climbing variants. `--profiles all` prepares all
20 metrics. Passing a comma-separated list prepares only those IDs. The
profiles are explicit initial policy values; rider validation is still needed.

The package includes all geometry, snap cells, endpoint states and CH pages.
It is separate from map tiles. See [the package contract](../../specs/route-package.md).
Preserve OpenStreetMap attribution and ODbL notices when distributing the data.

```sh
obc test -p route-build
cargo clippy -p route-build --all-targets --all-features -- -D warnings
```
