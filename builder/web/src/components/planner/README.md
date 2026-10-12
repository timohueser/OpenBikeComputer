# Route planner

The public planner at `/plan/` uses one active release for maps, routing, and
search.

## Prepare a region

Run from the checkout. Install Rust, Node 24+, Python 3.12+, `uv`, `gh`,
and the [PMTiles CLI](https://docs.protomaps.com/pmtiles/cli).
Authenticate `gh` for the query model release. The [region recipe](../../../../../tools/planner-regions/baden-wuerttemberg-switzerland.json)
pins the OSM extract, map, elevation, and data-layer inputs, and routing profiles.
Its name and box are in the [region file](../../../../../data/regions/baden-wuerttemberg-switzerland.toml)
with the same id.

The basemap builder needs Java 21 and Maven. The Rust search baker reads the same
OSM snapshot as routing and maps. It needs no database import. Builders use every core.

```sh
obc planner prepare --data-dir /srv/planner/bw-source --reference /srv/obc-reference
obc planner grid --input-release /srv/planner/bw-source --data-dir /srv/planner/bw
```

These commands publish nothing. `obc data update` publishes a target. The VPS follower of
[`tools/vps/README.md`](../../../../../tools/vps/README.md) is installed once `obc data serve prod`
serves an R2 target.
The [release contract](../../../../../specs/planner-release.md) defines the files.

`grid` builds reusable cells in a fresh directory.

`prepare` accepts `--osm PATH` for a local copy of the pinned extract.
Use `--inputs DIRECTORY` to supply a prepared `basemap.pmtiles` and `inputs.json`.
The manifest names the OSM hash, bounds, tool versions, and output hashes.
Search always builds from the verified OSM input.

## Replace a release

For a larger region, add a recipe with its ID, name, bounds, time zone, and
pinned inputs. Build into a fresh data directory with `--recipe PATH`. Pass
`--device-catalog URL` for that region's published device catalogue. Use the
same two commands.

For a component update, keep the recipe's pinned OSM snapshot and bounds:

```sh
obc planner plan --input-release /srv/planner/bw-source --component pois
obc planner prepare --input-release /srv/planner/bw-source --component pois --data-dir /srv/planner/bw-source-next
obc planner grid --input-release /srv/planner/bw-source-next --data-dir /srv/planner/bw-next
```

Repeat `--component` for:
`pois`, `addresses`, `basemap`, `places`, `terrain`, `routing`, `overlays`,
`assets`, `model`, or data layers. Routing updates include overlays;
POI updates include place tiles. Elevation changes select terrain,
routing, and overlays. Tile encoding changes reuse routing.

`plan` and `prepare --dry-run` read no large artifacts and download nothing.
Updates need a modular regional release and its cache. Source refreshes keep
one OSM snapshot. Preparation and grid publication share `--source-cache`
(default `~/.cache/obc/planner/sources`). `obc planner inventory` reports
component sizes and measured costs. Completed components and cells resume.

POI transforms reuse addresses and unrelated grid objects. Search enrichment and
routing construction remain regional.

## Local data

For `obc data`, configure the [runtime target](../../../../../specs/obc-data.md#service-runtimes)
in [`data/planner-runtime.toml`](../../../../../data/planner-runtime.toml).

```sh
obc planner setup
obc planner verify
```

Setup runs `prepare` with the region recipe and `~/obc-reference`; run `prepare`
directly for other inputs. `verify` checks map hashes, SQLite integrity, and
routing object closure.

Seed the cache from Geofabrik extracts of one date:

```sh
osmium merge baden-wuerttemberg-latest.osm.pbf switzerland-latest.osm.pbf -o merged.osm.pbf
mkdir -p ~/.cache/obc/planner/sources/downloads/auxiliary
osmium extract -b 5.95,45.8,10.5,49.85 merged.osm.pbf -o ~/.cache/obc/planner/sources/downloads/SHA256.osm.pbf
cp AUXILIARY_FILE ~/.cache/obc/planner/sources/downloads/auxiliary/NAME
```

| Setting | Default |
| --- | --- |
| `--region` | `baden-wuerttemberg-switzerland`; test regions `engadin`, `colorado-front-range` |
| `--data-dir` | `OBC_PLANNER_DATA/REGION` for every command; `OBC_PLANNER_DATA` is `~/.cache/obc/planner` |
| `--recipe` | The recipe of `--region` |
| `--pmtiles` or `PMTILES` | `pmtiles` |

## Client configuration

The planner reads the `active`
[catalogue](../../../../../specs/planner-release.md#catalogue) entry at page load.
`VITE_PLANNER_CATALOG` names another catalogue, such as the one that `obc data serve`
prints. The map builder then reads its device catalogue too. `VITE_PLANNER_CONFIG`
replaces the entry with one entry. `VITE_CATALOG_URL` gives the map builder another
device catalogue.

Basemap zooms are 0–14; terrain zooms are 0–12. Browser contours use terrain
neighbours. Highlighted places use zoom 11.

Extract a smaller map archive with bounds inside its source coverage:

```sh
python3 -m tools.planner_maps compact /srv/planner/bw/maps/terrain.pmtiles \
  /srv/planner/terrain.pmtiles --bbox=7.8,47.9,8.1,48.2 --terrain
```

Omit `--terrain` for a basemap. The command uses `uv` with pinned dependencies.
A map cutout does not change routing or search coverage.
Use `--no-recompress` to crop compressed terrain without encoding it again.

## Checks

From `builder/web`:

```sh
npx vitest run src/lib/planner/ src/components/planner/
npm run check
```

Type checking needs generated WASM.
Sunlight benchmark:

```sh
node ../../tools/planner_sun_bench.mjs http://127.0.0.1:4175
```

[Sunlight index](../../../../../specs/planner-sun-tiles.md).
See the [search README](../../../../../planner/search/README.md) for its
code and real-data suites. iOS rendering and offline downloads have separate
validation.
