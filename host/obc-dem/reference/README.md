# The reference archive

Copernicus GLO-30 is the native elevation source, but 30 m cannot hold a rock tower. The archive
holds finer national DTM heights on the OBCT lattice, and the bakery lifts a crest to them. See
`specs/OBCT_Spec.md` §9 for the lift rule.

`obc data source prepare terrain` fills the archive from the national DTMs, never a bake.

## Run it

`obc data source prepare terrain --region R` adds a region to the version before: per tile,
`ingest.py fetch <keys> --tile <ti>-<tj> --base …` folds each model into the tile before, then
`index-terrain` runs per pool. `merge.py --step` is `device/reference/<leaf>` and
`planner/reference`: the tiles of the version, cut to the leaf.

The steps need the uv group `terrain-reference` of the root `pyproject.toml`. One tile by hand:

```sh
uv run --group terrain-reference python3 host/obc-dem/reference/ingest.py \
    fetch ch --tile 4812-4223 --work /tmp/ch-work --out /tmp/ch
```

## The tile contract

`write_tile` in `ingest/archive.py` writes it, and `tile_problems` in `tests/test_ingest.py`
holds a tile to it.

- **Lattice**: OBCT `GRID_ORIGIN` (−2^28 µdeg on both axes), step `2^6` µdeg
  (`REFERENCE_STEP_LOG2 = 6`, about 7.1 m latitude and 4.9 m longitude at 47° N).
- **Tile**: `2^16` µdeg square = 1024 × 1024 pixels, at `16/<ti:04>/<tj:04>.tif` under the archive
  root, where `(ti, tj) = ((lat − ORIGIN) >> 16, (lon − ORIGIN) >> 16)`.
- **Value**: `int16` little-endian orthometric metres, the **maximum** of every source pixel centre
  inside the lattice square. A square that no centre reaches and that lies between four centres
  with a height has their bilinear height (`fill_gaps`). `−32768` is the only nodata. Single
  band, `PixelIsArea`, EPSG:4326, deflate, 256 × 256 internal tiles. The GeoTIFF stores rows
  north-up.
- **Mean** (prepared versions only): the **mean** of the same centres in `int32` decimetres, or
  the same bilinear height, nodata `−2^31`, predictor 2, in `mean/`.
- **Transform** must be exactly the lattice (`a = 2^6 / 1e6`, `e = −2^6 / 1e6`, origin on lattice
  lines) within 1e-9 degrees. The baker does no reprojection and no resampling.
- **`index.json`** at the archive root names every tile. A tile the index does not name is not in
  the archive.

```json
{ "schema": 1, "step_log2": 6, "tile_log2": 16, "pool": "max",
  "sources": { "ch": { "product": "swissALTI3D 2 m", "attribution": "© swisstopo",
                       "licence": "…", "vertical_datum": "LN02/LHN95", "fetched": "2026-09-21",
                       "glo30_bias_m": -1.42 } },
  "tiles": { "3410/2882": "ch" },
  "contributors": { "3410/2882": ["ch", "es"] },
  "sha256": { "3410/2882": "9f86d0818…" } }
```

`pool` is `max` or `mean`. `sha256` is over the tile's **pixels**, not the
file, because two GDAL builds deflate the same
heights into different bytes and the terrain bakery keys its skip decision on the digest.
`contributors` lists every source holding a pixel in the tile, best priority first; `tiles` names
the first of them. `glo30_bias_m` is the median of model − GLO-30 that the terrain prepare
measured once (`specs/obc-data.md` §`terrain`); the planner corrects GLO-30 by the bias of the
best source of a tile. `index-terrain` writes it.

## The code

| module | what is in it |
| --- | --- |
| `lattice.py` | Integer microdegrees: the grid, `Window`, tile ids, and the refusals for a box the lattice cannot hold. |
| `pool.py` | One source raster in, the lattice maximum and mean out: voids, the pooling rule and the fill of a coarse source. |
| `archive.py` | Tiles, `PRIORITY`, the per-pixel fold, the pooling of one tile, and the deliveries on disk. |
| `sources/` | One module per country, one row per product. `base.py` is what an adapter is; `__init__.py` is the `SOURCES` registry. |
| `cli.py` | The `fetch` and `index-terrain` steps. |

An adapter has one job: `fetch(bbox, workdir) -> list[Path]`, rasters in any CRS and dtype.
The shared tail turns voids, the row's `void` and heights outside −500 m to 9 000 m into
absence, and pools pixel centres onto the lattice tile.

`PRIORITY` in `ingest/archive.py` orders the sources, finest and best-maintained first. The fold
is per pixel, because coverage stops at borders: a better source keeps its own pixels and leaves
the others' pixels in its gaps.

Refused by name: a box at the antimeridian or outside the world box, a scaled band, an ESRI ASCII
or XYZ grid whose row names no `grid_epsg`, and a vertical datum `ORTHOMETRIC` in `sources/base.py` does
not recognise, unless the row has a `summit`. A row's `summit` is checked once per prepare before
the model is used: a model whose highest height within about 1 km is more than 10 m from it is
refused. An ellipsoidal product must be converted to orthometric first; only the `sk`
adapter does, with its agency's quasigeoid.

Memory is one source raster plus the tile. A **monolithic** raster, such as a
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

Every request goes through `with_retry` in `sources/base.py`: a dropped connection, a 408, a 429
and a 5xx, and an OWS `NoApplicableCode` or `LayerNotDefined` on any status, are retried for up to
twelve minutes; every other 4xx is refused at once with its body. A 404 is absence
only where the registry says a name is arithmetic, such as a grid square outside its state. A WCS
2.0.1 request is clipped to the envelope `DescribeCoverage` states; a subset outside it is refused.

## Sources

`SOURCES` in `ingest/sources/__init__.py` is the registry: each row holds its key, country,
product, resolution, licence and vertical datum. Its credit and its box are `attribution` and
`extent` of `dtm-<key>` in `data/sources.toml`.

Open services, no key: `ch`, `fr`, `us`, `no`, `es`, `nl`, `gb-eng`, `gb-sct`, `gb-wls`, `at`, `ca`,
`nz`, `pl`, `cz`, `sk`, `si`, `is`, `it`, `it-bz`, `it-tn`, `it-21`, and the German states `de-nw`, `de-he`, `de-bw`,
`de-mv`, `de-st`, `de-by`, `de-sn`, `de-th`, `de-ni`, `de-rp`. Germany publishes elevation per
state, so `de-*` is a family of rows. `it` and `is` are 10 m, coarser than the lattice: their
centres reach only some of its pixels, and `fill_gaps` gives the others the bilinear height of
the four centres around them.

Behind a free account: `dk`, `se`, `fi`, and `pt` and `pt-20`, which share one DGT account. The
machine that prepares sets the credential; each module names where the account is made. `au` is ordered by hand: `OBC_REFERENCE_AU_INPUT` names
the delivery as an absolute path, and `OBC_REFERENCE_AU_DATUM` its datum.

`de-sn` and `de-th` carry the Quellenvermerk their services state. One obligation the rows cannot settle by themselves:

- **`au` needs one answer by hand.** Some ELVIS datasets are ellipsoidal, so a fetch of `au`
  refuses until `OBC_REFERENCE_AU_DATUM` is `AHD`.

A fetch reads every `.tif`, `.tiff`, `.asc`, `.xyz` and `.zip` under a delivery and **writes
nothing into it**; unpacked members go into the work directory under the digest of their bytes.

## Attribution

The attribution of every source a map's cells read must travel with that map, beside the
Copernicus attribution the native heights carry. `index.json` states it per source; the terrain
bakery records per cell which sources its lifts came from, and the catalog's terrain block lists
each of them once in `references` (`OBCC_Spec.md` §13.1).

## A local archive

`obc-dem bake --reference DIR` and `obc-bake terrain --reference DIR` read a local archive, such as
the `reference/` layer of `device/reference/<leaf>`. `OBC_REFERENCE_ARCHIVE` in `tools/obc.local`
names one for every `obc bake`; a bake that forgets it re-bakes every lifted cell without lifts.

An archive must hold the tiles the *cells* read, not the rider's box: a terrain cell overhangs a
coverage polygon by up to its own side, and the node halo reads one tile further.

The two bakers answer a short archive differently. `obc-dem bake` warns and names the tiles.
`obc-bake terrain` refuses the run, names how many cells are short and prints the box that covers
them; `--allow-short-reference` publishes anyway. Either way the cell's skip key records the tiles
the bake could read, so completing the archive re-bakes exactly the short cells.
