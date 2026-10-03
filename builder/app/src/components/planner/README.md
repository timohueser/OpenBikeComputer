# Route planner

The public planner at `/plan/` uses one active release for maps, routing, and
search. The map builder uses its device catalogue.

## Prepare and publish a region

Run from the checkout. Install Rust, Node 24+, Python 3.12+, `uv`, `gh`, `rclone`,
and the [PMTiles CLI](https://docs.protomaps.com/pmtiles/cli).
Authenticate `gh` for the query model release. Set the R2 credential in
`tools/obc.local`. The [region recipe](../../../../../tools/planner-regions/baden-wuerttemberg.json)
pins the OSM extract, map inputs, elevation inputs, and routing profiles.

Map and search builders need Linux, Java 21, Maven, PostgreSQL 17,
PostGIS 3, osm2pgsql 2, zstd, and `nominatim-db==5.3.2`.
Add PostgreSQL's binary directory to `PATH`. Run preparation as a normal user.
Allow space for the temporary Nominatim database and Planetiler files.
Builders use two threads. Allow several hours.

```sh
obc planner prepare --data-dir /srv/planner/bw-source --reference /srv/obc-reference
obc planner grid --input-release /srv/planner/bw-source --data-dir /srv/planner/bw
obc planner publish --data-dir /srv/planner/bw --apply
obc planner deploy --data-dir /srv/planner/bw --host root@YOUR_VPS --apply
```

`publish` and `deploy` show their action without `--apply`. Publication uploads
files and verifies remote bytes. Deployment checks the routing
package, model readiness, CORS, tiles, search, and a real route. It updates
`planner/catalog.json` only after these pass. The
[release contract](../../../../../specs/planner-release.md) defines the files.

Online releases are grid releases: `grid` publishes into a new directory. It
builds reusable cells from the regional bake. Run one publication or deployment
at a time.

`prepare` accepts `--osm PATH` for a local copy of the pinned extract. On macOS,
use `--inputs DIRECTORY` to supply verified Linux builder outputs:
`basemap.pmtiles`, `search.jsonl.zst`, and `inputs.json`. This manifest names the
OSM hash, bounds, tool versions, and output hashes. Map preparation and routing
still use the local elevation readers.

The VPS needs Caddy, Python, Rust at `/root/.cargo/bin/cargo`, and Node 24+
at `/usr/local/bin/node`. Its existing API virtual host is
`releases.openbikecomputer.com`. Deployment installs routing, search, and offline selection services on loopback.

Deploy the [tile Worker](../../../../../apps/planner-tiles/README.md) first.

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
  --profiles touring,touring/shorter,touring/less-climbing,road,road/shorter,road/less-climbing,gravel,gravel/shorter,gravel/less-climbing,mtb,mtb/shorter,mtb/less-climbing,hiking,hiking/shorter,hiking/less-climbing
```

Copy the unchanged `maps`, `search`, and `sources` directories into the new
release, without `maps/overlays.pmtiles`. Set the recipe's `profiles` to the
same IDs. Run the three commands above with the new directory. Preparation
checks the profile selection and builds a matching overlay index and tiles.

Routing currently supports German access defaults. Preparation refuses other
countries. Add and verify their access rules before extending coverage.

Each kind of change goes out in one way:

- **Code only.** For a route server or planner-search change, deploy the same
  data directory again. `deploy` restarts the active slot in place. Open pages
  see a short outage. This is accepted during development.
- **New data release.** `deploy` installs it into the other slot. The old slot
  serves open pages until you remove it. Then run **Deploy site** and finalize.
- **New catalogue field.** Deploy the release from the branch first. Merge the
  branch second. **Deploy site** fails while the live catalogue does not have
  the field.

Before finalization, restore the previous release with:

```sh
obc planner rollback --apply
```

Then run **Deploy site** again and finalize. `rollback` and `site-config` need
every catalogue field, so a rollback across a format change fails.

`finalize` cleans R2 only. Remove the old VPS slot `N` and its release `OLD_ID`
by hand:

```sh
ssh root@YOUR_VPS 'systemctl disable --now obc-planner-routing-N obc-planner-search-N'
ssh root@YOUR_VPS 'rm /etc/caddy/planner/slot-N.caddy && caddy validate --config /etc/caddy/Caddyfile && systemctl reload caddy'
ssh root@YOUR_VPS 'rm -rf /opt/obc-planner/releases/OLD_ID /etc/systemd/system/obc-planner-routing-N.service /etc/systemd/system/obc-planner-search-N.service && systemctl daemon-reload'
```

## Local preview

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
| `VITE_PLANNER_PLACES_URL` | Rider places TileJSON or PMTiles archive |
| `VITE_PLANNER_OVERLAYS_URL` | Overlay TileJSON or PMTiles archive |
| `VITE_PLANNER_SNOW_URL` | Snow history PMTiles archive |
| `VITE_PLANNER_ROUTING_URL` | Routing API prefix |
| `VITE_PLANNER_SEARCH_URL` | Search API prefix |
| `VITE_PLANNER_SEARCH_REGIONS` | Comma-separated region IDs |
| `VITE_PLANNER_DEM_URL` | Terrarium WebP XYZ template |
| `VITE_PLANNER_TERRAIN_ATTRIBUTION` | Elevation source credits |
| `VITE_PLANNER_GLYPHS_URL` | Font template |
| `VITE_PLANNER_SPRITES_URL` | Sprite directory |
| `VITE_PLANNER_MAP_BOUNDS` | `west,south,east,north` |

Basemap zooms are 0–14. Terrain zooms are 0–12, with neighbouring tiles for
contours. The browser creates contours from terrain tiles. Highlighted places
read the zoom 11 places archive.

Extract a smaller map archive with bounds inside its source coverage:

```sh
python3 tools/planner_maps.py compact /srv/planner/bw/maps/terrain.pmtiles \
  /srv/planner/terrain.pmtiles --bbox=7.8,47.9,8.1,48.2 --terrain
```

Omit `--terrain` for a basemap. The command uses `uv` with pinned dependencies.
A map cutout does not change routing or search coverage.
Use `--no-recompress` to crop compressed terrain without encoding it again.

Build and install a local runtime bundle:

```sh
cargo build --release -p route-build --bin route-extract -p route-server --bin route-server
python3 tools/planner_cutout.py /srv/planner/bw /srv/planner/freiburg \
  --bbox=7.77,47.965,7.96,48.06 --region freiburg
python3 tools/planner_offline.py pack /srv/planner/freiburg /srv/planner/bundle
python3 tools/planner_offline.py verify /srv/planner/bundle
python3 tools/planner_offline.py install /srv/planner/bundle /srv/planner/offline
```

Installation also accepts an HTTP(S) bundle directory URL. Rerun to resume.
The [bundle contract](../../../../../specs/planner-offline.md) defines activation,
retained releases, deduplication, and size fields.

## Checks

From `builder/app`:

```sh
npx vitest run src/lib/planner/ src/components/planner/
npm run check
```

The full type check needs the generated WASM packages.
See the [search README](../../../../../apps/planner-search/README.md) for its
code and real-data suites. iOS rendering and offline downloads have separate
validation.
