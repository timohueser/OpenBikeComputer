# The reference archive

Copernicus GLO-30 is the best free *global* elevation source, but 30 m cannot hold a rock tower:
at Engelberg its bilinear surface runs 100 m below the Hahnen's summit. National mapping agencies
publish LiDAR one to two orders finer, and most of it is open data. The bakery adds the height a
finer DTM measures on a crest to the native lattice, so one surface serves Peak View, the contours,
the ascent, the profile and the altimeter.

The bakery cannot hold a country-sized reference in memory, and it must not learn seven service
protocols. So there is **one archive with one format**, and one offline tool that fills it:

- `ingest.py` turns a national DTM into archive tiles. Every country branch is an adapter in
  `ingest/sources/`. It runs once per source release, never during a bake.
- The archive holds max-pooled bare-earth height on the OBCT lattice, as `int16` metres, in
  GeoTIFF tiles. The baker streams the tiles of one cell and reads nothing else.
- The archive lives on R2 under `reference/v1/`. A bakery run mirrors the tiles its box needs.

## The archive tile contract

This is the normative contract. The baker and the ingest tool both hold to it.

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

### The additions to the index

`index.json` carries three things more than the contract above shows: two maps under the same tile
ids as `tiles` — **`sha256`**, the digest of each tile's pixels, and **`contributors`**, every
source that holds a pixel in the tile, best priority first — and **`vertical_datum`** in each
source's entry.

```json
{ "schema": 1, "step_log2": 6, "tile_log2": 16,
  "sources": { "ch": { "product": "…", "attribution": "…", "licence": "…",
                       "vertical_datum": "LN02/LHN95", "fetched": "2026-09-21" } },
  "tiles": { "3410/2882": "ch" },
  "contributors": { "3410/2882": ["ch", "es"] },
  "sha256": { "3410/2882": "9f86d0818…" } }
```

The digest is over the tile's **pixels** — row-major, little-endian `int16` — and not over the
file. Two GDAL or zlib builds deflate the same heights into different bytes, and the terrain bakery
keys its skip decision on this digest, so it has to mean "these heights". `tiles` keeps the
shape the contract states, so a reader that wants the source key only reads one map, as before.

`sources` holds the four fields the contract names, plus `vertical_datum`. The full facts of a
source — its country, its step in metres and the tiles it wrote pixels into — stay in its manifest
at `sources/<key>.json`, which is what `index` rebuilds the index from.

`tiles` names a tile's **best-priority contributor**, which is `contributors[tile][0]`. A tile can
hold pixels from more than one source, because coverage stops at borders, so a consumer that needs
the attribution of everything in a cell reads `contributors`, and one that needs the best source
reads `tiles`. `sources` lists every source that holds a pixel anywhere in the archive, which is
what a published map has to name.

A source loses its place in `contributors` when a better source has overwritten every pixel it
wrote in that tile. The check is over the tile as a whole, not per pixel, so a source keeps its
place while any earlier pixel survives, whoever wrote it.

## The tool

```sh
python3 ingest.py ingest ch --bbox 8.30,46.75,8.60,46.95 --archive ref/ --work /tmp/ch
python3 ingest.py wizard se --bbox 18.48,67.89,18.51,67.91 --archive ref/
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
| `ingest <key>` | The source adapter obtains rasters for the box; the shared tail writes tiles. `--input <dir>` takes hand-fetched files instead of the service. `--work <dir>` is where fetched rasters are cached, so a second run of the same box downloads nothing. |
| `wizard <key>` | Walks the account and download steps of a source behind a login, one step at a time, checks what landed, and then runs `ingest <key> --input <dir>`. It automates no part of a login. |
| `index` | Rebuilds `index.json` from the manifests in `sources/`. |
| `check` | Opens every tile and holds it against the contract — size, dtype, nodata, CRS, band count, `AREA_OR_POINT`, deflate, 256 × 256 blocks, little-endian, the exact transform, the pixel digest and the contributors entry — and refuses a tile the index does not name. |
| `publish` | `rclone copy` of the archive to `<bucket>/reference/v1/`: tiles and manifests first, then the index, which goes up as the merge of the index already on R2 with this archive's. Additive and idempotent: a publish never deletes. |
| `mirror` | `rclone copy` of the index plus the tiles one box needs, into a local directory. The box is padded by one tile on every side, because the baker's node halo reads over a cell edge. It names the needed tiles the archive does not hold. This is what a bakery run does before `obc-dem bake --reference`. |

### Where the code is

`ingest.py` is the entry point only. The tool is the `ingest/` package beside it, and the split
is the one the archive already had in it: a lattice, a pooling rule, an archive, a remote, and a
registry of sources.

| module | what is in it |
| --- | --- |
| `lattice.py` | Integer microdegrees: the grid, `Window`, the tile ids, and the refusals for a box the lattice cannot hold. |
| `pool.py` | One source raster in, lattice `int16` metres out: the void convention and the pooling rule. |
| `archive.py` | Tiles, the priority merge, the manifests, `index.json`, and `tile_problems`, which is the contract as code. |
| `publish.py` | The rclone remote and the plans `publish` and `mirror` run. |
| `sources/` | One module per country, and one row per product. `base.py` is what an adapter is, and `__init__.py` is the registry the CLI reads. The five publication models are `protocols.py` (a service that answers a box: ArcGIS, WCS 1.0.0, 1.1.1 and 2.0.1), `ch.py` (STAC items), `stac.py` (a STAC search), `grid.py` (tiles named on a national kilometre grid) and `cog.py` (a window out of a remote COG too large to download). |
| `wizard.py` | The steps of a portal behind a login, and the check on what it delivered. |
| `cli.py` | The subcommands, and nothing else. |

A credential is one field on a row and one seam in the tool. `Credential` in `sources/base.py`
reads it out of the environment — never out of argv, which every process on the box can read —
and `require_credential` refuses before a request is made, naming the variable, `--input` and
the wizard. A portal reads its key from the query or from an `Authorization` header, and those
are the two shapes `Credential` has: the request a keyed WCS sends is the request the keyless
one sends plus one parameter.

Every request in the registry goes through `with_retry` in `sources/base.py`, so one rule covers
all of them: a dropped connection, a 429 and a 5xx are retried three times; every other 4xx is the
server's final answer and is refused at once with its body, because that body is the only thing
that says what happened. A 404 is absence only where the registry says a name is arithmetic — a
grid square outside its state — and everywhere else it is a refusal.

Adding a country is a module in `sources/` and a row in that registry. Nothing above it changes.

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
   every lattice pixel a source pixel merely touches. Over the Engelberg box, area overlap read
   51.7 % of the 15.9 M pixels too high and none too low, by up to 286 m; it filled 2 659 pixels no
   source pixel centre reached; and it spread a one-pixel tower over two to four archive pixels.
3. The heights become `int16` metres, rounded half away from zero, with `−32768` for absence.
4. The window is cut into tiles. A tile is always whole: a box that reaches a corner of a tile
   still writes 1024 × 1024 pixels, with nodata where the box did not reach.

Then the priority rule decides which pixels the tile keeps.

### Priority

`PRIORITY` in `ingest/archive.py` is the one constant, finest and best-maintained national
product first. A test holds this block against the constant, so the two cannot drift:

```priority
dk, nl, it-tn,
de-nw, de-he, de-ni, de-by, de-sn, de-th, de-mv, de-st, de-bw,
fr, at, no, uk, se, us, ca, nz,
ch, fi, it-bz, es, au
```

The bands are the product's step and how much of it a service will part with: under half a
metre first, then the metre-grade national and state products, then two to two and a half
metres, then five. `de-bw` is last of the German states because its service answers whole
metres. `au` is last of all: an ELVIS order is 1 m where a survey covers it and 5 m where
none does, and one row cannot say which, so it never displaces a source whose step is known.

The rule is per pixel, because coverage stops at borders and survey edges: a better source over one
corner of a tile must not take the rest of the tile away from the source that does cover it.

- Only this source has been in the tile: the **maximum**, so a second box adds its footprint and
  raises the pixels both boxes cover.
- This source ranks **better** than every source that has been in the tile: its pixels win where it
  has them, and the other sources' pixels stay in its gaps.
- Otherwise: the pixels already there stay, and this source fills the gaps only.

A pixel does not record which source wrote it, so the comparison is against the tile's contributors
as a set. One case loses by that: a source that has already been in a tile a worse source also
reached falls into the third rule, so where its own two boxes overlap the **earlier** value stays
instead of the maximum. A per-pixel owner plane beside each tile is the exact fix; it would also
double the archive. A source key that is not in the list ranks last, so it cannot displace a listed
one.

### What a source may look like

The tail is deliberately blunt, because the sources are not uniform:

- **Any CRS.** A projected national grid (LV95, ETRS89/UTM) and EPSG:4326 are both one transform
  of the pixel centres. Bounds are transformed with densified edges and the window is padded by one
  pixel, so a curved projection edge cannot push a centre out of the window; the run prints how
  many centres fell outside it anyway, which must be none.
- **A source already in degrees is placed by integer arithmetic.** When the transform's terms are
  whole microdegrees — a national product on a round step — twice each pixel centre is a whole
  number of microdegrees, so a centre on a lattice line goes to the pixel the half-open square says
  and not to the one the last floating-point bit says. Everywhere else a tolerance of 1e-12 degrees
  does the same job.
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

The registry is `ingest/sources/`. A source with an adapter is fetched by `ingest <key>`; a source
without one is a row that records the licence, the attribution and the datum, and says how to get
the rasters for `--input`. Every probe is recorded below, including the ones that failed, so a
later pass does not start from nothing.

Alpine Italy was the border-ridge gap: a ridge along the Swiss or Austrian border was lifted on one
side only, and that step shows in contours and in a profile. `it-bz` closes the Ortler and the
Dolomites, and `it-tn` the Brenta and the Adamello. Lombardy, Piedmont and Aosta Valley are still
open, and the table says what each one answered.

### Open services, no key

| key | country or state | step | product | licence | attribution | protocol |
| --- | --- | --- | --- | --- | --- | --- |
| `ch` | Switzerland | 2 m | swissALTI3D | Open data, attribution required | `© swisstopo` | STAC items |
| `fr` | France | 1 m | RGE ALTI (IGN) | Licence Ouverte | `© IGN` | WMS, float32 BIL |
| `us` | United States | 1 m | 3DEP (USGS) | Public domain | `USGS 3DEP` | ArcGIS `exportImage` |
| `no` | Norway | 1 m | NHM DTM (Kartverket) | CC BY 4.0 | `© Kartverket` | WCS 1.0.0 |
| `es` | Spain | 5 m | MDT05 / PNOA LiDAR (IGN) | CC BY 4.0 | `© Instituto Geográfico Nacional` | WCS 2.0.1 |
| `nl` | Netherlands | 0.5 m | AHN DTM (PDOK) | CC BY 4.0 | `© Rijkswaterstaat / AHN` | WCS 2.0.1 |
| `uk` | England | 1 m | EA LIDAR Composite DTM | OGL v3 | `© Environment Agency` | WCS 2.0.1 |
| `at` | Austria | 1 m | ALS DTM (BEV) | CC BY 4.0 | `Bundesamt für Eich- und Vermessungswesen (BEV)` | remote COG window |
| `ca` | Canada | 1 m | HRDEM DTM (NRCan) | OGL Canada 2.0 | `Contains information licensed under the Open Government Licence – Canada` | WCS 1.1.1 |
| `it-bz` | Italy, South Tyrol | 2.5 m | DTM (Provincia autonoma di Bolzano) | CC0 1.0 | `Autonome Provinz Bozen – Provincia autonoma di Bolzano` | WCS 2.0.1 |
| `it-tn` | Italy, Trentino | 0.5 m | DTM (Provincia autonoma di Trento) | CC BY 4.0 | `Provincia autonoma di Trento` | 0.5 km grid of ESRI ASCII |
| `nz` | New Zealand | 1 m | LiDAR DEM (LINZ) | CC BY 4.0 | `Sourced from LINZ, CC BY 4.0` | remote COG window, NZTopo50 sheets |
| `de-nw` | Germany, North Rhine-Westphalia | 1 m | DGM1 | dl-de/zero-2-0 | `© Geobasis NRW` | WCS 2.0.1 |
| `de-he` | Germany, Hesse | 1 m | DGM1 | dl-de/zero-2-0 | `© HLBG Hessen` | WCS 2.0.1 |
| `de-bw` | Germany, Baden-Württemberg | 1 m | DGM1 | dl-de/by-2-0 | `Datenquelle: LGL, www.lgl-bw.de, dl-de/by-2-0` | WCS 2.0.1 |
| `de-mv` | Germany, Mecklenburg-Vorpommern | 1 m | DGM1 | Open data, attribution required | `© GeoBasis-DE/M-V` | WCS 2.0.1 |
| `de-st` | Germany, Saxony-Anhalt | 1 m | DGM1 | dl-de/by-2-0 | `© GeoBasis-DE / LVermGeo LSA` | WCS 1.0.0 |
| `de-by` | Germany, Bavaria | 1 m | DGM1 | CC BY 4.0 | `Bayerische Vermessungsverwaltung – www.geodaten.bayern.de` | 1 km tile grid |
| `de-sn` | Germany, Saxony | 1 m | DGM1 | dl-de/by-2-0 | `GeoSN` | 2 km tile grid |
| `de-th` | Germany, Thuringia | 1 m | DGM1 (2020–2025) | dl-de/by-2-0 | `© GDI-Th, Freistaat Thüringen` | 1 km tile grid |

`de-sn` and `de-th` carry the shortest Quellenvermerk their services state, because neither agency
publishes a full attribution sentence the way Bavaria and NRW do. **The owner has to confirm both
with GeoSN and GDI-Th** before a published map carries them.
| `de-ni` | Germany, Lower Saxony | 1 m | DGM1 | CC BY 4.0 | `© LGLN` | STAC search |

Germany publishes elevation per state, each on its own service, so `de-*` is a family of rows and
not one adapter. Eight of the sixteen states answer a keyless WCS and eight are a download; where
the protocol is the same the code is the same.

### Two indexes that turned out to be arithmetic

`it-tn` and `nz` were rows without an adapter, each because its index looked like a new kind.
Neither is:

- **Trentino** reaches its DTM through an asynchronous merge service: a WFS index of 25 201
  half-kilometre tiles, a POST that starts a job, a poll and a zip of zips. The same tiles are
  published as plain files under
  `siatservices.provincia.tn.it/stemdata/2014_lidar_dtm_asc/`, keyless, one ESRI ASCII grid of
  1000 × 1000 posts per square, and the WFS index says the name is the square's corner in
  hundreds of metres: the square at EPSG:25832 (643 500, 5 112 000) is `5h643551120_DTM.asc`.
  So the adapter is a grid of names, the same shape a German state's is, and the merge service
  is not used. A square the survey did not reach answers 404, which is a coverage edge.
- **New Zealand** names its tiles after NZTopo50 map sheets, and the static STAC carries 424
  item links with no box in the collection. NZTopo50 is a regular grid in NZTM2000, 24 km east
  by 36 km north: sheet `AS21` runs from (1 492 000, 6 198 000) and `BX15`, the sheet Aoraki
  stands on, from (1 348 000, 5 154 000). `sources/nz.py` holds those constants, so the sheet a
  box needs is arithmetic and the LINZ Data Service key that would fetch the sheet outlines is
  not needed. A wrong constant names a sheet whose window misses the box, which refuses rather
  than writing the wrong heights, and a test holds the constants against five published sheets.

An ESRI ASCII grid states its origin and its step and never its CRS, so a row published or
delivered as one names the grid in `grid_epsg` and the tool writes the `.prj` beside the file.
A row that names none refuses the grid rather than guessing: a `.prj` guessed wrong puts a
mountain in the wrong country.

### The vertical datum of each source

A row whose datum names no **recognised orthometric** height system is refused, and so is one that
names an ellipsoidal height: the archive is orthometric metres, and the bakery holds the reference
against Copernicus on EGM2008. `ORTHOMETRIC` in `ingest/sources/base.py` is the list a datum has to
name, and a new source's agency documentation is what adds to it. Every datum below is orthometric,
so no adapter here needs a geoid conversion.

| key | vertical datum | what the agency documents |
| --- | --- | --- |
| `ch` | LN02 / LHN95 | swisstopo's national levelling network |
| `fr` | NGF-IGN69 | IGN's levelling network for mainland France |
| `us` | NAVD88 | 3DEP heights, through the GEOID12B/GEOID18 model |
| `no` | NN2000 | Kartverket's current height system |
| `es` | REDNAP, EVRS-aligned | IGN levels MDT05 on REDNAP, whose origin is the mean sea level at Alicante, and documents REDNAP as connected to the European levelling network |
| `nl` | NAP | Normaal Amsterdams Peil |
| `uk` | Ordnance Datum Newlyn | the Environment Agency's surveys, through OSTN15 |
| `at` | EVRF2000 Austria (EPSG:9274) | BEV: "Grundsätzlich: EVRF2000 Austria, orthometrische Höhen (EPSG:9274)". The squares dated 2021-09-15 are the one exception, on the older Adria-Triest practical heights. |
| `ca` | CGVD2013 (EPSG:6647) | The HRDEM specification: "Elevations are orthometric and expressed in reference to the Canadian Geodetic Vertical Datum of 2013 (CGVD2013) (EPSG:6647)." |
| `nz` | NZVD2016 (EPSG:7839) | EPSG: normal-orthometric, realised by NZGeoid2016. It is **not** in the GeoTIFFs; the row carries it. |
| `it-bz`, `it-tn` | Italian levelling network | Both provinces publish "m s.l.m.", metres above sea level, and name no geoid model. |
| `de-*` | DHHN2016 | the German height reference. Thuringia's earlier 2014–2019 delivery is on DHHN92, so the adapter takes the 2020–2025 one. |
| `dk` | DVR90 | Klimadatastyrelsen's DHM/Terræn specification: heights are above mean sea level on DVR90, on the EPSG:25832 grid. |
| `se` | RH2000 | The tiles say it themselves: each one is EPSG:5845, the compound CRS "SWEREF99 TM + RH2000 height". |
| `fi` | N2000 | The NLS elevation model is levelled on N2000, the Finnish height system. |
| `au` | AHD | ELVIS states the Australian Height Datum for the DEMs this row names. Some other ELVIS products are ellipsoidal and are out of scope. |

An **ellipsoidal** product is a different matter, and it is refused rather than ingested: it stands
tens of metres away from an orthometric height, which is the size of a lift. Nothing in the
registry needs a conversion. The one product that is natively ellipsoidal, the ArcticDEM that fills
northern Canada, is converted to CGVD2013 by NRCan before it is published, and ELVIS's ellipsoidal
datasets are out of the `au` row's scope, which the row and the wizard both say.

### Sources behind an account

These are open data, but a download needs registration, so an unattended bake cannot pull them.
Each one is a full row: with its credential in the environment `ingest <key>` fetches live, and
without it `ingest <key> --input <dir>` takes what the portal delivered. `wizard <key>` walks the
account and the download, one step at a time, and ends by running the ingest.

| key | country | step | product | licence | attribution | datum |
| --- | --- | --- | --- | --- | --- | --- |
| `dk` | Denmark | 0.4 m | DHM/Terræn | Danish free geographic data, attribution required | `© Klimadatastyrelsen` | DVR90 |
| `se` | Sweden | 1 m | Markhöjdmodell (Lantmäteriet) | CC BY 4.0 | `© Lantmäteriet` | RH2000 |
| `fi` | Finland | 2 m | Korkeusmalli (NLS) | CC BY 4.0 | `© Maanmittauslaitos` | N2000 |
| `au` | Australia | 1–5 m | ELVIS (per-area order) | CC BY 4.0, licensor per survey | `Sourced from ELVIS – Elevation and Depth, © the contributing agency` | AHD |

| key | credential | how to obtain it | what `--input` takes | wizard |
| --- | --- | --- | --- | --- |
| `dk` | `OBC_REFERENCE_DK_TOKEN` | A free Dataforsyningen account, then `Min side` → `Token`. | GeoTIFF, EPSG:25832, float32 metres, −9999 for no data. | `wizard dk` |
| `se` | `OBC_REFERENCE_SE_USER` and `OBC_REFERENCE_SE_PASSWORD` | A free Geotorget account, then order `Markhöjdmodell Nedladdning, grid 1+` and create the consumer user. | GeoTIFF (COG) named `m<sheet>.tif`, 10 000 × 10 000 at 1 m, EPSG:5845, float32, −9999. | `wizard se` |
| `fi` | `OBC_REFERENCE_FI_TOKEN` | A free NLS account at `omatili.maanmittauslaitos.fi`, then `API keys`. | GeoTIFF, EPSG:3067, float32 metres, −9999, one 3 km square per file. | `wizard fi` |
| `au` | none; ELVIS answers no box at all | An ELVIS order and the link it sends by e-mail. | The order's zip, or its `*_DEM.tif` unpacked. ELVIS publishes each state in its own MGA zone, so the file's own CRS places it; an ASCII order must keep its `.prj`. | `wizard au` |

`ingest <key> --input` reads every `.tif`, `.tiff`, `.asc` and `.zip` under the directory. A zip is
opened into the work directory, not into the input directory, so a second run unpacks nothing. The
one thing the tool writes into the input directory is the `.prj` beside an ESRI ASCII grid that
came without one, because that is where the format keeps a CRS.

**Australia needs one check by hand.** Some ELVIS datasets are ellipsoidal, and an ellipsoidal
height stands tens of metres from an orthometric one, which is the size of a lift. The row is AHD;
an order whose metadata says otherwise must be converted before the ingest, and nothing here
converts one.

Two more are open data behind a login and are not rows, because neither answers a box at all:

| country | step | product | how to get it |
| --- | --- | --- | --- |
| Italy, Lombardy | 1 m | regional LiDAR | request by email, one to four weeks |
| Italy, Aosta Valley | 0.5 m | regional DTM | Italian identity login |

### The live probes

Every adapter is verified against one published summit. **Every number in the `got` column comes
from running the command for that source in the next section** and reading the maximum out of the
archive tile — not out of the service's own raster — so it is the number the baker would read
after max-pooling. The box is about 2 km on a side. `ch` is not in the table: it was verified when
its adapter landed.

| key | summit | expected | got | note |
| --- | --- | --- | --- | --- |
| `fr` | Puy de Sancy | 1885 m | 1880 m | The `HIGHRES` WMS layer resamples off IGN's Lambert-93 grid, so a sharp summit arrives a few metres low. At Le Brévent above Chamonix the same box reads 2507 m where the map says 2525 m. Low costs nothing: the bakery does not lift where the reference is below Copernicus. |
| `us` | Mount Elbert | 4401 m | 4401 m | |
| `no` | Galdhøpiggen | 2469 m | 2468 m | |
| `es` | Torre de Cerredo | 2650 m | 2647 m | MDT05 is a 5 m product and answers `int16`. |
| `nl` | Vaalserberg | 322 m | 323 m | Two of the four requests came back wholly void and a third is 22 % void: the box reaches into Belgium and Germany, where AHN stops. The run prints the void fraction per raster, so a nearly empty answer is visible. |
| `uk` | Scafell Pike | 978 m | 978 m | |
| `at` | Großglockner | 3798 m | 3798 m | One window out of one 7.7 GB square. |
| `ca` | Mont Royal, Montréal | 233 m | 235 m | EPSG:3979 stretches a box here by 9 %, so the split makes four requests of this 2 km box rather than one. |
| `it-bz` | Ortler / Ortles | 3905 m | 3896 m | A 2.5 m product on a glaciated summit. |
| `de-nw` | Langenberg | 843 m | 844 m | 7.9 % void: the summit is on the Hesse border and NRW's DGM stops there. |
| `de-he` | Wasserkuppe | 950 m | 954 m | |
| `de-bw` | Feldberg (Black Forest) | 1493 m | 1494 m | |
| `de-mv` | Helpter Berge | 179 m | 179 m | |
| `de-st` | Brocken | 1141 m | 1141 m | Verified, then the server answered HTTP 500 to the same command later the same day, including to the smallest request. Saxony-Anhalt's service is intermittent; the run refuses with the server's own body rather than writing a hole. |
| `de-by` | Zugspitze | 2962 m | 2962 m | All six squares the box needs are published, including the two that straddle the Austrian border. |
| `de-sn` | Fichtelberg | 1215 m | 1215 m | |
| `de-th` | Großer Beerberg | 983 m | 983 m | |
| `de-ni` | Wurmberg | 971 m | 972 m | The search answered two revisions of one square, 2013 and 2018. The archive keeps the maximum, as it does for swisstopo. |
| `it-tn` | Cima Tosa | 3136 m | 3135 m | The 3173 m the older maps carry included the summit ice cap; the province re-measured the rock at 3136 m with LiDAR, and this is the province's own LiDAR. 16 squares, none void. |
| `nz` | Aoraki / Mount Cook | 3724 m | 3718 m | One window out of sheet `BX15`, found by arithmetic and not by an index. The published height is the snow summit; a bare-earth DTM reads the rock a few metres below. |

`dk`, `se`, `fi` and `au` have no `got` column: this machine holds no credential for any of the
four portals, so no summit was measured. What each service answers **without** a credential is in
the next section, and each of the four is verified against a synthetic file in its own delivery
format in `tests/test_drop_in.py`.

### What the other candidates answered

Probed on the same day, with no key and no login. This is the record of what exists, so that the
next pass does not start from nothing.

| candidate | what it answered |
| --- | --- |
| **Tirol ImageServer** | Does not exist. `gis.tirol.gv.at/arcgis` has exactly one ImageServer, `HIK/HIK_MD`, and it is an 8-bit three-band image at 12 cm, not heights. The Tirol elevation services in `Basis` are MapServers, so they render relief. Tirol does publish a float32 0.5 m WCS, `Service_Public/terrain/MapServer/WCSServer`, coverage `Gelaendemodell_50cm_M28`, as multipart GML plus TIFF, and 0.5 m tiles under `gis.tirol.gv.at/geo/als/mosaik_50cm/`. Its vertical datum is not documented. |
| **Austria, per state** | Every state publishes its own model, in eight horizontal CRSs and three height systems, and Vienna's is on Wiener Null, 156.68 m from sea level. One national grid in one CRS is the better row, so `at` takes the BEV product and the state downloads stay documented alternatives for `--input`: Styria 0.5 m 1 km tiles, Carinthia's STAC, Salzburg 1 m tiles, Upper Austria per municipality, Burgenland behind a signed link, Lower Austria 10 m only. Vorarlberg has no float endpoint at all. |
| **Italy, Trentino WCS** | Answers, and lists no coverage. The geoportal reaches the data through an asynchronous merge service, which the `it-tn` row does not use: the same tiles are published as plain files. |
| **Italy, Piedmont** | No 1–2 m product. A 5 m DTM answers WCS 1.0.0 at `geomap.reteunitaria.piemonte.it`, coverage `DTM`, EPSG:32632, CC BY 4.0 — but its vertical datum is not documented anywhere, so it cannot be registered. |
| **Italy, Lombardy** | No. The 1 m LiDAR is an email request and takes one to four weeks. Only a 5 m picture service is open. |
| **Italy, Aosta Valley** | No. The bulk download is behind an Italian identity login, its WCS is empty and its WMS is image only. |
| **Italy, Tinitaly 10 m** | Answers WCS 2.0.1 nationwide, CC BY 4.0 with a required citation. The vertical datum is not documented, so it is not registered. It would be a gap-filler, not a source of contours. |
| **UK Environment Agency** | The ArcGIS ImageServer was withdrawn at the end of 2024 and answers `{"code": 400, "message": "Invalid URL"}`. The WCS replaced it, and the endpoint no longer carries the survey year, so the `uk` row is stable. |
| **LINZ** | Answers. `nz-elevation` on S3 is open and unsigned, and its listing names 424 published sheets under `new-zealand/new-zealand/dem_1m/2193/`. The licence names a different licensor per survey, which `capture-dates.geojson` carries. The LINZ Data Service WFS, which would answer the sheet outlines, needs a key — and the `nz` row does not need it, because the sheet grid is arithmetic. |
| **Germany, Brandenburg** | Answers float32 metres at `isk.geobasis-bb.de/ows/dgm_wcs`, coverage `bb_dgm`, and names no CRS at all. Stamped with the EPSG:25833 the subset was stated in, its heights near two published Brandenburg summits come out 70 m low, so what grid it answers in is not understood and it is not a row. |
| **Germany, Saarland** | Answers float32 **centimetres**, and its `ows:Fees` says any use beyond viewing "ist kostenpflichtig und bedarf einer vertraglichen Grundlage" while its catalogue entry says dl-de/by-2-0. The licence has to be settled with the LVGL before it can be a row. |
| **Germany, Rhineland-Palatinate, Schleswig-Holstein, Hamburg, Bremen, Berlin** | All open and all a download. Rhineland-Palatinate has a 42 320-tile Metalink index, Schleswig-Holstein and Hamburg publish ASCII XYZ, Bremen publishes two whole-state archives, and Berlin is inside Brandenburg's coverage. None is a row yet. |
| **Germany, national** | There is no national DGM service. BKG's 200 m grid is open and far too coarse for a crest. |

### What the credentialed portals answer without a credential

Each of the four rows above was probed with no key and no login, which is what can be verified
from a machine that holds none. Each answer is what shaped the adapter.

| portal | probe | answer |
| --- | --- | --- |
| **Denmark, Dataforsyningen** | `GET api.dataforsyningen.dk/dhm_wcs_DAF?service=WCS&request=GetCapabilities&token=` | **HTTP 200.** A WCS 1.0.0 capabilities document naming the coverages `dhm_terraen` and `dhm_overflade`, and the box 8.008, 54.435 → 15.598, 57.769, which is the row's extent. |
| **Denmark, Dataforsyningen** | the same endpoint, `request=DescribeCoverage&coverage=dhm_terraen` | **HTTP 403 — `User not authorized`.** So the capabilities are open and the data is not, and `token=` is the one parameter the adapter adds. |
| **Denmark, Datafordeler** | `GET wcs.datafordeler.dk/DHMNedboer/dhm_wcs/1.0.0/WCS?service=WCS&request=GetCapabilities` | **HTTP 401**, empty body. Datafordeler's own gateway wants a username and a password or an `apikey`. The row takes the Dataforsyningen gateway instead, because that is the one whose capabilities can be read and checked. |
| **Sweden, Lantmäteriet** | `GET api.lantmateriet.se/stac-hojd/v1/collections` and `…/v1/search?bbox=…&collections=dtm-cog` | **HTTP 200.** The STAC index is open. The search over Kebnekaise answered two items, each with a `data` asset at `dl1.lantmateriet.se/hojd/data/grid/mhm/75_6/m75x_6x.tif`, and the keyless `info` asset states the tile: GTiff, float32, nodata −9999, 10 000 × 10 000 at 1 m, EPSG:5845 ("SWEREF99 TM + RH2000 height"). |
| **Sweden, Lantmäteriet** | `HEAD dl1.lantmateriet.se/hojd/data/grid/mhm/75_6/m753_64.tif` | **HTTP 401** with `WWW-Authenticate: Basic realm="Authorization Server"`. So the index is open and only the download is signed, which is why `se` reads the index without a credential and sends HTTP Basic on the download. |
| **Finland, NLS** | `GET avoin-karttakuva.maanmittauslaitos.fi/ortokuvat-ja-korkeusmallit/wcs/v2?service=WCS&request=GetCapabilities` | **HTTP 401**, empty body. `avoin-paikkatieto.maanmittauslaitos.fi` answers 401 as well, on both the WCS path and `tiedostopalvelu/ogcproc/v1/`. Nothing about the service can be read without a key, so the coverage id `korkeusmalli_2m`, the `E`/`N` subsets and EPSG:3067 come from the agency's technical description and are **not** probe-verified. |
| **Australia, ELVIS** | `GET elevation.fsdf.org.au/` and any path under it | **HTTP 200**, and always the same 13 kB single-page application, including for `api/…` and for the bundle the page names. There is no endpoint that answers a box, so `au` has no adapter and never will have one. |

Nothing below `HTTP 200` was reached with a credential, so the four rows' request shapes are held
by their tests and by the agencies' documentation, not by a live answer.

### Ingesting a source

One box at a time, and the work directory is a cache: a second run of the same box downloads
nothing. These are the verification boxes the probe table was measured over.

```sh
cd host/obc-dem/reference
A=/tmp/reference/archive
W=/tmp/reference/fetch

python3 ingest.py ingest ch    --bbox 8.3800,46.7800,8.4200,46.8200        --archive $A --work $W/ch
python3 ingest.py ingest fr    --bbox 2.8019,45.5204,2.8259,45.5364        --archive $A --work $W/fr
python3 ingest.py ingest us    --bbox -106.4583,39.1091,-106.4323,39.1265  --archive $A --work $W/us
python3 ingest.py ingest no    --bbox 8.2925,61.6230,8.3325,61.6496        --archive $A --work $W/no
python3 ingest.py ingest es    --bbox -4.8666,43.1888,-4.8406,43.2062      --archive $A --work $W/es
python3 ingest.py ingest nl    --bbox 6.0079,50.7453,6.0339,50.7627        --archive $A --work $W/nl
python3 ingest.py ingest uk    --bbox -3.2227,54.4469,-3.2007,54.4615      --archive $A --work $W/uk
python3 ingest.py ingest at    --bbox 12.6829,47.0671,12.7049,47.0817      --archive $A --work $W/at
python3 ingest.py ingest ca    --bbox -73.5983,45.4968,-73.5763,45.5114    --archive $A --work $W/ca
python3 ingest.py ingest it-bz --bbox 10.5337,46.5016,10.5557,46.5162      --archive $A --work $W/it-bz
python3 ingest.py ingest de-nw --bbox 8.5462,51.2682,8.5722,51.2856        --archive $A --work $W/de-nw
python3 ingest.py ingest de-he --bbox 9.9287,50.4908,9.9507,50.5054        --archive $A --work $W/de-he
python3 ingest.py ingest de-bw --bbox 7.9934,47.8666,8.0154,47.8812        --archive $A --work $W/de-bw
python3 ingest.py ingest de-mv --bbox 13.5984,53.4794,13.6204,53.4941      --archive $A --work $W/de-mv
python3 ingest.py ingest de-st --bbox 10.6046,51.7918,10.6266,51.8064      --archive $A --work $W/de-st
python3 ingest.py ingest de-by --bbox 10.9743,47.4138,10.9963,47.4284      --archive $A --work $W/de-by
python3 ingest.py ingest de-sn --bbox 12.9432,50.4213,12.9652,50.4359      --archive $A --work $W/de-sn
python3 ingest.py ingest de-th --bbox 10.7351,50.6524,10.7571,50.6670      --archive $A --work $W/de-th
python3 ingest.py ingest de-ni --bbox 10.6084,51.7508,10.6304,51.7654      --archive $A --work $W/de-ni
python3 ingest.py ingest it-tn --bbox 10.8620,46.1510,10.8840,46.1656      --archive $A --work $W/it-tn
python3 ingest.py ingest nz    --bbox 170.1310,-43.6020,170.1530,-43.5880  --archive $A --work $W/nz

python3 ingest.py check --archive $A
```

The four sources behind an account take the same command with their credential set. These are
the boxes their tests name, and **none of them has been run**, because this machine holds no
credential for any of the four portals:

```sh
export OBC_REFERENCE_DK_TOKEN=…        # Møllehøj, the highest natural point in Denmark
python3 ingest.py ingest dk --bbox 9.8220,56.2948,9.8440,56.3094      --archive $A --work $W/dk

export OBC_REFERENCE_SE_USER=…         # Kebnekaise
export OBC_REFERENCE_SE_PASSWORD=…
python3 ingest.py ingest se --bbox 18.4830,67.8955,18.5050,67.9101    --archive $A --work $W/se

export OBC_REFERENCE_FI_TOKEN=…        # Halti, the highest point in Finland
python3 ingest.py ingest fi --bbox 21.2576,69.2995,21.2796,69.3141    --archive $A --work $W/fi

python3 ingest.py wizard au --bbox 150.3050,-33.4200,150.3270,-33.4054 --archive $A
```

A **country-scale** ingest is the same command with the country's box, and it is an owner-run job:
it moves hundreds of gigabytes and takes days per country. Run one source at a time, keep the work
directory on a disk with room for the source rasters, and `check` the archive before `publish`. The
tail holds one source raster plus the tiles it touches, so cost per box is flat for a product that
publishes per tile, per square kilometre or per window.

Five of the sources need a word about scale:

- **`at`** reads a window out of a 7.7 GB square. The box is split before any window is read, so
  one read is never larger than the pixel cap however large the box is, and a country-scale run
  costs one small request per sub-box rather than one square in memory. The date in its URL is the
  BEV delivery the row points at; a new delivery is a new date in `ingest/sources/at.py`.
- **`de-by`, `de-sn`, `de-th`** download whole grid squares. Bavaria alone is 71 979 squares, so a
  whole-state run wants the work directory on a big disk. Each state publishes an index with a
  SHA-256 per square, which is the way to check a bulk download that the tool does not do for you.
- **`nl`** is a 0.5 m product, so a box is four times the requests of a 1 m one, and `it-tn` and
  `dk` are finer still.
- **`nz`** reads a window out of one sheet of 864 M pixels, the same way `at` does, so the cost of
  a box is the box and not the sheet.
- **`se`** downloads a whole 10 km COG, a few hundred megabytes each, because that is what the
  STAC index names. A 2 km box needs one to four of them.

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

`mirror` pulls `index.json` first, then the tiles of the box and its one-tile halo, and nothing else. A
tile the archive does not hold is counted and named in the summary, not an error: a box that reaches
past the coverage is normal.
