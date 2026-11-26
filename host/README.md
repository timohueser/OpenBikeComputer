# Host crates

The Rust crates that run on a workstation or a server and never on the device: the `obc data`
engine and its steps, the producers of the device map, terrain, places and search, the TrailView
producers, and the test oracles and tools.

Code that the device can reach lives in `firmware/`. The shells that drive these crates live in
`sim/`, `builder/` (the WebAssembly bridges) and `planner/`. All crates are members of the root
workspace.

## Where to start

| Group | Crates |
| --- | --- |
| `obc data` | [`obc-data`](obc-data/README.md): the engine, store, registry, targets, releases, serve and the commands; it knows no product. `obc-data-steps`: the `obc-data` binary, the products `device` and `planner`, the producers of prepared sources and the operator page. |
| Device map | `obc-bake` (the bakery and the catalog tree; the `obc-pack` binary is here), `obc-pack` (content, [landmarks and peaks](obc-pack/src/landmarks/README.md), catalog), `obc-draw`, `obc-network`, `obc-map-core` (config, grid, framing), `obc-pbf` (PBF scans, polygons, GEOS), `obc-tree-walk`, `obcm-assemble` (cells to one `.obcm`). |
| Sources | `obc-dem` (terrain to OBCT; [`reference/`](obc-dem/reference/README.md) is the national terrain archive), `obc-osm` (Osmium steps). |
| Places and search | `obc-places`, [`obc-search-bake`](obc-search-bake/README.md). |
| TrailView | [`obc-trailview-features`](obc-trailview-features/README.md), [`obc-ground-plan`](obc-ground-plan/README.md), `obc-trailview-parts`. |
| Tools and oracles | `obc-inspect` (`obc inspect FILE`), `obc-mkimage` (wrap and sign an update), `obc-vectors` (the builders of `specs/vectors/`), `obcm-testkit` (hand-written OBCM bytes), `obc-fixtures` (the fixture paths), `obc-file-source` (a file behind `ByteSource`), `obc-fit` (FIT for the web). |

In `obc-data`, open `src/product.rs` (the `Product` trait) and `src/engine.rs` (`Layer`, `Step`,
`Run`, `Function`). In `obc-data-steps`, open `src/device.rs`, `src/planner.rs` and
`src/functions.rs`. The contracts are the `specs/obc-data-*.md` files.

## Run and test

```sh
obc data ...                 # cargo run --release -p obc-data-steps; obc help data
obc bake ...                 # also obc pack, obc inspect FILE
obc test -p CRATE
obc test fixtures -p obc-data-steps    # the tests that read fixture packages; also obc-dem
obc exec cargo clippy -p CRATE --all-targets -- -D warnings
```

Use `obc exec` for a raw cargo command: a plain shell lacks GEOS, `tools/obc.local` and the
Cargo target of the checkout. GEOS 3.14 or newer is a dependency of `obc-pbf`, `obc-pack` and
`obc-draw`, and so of everything that links them; `obc doctor --install` builds it. A real data
update also needs Osmium, uv, Java 21 with Maven, and Node; `obc data setup` lists what the
machine has.

## How `obc data` is divided

`obc-data` owns the commands, the store, the versions, the plans, the runs and the publication.
`obc-data-steps` declares what is baked: the `Layer` statics, the step lists of each product
and the `Function` table. The body of a Rust step lives in the crate that makes its bytes, as a
`step` module (`obc_bake::step`, `obc_dem::step`, ...). A step runs in a child process: the
binary runs itself again with the step name, and the request arrives as JSON on standard input.
A Python step runs through `uv run --offline`, so the uv groups must be synced first.

Before you add a step or a product:

- Add a Rust step function to `FUNCTIONS` in `obc-data-steps/src/functions.rs`. Its name is
  `<crate>/<function>` in kebab case.
- A changed producer does not rebuild its layer. Raise `revision` of the `Layer` in the same
  change. Raise `format` when readers need new data.
- A test binary that runs a Rust step needs a `#[test] fn obc_data_step()` that calls
  `obc_data::engine::child`. Without it the step "did not write its output".
- One binary links every product, so Cargo unifies features. A step crate enables every feature
  that its bytes depend on.
- Each step name of a product starts with `<product>/`, and each source that a product reads is
  a row of `data/sources.toml`. Tests hold both.

## Gotchas

- Every workspace package must be in exactly one group of
  `firmware/tools/dependency_rules.json`, or `firmware/tools/check_dependencies.py` fails. A
  new crate here goes in the group `host`. Device crates must not depend on that group: this is
  what keeps host code off the device.
- `obc-file-source` lives here but is a foundation crate: it depends only on `heapless`, `libm`
  and other foundation crates.
- `obc-data` must not depend on a GPL crate (`tools/check_obc_data_licence.py`). The operator
  page is in `obc-data-steps` for that reason.
- `obcm-assemble`, `obc-dem`, `obc-ground-plan` and `obc-trailview-parts` also compile to
  WebAssembly. Keep the file system, HTTP and GeoTIFF out of their default paths.
- `obc-ground-plan` must give the same bytes natively and in WebAssembly: `libm` only, no std
  `HashMap` or `HashSet`, no `mul_add`. A change of its width rules re-bakes the feature tiles.
- A terrain bake needs the reference archive. A bake without `--reference` bakes every lifted
  cell again without its lift; `OBC_REFERENCE_ARCHIVE` in `tools/obc.local` names it once.
- The fixtures of the `obc-bake` ingest tests are in `builder/tests/corpus/`.

## Decisions

- The engine knows no product, so the store, the versions and the publication are the same for
  the device maps and the planner.
- The operator page (`obc data ui`) runs the command line: each action is one
  `obc data --json` command. So the page can do only what the commands do, and a credential
  goes to standard input and never to an argument or a log. The page answers only this machine
  or an SSH tunnel, and only JSON posts from itself. `obc-data-steps/src/ui.rs` has the tests.
- `obc-file-source` is its own crate, so a host tool that reads a file takes no reader, packer
  or renderer with it.
- `obcm-assemble` uses the real `obc-reader` and the one OBCT writer of `obc-dem`, so its output
  is read back as the device reads it.
- `obc-vectors` builds its files from the spec text, not from the production codecs, so a drift
  on either side fails a test.
- `obc-inspect` reads each format through the crate that owns it, so it cannot disagree with
  the device.
- `obc bake terrain` is its own command: it needs terrain tiles and no OSM, and must never be a
  step of a re-bake after a schema change.
