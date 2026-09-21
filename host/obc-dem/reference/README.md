# The reference archive

Copernicus GLO-30 is the best free *global* elevation source, but 30 m cannot hold a rock tower:
at Engelberg its bilinear surface runs 100 m below the Hahnen's summit. National mapping agencies
publish LiDAR one to two orders finer, and most of it is open data. The bakery adds the height a
finer DTM measures on a crest to the native lattice, so one surface serves Peak View, the contours,
the ascent, the profile and the altimeter.

The bakery cannot hold a country-sized reference in memory, and it must not learn seven service
protocols. So there is **one archive with one format**, and one offline tool that fills it:

- `ingest.py` turns a national DTM into archive tiles. Every country branch is in it. It runs once
  per source release, never during a bake.
- The archive holds max-pooled bare-earth height on the OBCT lattice, as `int16` metres, in
  GeoTIFF tiles. The baker streams the tiles of one cell and reads nothing else.
- The archive lives on R2 under `reference/v1/`. A bakery run mirrors the tiles its box needs.

## The archive tile contract

This is the normative contract. The baker and `ingest.py` both hold to it.

- **Lattice**: OBCT `GRID_ORIGIN` (−2^28 µdeg on both axes), step `2^6` µdeg on both axes
  (`REFERENCE_STEP_LOG2 = 6`, ≈ 7.1 m latitude, 4.9 m longitude at 47° N).
- **Tile**: `2^16` µdeg square = 1024 × 1024 pixels. Tile id
  `(ti, tj) = ((lat − ORIGIN) >> 16, (lon − ORIGIN) >> 16)`. Path `16/<ti:04>/<tj:04>.tif` under the
  archive root (the OBCA id idiom, zero padded to four digits).
- **Pixel** `(r, c)` covers the half-open square
  `[ORIGIN + (ti·1024 + r)·2^6, +2^6) × [ORIGIN + (tj·1024 + c)·2^6, +2^6)` µdeg with `r` counting
  north. The GeoTIFF stores rows north-up (row 0 is the northernmost); the reader flips once, as
  `DemTile` already does.
- **Value**: `int16` little-endian, orthometric metres, **the maximum** of every source pixel whose
  centre lies inside the square (max-pooling on ingest is what makes a 7 m archive safe: a rock
  tower one source pixel wide survives). `−32768` = no source pixel. No other nodata. Single band.
  `PixelIsArea`. EPSG:4326. Deflate, internal tiles 256 × 256.
- **Transform** must be exactly the lattice: `a = 2^6 / 1e6`, `e = −2^6 / 1e6`, origin on lattice
  lines. The reader checks `|actual − expected| ≤ 1e-9` degrees and refuses otherwise, naming the
  file. No reprojection and no resampling in the baker.
- **`index.json`** at the archive root:
  ```json
  { "schema": 1, "step_log2": 6, "tile_log2": 16,
    "sources": { "ch": { "product": "swissALTI3D 2 m", "attribution": "© swisstopo", "licence": "...", "fetched": "2026-09-21" } },
    "tiles": { "3410/2882": "ch" } }
  ```
  A tile not in the index is not in the archive.

### One addition to the index

`index.json` carries one map more than the contract above shows: **`sha256`**, the digest of each
tile file, under the same tile ids as `tiles`.

```json
{ "schema": 1, "step_log2": 6, "tile_log2": 16,
  "sources": { "ch": { "product": "…", "attribution": "…", "licence": "…", "fetched": "2026-09-21" } },
  "tiles": { "3410/2882": "ch" },
  "sha256": { "3410/2882": "9f86d0818…" } }
```

The terrain bakery keys its skip decision on the reference a cell was baked from, and a digest is
the only statement of "these bytes" that survives a re-ingest of the same box. `tiles` keeps the
shape the contract states, so a reader that wants the source key only reads one map, as before.

`sources` holds the four fields the contract names. The full facts of a source — its country, its
step in metres and the tile ids it holds — stay in its manifest at `sources/<key>.json`, which is
what `index` rebuilds the index from.

## The tool

```sh
python3 ingest.py ingest ch --bbox 8.30,46.75,8.60,46.95 --archive ref/ --work /tmp/ch
python3 ingest.py index  --archive ref/
python3 ingest.py check  --archive ref/
python3 ingest.py publish --archive ref/
python3 ingest.py mirror --archive ref/ --bbox 8.30,46.75,8.60,46.95
```

It needs `rasterio`, `pyproj` and `numpy` (`tools/requirements-bake.txt`, or
`obc install replication`). `--bbox` is longitude first, like `obc-pack --bbox` and unlike
`obc-dem --bbox`.

| subcommand | what it does |
| --- | --- |
| `ingest <key>` | The source adapter obtains rasters for the box; the shared tail writes tiles. `--input <dir>` takes hand-fetched rasters instead of the service. `--work <dir>` is where fetched rasters are cached, so a second run of the same box downloads nothing. |
| `index` | Rebuilds `index.json` from the manifests in `sources/`. |
| `check` | Opens every tile and holds it against the contract — size, dtype, nodata, CRS, the exact transform, the digest — and refuses a tile the index does not name. |
| `publish` | `rclone sync` of the archive to `<bucket>/reference/v1/`, tiles and manifests first and `index.json` last. Idempotent. |
| `mirror` | `rclone copy` of the index plus the tiles one box needs, into a local directory. This is what a bakery run does before `obc-dem bake --reference`. |

### What the shared tail does

An adapter has one job: `fetch(bbox, workdir) -> list[Path]`, rasters in any CRS and any dtype.
Everything after that is shared, so a new country is an adapter and a row in the source table:

1. Voids become one value. A source marks them with a declared sentinel, with NaN, with the float
   maximum, or not at all. All four become absence before the warp.
2. `rasterio.warp.reproject(..., resampling=Resampling.max)` puts the raster **directly onto the
   lattice window that covers its bounds**. The destination transform is built from the lattice
   integers, so no pixel is resampled twice and there is no `calculate_default_transform`.
3. The heights become `int16` metres, rounded half away from zero, with `−32768` for absence.
4. The window is cut into tiles. A tile is always whole: a box that reaches a corner of a tile
   still writes 1024 × 1024 pixels, with nodata where the box did not reach.

Then the priority rule decides who keeps the tile.

### Priority

`PRIORITY` in `ingest.py` is the one constant, finest and best-maintained national product first:

```
nl, de-nw, fr, no, us, ch, es
```

- A tile a **higher-priority** source holds is left alone.
- A tile a **lower-priority** source holds is replaced, and its pixels go.
- A second ingest into a tile from the **same** source max-merges: the new box adds its footprint
  and raises the pixels both boxes cover.

A source key that is not in the list ranks last, so it cannot displace a listed one.

### What a source may look like

The tail is deliberately blunt, because the sources are not uniform:

- **Any CRS.** A projected national grid (LV95, ETRS89/UTM) and EPSG:4326 both warp in one step.
  Bounds are transformed with densified edges, so a curved projection edge cannot fall outside the
  window.
- **Any dtype.** `float32` with NaN, `int16` with a sentinel, and an undeclared void all mean
  absence. A magnitude above 10^6 is not a height, so a float maximum is a void as well.
- **A source coarser than 7 m is accepted.** `Resampling.max` upsamples by repeating the one source
  pixel it finds, which is the maximum over a footprint of one. The archive step then carries no
  more detail than the source had, which is correct: the maximum over an empty footprint stays
  nodata, so a coarse source never invents ground.
- **A box at the antimeridian is refused, by name.** The lattice does not wrap, so split the box at
  ±180°. A box outside the world box is refused as well.
- **Several revisions of one square are max-merged.** swisstopo publishes a square again when it
  re-flies it, and the STAC listing answers with every year. The archive keeps the maximum, which
  is the rule the whole archive is built on.

Memory is one source raster plus the tiles it touches. A published national tile (swisstopo
publishes one square kilometre at a time) is a few megabytes, so a country-sized ingest never holds
more than that.

## Sources

`ch` is implemented. The rest of the table is the registry of what the archive will hold: the
sources with an adapter, the ones that arrive through `--input` because a download needs a login,
and the candidates.

### Open services, no key

| key | country | step | product | licence | attribution | adapter |
| --- | --- | --- | --- | --- | --- | --- |
| `ch` | Switzerland | 2 m | swissALTI3D | Open data, attribution required | `© swisstopo` | STAC, in the tool |
| `fr` | France | 1 m | RGE ALTI (IGN) | Licence Ouverte | `© IGN` | planned |
| `us` | United States | 1 m | 3DEP (USGS) | Public domain | `USGS 3DEP` | planned |
| `no` | Norway | 1 m | NHM DTM (Kartverket) | CC BY 4.0 | `© Kartverket` | planned |
| `es` | Spain | 5 m | MDT05 / PNOA LiDAR (IGN) | CC BY 4.0 | `© Instituto Geográfico Nacional` | planned |
| `nl` | Netherlands | 0.5 m | AHN DTM (PDOK) | CC BY 4.0 | `© Rijkswaterstaat / AHN` | planned |
| `de-nw` | Germany, North Rhine-Westphalia | 1 m | DGM1 (Geobasis NRW) | dl-de/zero-2-0 | `© Geobasis NRW` | planned |

Germany publishes elevation per state, each with its own service. NRW is in the registry because
its WCS needs nothing else; the other states follow the same pattern and are a registry row each.

### Sources that need a key, a login or a bulk download

These are open data, but a download needs registration or comes as a whole-country archive, so an
unattended bake cannot pull them. Fetch them by hand and pass `--input <dir>`; the tail treats them
exactly like a fetched raster.

| country | step | product | how to get it |
| --- | --- | --- | --- |
| Italy | 1–2 m | per-region DTM | regional portals, coverage is partial |
| Austria | 1 m | per-state DGM (data.gv.at) | bulk download per state, no live service |
| Denmark | 0.4 m | DHM/Terræn | Datafordeler account, WCS token |
| Sweden | 1 m | Markhöjdmodell (Lantmäteriet) | Geotorget account |
| Finland | 2 m | Korkeusmalli (NLS) | API key from the NLS file service |
| United Kingdom | 1 m | LIDAR Composite DTM (EA) | DEFRA survey portal, England only |
| Canada | 1 m | HRDEM (NRCan) | open S3 bucket, per-project tiles |
| Australia | 1–5 m | ELVIS (Geoscience Australia) | ELVIS portal, per-area order |
| New Zealand | 1 m | LiDAR DEM (LINZ) | LINZ Data Service API key |

Alpine Italy is the first of these to matter: a border ridge with coverage on one side only shows
the step between a lifted and an unlifted node in contours and profiles.

### What makes a good reference

- **A bare-earth DTM is safe.** Copernicus GLO-30 is a surface model, so it sits above a DTM over
  forest and buildings. A lift cannot be negative, so the bakery simply does not lift there. On the
  bare rock that a crest is, the two models agree.
- **Five metres is enough.** The lift is gated on convexity, so a 5 m reference recovers a summit
  nearly as well as a 1 m one. Ingest the coarser product when it is faster.
- **A reference that is wrong low costs nothing.** RGE ALTI is weak over the Mont Blanc glaciers
  and reads below Copernicus there; the bake leaves those samples alone.

## Attribution

The attribution of every source a map's cells read must travel with that map, next to the
Copernicus attribution the native heights already carry. `index.json` states it per source, and the
catalog carries it into the published map.

## Publishing and mirroring

`publish` and `mirror` define their rclone remote entirely through the child process's environment,
the same pattern `host/obc-bake/src/publish.rs` uses: the secret is never an argument, because argv
is readable by every process on the box. The variables come from `tools/obc.local`:

```
OBC_R2_ACCOUNT_ID        Cloudflare account id (builds the endpoint)
OBC_R2_BUCKET            bucket name
OBC_R2_PREFIX            optional key prefix inside the bucket
OBC_R2_ACCESS_KEY_ID     R2 API token id
OBC_R2_SECRET_ACCESS_KEY
OBC_R2_ENDPOINT          optional, overrides the derived endpoint (an S3 test double)
```

The owner publishes an ingested archive with:

```sh
set -a; . tools/obc.local; set +a
python3 host/obc-dem/reference/ingest.py publish --archive /tmp/cp4-work/archive
```

`publish` is `rclone sync`, so the local archive is the whole truth: a tile that is not in the local
directory is deleted from R2. Publish from a full archive, not from a box.

A bakery run mirrors before it bakes:

```sh
python3 host/obc-dem/reference/ingest.py mirror --archive ref/ --bbox 8.30,46.75,8.60,46.95
obc-dem bake --reference ref/ …
```

`mirror` pulls `index.json` first, then the tiles the box needs and nothing else.
