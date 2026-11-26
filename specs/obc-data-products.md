# obc data products

The products `device` and `planner`: their steps, what each layer reads and writes, the route
sources, and the runtimes of the planner services.

- Writers: the steps of the `obc data` binary of `host/obc-data-steps`.
- Readers: the device, the web and iOS planners, the planner services.
- Code authority: `host/obc-data-steps`; `data/planner-runtime.toml` (the service runtimes).
- Related: [layers](obc-data-layers.md), [prepared sources](obc-data-prepared.md),
  [targets](obc-data-targets.md), [planner release](planner-release.md),
  [offline planner bundle](planner-offline.md), [TrailView tiles](trailview-tiles.md),
  [OBCC catalog](obcc-catalog.md).

## Products

A product is a set of steps that makes part of a release, such as `planner` or `device`. The
`obc data` binary (`host/obc-data-steps`, GPL-3.0-only) gives the list of products to the
commands of `host/obc-data`. The `obc-data-plumbing` binary of `host/obc-data` has the same
commands without products, for scripts that use R2. Each step name starts with `<product>/`. A
product can list a step of another product, such as the landmark content that the planner search
reads; both products declare it the same.

A product gives its steps for the settings of a target, the versions of [Versions](obc-data-layers.md#versions),
its regions and the store. When the step list depends on a snapshot that the store does not have,
such as the `.poly` of a region, the product names those fetches instead. The command fetches them
and then asks the product again, as long as each round names only new fetches: a fetched file can
name the next fetch. A fetch that fails, fails the command with its own code, `fetch_failed` or
`blocked`. A product that names a fetch it named before, that still names fetches after 8 rounds,
a step name without the name of a product, or a step that differs from the same step of another
product, fails the command with `failed` and a fix that points at the code of the product. A
product that has no steps for the settings, such as for a kind of region that it does not read, is
blocked. An error of the store, a file or the data of a fetch in a step list fails the command with
`failed`.

A product returns usable steps and blocked layers together. Each blocked layer has `step`,
`layer` and `reason`. An update with a blocked layer fails with `blocked` before it bakes. A
failed capture fetch blocks its content and artifacts; other map layers keep their steps. `obc
data` lists blocked layers with the reason. A read-only command fetches only small metadata: the
Geofabrik polygons, the Geofabrik index and the GLO-30 tile list. Another fetch that it needs
blocks the product with the fix `obc data update TARGET`.

A product also gives what clients read of a release: its pointer, which is one of the two pointer
keys with the client file of a step that a client reads there, and its named files, the client
files that a client finds by path. A product checks a release before a target serves it. The
device checks changed selections with the production assembler and reader. The planner verifies
stored grid and runtime artifacts.

Each step selects the files that clients read: the device, the web planner or a service on the
VPS. Other files are intermediate: only other layers read them. A target holds only selected
files (see [Targets](obc-data-targets.md#targets)). A change of the selection makes the layer stale with the cause
`input` `options`, and reuses the bytes of its key.

## `device`

The device maps have layers per leaf: a cell of size `2^23` µdeg of the OBCA grid that the
outline of the region touches. The outline of a `box` region is its box; the outline of a
`geofabrik` region is the union of its source `.poly` files from `geofabrik-poly`,
`area=<source path>`. Existing union definitions select the union of their leaves.

Both products resolve the same source areas. A Geofabrik definition names its areas directly.
For a Box, the cached index selects candidates by polygon coverage: of the areas whose polygons
meet the Box, each area that the others cover drops out, largest first. Full `.poly` files must then cover the
complete Box. A gap tries the parent extract before selecting bulk inputs.
An uncovered Box gives an actionable error. Country metadata does not remove required geography.

Each selected area has a private `<product>/source/<area>` step. It reads the extract and full
`.poly` at their separate exact versions, and writes `source.osm.pbf` and `source.poly`. A Box
also records its selected index version. These receipts retain active request provenance even
when a capture holds older extract or polygon bytes. Each product sorts and deduplicates areas.

A multi-area input uses prepared `osmium merge --with-history`, then `time-filter` at the latest
object timestamp present in the union. For an overlapping object, its highest supplied version
wins. Unique supplied objects remain. Conflicting payloads
at the same object type, id and version are malformed OSM inputs.

The merge, the map crop, the cut and the terrain of the planner run the Osmium of `OBC_OSMIUM`, or
else `osmium` on the `PATH`. Without it, an update blocks the device layers that need it, the plan
of the planner fails for more than one source area, a box or `planner/trailview-features`, and a
build of `planner/terrain` fails. Each asks to install Osmium or set `OBC_OSMIUM`. It does not
download or install a tool.

The steps read `copernicus-glo-30` and the national terrain models of `terrain`,
and the step list reads files such as a `.poly` and the GLO-30 tile list, each at its version (see
[Versions](obc-data-layers.md#versions)). `copernicus-glo-30` and its tile list are `manual`.

A leaf reads the archive tiles of its terrain cells: those of the windows that the crest rule of
`obct-terrain.md` §9 reads, which have a halo of two postings. A model whose `extent` meets one of
those tiles in the version of `terrain` that the target reads (see [Prepared sources](obc-data-prepared.md#prepared-sources)):
the step list reads its manifest, and the leaf reads the blocks `18/<bi:04>/<bj:04>.tif` of the
manifest that hold those tiles and its `index.json`. A leaf where the version has no block of its
tiles has no `device/reference` layer. Where a tile whose block the version lacks meets the
outline of the region and is in the `geofabrik-poly` polygon of one of the `areas` of a `dtm-*`
model, a note on `device/terrain` names the model and the command that adds
the region; the version that adds it is a new input of the leaves that read it.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `device/region-osm` | The selected PBF of each `device/source/<area>`; only for multiple areas | None | `osm.pbf`: the available-snapshot union |
| `device/osm` | The PBF of one `device/source/<area>`, or `device/region-osm` | `leaves`: `[i, j]` of each leaf | `osm/<i>-<j>.osm.pbf`: the `osmium extract --strategy smart --set-bounds --generator obc-data` of the square of the leaf and one µdeg around it, without the header options of the input, such as its replication fields. Each Osmium run writes at most two extracts: the rectangle around the leaves is halved along its longer side, and each half is cut from the extract of its parent, until each part is one leaf. Each leaf has the bytes of the direct extract. The metrics name the version (`osmium`) |
| `device/map/<i>-<j>` | `device/osm`, the file of the leaf; `land-polygons`; `device/terrain/<i>-<j>`, which the contours, the nav graph and the POIs read | `leaf`: `[23, i, j]`; `cells`: for each band of the recommended band table (`obca-assembly.md`), `[ci, cj]` of each cell of the band in the leaf that the outline touches; `partial_cells`: the ids of those cells that the source coverage does not hold | `cells/<band>/<ci>/<cj>.obcm` for each cell with content or with partial coverage; `cells/<band>/empty.json`: the ids of the other cells of the band. A cell has the bytes that one cut of the whole leaf with all bands writes, with `builder/presets/schema.json` and without landmarks or peaks. The step decodes the OSM of the leaf once |
| `device/reference/<i>-<j>` | `terrain`, without params: the blocks of those tiles in its version and `index.json`; only when the version has one of them | `source`: `terrain`; `tiles`: the ids `<ti:04>/<tj:04>` of the archive tiles that the terrain cells of the leaf read | `reference/`: the reference archive ([`obct-terrain.md` §10](obct-terrain.md#10-the-reference-archive)) of those tiles that the index names, each cut out of its block, and the `index.json` of the version cut to them, with `tiles` and the `sha256` of the pixels of each tile. A Python step with the group `terrain-reference` |
| `device/terrain/<i>-<j>` | `copernicus-glo-30`, `tile=` of each tile that the square of a cell reaches and that `copernicus-glo-30-tiles` names. A square without a tile is sea. A leaf without a tile reads no snapshot. `device/reference/<i>-<j>` when the leaf has one | `posting_log2` and `cell_log2` of OBCT v1; `cells`: `[ci, cj]` of each terrain cell in the leaf that the outline touches | `terrain/<ci>/<cj>.obcd` for each cell with a height (`obcc-catalog.md` §13), the bytes that `obc-bake terrain --reference` writes from the same tiles and archive; `metadata/empty.json`: the ids of the cells without a height; `metadata/credits.json`, when a cell reads a national model: `key`, `product`, `attribution` and `licence` of each model that a cell reads, as the reference archive states them |
| `device/landmark-content[/<area>]`, `device/peak-content[/<area>]` | `wiki`, without params: `common.tar.zst` and the shards of the `2^20` µdeg cells that the `.poly` of the area touches, of its manifest; regional `geofabrik-extracts` and `geofabrik-poly` | None | `landmarks/content.json` or `peaks/peaks.json` and device photos. The metrics count the facts that the version lacks, by kind (`absent`), and the run log names them. No client reads the layer: the device cell layers publish the final assets |
| `device/landmarks/<i>-<j>`, `device/peaks/<i>-<j>` | Every area's landmark content and `device/osm`, the file of the leaf; or every area's peak content | `cell_log2`: 18; `cells`: `[ci, cj]` of each network cell of the leaf: the `network` list of `cells` of `device/map/<i>-<j>` | `landmarks/<ci>/<cj>.bin` or `peaks/<ci>/<cj>.bin` for each cell that owns content (`obcc-catalog.md` §14.3). A landmark joins the OSM objects of the leaf that name it |

`<i>`, `<j>`, `<ci>` and `<cj>` have four digits or more, as in a cell id.

The content of a collection reads the shards of [`wiki`](wikimedia-content.md#the-wiki-source)
at the version that the target reads, and the extract and the `.poly` of its area. The step makes
the boundary and the candidates or the summits again from them, offline. A fact that the version
lacks is no content, as an OSM object newer than the version has none. Without a version of
`wiki`, the content layers block with "Fix: Run `obc data source prepare wiki`.", and the layers
that read them block too. A version whose manifest lists no shard of the area blocks them as well.

A single-area definition keeps the unsuffixed content layer names. Other definitions have one
content layer per collection and source area. Article inputs keep separate file views;
canonical content merge rules deduplicate overlaps without losing unique articles.

`device/source/<area>`, `device/region-osm`, `device/osm`, `device/reference/<i>-<j>` and all
content layers are private intermediates. Their complete records stay in release provenance.

## Device-map publication

The `device/catalog` step consumes selected layer paths and their receipt metadata. Map and terrain
layers list their empty cells beside the cells: a map layer in `cells/<band>/empty.json`, a terrain
layer in `metadata/`, with the decoded reference credits. The `cells/` and `terrain/` payloads
and `metadata/` are client files: a target holds the metadata that the catalog and the map cells
read, so another machine bakes them without the leaf. Every file of the catalog layer is a client
file: the pinned satellites in `objects/`, which it names `/objects/<sha256>`, and the root `catalog.json`
with the schema, terrain, licence and region files, which are the named files of the device. The
option `credits` holds the OSM source entry and the credits of GLO-30 and of the landmark
articles, from `data/sources.toml`, so that a credit edit changes the key.

The catalog selects only Geofabrik picks whose complete band and terrain coverage is available.
A saved Box or multi-area definition has its own complete catalog pick. Source polygons determine
which edge cells are partial. Missing required map, terrain or article layers block the catalog;
a terrain-only release cannot publish.
Before a target serves a release, the product compares picks with the serving release and
assembles changed picks with the real assembler. The production reader verifies each result.
Missing inputs and invalid artifacts fail verification before the target changes. An unchanged
pick can reuse prior verification.

## `planner`

The planner reads the `countries` of the region, or else the countries of its source areas in
the Geofabrik index, sorted; a region without countries is blocked. It reads the `time_zone` of the
region, or else `UTC`. It uses the shared source selection above. Its bounds enclose the
requested Box or union of selected outlines. Each snapshot, such as `copernicus-glo-30`, the extract, the `.poly` and the GLO-30 tile list, is
at its version (see [Versions](obc-data-layers.md#versions)), as for `device`. The other options come from
[`data/planner.toml`](obc-data-registry.md#dataplannertoml). A GLO-30 input reads the tile
of each 1° square that its box touches and that `copernicus-glo-30-tiles` names; a box at sea
reads no snapshot. `planner/terrain` reads the mean of the national terrain models as `device`
reads their maximum: the blocks `mean/18/<bi:04>/<bj:04>.tif` that hold the archive tiles of its
`bounds` in the version of `terrain` that the target reads, and `index.json`.

| Layer | Reads | Options | Files |
| --- | --- | --- | --- |
| `planner/osm` | The selected PBF of each `planner/source/<area>` | `path`: `osm.pbf` for one area; none for multiple areas | `osm.pbf`: one extract as it is, or the available-snapshot union |
| `planner/osm/bounds` | `planner/osm` | For a region with a box, `bbox`: west, south, east and north of the bounds of the region, widened by `osm.margin_m`, within ±180° and ±90°. For a region without a box, `path`: `osm.pbf` | `osm.pbf`: for a region with a box, the `osmium extract --strategy smart -S types=multipolygon,boundary --set-bounds --generator obc-data` of `bbox`, in which each multipolygon and each boundary that `bbox` touches is complete, and the metrics name the version (`osmium`). For a region without a box, `planner/osm` as it is: its extracts are its whole area |
| `planner/basemap` | `planner/osm/bounds`; The selected jar of `protomaps-basemaps`; `natural-earth`, `water-polygons`, `land-polygons`, `daylight-landcover`, `qrank`, `pgf-encoding` | `bounds` of the region; `attribution` of `osm-planet`, `natural-earth` and `daylight-landcover` | `basemap.pmtiles`: the Protomaps map at zooms 0 to 14 |
| `planner/reference` | `terrain`, without params: the mean blocks of the tiles in `bounds` of `planner/terrain` and in the ring of tiles around them, and `index.json`; only when the version has one of them | `source` and `tiles` as for `device/reference`, of the tiles of `bounds`; `pool`: `mean` | `reference/`: the mean archive of those tiles. A Python step with the group `terrain-reference` |
| `planner/terrain-osm/<area>` | `terrain-osm`, `area=<area>`, for each source area | `path`: `source.osm.pbf` | `source.osm.pbf`: the extract as it is |
| `planner/terrain-osm` | Only for more than one source area: `source.osm.pbf` of each `planner/terrain-osm/<area>` | None | `osm.pbf`: their union, as `planner/osm` merges |
| `planner/terrain` | The GLO-30 tiles of `bounds`; `planner/reference` when there is one; `planner/terrain-osm`, or for one source area its `planner/terrain-osm/<area>` | `bounds`: west, south, east and north of the zoom 10 tiles that the bounds of the region touch and of their neighbours; `attribution`: the credit of GLO-30; `osm_attribution`: the credit of `osm-planet` | `terrain.mbtiles`: the terrain tiles of [the planner release](planner-release.md#terrain-tiles), the bytes that `planner-dem` writes from the same tiles, archive and OSM. `terrain-osm` is the Geofabrik extract with `refresh = "manual"`: the terrain keeps the extract of each area that it read first, and a newer `geofabrik-extracts` does not build the terrain again. `obc data fetch terrain-osm area=<area>` puts a newer extract in the store, which the next update reads. Where GLO-30 has weight, its heights follow the OSM within 300 m of `bounds` (`host/obc-dem/src/shape.rs`): the water side of `natural=coastline` is at 0 m up to 45 m from the line; within 45 m of the top of GLO-30 within 30 m of a `natural=peak` or `natural=volcano` with a numeric `ele`, the cone of its flanks continued from 45 m and 90 m, only rising and never above `ele`. With OSM, the metrics name the Osmium version (`osmium`) |
| `planner/routing-source` | `planner/osm/bounds` | `bounds` | `source/`: the ways with their node references, tags and facts, the relations, the present node members, the turn rules of the relations, the closure sets, `source.json` (the counts of the import, the SHA-256 of the PBF from `layer_files`) and `layout.json`. The columns are stored columns of `planner/router-build/src/column.rs`: little-endian values of one type each, which `layout.json` names with their count. The import applies the country access defaults |
| `planner/routing-roads` | `planner/routing-source`, the GLO-30 tiles of the bounds of the region, and `planner/reference` when there is one. The heights of the roads are those of `planner/terrain` without the OSM corrections | `bounds`; `terrain_attribution`: the credit of GLO-30; `cell_sizes`: `routing.cell_sizes` of `data/planner.toml`, the largest number of junctions in a cell of level 1, 2, …: 1 to 8 increasing values, the first from 2 to 2^24 − 1 | `roads/`: the junctions and the roads in cell order, the roads with their heights, the cells (`cells.*`), the forbidden turns, the closure sets, the road topology, `roads.json` (the warnings of `package.json`, the identities of the PBF and the terrain) and `layout.json` |
| `planner/routing-costs` | `planner/routing-source`, `planner/routing-roads` | `bounds`, `profiles` | `costs/`: the cost dictionary and, for each profile `<i>` in order, `<i>.bases`, `<i>.totals`, `<i>.turns` and `<i>.snap`; for each level `<l>`, the cost tables of its cells, `tables.<l>.bytes` and `tables.<l>.at`; `costs.json` (the profile IDs) and `layout.json` |
| `planner/route-sources` | The snapshot of each route source of `data/routes/` whose `countries` share a code with the `countries` of the region and whose `extent` meets the bounds, in the order of `data/sources.toml`. Only with the part `route-sources`; a region with no such source has no step | `bounds`; `sources` (see [Route sources](#route-sources)) | `route-sources.json` of [Route sources](#route-sources) |
| `planner/routing` | `planner/routing-source`, `planner/routing-roads`, `planner/routing-costs`, and `planner/route-sources` when the list has it | `region` (the last part of the region id) and `bounds`; `attribution`: the credit of OSM; `cell_sizes`: as for `planner/routing-roads`; `download_level`: `routing.download_level` of `data/planner.toml`, 1 to 8. The step list checks the profiles, `cell_sizes` and `download_level` when it is built | `routing/`: `overlays.sqlite` and `route-catalog.json`; `cells/`: the objects, `package.json` and `directory.bin` of [the route package contract](route-package.md); `routes/<cell>.json`: the records of `route-catalog.json` that name the cell, with a final newline; `routes/trails/<cell>.json` and `routes/long/<id>.json`: the far files and the trail record files of [the route catalog](route-catalog.md#trail-files) |
| `planner/route-facts` | `planner/osm/bounds`, `planner/routing-source`, `planner/routing`, the GLO-30 tiles of the bounds of the region, `trailview-canopy.pmtiles` of `planner/trailview-canopy` and `snow-seasons.pmtiles` of `planner/snow` when their steps are in the list | `bounds`; `terrain_attribution`: the credit of GLO-30 | `facts/<cell>.json`: the [facts file](route-catalog.md#facts-files) of each grid cell, with a final newline |
| `planner/overlays` | `planner/routing` | `attribution` of `osm-planet` | `overlays.pmtiles`: the route networks and the access of `routing/overlays.sqlite`, as [the planner release](planner-release.md) defines it |
| `planner/assets` | `protomaps-assets`, `tangrams-icons` | None | `assets/fonts/` and `assets/sprites/` of the assets archive; `assets/sprites/LICENSE.txt`: the MIT notice of `tangrams-icons` |
| `planner/model` | `query-model` | None | `model/`: `model.int8.onnx`, `tokenizer.json` and `tokenizer_config.json` of the archive, and `labels.json`, the labels of the query schema |
| `planner/search/policy` | `nominatim-country-data` | None | `policy.json`: the country names, postcode formats and address levels of the archive; `country_osm_grid.sql.gz`: the country grid of the archive, as it is |
| `planner/search/dump` | `planner/osm/bounds`, `planner/search/policy` | `country`: the first of `countries` in lowercase, the country of a place outside each country polygon | `search.jsonl.zst`: the search records that `obc-search-bake` writes, one JSON object per line, with the keys of each object in byte order |
| `planner/search/records` | `planner/search/dump` | None | `pois.jsonl.zst` and `addresses.jsonl.zst`: the records of the dump with a POI and with an address |
| `planner/search/pois`, `planner/search/addresses` | `planner/search/records`; POIs also read the selected `device/landmark-content*` and `device/peak-content*` outputs | `component` (`pois` or `addresses`); `region`, `bounds` and `countries` as for `planner/routing`; `time_zone` of the region; `attribution` of `osm-planet` | `<component>/<region>.sqlite`: the search database of the component, schema 6. The key holds no SQLite version: another Python patch release can give other database bytes. The metrics name the version as `sqlite` |
| `planner/places` | `planner/search/pois` | None | `places.pmtiles`: the rider places of the POI database, at zoom 11 |
| `planner/climate` | The files of [`climate`](obc-data-producers.md#climate) of the blocks of the outline of the region, as `climate` defines the blocks of a region: the orography and the ten years from `climate.first_year`. A version that lacks one of them refuses | `bounds`, `first_year`, and `attribution` of `era5-land`, whose `{year}` is the year after the ten years, `first_year` + 10; `climate`: the version that it reads | `climate.pmtiles`: [the climate archive](planner-climate-tiles.md). A cell of a block that the step does not read is missing |
| `planner/snow` | The files of [`snow`](obc-data-producers.md#snow) of the blocks of the outline of the region, as `snow` defines the blocks of a region, for each season of `snow.seasons`. A version that lacks one of them refuses | `bounds`, `seasons`; `attribution`: the credits of `hr-wsi` when its `extent` meets the bounds, `modis-snow` and `hansen-gfc`; `year`: the year of the `snow` version, which fills the `{year}` of `hr-wsi` | `snow.pmtiles`, `snow-levels.pmtiles` and `snow-seasons.pmtiles`: [the snow archives](planner-snow-tiles.md). A tile of zoom 6 or more of `snow` and `snow-levels`, and of zoom 8 or more of `snow-seasons`, that lies inside the bounds has the same bytes in each region whose step reads its block |
| `planner/trailview-features` | `planner/osm`, `planner/terrain/grid`, `water-polygons` | `bounds`; `kinds`: the kind table of `data/trailview/kinds.json`; `roads`: the name, the grade, the smoothing length, the alignment fields and the sidewalk fields (`sidewalk_m`, `sidewalk_margin_m`) of each class of `data/trailview/roads.json`, as an array in its order; `waterways`: `width_m`, `cover_m`, `cover_lift_max_m` and `culvert_max_m` of each `waterway` value of `data/trailview/waterways.json`; `attribution` of `osm-planet` | `trailview-features.pmtiles`: [the feature archive](trailview-features.md) at zooms 8 to 13 |
| `planner/trailview-stamps` | `planner/trailview-features` | `bounds`; `attribution` of `osm-planet`; `stamp_sources`: the digest of the files that the stamp code can read, so a change of them bakes the step again | `trailview-stamps.pmtiles`: [the stamp archive](trailview-tiles.md#stamps), made by the TrailView stamp code in Node |
| `planner/trailview-rules` | `data/trailview/` of the repository | `ruleset`: the digest of its JSON files by their paths in it. The step fails when the files differ | `rules.json`: [the TrailView rules](trailview-tiles.md#rules); `regions.json`, `materials.json` and `ground.json`: the region ids, the region areas, the ecoregions, the places, the vocabulary and the ground sources that the ground steps read |
| `planner/trailview-material` | `materials.json` and `ground.json` of `planner/trailview-rules`; `lithology.pmtiles` of `planner/trailview-lithology`; each [material source](trailview-tiles.md#ground-sources) that meets the bounds: `osm.pbf` of `planner/osm` for `osm-planet`; `esa-worldcover`, `tile=` of each 3° tile that the bounds touch and that `esa-worldcover-grid` lists; the files of [`colour`](obc-data-producers.md#colour) of `trailview.colour_year` of the blocks that the outline of the region meets, for `colour`. A version that lacks one of them refuses; a `capture` source with `bbox=<bounds>`, another without params | `layer`: `material`; `bounds`; `year`: `trailview.colour_year` | `material.pmtiles`: the material bytes and the barren shares of [the ground](trailview-tiles.md#ground) |
| `planner/trailview-lithology` | `materials.json` and `ground.json` of `planner/trailview-rules`; each lithology source that meets the bounds, as for the material | `layer`: `lithology`; `bounds` | `lithology.pmtiles`: the lithology bytes |
| `planner/trailview-region` | `regions.json` of `planner/trailview-rules` | `layer`: `region`; `bounds` | `region.pmtiles`: the region bytes |
| `planner/trailview-ground` | `planner/trailview-material`, `planner/trailview-lithology`, `planner/trailview-region` | `layer`: `ground`; `bounds`; `attribution` of the sources of the three | `ground.pmtiles`: [the ground archive](trailview-tiles.md#ground) |
| `planner/trailview-colour` | The files of `colour`, as for the material | `bounds`, `year`, `variant` (`trailview.colour`); `attribution` of `sentinel-2-mosaics` with the year | `q1.pmtiles` to `q4.pmtiles`: [the colour archives](trailview-tiles.md#colour) |
| `planner/trailview-canopy` | The blocks of [`forest`](obc-data-producers.md#forest) that the outline of the region meets, as `forest` defines them. A version that lacks one of those blocks refuses | `bounds`; `attribution`: the credits of the canopy models with a tile in those blocks and of the leaf type | `trailview-canopy.pmtiles`: [the canopy archive](trailview-tiles.md#canopy) |

Each map producer has a `planner/<kind>/grid` step, where `<kind>` is `basemap`, `places`,
`terrain`, `overlays`, `climate`, `trailview-features`, `trailview-stamps`,
`trailview-ground` or `trailview-canopy`. It reads only the archive of its producer.
`planner/trailview-colour/<archive>/grid` reads one archive of the colour step, as the kind
`trailview-colour-<archive>`. `planner/snow/<kind>/grid` reads one archive of the snow step, as the
kind `snow`, `snow-levels` or `snow-seasons`. It converts terrain MBTiles with the pinned Python
PMTiles library. It groups the tiles as
[grid publication](planner-offline.md#grid-publication-and-selection) says: each tile goes into
a map pack of the square of its base level, split where a pack is larger than 15 MiB, and zoom 13
terrain has archives of its own ([terrain tiles](planner-release.md#terrain-tiles)). Each
archive is packed as `maps/tiles/<kind>/<z>-<x>-<y>.pmtiles`, and its TileJSON
as `maps/<kind>.json`, with the transport rules of [the offline contract](planner-offline.md).
The step writes the packed bytes in `objects/<transport sha256>`, and selects `objects` and
`index.json` for clients. Its `index.json` has `format: 1`, `kind`, `files`,
`source` and `metadata`. `files` maps logical names to source and transport hashes, sizes and
encoding. `source` names the converted archive bytes, or the raw MBTiles when it has no tiles. `metadata`
is the TileJSON. Empty terrain has TileJSON and no tile archives.

The parts of the planner are `map`, `routing`, `route-facts`, `route-sources`, `search`, `climate`, `snow`,
`trailview-features`, `trailview-rules`, `trailview-ground`, `trailview-colour` and
`trailview-canopy`; the device maps are the part `device`. A planner step is in the list when a
part that is on makes it or reads it: `map` makes the basemap, places, assets and terrain grids,
`routing` the routing, routing cells and overlays grids, `search` the model and the two search grids; the other
parts make their own steps. The fonts, the offline grid, the index and the catalog read only the grids that stay.
`trailview-features` makes the stamps step too.
`trailview-ground` makes the material, lithology and region steps, and needs `trailview-rules`.
`route-facts` needs `routing`. `route-sources` makes the step `planner/route-sources`, which
adds the routes of the second sources to the catalog, and needs `routing`.
All producers are intermediate layers. Grid steps select their `objects/` and their
`index.json`.

| Layer | Reads | Files |
| --- | --- | --- |
| `planner/routing/grid` | `planner/routing` | Packed `routes/tiles/<cell>.json`, `routes/tiles/trails/<cell>.json` and `routes/tiles/long/<id>.json` |
| `planner/routing/cells/grid` | `planner/routing` | `cells/` of `planner/routing`: `routing/package.json`, `routing/directory.bin` and `routing/objects/<sha256>` of [the route package contract](route-package.md), each with the transport `identity`. Its index has the kind `routing-cells` and `metadata`: `region`, `bounds`, `source_sha256` and the sorted profile names of `package.json` |
| `planner/route-facts/grid` | `planner/route-facts` | Packed `routes/tiles/facts/<cell>.json` |
| `planner/search/pois/grid`, `planner/search/addresses/grid` | The database of its search component | `search/tiles/<component>/<cell>.sqlite`, packed. A cell retains intersecting places, addresses, their streets and contexts. The index records regional metadata and clipped cell bounds |
| `planner/assets/grid`, `planner/model/grid`, `planner/trailview-rules/grid` | `planner/assets`, `planner/model` or `rules.json` of `planner/trailview-rules` | The input files, packed as `maps/assets/`, `search/model/` or `trailview/`. The index maps logical paths to file and transport identities |
| `planner/fonts/grid` | `planner/basemap/grid`, `planner/places/grid`, `planner/overlays/grid` and `planner/assets/grid`: client files only. The route references are the `ref` of the overlay tiles | Packed `offline/fonts/<stack>.pbf`: the joined used glyph ranges of [the offline contract](planner-offline.md). The index maps each original glyph range to its joined stack |
| `planner/offline/grid` | Only `index.json` of the grid steps of the parts `map`, `routing` and `search` and of `planner/fonts/grid`, so an optional layer does not change it | Packed `offline/catalog.json` and one `offline/cells/<cell>.json` for each cell of [the offline contract](planner-offline.md#grid-publication-and-selection) |
| `planner/index` | Only `index.json` of each grid step | Packed `search/<region>.grid.json` in `objects/`; `release.json`, which names the index of each grid step, and the small tile pointers in `public/` |
| `planner/catalog` | `planner/index`, and `index.json` of each grid step | `catalog.json`: the pointer that a planner client reads first. Its `active.id` is the planner release id: the digest of the named files of the planner. `origins` is the `[publication]` table of `data/planner-runtime.toml` |

The overlay grid reads only `index.json` of the routing cells grid too. It fails when
`routing_package` of its archive is not the SHA-256 of `routing/directory.bin` in that index, and
keeps the value in its TileJSON. The final index
checks the OSM source, region and coverage of `metadata` of the routing cells index, the search
schema and time zone, and the terrain inputs. It reads no payload to compose metadata. A climate change builds
only its producer, its grid, the final index and the pointer. The named files of the planner are
`release.json` and `public/` of `planner/index`, and the `index.json` of each grid step at
`indexes/<step>/index.json`. The engine release manifest holds source versions
and preparation provenance; the client manifest holds the [planner release](planner-release.md)
data. It contains no deployment probe, device catalogue snapshot or source mirror.

A Python step runs `env PYTHONHASHSEED=0 uv run --locked --offline --no-default-groups
--no-python-downloads --group <group> python <entry> --step` in the repository root.
`uv sync --all-groups` prepares the packages on a machine. A credit that it writes comes in its
options.

### Route sources

`python -m tools.planner_route_sources --step` (group `route-sources`) reads the files of each route source by
its [description](obc-data-registry.md#dataroutesidtoml). Its options are `bounds` and `sources`: a list of `{id,
credit, description}`, where `credit` is the `attribution` of the row, with `{year}` the year of
the version that the step reads, and `description` the text of its file in `data/routes/`. It reads the snapshot of each source, in the order of `sources`, and
writes `route-sources.json`: UTF-8 JSON, the keys of each object in byte order, written whole.

```json
{"format":1,"sources":[{"credit":"Bundesamt für Strassen, Kanton, Stiftung SchweizMobil","id":"schweizmobil-hiking","origin":1,
 "routes":[
  {"key":3,"kind":"hiking","name":"Alpenpanorama-Weg","rank":3,"ref":"3","stages":[
    {"members":[],"name":"Alpenpanorama-Weg, Rorschach – Trogen","stage":1},
    {"ascent_m":444,"members":[[8309770,47051860,8309910,47051950]],"name":"Alpenpanorama-Weg, Luzern – Malters","stage":11}]},
  {"ascent_m":620,"key":561,"kind":"hiking","members":[[8461200,47052310,8461350,47052400]],"name":"Rigi Panoramaweg","rank":1,"ref":"561"}]}]}
```

- `sources`: one entry for each source of the options, in their order; `id` and `credit` from the
  options, `origin` from the description.
- A route: `key`; `kind`; `rank` (0 when the description maps none); `ref`, `name`,
  `description`, `operator`, `website`: the filled template, with outer spaces removed, absent
  when empty; `ascent_m`: an integer, absent when the description maps no field or the value is
  empty.
- `members`: each line of each feature of the route, in the order of the feature ids and then of
  the parts of a multi-line or a collection; a point or a polygon is no member: a flat list of integer microdegrees, longitude then latitude, in WGS
  84, each line simplified within 1 m (Douglas–Peucker, so its ends stay) and with repeated points
  removed. A line with fewer than two points is left out.
- A route with two or more stages has `stages` and no `members`: `stage` (the number), `name`,
  the other filled templates, `ascent_m`, `members`. A route with one stage or none takes its
  `members` from the route layer.
- A route is in the file when a point of one of its members is inside `bounds` (edges included).
  A route with stages is in the file when a point of one of its stages is inside; it then lists
  all its stages, and a stage with no point inside has `"members": []`. A stage row whose route has
  no row is not read.
- The routes are in the order of `key`, the stages in the order of `stage`.
- The program fails, with the source and the feature, when: a layer or a field of the description
  is not in the file; a description without `layer` reads a file of more layers; two routes of one source have one key from two texts; a stage number is not
  an integer from 1 to 99, or comes twice in one route.

## Service runtimes

`data/planner-runtime.toml` names the Linux target triple and the `major.minor` version of
Node. Its `[publication]` table names HTTPS `site_origin`, `api_origin`, `objects_origin` and
`tiles_origin`. The services run on the Python of `.python-version`. The shared pool is
`<objects_origin>/objects`. Tile URLs
are under `<tiles_origin>/releases/<id>/`: the tile Worker reads `releases/<id>/public/` and
`objects/` of the bucket.

Each service prints the formats of the layers that it reads with `--formats`, as one JSON object
of layer and format, and reads no data: `planner-service --formats` gives `planner/routing` 10, `node planner/search/server.mjs --formats` gives `planner/search` and
`planner/index`, and `python3 -m tools.planner_downloads --formats` gives `planner/index`. The format of
`planner/index` is that of the two files of it that the services check: the offline catalog and
`search/<region>.grid.json`.

The pointer names routing, search and downloads under
`<api_origin>/planner-api/releases/<id>/SERVICE`, and the release document at
`<objects_origin>/releases/<id>/release.json`. Its `device_catalog` names
`<objects_origin>/cell-catalog/catalog.json`. [`obc data serve`](obc-data-serve.md#serve) runs the services for a
`dir` target. [`obc data deploy`](obc-data-serve.md#deploy) puts the follower of `prod` on its server and the tile
Worker on Cloudflare.

The [download service](planner-offline.md#download-service) returns a ready quote with an
absolute pinned `source` URL. The client reads the bundle, release and objects through that
source. The job ID hashes the URL of the object pool as well as the catalog and the bounds. A
quote stays valid while its bundle exists; a missing bundle requires a new quote. Job creation
completes synchronously; there is no polling path.

A grid cell is a zoom 9 Web Mercator tile that the bounds of the region overlap, clipped to the
bounds, with the id `9-<x>-<y>`. The JSON objects that `planner/routing` writes have their keys in
byte order.
