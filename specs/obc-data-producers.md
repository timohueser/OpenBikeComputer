# obc data producers

The producers `forest`, `climate`, `colour`, `snow` and `wiki` of the
[prepared sources](obc-data-prepared.md): the blocks and the files of their versions.

- Writers: the producers of the `obc data` binary of `host/obc-data-steps` and their programs in
  `tools/`.
- Readers: the steps of [the products](obc-data-products.md).
- Code authority: `host/obc-data-steps` (`*_reference.rs`), `tools/`.
- Related: [Shared Wikimedia content](wikimedia-content.md#the-wiki-source) (`wiki`),
  [climate tiles](planner-climate-tiles.md), [snow tiles](planner-snow-tiles.md),
  [TrailView tiles](trailview-tiles.md).

## `forest`

The producer `forest`, format 4, makes the forest of each cell of the zoom 12 tiles at
256 × 256 cells, which [the canopy archive](trailview-tiles.md#canopy) packs, in one file per zoom
10 tile. A version adds a region to the newest complete version of `forest` in the home, in the
same format.

- A block is a zoom 10 tile: block `(x, y)` holds the zoom 12 tiles `(x', y')` with `x' >> 2 = x`
  and `y' >> 2 = y`.
- The canopy models are the `canopy-<key>` rows: one per country, whose key is the lowercase
  country code, and `canopy-global`. The tiles of a model are the zoom 12 tiles whose inside meets
  its `extent`.
- The blocks of a region are the blocks with a tile of a model that a box or a `geofabrik-poly`
  polygon of a leaf of the region meets; for a version of everything, the blocks with a tile of a
  national model. A run keeps the version of each polygon that the first run of the version read, as for
  [`terrain`](obc-data-prepared.md#terrain). A version has a file for each block of its region, also for a block
  without a pixel, and makes each zoom 12 tile of the block from each model that has the tile, also
  from a model of a country that the region does not name. So the pixels of a block do not depend
  on the region. Each other block of the version before stays in `reused`; the manifest names the
  scopes of the versions before in `extends`. The review counts the blocks of a region of boxes and
  names the models of their tiles; for a region with a `geofabrik` leaf, `files` is `null`, and the
  review names every canopy model.
- For each block, `python -m tools.forest_reference fetch --block 10-<x>-<y> 12-<x'>-<y'>=<keys> …`
  writes `10/<x>/<y>.tif` and then `10-<x>-<y>.sha256`, the SHA-256 of its pixels. The keys of a
  tile are the models that have it, the national ones in the order of `data/sources.toml` and then
  `global`; a cell takes the first model that has a pixel in it. It keeps no raw raster. The run
  keeps the SHA-256 of each block until the version is complete, so a resume makes no block again
  that it has.
- A block is a GeoTIFF in EPSG:3857 on the 1024 × 1024 cells of its zoom 12 tiles: four bands of
  `uint16`, deflate, predictor 2, 65535 unknown, with 256 × 256 internal tiles. Tile `(x', y')` is
  the internal tile `(x' − 4x, y' − 4y)`; GDAL leaves out an internal tile without a known cell,
  which a reader reads as unknown. The SHA-256 of the pixels of a block is over the bands in
  order, rows north first, little endian.

  | Band | Value of a cell |
  | --- | --- |
  | `height_dm` | The 95th percentile of the canopy pixels in decimetres, at most 800; 0 without a canopy pixel |
  | `cover_pct` | The share of the pixels that are canopy, in % |
  | `stems_per_ha` | The tree tops per hectare of the known pixels |
  | `conifer_pct` | The share of coniferous trees of the leaf type samples with a tree, in %; 65535 without one |

- A canopy pixel is 3 m high or more. A tree top is a pixel of the canopy that a binomial 3 × 3
  mean smooths, 5 m high or more, that no pixel within its crown radius exceeds: 0.5 m plus 8 % of
  its height, from 1 m to 4 m. Tops of one height within the crown radius of each other, directly
  or through other such tops, are one tree, such as a flat crown; its top is the first in raster
  order.
- `global` reads `canopy-global`, Canopy Height Maps v2, through HTTP range requests: a COG per zoom
  10 tile, whose pixels of zoom 17 lie on the cells, 32 × 32 to a cell. Its heights are whole metres,
  where 0 is also no data. A cell takes 0.982 times its height plus 3.73 m, and the stems that fill
  41.7 % of its cover with crowns of that height.
- `ch` reads `canopy-ch` through HTTP range requests, in strips of whole rows of its internal
  tiles, so each internal tile downloads once: at each 0.5 m pixel, the newest acquisition that has
  it.
- The leaf type of a cell with a height is `hrl-dominant-leaf-type` of 2024 at 4 × 4 samples per
  cell, from its tiles on CDSE S3.
- The run moves the blocks into the home in batches of 64. The manifest names each model that the
  version read and `hrl-dominant-leaf-type` at the date of the version, with the [digest](obc-data-layers.md#keys) of
  the pixel SHA-256 of the blocks that it made.

## `climate`

The producer `climate`, format 1, makes the daily values of `era5-land` for each cell of
[the climate grid](planner-climate-tiles.md#cells), without a calibration. `planner/climate`
bakes the climate archive from them, and applies `wet_day_mm`, `rain_factors` and `wind_factor`.
A version adds blocks and years to the newest complete version of `climate` in the home, in the
same format.

- A block holds 96 × 64 cells, 12 × 16 chunks of the ARCO stores, so no two blocks read one
  chunk. Block x, y holds the cells whose Zarr column, (i − 1) mod 3600 for the cell column i, is
  96x to 96x + 95, and whose Zarr row, 1800 − j for the cell row j, is 64y to 64y + 63. A block
  with x = 37 or y = 28 holds fewer cells. A block has the size of a zoom 6 climate tile, but it is
  not a tile of the archive.
- The blocks of a region are those that a box or a `geofabrik-poly` polygon of a leaf of the
  region meets, after the block grows by 0.2° on each side: the blocks of the cells that overlap
  the outline and of the two cells around each, which the bake of a box reads for the lapse
  rates. A run keeps the version of each polygon that its first run read, as for
  [`terrain`](obc-data-prepared.md#terrain). A version of everything has every block. A region within 0.3° of the
  antimeridian refuses: its bake would read cells on both sides.
- The years are the ten years from `climate.first_year` of [`data/planner.toml`](obc-data-registry.md#dataplannertoml).
  For each block of its region, a version holds `orography/<x>/<y>.gz` and `<year>/<x>/<y>.gz` of
  each year, also for a block without land. A file that the version before holds stays in
  `reused`, and the run does not make it again: final data of a year does not change. The manifest
  names the scopes of the version before in `extends`.
- For each block, `python -m tools.planner_climate prepare --block <x>-<y> --years <year>,…`
  writes the files that the version lacks: with `--orography DIR` also the orography, from the
  orography file that the first program downloads into `DIR` of the run. It reads the chunks of
  the block with the `credential` of `era5-land`, final data only, as the
  [climate archive](planner-climate-tiles.md#source) says, at most 2 at once, so the 8 programs
  open at most 16 connections to the stores. It tries a request again after a 429 or 5xx answer
  or a network error, for about 35 minutes with longer waits. Then it writes
  `<x>-<y>.orography.sha256` and `<x>-<y>.<year>.sha256`, the SHA-256 of each file, and deletes
  its chunks; a block that fails keeps them for the next run.
- The review counts into the store the chunks and the files of the programs that run at once, and
  a batch of files that waits for its move into the home.
- A file is gzip, with no name and time 0, of planes in the order of the table. A plane has one
  value per cell for each of its indexes. For each index, from 0 in order, the cells of the block
  follow in row order: the north row first, the west cell first. A value is little-endian. A
  missing value is NaN, or 255 for `sector`. A day is a local solar day of
  [the climate archive](planner-climate-tiles.md#time), from day 0 on 1 January; a day with a
  missing hour is missing.

  | Plane | Type | Indexes | Value of a cell |
  | --- | --- | --- | --- |
  | `tmax` | float32 | day | The maximum of the 24 hourly 2 m temperatures of the day, in K, as the store holds it |
  | `tmin` | float32 | day | The minimum of the same 24 values, in K |
  | `rain` | float64 | day | The sum, in hour order, of the 24 hourly precipitation totals of the day in mm, each at least 0 |
  | `wind` | float64 | day | The mean of the 10 hourly 10 m wind speeds of the day time, in m/s |
  | `sector` | uint8 | 10 × day + hour − 9 | The [sector](planner-climate-tiles.md#values), 0 to 15, of the wind at the local hour 09:00 to 18:00 |

  The orography file has one plane: the geopotential of the cell in m²/s², float64, from the
  orography file of the climate archive. With these types, `planner/climate` computes the same
  values as from the hourly data, so its archive has the same bytes except `inputs`.
- The run moves the files into the home in batches of 64. The manifest names `era5-land` at the
  date of the version, with the [digest](obc-data-layers.md#keys) of the SHA-256 of the files that it made.

## `colour`

The producer `colour`, format 1, makes the Sentinel-2 measurements of each texel of the zoom 12
tiles, per quarter of a year, from `sentinel-2-mosaics`: the CDSE collection
`sentinel-2-global-mosaics`. `planner/trailview-colour` and `planner/trailview-material` read
them and apply the rules of [the colour](trailview-tiles.md#colour): the look minimum, the NDVI,
the shadow rule and the snow rule. A version adds blocks and years to the newest complete version
of `colour` in the home, in the same format.

- A block is a zoom 10 tile, which holds 4 × 4 zoom 12 tiles. The blocks of a region are those
  whose inside a box or a `geofabrik-poly` polygon of a leaf of the region meets. A run keeps the
  version of each polygon that its first run read, as for [`terrain`](obc-data-prepared.md#terrain). A version without
  a region refuses.
- The year is `trailview.colour_year` of [`data/planner.toml`](obc-data-registry.md#dataplannertoml). For each block
  of its region, a version holds `<year>/<x>/<y>.pmtiles`, with x and y of zoom 10. The file of
  a block without data is empty. A file that the version before holds stays in `reused`, and the
  run does not make it again. So a new region or a new year adds files, and a new format makes each
  file again. The manifest names the scopes of the version before in `extends`.
- For each block, `python -m tools.trailview.colour prepare --block <x>-<y> --year <year>`
  writes the file of the block and then `<x>-<y>.sha256`, the SHA-256 of the file. It reads the
  window over the block, one pixel wider on each side, of each band of each mosaic tile of the
  year that the STAC search of the collection lists, from CDSE S3 with the `credential` of
  `sentinel-2-mosaics`, one window at a time. CDSE S3 allows 4 connections per user, so 4
  programs run at once. It deletes its windows when the file is complete; a block that fails
  keeps them for the next run.
- In each quarter, the program puts the bands B02, B03, B04, B08 and `observations` on the
  512 × 512 texels of each zoom 12 tile in EPSG:3857: bilinear where the texel is under 10 m, the
  mean of the pixels otherwise. The mosaic tiles go in the order of their ids, and a later tile
  writes over an earlier one where it has a pixel. `refl` is a band ÷ 10000, clipped to 0 to 1,
  and 0 without a pixel: a negative band value, as over deep water, is 0.

  | Plane | Value of a texel |
  | --- | --- |
  | Red, green, blue, near-infrared | round(255 × refl^(1/2.2)) of B04, B03, B02 and B08 |
  | Look | min(15, ⌊`observations`⌋) where B04 has a pixel; else 0 |

- A file is PMTiles v3 at zoom 12 only: tile type `unknown` (0), no tile compression, metadata
  `{}`. A tile is five lossless WebP images of 512 × 512 texels, one after the other: the quarters
  1 to 4 as RGBA, with the near-infrared in alpha; then the looks of the quarters 1 to 4 as red,
  green, blue and alpha. A texel keeps each byte where its alpha is 0 (WebP `exact`). The length
  of an image is 8 + the RIFF size in bytes 4 to 7, little endian. A tile whose look is 0 in each
  quarter is not in the file.
- A file depends only on its block, its year and the mosaics, never on the region.
- The review counts into the store the windows of the 4 programs and a batch of files that waits
  for its move into the home.
- The run moves the files into the home in batches of 64. The manifest names
  `sentinel-2-mosaics` at the date of the version, with the [digest](obc-data-layers.md#keys) of the SHA-256 of the
  files that it made.

## `snow`

The producer `snow`, format 1, makes the per-year tiles of zoom 6 to 12 of
[the snow tiles](planner-snow-tiles.md), one season at a time. `planner/snow` joins the seasons of
a region and makes its three archives. A version adds blocks and seasons to the newest complete version of `snow` in the home,
in the same format.

- A block is a zoom 6 tile of Web Mercator, and holds its tiles of zoom 6 to 12. The blocks of a
  region are those that a box or a `geofabrik-poly` polygon of a leaf of the region meets. A run
  keeps the version of each polygon that its first run read, as for [`terrain`](obc-data-prepared.md#terrain). A
  version of everything has every block.
- The seasons are those of `snow.seasons` of [`data/planner.toml`](obc-data-registry.md#dataplannertoml). For each
  block of its region, a version holds `<season>/<x>/<y>.pmtiles` of each season, also for a
  block without data, whose file is empty: PMTiles has no archive without a tile. A file that
  the version before holds stays in `reused`, and the run does not make it again: the data of a
  past season does not change. So a new region or a new season adds files, and a new format makes
  each file again. The manifest names the scopes of the version before in `extends`.
- A file is PMTiles v3 of the tiles of the block that the season has, as
  [the snow tiles](planner-snow-tiles.md#zoom-levels) define them: tile type `unknown`, gzip
  with no name and time 0, the metadata `{"season": <season>}`. The body of a tile is its season
  block: the onset and the melt-out planes, 131,072 bytes. The rules of the archive read only the
  source pixels near a tile, so a block has the same bytes in each version that makes it.
- For each block, `python -m tools.planner_snow prepare --block <x>-<y> --seasons <season>,…
  --modis <list> --canopy <file>,… --slots <dir>` writes the files of the seasons that the version
  lacks, one season at a time. Of each season, the program reads:
  - The MODIS files of the archive dataset `modis-snow` in place, from the root that holds its
    manifest: `<list>` names each file of a MODIS tile that the block meets with two MODIS pixels
    of margin, the most that the MODIS grid of the program reaches past the block. A day and a
    platform of a tile take the COG of the Planetary Computer, else the HDF4 granule of NSIDC. A
    prepare refuses when no root holds the dataset, and when a MODIS tile of the block lacks a
    file of either platform on more than 7 days in a row of a season: the nearest clear day fills
    such a run as it fills a cloudy week, and a longer one is a gap of the source. The number is
    chosen. A tile that `data/archive/modis-snow.toml` names and the block meets lacks every day
    when the dataset has no file of it; a tile that it does not name is sea or outside the
    dataset. The checks run before the program fetches anything.
  - The `hansen-gfc` tiles at the `start` of its row that the block meets with two MODIS pixels of
    margin, which the run fetches.
  - SCO, SCM and SCD of HR-WSI from CDSE S3, with the `credential` of `hr-wsi`: the products of
    the season that the catalogue lists for the block, in the order of their names, a window of
    each zoom 10 tile at a time. A block fails when its catalogue lists products for one season
    and none for another of the seasons that it makes, and when those seasons have other tiles. An empty listing is asked once more; a block outside
    HR-WSI or at sea has no product. The programs share the 4 connections that CDSE allows a user
    through lock files in `<dir>`, and GDAL retries a refused request.

  Then it writes `<x>-<y>.<season>.sha256`, the SHA-256 of each file.
- The review counts into the store the files of the programs that run at once, and a batch of
  files that waits for its move into the home.
- The run moves the files into the home in batches of 64. The manifest names `hr-wsi` at the date
  of the version with the [digest](obc-data-layers.md#keys) of the SHA-256 of the files that it made, `modis-snow`
  at the version of its dataset with the digest of the SHA-256 of the files of the lists, and the
  `hansen-gfc` version that it fetched.

## `wiki`

The producer `wiki`, format 1, makes the Wikimedia facts and the article text of the OSM subjects
in the shards of [Shared Wikimedia content](wikimedia-content.md#the-wiki-source):

- It reads cached archives in place. `OBC_WIKI_DUMPS` overrides the default `wiki-dumps/` in the
  store. Of each kind it reuses the newest cached day: `wikidata-<day>-all.json.bz2`,
  `wikidatawiki-<day>-page.sql.gz` and `-redirect.sql.gz`, and for each selected article language
  `<lang>wiki-<day>-langlinks.sql.gz` and the Wikimedia Enterprise HTML
  `<lang>wiki-NS0-<day>-ENTERPRISE-HTML.json.tar.gz`. A preparation without a region, or with
  a box, reads `planet-<yymmdd>.osm.pbf`. Each Geofabrik area of a region reads
  `<area>-<yymmdd>.osm.pbf` under the dump directory, such as `europe/dach-261009.osm.pbf`.
  The path of the area supplies the subdirectories. These extracts select the regional subjects
  before article selection or Commons acquisition. For a missing kind, the review discovers a
  completed upstream version through metadata only. SQL jobs must be `done`; Wikidata JSON must
  have a checksum entry. Wikipedia HTML uses the Enterprise Snapshot API. The review shows each
  file, date, size and bytes to download. It does not download archives. Start downloads missing
  archives serially, with Range resume and the reviewed size and ETag, then prepares the source.
  A changed remote identity refuses. The archive becomes visible only after the download ends.
  Credentials stay in the local environment and private token cache. No review or manifest holds
  them. The manifest names `wikidata`, `wikipedia` and
  `osm-planet` or `geofabrik-extracts` at the days of their dumps, with the [digest](obc-data-layers.md#keys) of the
  SHA-256 of the files.
- `--languages en,de` selects article languages from `specs/content-languages.json`. Without
  the flag, all supported languages are selected. Unknown or empty codes refuse; duplicates
  are removed and codes have supported order. The review, its id and the manifest keep the
  selection. Only selected languages require HTML and language-link dumps.
  [The `wiki` source](wikimedia-content.md#the-wiki-source) gives the facts of an excluded
  language.
- Start saves the reviewed input plan before it downloads. A resume uses this plan, including
  the URLs, sizes and object identities. The review ID covers these decisions, but not whether
  the download has since completed. After acquisition, the run pins each dump with its path, size,
  modification time and SHA-256, and `publish_text`. A resume reads exactly those dumps, and refuses when one is gone or changed,
  or `publish_text` changed; the fix deletes the work of the version, so the next run starts a
  new one. A dump of the pinned size with another modification time is the pinned dump when its
  SHA-256 is. `tools.wiki` gets each SHA-256 and does not hash a dump again. A row without
  `publish_text` refuses.
- `python -m tools.wiki` runs the steps: `subjects` lists the OSM objects with a `wikidata` or
  `wikipedia` tag, `import` reads the Wikidata dump twice, `candidates` gives the classes, the
  subjects and the peaks, `articles` reads the language links and the HTML, and `shards` writes
  the version. A step whose output the run has does not run again, and `tools.wiki` does not
  repeat an import that finished with the same input.
- The selection is the rule of the compilers: each subject of a landmark category, and each peak
  with a resolved link of a node that the map takes as a summit. Only their articles are read, and
  only the photos that the compilers choose for them come from Commons, as [Shared Wikimedia content](wikimedia-content.md#the-wiki-source)
  says.
- A box keeps the subjects whose box meets it. A region whose leaves are all Geofabrik regions
  reads the extract of each of their areas. An object in two extracts is the object of its
  highest version; extracts with one version give the box around their boxes. Other scopes, such
  as a union of a box and a Geofabrik region, refuse.
- A version of a region extends the newest complete version of `wiki` in the home, in the same
  format, and the manifest names the scopes of that version in `extends`. The version holds each
  shard of the version before, and each shard of its run. A shard of both holds the facts of the
  run and the earlier facts. A fact has one form in every shard of the version: the form of the
  run, unless the run lacks the fact, has an older revision of it, or has it only as an article
  of a language that the run did not prepare. Then the fact keeps its earlier bytes and its asset,
  also its languages. A shard writes its facts in the order of kind and key, and an earlier shard
  whose members do not change stays in `reused`. A version of everything extends no version.
- An extension whose row has another `publish_text` than the version before refuses before its
  first step. The fix is the `publish_text` of the
  version before, or a version of everything.
- The run moves the shards into the home in batches of 64, then `index.json`. A resume names the
  shards in the home to `shards`, which does not write them again.
- With `publish_text = false`, each article is a `missing` fact, as
  [the `wiki` source](wikimedia-content.md#the-wiki-source) says. The review says "text published"
  or "text kept local".
