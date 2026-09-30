# Route planner

The public planner runs at `/plan/`. The site build reads one active regional
release. Maps, routing, and search use that release. The map builder reads its
snapshot of the device catalogue.

## Prepare and publish a region

Run from the checkout. Install Rust, Node 24+, Python 3.12+, `uv`, `gh`, `rclone`,
and the [PMTiles CLI](https://docs.protomaps.com/pmtiles/cli).
Authenticate `gh` for the query model release. Set the R2 credential in
`tools/obc.local`. The [region recipe](../../../../../tools/planner-regions/baden-wuerttemberg.json)
pins the OSM extract, map inputs, elevation inputs, and routing profiles.
The BW recipe selects Balanced and Less climbing for each rider mode.

The raw map and search builders require Linux, Java 21, Maven, PostgreSQL 17,
PostGIS 3, osm2pgsql 2, zstd, and `nominatim-db==5.3.2` in the build environment.
Add PostgreSQL's binary directory to `PATH`. Run preparation as a normal user.
Allow space for the temporary Nominatim database and Planetiler files.
The builders use two threads. Preparation can take several hours.

```sh
obc planner prepare --data-dir /srv/planner/bw --reference /srv/obc-reference
obc planner publish --data-dir /srv/planner/bw --apply
obc planner deploy --data-dir /srv/planner/bw --host root@YOUR_VPS --apply
```

`publish` and `deploy` show their action without `--apply`. Publication uploads
immutable files and verifies the remote bytes. Deployment checks the routing
package, model readiness, CORS, tiles, search, and a real route. It updates
`planner/catalog.json` only after those checks pass. The
[release contract](../../../../../specs/planner-release.md) defines the files.

`prepare` accepts `--osm PATH` for a local copy of the pinned extract. On macOS,
use `--inputs DIRECTORY` to supply verified Linux builder outputs:
`basemap.pmtiles`, `search.jsonl.zst`, and `inputs.json`. This manifest names the
OSM hash, bounds, tool versions, and output hashes. Map preparation and routing
still use the local elevation readers.

The VPS needs Caddy, Python, Rust at `/root/.cargo/bin/cargo`, and Node 24+
at `/usr/local/bin/node`. Its existing API virtual host is
`releases.openbikecomputer.com`. Deployment installs two services on loopback.
Routing uses at most two workers. Search runs SQLite and the query model.
PostgreSQL, Nominatim, and Photon are build tools. They are not public services.

Deploy the [tile Worker](../../../../../apps/planner-tiles/README.md) first.
It reads two regional PMTiles archives from R2 and caches XYZ tiles at the edge.
Fonts, sprites, and archive downloads use `maps.openbikecomputer.com`.

Set the GitHub repository variable `OBC_PLANNER_CATALOG_URL` to
`https://maps.openbikecomputer.com/planner/catalog.json`. Run **Deploy site**
from `develop`. The workflow publishes `/plan/` and adds **Route planner** to
site navigation. It uses the release's device catalogue for `/builder/`.
After the workflow succeeds, finish the rollout:

```sh
obc planner finalize
obc planner finalize --apply
```

Finalization checks the live services and web planner before it removes inactive
planner releases and unused source mirrors from R2. It keeps one regional dataset.
It preserves device cell objects and terrain reference data. Publication refuses
another release while an inactive dataset remains.

## Replace or restore a release

For a larger region, add a recipe with a new region ID, bounds, and pinned
inputs. Build into a fresh data directory with `--recipe PATH`. Pass
`--device-catalog URL` for that region's published device catalogue. Use the
same three commands, then run **Deploy site** again.

To reduce an existing package without preparing its metrics again:

```sh
cargo run --release -p route-build --bin route-select -- \
  /srv/planner/old/routing --output /srv/planner/new/routing \
  --profiles touring,touring/less-climbing,road,road/less-climbing,gravel,gravel/less-climbing,mtb,mtb/less-climbing,hiking,hiking/less-climbing
```

Copy the unchanged `maps`, `search`, and `sources` directories into the new
release directory. Set the recipe's `profiles` list to the same IDs. Run the
three commands above with the new directory. Preparation checks the profile
selection and builds a matching overlay index.

Routing currently supports German access defaults. Preparation refuses other
countries. Add and verify their access rules before extending coverage.
No service code needs a new region name.

Deployment keeps both VPS slots during rollout. Before finalization, restore
the previous release with:

```sh
obc planner rollback --apply
```

Then run **Deploy site** again and finalize. Finalization removes the previous
dataset from R2 and clears its catalogue entry. Reload old planner pages after rollout.
The upload preview reports size and a storage cost ceiling before free allowances.
Worker requests and the VPS have separate costs.

## Local preview

The prepared preview uses independent upstream map and search snapshots:

```sh
obc planner setup
obc planner
obc planner verify
```

Open `http://127.0.0.1:4175/planner.html`. Setup downloads regional PMTiles,
prepared Photon records, and the query model. It builds BW routing with all
profiles. Normal launch has no downloads. Ctrl-C stops the local services.
`verify` checks map hashes, SQLite integrity, and routing object closure.

| Setting | Default |
| --- | --- |
| `OBC_PLANNER_DATA` or `--data-dir` | `~/.cache/obc/planner/baden-wuerttemberg` |
| `OBC_PLANNER_RELEASE` | `~/.cache/obc/planner/bw-online` |
| `--port` | Planner `4175` |
| `--tile-port` | Terrain `8789` |
| `--route-port` | Routing `8787` |
| `--search-port` | Search `8786` |
| `--reference` or `OBC_REFERENCE_ARCHIVE` | `~/obc-reference` if present |
| `--dem-dir` | `~/.cache/obcm/dem` |

## Client configuration

`obc planner site-config --output ENV_FILE` writes these settings from the active
release. Use them for a hosted build with the configured API origin.

| Variable | Value |
| --- | --- |
| `VITE_PLANNER_TILEJSON_URL` | Hosted basemap TileJSON |
| `VITE_PLANNER_PMTILES_URL` | Local basemap archive, when TileJSON is absent |
| `VITE_PLANNER_ROUTING_URL` | Routing API prefix |
| `VITE_PLANNER_SEARCH_URL` | Search API prefix |
| `VITE_PLANNER_SEARCH_REGIONS` | Comma-separated region IDs |
| `VITE_PLANNER_DEM_URL` | Terrarium WebP XYZ template |
| `VITE_PLANNER_TERRAIN_ATTRIBUTION` | Elevation source credits |
| `VITE_PLANNER_GLYPHS_URL` | Font template |
| `VITE_PLANNER_SPRITES_URL` | Sprite directory |
| `VITE_PLANNER_MAP_BOUNDS` | `west,south,east,north` |

Basemap zooms are 0–14. Terrain zooms are 0–12, with neighbouring tiles for
contours. The browser creates contours from terrain tiles. Highlighted rider
places read detailed basemap tiles along the route.

## Checks

From `builder/app`:

```sh
npx vitest run src/lib/planner/ src/components/planner/
npm run check
```

The full type check needs the generated WASM packages from the builder README.
See the [search README](../../../../../apps/planner-search/README.md) for its
code and real-data suites. iOS rendering and offline downloads have separate
validation.
