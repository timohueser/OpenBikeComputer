# TrailView features

The step `planner/trailview-features` of `obc data`: the TrailView feature tiles of a region, with
the heights of roads, paths, tracks, bridges, tunnels, rivers and lakes. The
[spec](../../specs/trailview-features.md) defines the archive and what the heights promise.

`obc-data-steps` runs the step as the function `obc-trailview-features/features`. The step needs
Osmium (`osmium-tool`) on the path.

## Direct bake

The binary bakes the archive of a box without `obc data`, with the kind table and the road classes
of the checkout, and the terrain of a `planner-dem` MBTiles:

```sh
cargo build --locked --release -p obc-trailview-features --bin trailview-features
target/release/trailview-features --osm OSM.pbf --terrain TERRAIN.mbtiles --bounds W,S,E,N --output OUT.pmtiles
```

`builder/web/tests/browser/trailview/box.sh` and the engine fixture recipe use it.

| Variable | Effect |
| --- | --- |
| `OBC_OSMIUM` | The Osmium executable. The default is `osmium` on the path. |
| `OBC_DATA_MEMORY_BYTES` | The road jobs that run at once fit in this memory. `obc data` sets it. |

## Memory

The water networks run on all cores. The road jobs run on a core each, the largest first, as many
at once as fit in the memory that is free past a quarter of RAM, less what the process holds. With
no memory free, one job runs at a time.

## Tests

`obc test -p obc-trailview-features`. Without Osmium, the tests of the cut and of the bake of a small
region return at once, except in CI (`CI` set), where they fail.
