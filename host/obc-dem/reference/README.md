# The reference archive

Copernicus GLO-30 is the native elevation source, but 30 m cannot hold a rock tower. The archive
holds finer national DTM heights on the OBCT lattice, and the bakery lifts a crest to them. See
`specs/OBCT_Spec.md` §9 for the lift rule.

`ingest.py` fills the archive from a national DTM. It runs once per source release, never during
a bake. The archive lives on R2 at `<bucket>/reference/v1/`, beside the catalog prefix; the
bakery reads a local copy, the owner's own archive or a mirror of one box.

## Run it

Needs `rasterio`, `pyproj` and `numpy` (`tools/requirements-bake.txt`).

```sh
cd host/obc-dem/reference
python3 ingest.py ingest ch --bbox 8.30,46.75,8.60,46.95 --archive ref/ --work /tmp/ch
python3 ingest.py ingest ch --bbox 5.9,45.8,10.5,47.9 --archive ref/ --work /tmp/ch --per-tile
python3 ingest.py wizard se --bbox 18.48,67.89,18.51,67.91 --archive ref/
python3 ingest.py index   --archive ref/
python3 ingest.py check   --archive ref/
python3 ingest.py publish --archive ref/
python3 ingest.py mirror  --archive ref/ --bbox 8.30,46.75,8.60,46.95
```

`--bbox` is longitude first, like `obc-pack --bbox` and unlike `obc-dem --bbox`.
`ingest --input <dir>` takes hand-fetched files instead of the service. `wizard` walks the
account and download steps of a source behind a login, then runs the ingest. `check` holds every
tile against the contract below. `publish` and `mirror` are `rclone copy`; a publish only ever
adds, and it merges its index with the one already on R2.

**A country-scale ingest is an owner-run job**, and it takes `--per-tile`: one archive tile at a
time, the work directory wiped after each, each finished tile recorded as done in the source's
manifest, so disk holds one tile's rasters and a stopped run resumes. The index is rebuilt once at
the end. It still takes days per country: run one source at a time, and `check` before
`publish`. To remove a tile from R2, delete the object by hand and publish again.

## The tile contract

The baker and the ingest tool both hold to this. `tile_problems` in `ingest/archive.py` is the
contract as code, and `check` runs it.

- **Lattice**: OBCT `GRID_ORIGIN` (−2^28 µdeg on both axes), step `2^6` µdeg
  (`REFERENCE_STEP_LOG2 = 6`, about 7.1 m latitude and 4.9 m longitude at 47° N).
- **Tile**: `2^16` µdeg square = 1024 × 1024 pixels, at `16/<ti:04>/<tj:04>.tif` under the archive
  root, where `(ti, tj) = ((lat − ORIGIN) >> 16, (lon − ORIGIN) >> 16)`.
- **Value**: `int16` little-endian orthometric metres, the **maximum** of every source pixel centre
  inside the lattice square. `−32768` is the only nodata. Single band, `PixelIsArea`, EPSG:4326,
  deflate, 256 × 256 internal tiles. The GeoTIFF stores rows north-up.
- **Transform** must be exactly the lattice (`a = 2^6 / 1e6`, `e = −2^6 / 1e6`, origin on lattice
  lines) within 1e-9 degrees. The baker does no reprojection and no resampling.
- **`index.json`** at the archive root names every tile. A tile the index does not name is not in
  the archive.

```json
{ "schema": 1, "step_log2": 6, "tile_log2": 16,
  "sources": { "ch": { "product": "swissALTI3D 2 m", "attribution": "© swisstopo",
                       "licence": "…", "vertical_datum": "LN02/LHN95", "fetched": "2026-09-21" } },
  "tiles": { "3410/2882": "ch" },
  "contributors": { "3410/2882": ["ch", "es"] },
  "sha256": { "3410/2882": "9f86d0818…" } }
```

`sha256` is over the tile's **pixels**, not the file, because two GDAL builds deflate the same
heights into different bytes and the terrain bakery keys its skip decision on the digest.
`contributors` lists every source holding a pixel in the tile, best priority first; `tiles` names
the first of them. `index` rebuilds all of this from the manifests in `sources/<key>.json`.

## The code

`ingest.py` is the entry point. The tool is the `ingest/` package beside it.

| module | what is in it |
| --- | --- |
| `lattice.py` | Integer microdegrees: the grid, `Window`, tile ids, and the refusals for a box the lattice cannot hold. |
| `pool.py` | One source raster in, lattice `int16` metres out: the void convention and the pooling rule. |
| `archive.py` | Tiles, `PRIORITY`, the per-pixel merge, the manifests, `index.json` and `tile_problems`. |
| `publish.py` | The rclone remote and the `publish` and `mirror` plans. |
| `sources/` | One module per country, one row per product. `base.py` is what an adapter is; `__init__.py` is the `SOURCES` registry. |
| `wizard.py` | The steps of a portal behind a login, and the check on what it delivered. |
| `cli.py` | The subcommands. |

An adapter has one job: `fetch(bbox, workdir) -> list[Path]`, rasters in any CRS and any dtype.
The shared tail turns voids and heights outside −500 m to 9 000 m into absence, max-pools pixel
centres onto the lattice as `int16` metres, and cuts the window into whole tiles.

`PRIORITY` in `ingest/archive.py` orders the sources, finest and best-maintained first. The merge
is per pixel, because coverage stops at borders: a better source keeps its own pixels and leaves
the others' pixels in its gaps.

Refused by name: a box at the antimeridian or outside the world box, a scaled band, an ESRI ASCII
or XYZ grid whose row names no `grid_epsg`, and a vertical datum `ORTHOMETRIC` in `sources/base.py` does
not recognise. An ellipsoidal product must be converted to orthometric first; nothing here does.

Memory is one source raster plus the tiles it touches. A **monolithic** raster, such as a
whole-state DGM of several gigabytes, is held whole: cut it up with `gdal_retile` first.

### Credentials

A credential is one field on a row. `Credential` in `sources/base.py` reads
`OBC_REFERENCE_<KEY>_TOKEN`, or `OBC_REFERENCE_<KEY>_USER` and `_PASSWORD`, out of the
environment — never out of argv, which every process on the box can read.

Three rules keep it there, and each one is a test:

- **A refusal never quotes it.** A keyed request is labelled `<key> <box>`, and every message and
  error body goes through `redact`, which removes every `OBC_REFERENCE_*` value in every form,
  including percent-escaped.
- **It goes to the row's own hosts only, over https.** `credential_hosts` on the row is the allowed
  suffix; a `href` elsewhere is refused by name.
- **A redirect does not carry it.** `DropAuthOnRedirect` removes `Authorization` on any host
  change. A portal that redirects needs its real download host named in its index.

`credential_style` on the adapter and the row's credential shape are checked at the row.

Every request goes through `with_retry` in `sources/base.py`: a dropped connection, a 429 and a
5xx, and an OWS `NoApplicableCode` on any status, are retried for up to twelve minutes; every
other 4xx is refused at once with its body. A 404 is absence
only where the registry says a name is arithmetic, such as a grid square outside its state. A WCS
2.0.1 request is clipped to the envelope `DescribeCoverage` states; a subset outside it is refused.

## Sources

`SOURCES` in `ingest/sources/__init__.py` is the registry, and each row holds its key, country,
product, resolution, licence, attribution and vertical datum as data. Read the row, not a table
here. `index.json` carries the same fields into the archive.

Open services, no key: `ch`, `fr`, `us`, `no`, `es`, `nl`, `uk`, `at`, `ca`, `nz`, `it-bz`,
`it-tn`, and the German states `de-nw`, `de-he`, `de-bw`, `de-mv`, `de-st`, `de-by`, `de-sn`,
`de-th`, `de-ni`. Germany publishes elevation per state, so `de-*` is a family of rows.

Behind a free account, so an unattended bake cannot pull them: `dk`, `se`, `fi`, `au`. Set the
credential and `ingest <key>` fetches live; without it, `ingest <key> --input <dir>` takes what the
portal delivered. `wizard <key>` holds the account and download steps, and asks for the credential
in its own process so it never reaches a command line.

`de-sn` and `de-th` carry the Quellenvermerk their services state. One obligation the rows cannot settle by themselves:

- **`au` needs one answer by hand.** Some ELVIS datasets are ellipsoidal. No row can tell which an
  order held, so `ingest au --input` refuses until `--datum AHD` is passed, and `wizard au` asks.
  `au` also states no step: the step of an order is the step of whichever survey it covered.

`ingest --input` reads every `.tif`, `.tiff`, `.asc` and `.zip` under the directory and **writes
nothing into it**; unpacked members go into the work directory under the digest of their bytes,
so a re-issued delivery of the same name is a different directory.

## Attribution

The attribution of every source a map's cells read must travel with that map, beside the
Copernicus attribution the native heights carry. `index.json` states it per source; the terrain
bakery records per cell which sources its lifts came from, and the catalog's terrain block lists
each of them once in `references` (`OBCC_Spec.md` §13.1).

## Publishing and mirroring

`publish` and `mirror` define their rclone remote through the child process's environment, the
same way `host/obc-bake/src/publish.rs` does. The variables come from `tools/obc.local`:

```
OBC_R2_ACCOUNT_ID        Cloudflare account id (builds the endpoint)
OBC_R2_BUCKET            bucket name; the archive is at <bucket>/reference/v1/
OBC_R2_ACCESS_KEY_ID     R2 API token id
OBC_R2_SECRET_ACCESS_KEY
OBC_R2_ENDPOINT          optional, overrides the derived endpoint
```

```sh
set -a; . tools/obc.local; set +a
python3 host/obc-dem/reference/ingest.py publish --archive /tmp/cp4-work/archive
```

The archive sits beside the catalog prefix, which `obc bake clean-r2` purges. The bakery reads a
local archive, named as `OBC_REFERENCE_ARCHIVE` in `tools/obc.local` so that every `obc bake`
passes `--reference`; a bake that forgets it re-bakes every lifted cell without lifts. Another
machine mirrors the box it bakes first:

```sh
python3 host/obc-dem/reference/ingest.py mirror --archive ref/ --bbox 8.30,46.75,8.60,46.95
obc-dem bake --reference ref/ …          # one box, straight to containers
obc-bake terrain --reference ref/ …      # the curated coverage, into a published tree
```

Mirror the box the *cells* cover, not the rider's box: a terrain cell overhangs a coverage
polygon by up to its own side. `mirror` pads the box by one tile for the baker's node halo, and
names the tiles the archive does not hold.

The two bakers answer a short mirror differently. `obc-dem bake` warns and names the tiles.
`obc-bake terrain` refuses the run, names how many cells are short and prints a copy-pasteable
`mirror --bbox` that covers them; `--allow-short-reference` publishes anyway. Either way the
cell's skip key records the tiles the bake could read, so completing the mirror re-bakes exactly
the short cells.
