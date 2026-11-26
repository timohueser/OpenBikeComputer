# Specs

Each file here is a contract: what the bytes, the JSON or the API must be. When the code and a
spec differ, the code is wrong. A spec holds no reasons and no history: a guide page in
`docs/content/` says why and links the spec with `[text](src:specs/<file>)`.
[`vectors/`](vectors/README.md) pins the bytes that more than one codec reads.

The header of a spec names its writers, its readers and, for most, the code that holds its
constants (`Code authority`). Start there to find the code. For the device formats that code is
`firmware/obc-formats`: a copy of the constants with no reader, so that a producer and a
consumer cannot disagree about a byte.

## Reference

| Spec | Defines |
| --- | --- |
| **Device formats** | |
| [obcm-map.md](obcm-map.md) | OBCM: the map file the device renders and routes on |
| [obca-assembly.md](obca-assembly.md) | OBCA: the cell grid, and how baked cells assemble into one map |
| [obct-terrain.md](obct-terrain.md) | OBCT: the terrain raster and its surface index |
| [obcr-route.md](obcr-route.md) | OBCR: a route with elevation, surface and waypoints |
| [obcu-update.md](obcu-update.md) | OBCU: the signed firmware update, the boot-state page and the stage carve |
| [ride-archive-metadata.md](ride-archive-metadata.md) | The device's proof that a client holds a finished ride |
| **Device storage and link** | |
| [flat-store-format.md](flat-store-format.md) | The on-card layout of the flat store |
| [flat-store-protocol.md](flat-store-protocol.md) | The store seam, wire protocol v4 and its BLE and USB bindings |
| [ble-interface.md](ble-interface.md) | The GATT services, characteristics, pairing and the pairing QR link |
| [ride-archive-receipts.md](ride-archive-receipts.md) | The iOS archive boundary and the receipts that carry proof to the device |
| **Map catalog and data pipeline** | |
| [obcc-catalog.md](obcc-catalog.md) | OBCC: the JSON catalog of baked cells |
| [obc-data-registry.md](obc-data-registry.md) | The `data/` files of `obc data`, the settings of a target and the state of a source |
| [obc-data-store.md](obc-data-store.md) | The store of a machine, its collection and the fetchers |
| [obc-data-archive.md](obc-data-archive.md) | The archive of whole datasets |
| [obc-data-layers.md](obc-data-layers.md) | Layers: keys, the step contract, receipts, plans, runs, versions and state |
| [obc-data-products.md](obc-data-products.md) | The products `device` and `planner`, the route sources and the service runtimes |
| [obc-data-targets.md](obc-data-targets.md) | Targets and releases: update, rollback and collection |
| [obc-data-serve.md](obc-data-serve.md) | `obc data serve` and `obc data deploy` |
| [obc-data-prepared.md](obc-data-prepared.md) | The prepared sources and the producer `terrain` |
| [obc-data-producers.md](obc-data-producers.md) | The producers `forest`, `climate`, `colour`, `snow` and `wiki` |
| [obc-data-cli.md](obc-data-cli.md) | The `obc data` commands, the overview, the operator page, the R2 client, the errors and the JSON output |
| [obc-data.schema.json](obc-data.schema.json) | The JSON Schema of the `--json` output of `obc data`, generated |
| [wikimedia-content.md](wikimedia-content.md) | The prepared `wiki` source of landmark and peak content |
| **Planner** | |
| [planner-release.md](planner-release.md) | A planner release: `release.json`, grid storage, tile paths and the catalogue |
| [planner-offline.md](planner-offline.md) | The offline bundle of one release |
| [planner-plan.md](planner-plan.md) | The `.obcplan` file of an editable plan |
| [library-backup.md](library-backup.md) | The backup file of the library: routes, rides, ride files, photos, state and settings |
| [home-views.md](home-views.md) | The home views: their keys in the bucket, the manifest of a set, and the choice of an image |
| [route-pictures.md](route-pictures.md) | The covers of the signed routes: their keys in the bucket, the manifest and the cell files of a set |
| [planner-climate-tiles.md](planner-climate-tiles.md) | `climate.pmtiles`: rain, temperature and wind per week |
| [planner-snow-tiles.md](planner-snow-tiles.md) | `snow.pmtiles`, `snow-levels.pmtiles`, `snow-seasons.pmtiles`: the snow runs of the bands and the snow onset and melt-out per season |
| [route-package.md](route-package.md) | The regional routing package |
| [route-api.md](route-api.md) | The route service HTTP API |
| [route-catalog.md](route-catalog.md) | The signed-route catalog of a region |
| [trailview-tiles.md](trailview-tiles.md) | The TrailView ground, colour, snow-day and canopy rasters |
| [trailview-features.md](trailview-features.md) | The TrailView feature tiles |
| [trailview-models.md](trailview-models.md) | The TrailView model files and packs |
| **Data files** | |
| [content-languages.json](content-languages.json) | The content languages and their Wikidata IDs |
| [photo-pools.json](photo-pools.json) | The photo source properties and fallback bounds |

## Writing a spec

- File name: lowercase kebab case, `<subject>.md`, or `<format>-<subject>.md` for an OBC binary
  format. No `spec` suffix.
- Header: an H1 that names the artifact (no version), one to three sentences of scope, then the
  bullets that apply: `Version`, `Writers`, `Readers`, `Code authority`, `Test vectors`, `Related`.
  [obcr-route.md](obcr-route.md) is the model.
- An optional unnumbered `## Conventions` section holds the rules for the whole file.
- Headings and `§` numbers are stable: code, configs and other docs cite them. Do not renumber.
- No rationale beyond one sentence, no history, no dates. Link a rule that another spec owns.

## Changing the bytes

Change the spec first, then the code, then the vectors. A format may break: nothing is deployed
to riders, so no migration and no old reader is kept. The one exception is the settings blob in
RRAM (`firmware/obc-app/src/settings.rs`; no spec covers it): devices in the field get firmware
updates, so its versioned codec stays.

```sh
obc test -p obc-vectors                                  # the builders against the files and the production readers
cargo run -p obc-vectors --example regenerate --locked   # after a deliberate change of a spec
cargo run -p obc-link --features std --example flat_vectors --locked   # the flat-store vectors
OBC_UPDATE_DATA_SPEC=1 cargo test -p obc-data            # the generated parts of obc-data-cli.md and the schema
```

A change under `specs/` selects `obc-formats`; a change under `specs/vectors/` also selects
each crate, web suite and iOS suite that reads the vectors. `tools/testing/suites.toml` has the
triggers: no Cargo edge carries a vector file, so each reader declares a path trigger.

## Gotchas

- The builders in `host/obc-vectors` are written from the spec text, not from the production
  codecs, so a drift on either side fails a test. Do not make a builder call the codec it checks.
- Two fixtures are the exception: the `route-*.obcr` files and `track-export.gpx` come from the
  real converters. They pin agreement between the platforms, not correctness.
- `regenerate` does not write `vectors/manifest.json`. Edit it by hand.
- Not every vector has a builder. The JSON and CSV vectors (`climbs.json`, `eta.csv`,
  `route-answer.json`, `signed-routes.json`, the TrailView features) and
  `ride-archive-metadata/two-rides.bin` are written by hand; the tests of their readers state
  them.
- The sections "Error codes" and "JSON schemas" of `obc-data-cli.md`, and
  `obc-data.schema.json`, are generated from the Rust types of `host/obc-data`. A hand edit
  fails `cargo test -p obc-data`.
- `content-languages.json` and `photo-pools.json` are read by code at run time
  (`tools/landmark_capture.py`). They are inputs, not only documents.
- Swift and web tests read `vectors/` too. A changed vector can break `companion-ios` and
  `builder/web` with no Rust failure.
- `tools/check_paths.py` checks that each `src:` target in the guide exists. A renamed spec
  breaks the guide pages that link it.
