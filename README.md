# OBC — OpenBikeComputer

<img width="483" height="692" alt="image" src="https://github.com/user-attachments/assets/a222e908-a12b-4e26-a53c-6c227f30005a" />


A from-scratch bikepacking computer: a custom binary map format, a Rust map
packer, a web-based map builder, and the device firmware + a desktop simulator
that share **one** rendering path. The target hardware is an **nRF54L** driving a
**LS021B7DD02** memory LCD (240×320, 64 colors), but everything here runs and is
developed on the desktop today.

**Docs & live demo:** the conceptual guide — system architecture, the render
pipeline, the data formats, the UI, the display protocol — lives at
<https://openbikecomputer.com/>, which also runs the firmware's
own render path **live in your browser** (compiled to wasm).

```
  .osm.pbf  ──►  obc-pack  ──►  *.obcm (v12) ─┐
 (OSM data)    (Rust packer)   (binary map)   │
                                              ├─►  obc-app  ──►  obc-sim   (desktop simulator, today)
  *.gpx     ──►  obc-route ──►  *.obcr        │   (shared        └─►  nRF54L firmware (on the DK)
 (a route)    (route import)  (binary route) ─┘    app + render)
```

The whole project is built around two compact binary formats — **OBCM** (`.obcm`,
maps: a self-contained LOD pyramid of quadtree-indexed geometry) and **OBCR**
(`.obcr`, routes: geometry + map-matching + an elevation profile) — designed to
be read *directly* off flash by a microcontroller, with no JSON, reparsing, or
heap churn. How they work: the
[data-formats guide](https://openbikecomputer.com/software/formats/);
the normative byte layouts: [`OBCM_Spec.md`](specs/OBCM_Spec.md) /
[`OBCR_Spec.md`](specs/OBCR_Spec.md).

---

## Repository layout

| Path | What it is |
| :-- | :-- |
| `fixtures/` | The tracked catalog, scenario/profile definitions, provenance, and reproducible builders for large developer fixtures. Package bytes live in the dedicated fixture object store and are acquired with `obc fixtures`; see [`fixtures/README.md`](fixtures/README.md). |
| `firmware/` | The crates the **device image actually reaches** — nothing else. See [`firmware/README.md`](firmware/README.md). |
| `firmware/docs/` | Hardware notes that live nowhere else — the LS021 bring-up log, the FLPR blob's timing policy — plus the frozen resource baseline. Concepts belong on the [docs site](https://openbikecomputer.com/), not here. |
| `firmware/obc-app/` | `no_std` — the **application layer**: camera, camera mode (follow-user / free), screen stack, input model, and route tracking over the semantic `obc-ports` boundary. One per-frame entry point (`App::render_frame`) both hosts call. Builds for `thumbv8m.main-none-eabihf`. |
| `firmware/obc-ble/` | `no_std` — the **BLE data-plane core** (epic #267): the S0 control-plane descriptor codecs, CRC-32, list objects, and the whole-object transfer state machine. Radio-free and host-tested; the board crate drives the L2CAP bytes through it. |
| `firmware/obc-boot/` | The **32 KB nRF54L bootloader** — reads the `BOOT_STATE` page, runs `obc-dfu`'s install engine, flashes the app slot via RRAMC (LED codes, no display). Workspace-excluded + standalone like the board crate; flashed once. |
| `firmware/obc-dfu/` | `no_std` — the **SD-staged DFU core** (epic #615): the `OBCU` update-image container + boot-state page codecs, the bootloader's install engine (verify → flash → readback → trial/rollback), and the app-side armer. Host-tested with mock IO; both `obc-boot` and the board crate are thin drivers over it. |
| `firmware/obc-formats/` | Dependency-free `no_std` persistent-format authority — versions, fixed layouts, flags, sentinels, primitive codecs, and the shared byte-I/O seam. |
| `firmware/obc-fw-nrf54l/` | The **nRF54L15 board crate** — the real device target, driving the LS021 panel through the FLPR coprocessor. Its own cargo root (own target + `.cargo/config.toml`), built on its own. |
| `firmware/obc-map-scene/` | Dependency-light `no_std` streamed map-scene seam — neutral bounds/styles plus allocation-free candidate/decode visitors shared by map sources and the renderer. |
| `firmware/obc-platform/`, `obc-display/`, `obc-sensors/`, `obc-storage/` | The **platform adapters** beneath the app facade: concrete implementations of the semantic ports, the display seam and its `ls021` geometry, sensor plumbing, and the SD/RRAM stores. |
| `firmware/obc-ports/` | Dependency-free `no_std` semantic ports — fixes, sensor/input/settings traits, clocks, and recorded-track points shared without depending upward on app, platform, or host policy. |
| `firmware/obc-reader/` | `no_std + alloc` — current OBCM parsing (header, style table, LOD table, per-LOD quadtree query + chunk decode). Dependency-light. |
| `firmware/obc-render/` | `no_std` — the **shared rendering path**: `Viewport` projection, meters-per-pixel LOD selection, painter z-ordering, even-odd scanline polygon fill, weighted polylines, text. Generic over an `embedded-graphics` `DrawTarget` so host and MCU run identical drawing code. |
| `firmware/obc-route/` | `no_std` — the OBCR route reader **and** the GPX → OBCR converter. |
| `firmware/obc-weather/` | `no_std` — allocation-free OBCW validation and one-tile-at-a-time weather reads over the shared byte-source seam; storage/cache policy stays outside it. |
| `host/obc-bench/` | The **render benchmark + pixel-hash tripwire**: seven fixed scenes through the real pipeline, timings printed and frame hashes gated against `hashes.txt` in CI. |
| `host/obc-host-core/` | Host glue **shared by the two simulator hosts** (desktop + web): GPX replay stepping, the frame-interleaved route planner, in-memory stores. |
| `host/obc-mkimage/` | Host tool (`wrap` / `inspect`) — prepends the 64-byte `OBCU` header to a raw app image to make an `UPDATE.BIN`, and decodes + CRC-verifies one. The release pipeline's image producer. |
| `host/obc-dem/` | The **terrain baker**: Copernicus GLO-30 GeoTIFF → `.obcd` OBCT terrain cells and shards (`obc-dem fetch` / `bake`). Pure Rust, no native dependency; deterministic by contract. The packer never sees a DEM — it samples what this baked. |
| `host/obc-pack/` | The **map packer** (Rust): OSM `.osm.pbf` → `.obcm` — ingest, multipolygon assembly, land generation, quadtree build, streaming serialize. |
| `host/obc-wx-client/` | The **weather client**: `wx/v1/manifest.json` + OBCG corridor Range reads + MET Locationforecast → one OBCW bundle. A second, independent implementation of the client contract the iOS companion implements (the phone stays the reference), and what `obc-sim --weather live` runs. The one place in the tree that talks to the live weather service. |
| `host/obc-replay/`, `host/obc-usb-host/` | GPX replay stepping shared by the simulator hosts, and the VCOM feeder that drives a debug-uart board from a recorded ride. |
| `host/obcm-testkit/`, `host/obc-vectors/` | The **test oracles**. `obcm-testkit` hand-assembles OBCM bytes from the spec's constants — deliberately independent of the production serializer, so reader tests prove agreement with the *format* rather than with the writer. `obc-vectors` builds the shared `specs/vectors/` fixtures. |
| `apps/obc-desktop/` | The **Tauri desktop app**: the shared catalog builder UI in a native window, native same-origin downloads and atomic map-set output, plus the thumbdrive device page. Its own cargo root, like the board crate. |
| `apps/obc-sim/` | Desktop **simulator host** (eframe/egui, pure Rust — no SDL): renders `obc-app` into a framebuffer at the device's 240×320 / 64-color look, plus a control panel, GPX replay, and headless capture. |
| `apps/obc-web-convert/` | The web builder's **conversion bridge**: `obc-route`'s GPX → OBCR and track → GPX compiled to wasm behind two functions and a typed error, so route conversion runs in the visitor's browser instead of on a server. |
| `apps/obc-web-assemble/` | The web builder's **assembly bridge**: `obcm-assemble` compiled to wasm, so downloaded OBCA map cells become one `.obcm` (or a volume set) in the tab — spec-verified before anything leaves it, and byte-identical to the CLI's output. |
| `apps/obc-web-demo/` | The website's **live-demo host**: the same shared crates compiled to wasm behind a small `obc_demo_*` API — the landing page's JS owns the frame loop and canvas, no GUI framework in the tree. |
| `builder/` | The **map builder** — one Svelte app in `app/`, three hosts (static web, Tauri desktop, and the FastAPI maintainer server in `server/`). All consume the same published cell catalog and shared assembler. |
| `builder/app/` | The shared **Svelte UI**. `vite.config.ts` resolves `$host` at build time to exactly one of `web.ts` / `desktop.ts` / `dev.ts`, so the hosts you didn't build have no path into the bundle. |
| `builder/palette.json` | The device's 64-color (RGB222) gamut, offered as the web builder's default color picker so the editor and the panel agree. |
| `builder/presets/` | The shipped style documents. `schema.json` ("Bikepacking") is the one **schema** — a complete packer config (features + LODs + marker, plus a `_meta` block); `skins/<id>.json` are presentation-only documents stamped onto an assembled map's style table rather than packed ([`OBCC_Spec.md` §4–§5](specs/OBCC_Spec.md)). |
| `builder/server/` | **Maintainer server** (FastAPI): serves the advanced schema editor, palette, JSON Schema, and presets. It does not build product maps. |
| `companion-ios/` | The **iOS companion app** (SwiftUI + the `OBCKit` package): import GPX/TCX, encode OBCR, and sync routes/rides with the device over BLE. |
| `specs/vectors/` | The **executable half of the specs** — shared binary fixtures pinning the BLE wire contract, the OBCR route format and the recorded-track log + its GPX export — asserted byte-exact by `cargo test`, `swift test`, and the web builder's wasm conversion tests. |
| `tools/` | Dev scripts: the `justfile` behind `obc <task>`, the GEOS and RISC-V toolchain installers, and shell completion. |
| `ops/` | How the one **deployed service** runs: `weather/` holds the installer, the per-adapter systemd units and their cadence table, the external freshness probe, and [`RUNBOOK.md`](ops/weather/RUNBOOK.md) — the whole life of the `host/obc-wx-bake` weather bakery, from a bare VPS to the outage drill. Not dev tooling: nothing here is needed to build the firmware, the maps, or the apps. |
| `docs/` | The public docs site — `content/` is the source, `index.html` the landing page with the live wasm demo. |
| `specs/` | The **normative contracts**, in one place because all three languages read them. `OBCM_Spec.md` / `OBCR_Spec.md` / `OBCU_Spec.md` are the binary map / route / firmware-update-image layouts; `obc-ble-interface-spec.md` is the BLE wire contract; `OBCC_Spec.md` is the map catalog manifest — the JSON contract between a map bakery and the sites/apps that hand artifacts to a device, plus the OBCM version law that keeps them honest. The docs site's pages are the readable tours; these are the byte tables they link to. |

The split between the three Rust trees is **computed, not judged**: a crate lives in
`firmware/` if and only if the device image reaches it through normal dependencies.
Everything else is a tool (`host/`) or a shell (`apps/`). Dev-dependencies are allowed to
cross the line — `obc-render` tests against `obcm-testkit`, `obc-route` against `obc-pack` —
because a dev-dep never touches the `no_std` build.

The dependency direction includes `obc-sim → obc-app → obc-render → obc-map-scene`, with
`obc-reader → obc-map-scene` independently adapting streamed OBCM data. The dependency-light
`obc-formats`, `obc-map-scene`, and `obc-ports` foundations sit beneath the reader/route,
renderer, and app/route layers respectively. The nRF54L firmware and the website's
`obc-web-demo` are *sibling hosts* beside `obc-sim`, reusing `obc-app` / `obc-render` /
`obc-reader` / `obc-route` unchanged; `obc-weather` is the parallel OBCW reader below future
weather application policy. `firmware/tools/check_dependencies.py` enforces the
whole layering mechanically, and CI runs it.

One cargo workspace spans all three trees, rooted at `Cargo.toml` here — so one `Cargo.lock`
and one `target/`. Three crates stand outside it, each because it drags a toolchain the rest
has no use for: `obc-fw-nrf54l`, `obc-boot`, and `obc-desktop`.

---

## Prerequisites

| For… | You need |
| :-- | :-- |
| Building anything Rust | A stable Rust toolchain (`rustup`). |
| The packer (`obc-pack`) | System **GEOS ≥ 3.14** (`brew install geos`; `tools/install-geos.sh` builds it if your distro's is older) — linked for multipolygon area assembly, and the packer's only native dependency. |
| The desktop app (`obc-desktop`) | **Node 22+** for the embedded frontend plus the platform webview dependencies listed in [its README](apps/obc-desktop/README.md). It does not link the packer or GEOS. |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui, **no SDL/Homebrew setup**. |
| The web builder (optional) | Python 3.13 + the deps in `builder/requirements.txt`, and **Node 22+** for the UI. The maintainer schema lab also uses `osmium-tool` once to prepare its small reference-complete preview source (`obc doctor` checks it). |
| Checking the shared crates build for the device | `rustup target add thumbv8m.main-none-eabihf`. |

---

## Building

```sh
# The whole firmware workspace (simulator + shared crates + packer):
cargo build --release
```

This produces, in `target/release/`:

- `obc-sim` — the desktop simulator
- `obc-pack` — the map packer

To confirm the shared stack still compiles for the device target:

```sh
cargo build -p obc-app --target thumbv8m.main-none-eabihf
```

---

## Packing a map

Download an OSM extract (e.g. from [Geofabrik](https://download.geofabrik.de/)),
then:

```sh
target/release/obc-pack region.osm.pbf builder/presets/schema.json region.obcm
```

```
usage: obc-pack <pbf...> <config.json> <out.obcm> [--bbox W,S,E,N] [--chunk-size N]
                [--no-land] [--terrain <path>] [--dump-pois] [--dump-hours]
       obc-pack schema | catalog <bake-tree> --base-url <url> | cells <pbf...> <config.json> <out-dir>
```

- **Multiple `.pbf` inputs** are merged **during ingest** — no external tool and
  no merged intermediate on disk. Where two regions overlap (adjacent extracts
  share their border), an object present in several files is taken from the
  **first one listed**, whatever the later copies say. List your regions in the
  order you'd want a tie resolved, and prefer extracts downloaded on the same
  day: two copies of the same road from different dates are two different roads
  as far as any merge can tell.
- `--bbox W,S,E,N` crops the sources to a box (degrees) **during ingest** — no
  `osmium extract` step and no temporary cropped `.pbf`. Ways crossing the
  boundary are kept whole, and renderable area relations reached by those ways
  pull in all of their member ways. Roads and the nav graph therefore do not
  fray at the edge, while residential, forest, and farmland multipolygons do
  not disappear because one ring segment lies outside the box. Cropping needs
  each input type-sorted (nodes, then ways, then relations); every Geofabrik
  extract is.
- `--chunk-size N` sets the quadtree chunk payload target (default `4096`).
- `--no-land` skips coastline/land-polygon generation.
- LOD tiers and feature styling come from `config.json`. When `natural.land` is
  configured, coastline/land polygons are generated automatically — the global
  land-polygon dataset is downloaded and cached under `~/.cache/obcm/` on first
  use.

### The config — features, styles, and LODs

The config JSON (the shipped `builder/presets/schema.json`, or your own file) is the
single source of truth for *what* gets packed and *how it looks*
(`obc-pack schema` prints its JSON Schema):

- **`lods`** — the LOD pyramid. Each tier has a `max_mpp` (meters-per-pixel
  ceiling, `null` = the coarsest/overview tier) and a `simplify` tolerance.
- **`features`** — an OSM `key → value → style` tree. Each style sets `color`
  (RGB565, chosen to land on the 64-color RGB222 grid), `z_index` (paint order),
  `weight` (line thickness), `min_lod` (the finest tier it first appears in), and
  `priority` (drop order when a chunk overflows).

### The published cell catalog

The hosted builder and desktop app consume the same [OBCC](specs/OBCC_Spec.md)
catalog. The bakery publishes one schema, a set of presentation-only skins, named
region selections, digest-pinned per-band cell indexes, and the cells themselves.
A selected region, box, or GPX corridor is resolved to cells and assembled locally
by the same `obc-web-assemble` bridge in both products. The resulting volume-set
bytes are therefore identical for the same catalog root, coverage, and skin.

`catalog.json` is intentionally small. It pins the region cell lists and band
indexes by byte length and SHA-256; those indexes pin every cell. Consumers reject
a missing, truncated, or mismatched object instead of assembling a mixed publish.
Referenced cells, satellites, and previews carry that digest in their R2 key, so
an older cached root remains readable while a replacement propagates; unchanged
planet cells retain their key and are skipped on later publishes.
The byte-level cell and assembly rules are normative in
[OBCA_Spec.md](specs/OBCA_Spec.md), and the catalog envelope is normative in
[OBCC_Spec.md](specs/OBCC_Spec.md).

For maintainer work, `obc-pack cells` can cut an extract directly:

```sh
obc-pack cells germany-latest.osm.pbf builder/presets/schema.json ./obc-bake \
    --source europe/germany@2026-07-01=5.8,47.2,15.1,55.1
```

That low-level command is useful while developing the schema and LOD ladder.
Product users do not run it: the web builder and desktop app both use the
published catalog.

### Baking and publishing the catalog

`obc-bake` is the operator-facing compiler. Region ids are Geofabrik ids curated
in [`host/obc-bake/regions.toml`](host/obc-bake/regions.toml):

```sh
# Inspect the curated list.
obc bake regions

# Bake one region into $OBC_BAKE_TREE (default: ~/obc-bake).
obc bake europe/germany/baden-wuerttemberg

# With no region ids, bake every regions.toml entry into the same tree.
obc bake

# Bake the terrain artifact class into the same tree, from fetched DEM tiles.
# Its own revision track: this re-bakes no map cell, and a schema bump re-bakes
# no terrain cell (OBCC_Spec.md §13).
obc bake terrain --sources /tmp/dem --terrain-revision 1

# Verify catalog pins, cell headers, lockstep, and reader round-trips.
obc bake verify
```

Run `terrain` **before** `bake`: the cell bake picks up whatever terrain the tree
already holds, integrates each navigation edge's climb from it, and records which
`terrain_revision` it sampled. Re-baking terrain afterwards therefore leaves the
routing band stale, and `obc bake verify` says so — naming both revisions — rather
than letting the router's numbers drift from the raster the device draws. The DEM
tiles themselves come from `obc-dem fetch` (see
[firmware/README.md](firmware/README.md#terrain-tiles-obct)).

Several positional ids are baked together. This matters at borders: neighbouring
extracts are co-ingested for shared edge cells, so canonical cells are complete
instead of being clipped independently. Re-running is resumable and skips plans
whose source, schema, and output are already current. The tree is self-contained:
cells, sidecars, region metadata and outlines, `schema.json`, skins, their
production-rendered Teningen previews, satellites, and `catalog.json` all live
below the output directory.

No selector and `--all` deliberately mean different things. Omitting selectors
bakes the curated TOML list; `obc bake --all` bakes a planet snapshot while retaining
those entries as named catalog selections. Planet mode requires `osmium` and the
official Pyosmium replication client; `obc doctor --install` installs the latter in
the repository venv. The first run downloads and caches `planet-latest.osm.pbf`.
Later runs advance that same file through its embedded OSM replication state in
atomic, bounded batches; a cache older than 90 days is replaced by a fresh snapshot
instead of replaying months of diffs.

Every snapshot is split through a binary hierarchy of grid-aligned source leaves,
with only one bounded leaf handed to the Rust cutter at a time. The new leaf bytes
are compared with the preceding generation: byte-identical leaves refresh
provenance without a cell re-bake, while changed leaves replace their complete cell
set. This post-update comparison, rather than diff bounding boxes, also handles
deletions and relation membership changes. An interrupted run reuses hash-verified
source leaves and current cells. Every geographic cell ends as either an OBCM
artifact or a zero-byte known-empty claim, and an incomplete cell transition cannot
pass `verify` or `publish`.

```sh
# Install/check Osmium, Pyosmium, and the rest of the maintainer toolchain.
obc doctor --install

# Bake a complete planet snapshot. This is intentionally separate from publish.
obc bake --all

# Run the same command later: it applies replication updates and only re-bakes
# source leaves whose canonical content changed.
obc bake --all

# The same pipeline from an already-downloaded planet PBF.
# A local source is treated as a fixed snapshot and is never modified.
obc bake --all --source /data/planet-latest.osm.pbf
```

Publishing is a separate operation. It regenerates URLs for the public origin,
regenerates the square preview for every skin, uploads content first, verifies
remote sizes, and replaces `catalog.json` last. Root-referenced objects use
immutable digest-suffixed keys; old objects are retained so a browser holding the
previous root never receives mixed-generation bytes during an ordinary publish.
Updating preview code therefore needs only another publish, not a cell rebake:

```sh
# Copy tools/obc.local.example to the gitignored tools/obc.local and add the
# credential values. The production URL, prefix, and bake-tree defaults are ready.

# Plan only (the default target is a dry run).
obc bake publish

# Publish to Cloudflare R2; requires rclone.
# R2 publishes show every upload, cumulative bytes, ETA, remote verification,
# and the final catalog-root swap automatically.
obc bake publish --target r2

# A deliberate full-store replacement may instead preview and then purge every
# object in the configured maps bucket. The second command leaves the catalog
# offline until the publish succeeds, so verify the tree first and run the purge
# and publish back-to-back.
obc bake verify
obc bake clean-r2
obc bake clean-r2 --apply && obc bake publish --target r2
```

`clean-r2` requires an explicit non-root `OBC_R2_PREFIX`, requires
`OBC_MAPS_BASE_URL` to end in that same prefix, and confirms that the prefix's
current `catalog.json` is readable before either a dry run or a real purge. That
check proves the configured R2 bucket is the one serving the catalog; cleanup
then deletes every object in that bucket, including objects outside the catalog
prefix. Use it for an intentionally disruptive format cutover, not for an
ordinary incremental publish.

When `OBC_R2_PREFIX` is set, `OBC_MAPS_BASE_URL` must be the public URL of that
same prefix. For example, `OBC_R2_PREFIX=cell-catalog` pairs with
`OBC_MAPS_BASE_URL=https://maps.openbikecomputer.com/cell-catalog`; the root to put
in `OBC_CATALOG_URL` is then
`https://maps.openbikecomputer.com/cell-catalog/catalog.json`.

Set `OBC_BAKE_TREE` in `tools/obc.local` to move the operator tree. An explicit
tree may also follow `verify` or `publish`. For curated bakes, `--source DIR` uses
local Geofabrik-shaped `.osm.pbf` and `.poly` inputs. With `--all`, `--source`
accepts a planet PBF URL, a local `planet-latest.osm.pbf`, or its containing
directory. The default/URL source is cached and replication-updated; a local file is
read-only. Planet updates need temporary disk for one additional full PBF while
Pyosmium writes its atomic replacement. Downloads, source shards, and replication
state live in the shared cache. The terminal and `--summary-json` report the
replication sequence range, source leaves that were current/byte-identical/changed,
and cell leaves that were cut/refreshed/unchanged.
Run `obc bake help` for the complete operator surface.

### Web builder and desktop app

The Svelte builder has one coverage UI and one assembly pipeline. It supports named
regions, drawn boxes, and corridors around GPX routes, prices the exact selected
cells, downloads digest-verified inputs, and runs assembly in a worker. A skin
changes only presentation; schema and LOD changes require a maintainer bake.

The same source is built for three hosts:

| Script | Host | Output |
| :-- | :-- | :-- |
| `npm run build` | local maintainer server | `builder/server/static/dist/` |
| `npm run build:web` | static website | `builder/app/dist/web/` |
| `npm run build:desktop` | Tauri desktop app | `builder/app/dist/desktop/` |

The hosted build reads the catalog from `VITE_CATALOG_URL` (or the deployed
default). The desktop URL is compiled with `OBC_CATALOG_URL` and defaults to the
OpenBikeComputer map origin. Desktop catalog and cell reads go through its native
HTTPS transport and are restricted to that configured origin. Assembled files are
written atomically into one uniquely named folder under
`Documents/OpenBikeComputer`; the web host offers the same bytes as downloads.

The local Python server remains the maintainer host for the shared Maps UI and
the advanced schema editor. Prepare its small preview source once, then start it:

```sh
obc web preview-source
obc web
```

The first command reuses the bakery's cached Freiburg-regbez extract when
available (downloads it otherwise), then atomically asks Osmium for one
reference-complete Teningen crop. `OBC_SCHEMA_PREVIEW_PBF` in `tools/obc.local`
may instead name an already-prepared absolute `.osm.pbf`; that maintainer-owned
path is never downloaded to or overwritten.

The Maps tab reads `OBC_CATALOG_URL` at server runtime and moves catalog objects
through a same-origin, catalog-tree-restricted proxy, so an old Vite bundle or an
R2 CORS policy cannot redirect the local host to `./data/catalog.json`. Advanced
edits live in the browser and debounce into one native pack of only the prepared
Teningen source—normally about 5–15 seconds, never a region or planet bake. The
result renders on a real 240×320 device map plane through `obc-reader` and
`obc-render`, with the full LOD ladder and the production feature/span/point/ring
limits visible. The exported complete config becomes public only through an
explicit maintainer bake.

`obc web` refreshes the native packer, frontend, and all three wasm bridges. The lower-level
equivalent remains:

```sh
cd builder/app
npm ci
npm run build:wasm
npm run build:all
```

`obc-web-convert` handles GPX/route conversion, `obc-web-assemble` assembles cell
sets, and `obc-skin-preview` exposes the production renderer to both preview
surfaces. There is no separate product preview renderer or desktop PBF build path.

### Driving the device step without a device

The USB writes (map, route, firmware) need an OBC on the other end of a cable.
The device half ships in every firmware build (#889, `obc-fw-nrf54l/src/usb/`)
and is verified on hardware. `dev-harness/` is a second
entry point that mounts the whole app against the **simulated device** —
`lib/usb/loopback.ts`, the real protocol over an in-memory pipe, paced to a fixed
**~0.68 MB/s** — binary MB, the convention `lib/format.ts` uses everywhere the app
shows a rate, so this is the figure the harness's own progress bar reports — so
that progress, throughput and the remaining-time estimate behave
plausibly. That pacing is **deliberately pessimistic and not a measurement**: it
was the retired SPI transport's write ceiling, and the sEMMC pivot (#1158) raised
the card's raw write bandwidth to 8.2 MB/s. The upload pipeline was retuned for
that (windowed host writes, a double-buffered cluster-sized device stage, FAT
pre-allocation), but nothing end to end has been measured on glass yet — so the
harness stays slow on purpose rather than promising a number the hardware has not
confirmed. It lives outside `src/` because no build has it as
an input, which is what keeps the simulated device out of every shipped bundle.

```sh
cd builder/app
VITE_DATA_BASE=/data npm run dev -- --mode web   # then open /dev-harness/
```

`VITE_DATA_BASE` is root-relative here because the harness is served from a
sub-path, and the default `./data` would resolve under it.

All of them — and `npm run check` and `npm test` — need the three wasm bridges
built once first (`npm run build:wasm`, which wants a Rust toolchain and
`wasm-pack`): route conversion, cell assembly, and production previews run
client-side through the project's own code, so the TypeScript imports bindings
that do not exist until they are built. See
[`firmware/README.md`](firmware/README.md#build-the-web-builders-wasm-bridges-obc-web-convert-obc-web-assemble).

---

## Viewing & simulating

`obc-sim` renders `.obcm` maps through the exact code path the firmware runs.
Maps must be **v12**.

```sh
# Interactive, simulating the device (240×320, 64 colors), 3× window scale:
target/release/obc-sim region.obcm

# Larger window / different simulated resolution:
target/release/obc-sim region.obcm --size 480x640 --scale 2

# Full color (skip the 64-color quantization) for comparison:
target/release/obc-sim region.obcm --true-color
```

**Interactive controls:** drag to pan, scroll to zoom, Esc/Q to quit. The
simulator boots to the device's Home screen and drives the full on-device UI —
the whole screen stack, driven by the four-button input model, plus a control
panel for feeding it a simulated GPS.

Useful flags:

| Flag | Purpose |
| :-- | :-- |
| `--gpx TRACK.gpx` | Preload a GPX and replay it as a simulated GPS (or drag a file onto the window). |
| `--routes-dir DIR` | Folder of `.obcr` routes the Route menu lists (device-SD stand-in; default `routes/`). |
| `--tracks-dir DIR` | Folder for saved ride `.gpx` + the in-progress log (default `tracks/`). |
| `--import GPX` | Convert a GPX into an `.obcr` route in the routes folder and exit (what the device does on a USB drop). |
| `--physical` / `--calibrate` | Render at the panel's true physical size — calibrate once with a ruler. |
| `--palette` | Show the device's 64-color gamut and nothing else (color test). |
| `--png OUT.png` | Headless: render one frame to PNG (no window). |
| `--screenshot OUT.png` | Launch the GUI, capture its first composited frame, then exit. |
| `--center LON,LAT` `--zoom MULT` | Aim the headless camera at a spot / zoom level (e.g. to inspect a chunk boundary). |
| `--script TOKENS` `--boot` | Drive a gesture script before a headless render (e.g. walk Home → Route menu → Map). |

The GUI host is pure Rust (eframe/egui) — no SDL or system libraries to install.
See [`firmware/README.md`](firmware/README.md) for the full flag reference.

---

## Routes & tracks

The route workflow mirrors what the device will do:

1. **Import** — a `.gpx` route is converted to a compact `.obcr` (`obc-route`,
   shared with the firmware). On the device this happens on a USB drop; in the
   simulator use `--import` or drag-and-drop.
2. **Ride** — pick a route from the Route menu. The Map draws it, and the app
   live **map-matches** the current fix (progress along route / off-route),
   tracks actually-ridden distance / time / climb (barometer), shows a zoomable
   elevation profile, and records a breadcrumb.
3. **Save** — the finished ride is stored durably: the simulator exports a
   `.gpx` to the tracks folder; the device writes the compact ride object the
   companion app syncs (and exports to GPX phone-side).

---

## The companion app

Routes are usually planned on a phone and rides are worth keeping, so a SwiftUI
**iOS companion app** ([`companion-ios/`](companion-ios)) syncs with the device
over **Bluetooth Low Energy** — push a route, pull a ride, rename the device,
read diagnostics. The phone does all the format conversion (GPX/TCX → OBCR) and
the device writes the bytes to its card verbatim. How the link is shaped — the
GATT control plane, the L2CAP data plane, pairing and reconnect — is
[the companion link](https://openbikecomputer.com/software/companion-link/);
the normative byte contract is
[`obc-ble-interface-spec.md`](specs/obc-ble-interface-spec.md), and its host-tested
core (descriptor codecs, CRC-32, the transfer state machine) is
[`firmware/obc-ble/`](firmware/obc-ble), which rides the normal `cargo test`.

The firmware's BLE support is a build-time variant of the board crate:

```sh
cd firmware/obc-fw-nrf54l
cargo build --release --no-default-features --features ble
```

Flashing, the dependency pins, and the on-glass verify steps live in the
[board crate README](firmware/obc-fw-nrf54l/README.md).

## Firmware updates

The device updates itself in the field — no probe needed. An update ships as a
single **`UPDATE.BIN`** file (an [`OBCU`](specs/OBCU_Spec.md) container: a 64-byte
header plus the raw app image). The trust model — verify before erase, a single
trial boot with rollback — is the
[firmware updates](https://openbikecomputer.com/software/firmware-updates/)
docs page.

**Installing one (on the device):**

1. Copy `UPDATE.BIN` to the **root** of the device's SD card (from any computer),
   or push it from the companion app over BLE.
2. On the device: **Settings → System → "Install update from card"**.
3. Confirm on the glass. The device validates the image, snapshots the running
   firmware, and reboots into the bootloader to flash it (its LED takes over while
   the display is off). If the new image doesn't come up healthy, the next boot
   rolls back automatically.

**Cutting a release (maintainers):** push a `v*` tag. The
[`release` workflow](.github/workflows/release.yml) builds the shipping firmware
(`obc-fw-nrf54l` `--features ble`) and the `obc-boot` bootloader, wraps the app
into `UPDATE.BIN` tagged with the version, gates on `obc-mkimage inspect`, and
attaches `UPDATE.BIN`, both ELFs, and a `SHA256SUMS.txt` to the GitHub release:

```sh
git tag v0.4.0
git push origin v0.4.0        # → the release + UPDATE.BIN appear on the Releases page
```

To dry-run the pipeline without tagging — validate that a candidate `UPDATE.BIN`
builds and passes `inspect` — trigger the workflow manually (**Actions → Release →
Run workflow**, with a version string, or `gh workflow run release.yml -f
version=v0.4.0-rc1`); it uploads the same artifacts as a downloadable bundle and
publishes no release. Building `UPDATE.BIN` by hand (the `objcopy → wrap`
pipeline) is in the [firmware README](firmware/README.md#firmware-update-images-obcu).

## Testing

```sh
cargo test -p obc-pack   # the packer
cargo test                                                # the whole workspace
```

The `obc-pack` tests use fixtures under `builder/tests/corpus/` — the committed
`tiny/tiny.osm` plus a `config.json`. Regenerate the binary fixtures with
`builder/tests/corpus/build_corpus.sh`, the one thing left in the tree that wants
`osmium-tool` (for `osmium cat`, XML → PBF; it packs no map).

---

## Status & roadmap

The full app runs on the desktop simulator **and on the nRF54LM20 DK** today: the
shared stack (`obc-map-scene`, `obc-reader`, `obc-route`, `obc-render`, `obc-app`) runs `no_std`
on the device, streaming maps/routes from a microSD card over a native 4-bit
sEMMC transport and driving the panel over a parallel bus — both clocked by the
FLPR coprocessor.

**Working now:** OBCM v13 packing (CLI + web builder) and the baked cell catalog
behind it — `obc-bake` cuts regions or a whole planet snapshot into OBCA cells,
and the browser assembles a selection back into one map without a server; the
shared LOD-pyramid renderer (quadtree query, polygon fill with holes, weighted
and two-color lines, z-ordering, RGB565 → RGB222 quantization) plus baked OBCT
terrain drawn as contours; on-device routing over the packed nav graph, with
route loading, live map-matching, detours, ride logging and ride saving; the
whole screen stack driven by the four-button input; the BLE companion link
(routes in, rides out) and self-service firmware updates from the card; and the
reflective **LS021B7DD02** panel driver, its waveform backend running on the
nRF54L's FLPR coprocessor with partial / dirty-row updates.

---

## License

The **software** in this repository — the firmware, the `obc-*` crates, the Rust
packer, and the web map builder — is licensed under the **GNU GPL v3.0** (see
[`LICENSE`](LICENSE)). Use it for anything, commercial or not, as long as
derivatives stay open source under the same terms.

The **hardware** design files (the 3D-printed case, and any future board designs)
are licensed under **CERN-OHL-S v2** — strongly reciprocal; see
[`LICENSE.hardware`](LICENSE.hardware). It's the open-hardware counterpart to the
GPL: build it, modify it, even sell it, but share the editable source under the
same licence.
