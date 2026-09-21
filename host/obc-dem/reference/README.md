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

### The vertical datum

The contract says orthometric metres. It means orthometric on the same datum family as the
native lattice, which is Copernicus GLO-30 on EGM2008. A national geoid model is not EGM2008
— `ch` heights are on LN02/LHN95 — and the two differ by decimetres over the Alps. The
bakery only lifts a node where the reference stands more than 10 m above the native surface,
so a decimetre of datum difference cannot make a lift, and the archive records the datum per
source rather than converting between geoid models.

An **ellipsoidal** product is a different matter: it differs from an orthometric height by
tens of metres, which is a lift. An adapter for an ellipsoidal product (HRDEM and ELVIS both
publish some) **must** convert to orthometric before it hands the raster to the tail. No
conversion code is in the tool yet, because no adapter needs one yet.

`vertical_datum` is in `sources/<key>.json` and in `index.json`'s `sources`.

### One addition to the index

`index.json` carries one map more than the contract above shows: **`sha256`**, the digest of each
tile file, under the same tile ids as `tiles`.

```json
{ "schema": 1, "step_log2": 6, "tile_log2": 16,
  "sources": { "ch": { "product": "…", "attribution": "…", "licence": "…",
                       "vertical_datum": "LN02/LHN95", "fetched": "2026-09-21" } },
  "tiles": { "3410/2882": "ch" },
  "sha256": { "3410/2882": "9f86d0818…" } }
```

The digest is over the tile's **pixels** — row-major, little-endian `int16` — and not over the
file. Two GDAL or zlib builds deflate the same heights into different bytes, and the terrain bakery
keys its skip decision on this digest, so it has to mean "these heights". `tiles` keeps the
shape the contract states, so a reader that wants the source key only reads one map, as before.

`sources` holds the four fields the contract names, plus `vertical_datum`. The full facts of a
source — its country, its step in metres and the tiles it wrote pixels into — stay in its manifest
at `sources/<key>.json`, which is what `index` rebuilds the index from.

`tiles` names a tile's **best-priority contributor**. A tile can hold pixels from more than one
source, because coverage stops at borders: `tiles` answers "who is the best source in this tile",
the manifests answer "which tiles did this source write", and `sources` lists every source that
contributed a pixel anywhere, so every attribution travels with the map.

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
| `publish` | `rclone copy` of the archive to `<bucket>/reference/v1/`: tiles and manifests first, then the index, which goes up as the merge of the index already on R2 with this archive's. Additive and idempotent: a publish never deletes. |
| `mirror` | `rclone copy` of the index plus the tiles one box needs, into a local directory. This is what a bakery run does before `obc-dem bake --reference`. |

### What the shared tail does

An adapter has one job: `fetch(bbox, workdir) -> list[Path]`, rasters in any CRS and any dtype.
Everything after that is shared, so a new country is an adapter and a row in the source table:

1. Voids become one value. A source marks them with a declared sentinel, with NaN, or not at all.
   All three become absence, and so does any height outside −500 m to 9 000 m, because an
   undeclared −9999 is a sentinel and not ground. The run prints the void fraction per raster, so
   a service that answered with a nearly empty raster is visible.
2. Every source pixel centre is mapped to WGS84 (a `pyproj` transform, per block of source rows),
   and the integer microdegrees of that point give the lattice pixel that holds it. The lattice
   pixel keeps the **maximum** of the centres that land in it. This is the contract's rule, done
   directly, and it is not what `Resampling.max` does: that pools by area overlap, which raises
   every lattice pixel a source pixel merely touches. Measured over an alpine box, area overlap
   read 39 % of the pixels too high and spread a one-pixel tower over two to four archive pixels.
3. The heights become `int16` metres, rounded half away from zero, with `−32768` for absence.
4. The window is cut into tiles. A tile is always whole: a box that reaches a corner of a tile
   still writes 1024 × 1024 pixels, with nodata where the box did not reach.

Then the priority rule decides which pixels the tile keeps.

### Priority

`PRIORITY` in `ingest.py` is the one constant, finest and best-maintained national product first:

```
nl, de-nw, fr, no, us, ch, es
```

The rule is per pixel, because coverage stops at borders and survey edges: a better source over one
corner of a tile must not take the rest of the tile away from the source that does cover it.

- Only this source has been in the tile: the **maximum**, so a second box adds its footprint and
  raises the pixels both boxes cover.
- This source ranks **better** than every source that has been in the tile: its pixels win where it
  has them, and the other sources' pixels stay in its gaps.
- Otherwise: the pixels already there stay, and this source fills the gaps only.

A pixel does not record which source wrote it, so the comparison is against the tile's best
contributor. One case loses by that: a best-ranking source that ingests two overlapping boxes into
a tile a worse source also reached takes the later value there instead of the maximum. A source key
that is not in the list ranks last, so it cannot displace a listed one.

### What a source may look like

The tail is deliberately blunt, because the sources are not uniform:

- **Any CRS.** A projected national grid (LV95, ETRS89/UTM) and EPSG:4326 are both one transform
  of the pixel centres. Bounds are transformed with densified edges and the window is padded by one
  pixel, so a curved projection edge cannot push a centre out of the window.
- **A rotated or sheared transform is accepted.** A pixel centre is a point, and the affine gives
  it whatever the raster's grid is turned to. No `gdalwarp` step is needed first.
- **Any dtype.** `float32` with NaN, `int16` with a sentinel, and an undeclared void all mean
  absence, and so does any height outside −500 m to 9 000 m, which is what catches an undeclared
  −9999.
- **A source coarser than 7 m is accepted.** Its pixel centres reach fewer lattice pixels than the
  lattice has, and the lattice pixels no centre reached stay nodata. A coarse source therefore
  leaves gaps rather than inventing ground, and the archive carries no more detail than the source
  had. A source that must be dense on the lattice has to be resampled before the ingest.
- **A box at the antimeridian is refused, by name.** The lattice does not wrap, so split the box at
  ±180°. A box outside the world box is refused as well.
- **A scaled band is refused by name.** A band with a scale or an offset is not metres; undo it
  with `gdal_translate -unscale` first.
- **Several revisions of one square are max-merged.** swisstopo publishes a square again when it
  re-flies it, and the STAC listing answers with every year. The archive keeps the maximum, which
  is the rule the whole archive is built on.

Memory is one source raster plus the tiles it touches. That is small for a product that publishes
per tile — swisstopo publishes one square kilometre at a time — and a country-sized ingest of such a
product never holds more. A **monolithic** raster, such as a whole-state DGM of several gigabytes,
is held whole: read it in blocks, or cut it up with `gdal_retile` before the ingest. Blockwise
reading in the tail is a follow-up.

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

`publish` only ever adds. Every call is `rclone copy`, so an archive of one box cannot delete another
region's tiles. Because of that, the index it uploads is not the local one: `publish` copies the
tiles, pulls the `index.json` that is already on R2, merges its `tiles`, `sha256` and `sources`
entries with this archive's — this archive wins each tile it holds, because its bytes are the ones
just uploaded — and uploads the merged index last. A publish of the Engelberg box therefore leaves
every other region visible.

A tile that must leave R2 is a deliberate step by hand: remove the object, then publish a full
archive again.

A bakery run mirrors before it bakes:

```sh
python3 host/obc-dem/reference/ingest.py mirror --archive ref/ --bbox 8.30,46.75,8.60,46.95
obc-dem bake --reference ref/ …
```

`mirror` pulls `index.json` first, then the tiles the box needs and nothing else.
