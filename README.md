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
  .osm.pbf  ──►  obc-pack  ──►  *.obcm (v5)  ─┐
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
| `firmware/` | The crates the **device image actually reaches** — nothing else. See [`firmware/README.md`](firmware/README.md). |
| `firmware/docs/` | Hardware notes that live nowhere else — the LS021 bring-up log, the FLPR blob's timing policy — plus the frozen resource baseline. Concepts belong on the [docs site](https://openbikecomputer.com/), not here. |
| `firmware/obc-app/` | `no_std` — the **application layer**: camera, camera mode (follow-user / free), screen stack, input model, route tracking, plus compatibility re-exports of `obc-ports`. One per-frame entry point (`App::render_frame`) both hosts call. Builds for `thumbv8m.main-none-eabihf`. |
| `firmware/obc-ble/` | `no_std` — the **BLE data-plane core** (epic #267): the S0 control-plane descriptor codecs, CRC-32, list objects, and the whole-object transfer state machine. Radio-free and host-tested; the board crate drives the L2CAP bytes through it. |
| `firmware/obc-boot/` | The **32 KB nRF54L bootloader** — reads the `BOOT_STATE` page, runs `obc-dfu`'s install engine, flashes the app slot via RRAMC (LED codes, no display). Workspace-excluded + standalone like the board crate; flashed once. |
| `firmware/obc-dfu/` | `no_std` — the **SD-staged DFU core** (epic #615): the `OBCU` update-image container + boot-state page codecs, the bootloader's install engine (verify → flash → readback → trial/rollback), and the app-side armer. Host-tested with mock IO; both `obc-boot` and the board crate are thin drivers over it. |
| `firmware/obc-formats/` | Dependency-free `no_std` persistent-format authority — versions, fixed layouts, flags, sentinels, primitive codecs, and the shared byte-I/O seam. |
| `firmware/obc-fw-nrf54l/` | The **nRF54L15 board crate** — the real device target, driving the LS021 panel through the FLPR coprocessor. Its own cargo root (own target + `.cargo/config.toml`), built on its own. |
| `firmware/obc-map-scene/` | Dependency-light `no_std` streamed map-scene seam — neutral bounds/styles plus allocation-free candidate/decode visitors shared by map sources and the renderer. |
| `firmware/obc-platform/`, `obc-display/`, `obc-sensors/`, `obc-storage/` | The **platform adapters** beneath the app facade: concrete implementations of the semantic ports, the display seam and its `ls021` geometry, sensor plumbing, and the SD/RRAM stores. |
| `firmware/obc-ports/` | Dependency-free `no_std` semantic ports — fixes, sensor/input/settings traits, clocks, and recorded-track points shared without depending upward on app, platform, or host policy. |
| `firmware/obc-reader/` | `no_std + alloc` — pure OBCM **v5** parsing (header, style table, LOD table, per-LOD quadtree query + chunk decode). Dependency-light. |
| `firmware/obc-render/` | `no_std` — the **shared rendering path**: `Viewport` projection, meters-per-pixel LOD selection, painter z-ordering, even-odd scanline polygon fill, weighted polylines, text. Generic over an `embedded-graphics` `DrawTarget` so host and MCU run identical drawing code. |
| `firmware/obc-route/` | `no_std` — the OBCR route reader **and** the GPX → OBCR converter. |
| `host/obc-bench/` | The **render benchmark + pixel-hash tripwire**: seven fixed scenes through the real pipeline, timings printed and frame hashes gated against `hashes.txt` in CI. |
| `host/obc-host-core/` | Host glue **shared by the two simulator hosts** (desktop + web): GPX replay stepping, the frame-interleaved route planner, in-memory stores. |
| `host/obc-mkimage/` | Host tool (`wrap` / `inspect`) — prepends the 64-byte `OBCU` header to a raw app image to make an `UPDATE.BIN`, and decodes + CRC-verifies one. The release pipeline's image producer. |
| `host/obc-pack/` | The **map packer** (Rust): OSM `.osm.pbf` → `.obcm` — ingest, multipolygon assembly, land generation, quadtree build, streaming serialize. |
| `host/obc-replay/`, `host/obc-usb-host/` | GPX replay stepping shared by the simulator hosts, and the VCOM feeder that drives a debug-uart board from a recorded ride. |
| `host/obcm-testkit/`, `host/obc-vectors/` | The **test oracles**. `obcm-testkit` hand-assembles OBCM bytes from the spec's constants — deliberately independent of the production serializer, so reader tests prove agreement with the *format* rather than with the writer. `obc-vectors` builds the shared `specs/vectors/` fixtures. |
| `apps/obc-desktop/` | The **Tauri desktop app**: the builder UI in a native window with `obc-pack` linked in and a vendored GEOS, plus the thumbdrive device page. Its own cargo root, like the board crate. |
| `apps/obc-sim/` | Desktop **simulator host** (eframe/egui, pure Rust — no SDL): renders `obc-app` into a framebuffer at the device's 240×320 / 64-color look, plus a control panel, GPX replay, and headless capture. |
| `apps/obc-web-convert/` | The web builder's **conversion bridge**: `obc-route`'s GPX → OBCR and track → GPX compiled to wasm behind two functions and a typed error, so route conversion runs in the visitor's browser instead of on a server. |
| `apps/obc-web-assemble/` | The web builder's **assembly bridge**: `obcm-assemble` compiled to wasm, so downloaded OBCA map cells become one `.obcm` (or a volume set) in the tab — spec-verified before anything leaves it, and byte-identical to the CLI's output. |
| `apps/obc-web-demo/` | The website's **live-demo host**: the same shared crates compiled to wasm behind a small `obc_demo_*` API — the landing page's JS owns the frame loop and canvas, no GUI framework in the tree. |
| `builder/` | The **map builder** — one Svelte app in `app/`, three hosts (static web, Tauri desktop, and the FastAPI dev server in `server/`). Nothing here packs anything: they all drive `host/obc-pack`. |
| `builder/app/` | The shared **Svelte UI**. `vite.config.ts` resolves `$host` at build time to exactly one of `web.ts` / `desktop.ts` / `dev.ts`, so the hosts you didn't build have no path into the bundle. |
| `builder/palette.json` | The device's 64-color (RGB222) gamut, offered as the web builder's default color picker so the editor and the panel agree. |
| `builder/presets/` | Style presets — complete packer configs (features + LODs + marker, plus a `_meta` block). `default.json` ("Bikepacking") is the read-only factory default; `minimal.json` and `high-detail.json` ship alongside. |
| `builder/server/` | **Web builder** (FastAPI): pick regions on a map, edit styles, and build an `.obcm` in the browser — shells out to `obc-pack`. |
| `companion-ios/` | The **iOS companion app** (SwiftUI + the `OBCKit` package): import GPX/TCX, encode OBCR, and sync routes/rides with the device over BLE. |
| `specs/vectors/` | The **executable half of the specs** — shared binary fixtures pinning the BLE wire contract, the OBCR route format and the recorded-track log + its GPX export — asserted byte-exact by `cargo test`, `swift test`, and the web builder's wasm conversion tests. |
| `tools/` | Dev scripts: the `justfile` behind `obc <task>`, the GEOS and RISC-V toolchain installers, and shell completion. |
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
`obc-reader` / `obc-route` unchanged. `firmware/tools/check_dependencies.py` enforces the
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
| The desktop app (`obc-desktop`) | **No GEOS** — it compiles a vendored one into the binary. It wants **CMake** (to build that) and **Node 22+** (it embeds the built frontend) instead. See [its README](apps/obc-desktop/README.md). |
| The desktop simulator | Just Rust — the GUI is pure eframe/egui, **no SDL/Homebrew setup**. |
| The web builder (optional) | Python 3.13 + the deps in `builder/requirements.txt`, and **Node 22+** for the one-time UI build (`npm ci && npm run build` in `builder/app/`). |
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
target/release/obc-pack region.osm.pbf builder/presets/default.json region.obcm
```

```
usage: obc-pack <pbf...> <config.json> <out.obcm> [--bbox W,S,E,N] [--chunk-size N] [--no-land]
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
  boundary are kept whole (osmium's `complete_ways` strategy), so the map and
  the nav graph don't fray at the edge. Cropping needs each input type-sorted
  (nodes, then ways, then relations); every Geofabrik extract is.
- `--chunk-size N` sets the quadtree chunk payload target (default `4096`).
- `--no-land` skips coastline/land-polygon generation.
- LOD tiers and feature styling come from `config.json`. When `natural.land` is
  configured, coastline/land polygons are generated automatically — the global
  land-polygon dataset is downloaded and cached under `~/.cache/obcm/` on first
  use.

### The config — features, styles, and LODs

The config JSON (any preset under `builder/presets/`, or your own file) is the
single source of truth for *what* gets packed and *how it looks*
(`obc-pack schema` prints its JSON Schema):

- **`lods`** — the LOD pyramid. Each tier has a `max_mpp` (meters-per-pixel
  ceiling, `null` = the coarsest/overview tier) and a `simplify` tolerance.
- **`features`** — an OSM `key → value → style` tree. Each style sets `color`
  (RGB565, chosen to land on the 64-color RGB222 grid), `z_index` (paint order),
  `weight` (line thickness), `min_lod` (the finest tier it first appears in), and
  `priority` (drop order when a chunk overflows).

### The catalog manifest — describing a pile of baked maps

A distribution that bakes maps centrally and serves them as static files needs one
JSON document saying what exists. `obc-pack catalog` walks a bake output tree and
writes it:

```sh
obc-pack catalog <bake-tree> --base-url https://maps.example.org/catalog/v1
# → <bake-tree>/catalog.json           (written atomically: temp file, fsync, rename)

obc-pack catalog <bake-tree> --base-url … --out -   # print instead, for inspection
obc-pack schema --catalog                           # the manifest's JSON Schema
```

The tree is self-describing — `presets/<id>.json` (the preset's current definition) and
`regions/<a>/<b>/<preset>.obcm` next to a small `<preset>.obcm.json` sidecar. The
sidecar records what the *bake* knew and the bytes can't state: region name, the preset
version it was packed with, build time, and the Geofabrik snapshot date. Everything
else is read out of the artifact itself, including the **OBCM version**, which is what
lets a consumer refuse a map a device can't read before downloading a few hundred
megabytes.

Nothing in the sidecar is re-derived at generation time, which is what lets the manifest
stay honest across a *partial* re-bake: restyle one preset, re-bake half the regions,
and the untouched artifacts keep reporting the version they were actually built with
instead of being relabelled with the new one.

The manifest layout, the sidecar, and the version law (an OBCM bump invalidates every
baked artifact) are normative in [`OBCC_Spec.md`](specs/OBCC_Spec.md); pass
`--generated-at` in CI to make a re-run byte-reproducible.

#### The cell catalog (`--v2`)

The same subcommand walks a **cell** tree instead, and writes the `schema_version 2`
catalog of [`OBCC_Spec.md` §11](specs/OBCC_Spec.md) — one schema, a set of skins, and
named regions that are cell-set *selections* with a drawable boundary:

```sh
obc-pack catalog <cell-tree> --base-url https://maps.example.org/catalog/v2 --v2
# → <cell-tree>/catalog.json, cells/<band>/index.json, regions/<id>/cells.json
#   (satellites first, root last: the root pins each satellite by size + sha256)

obc-pack catalog <cell-tree> --base-url … --v2 --boundary-tolerance 2000  # µdeg, default 2000
obc-pack catalog <cell-tree> --base-url … --v2 --out -                    # print the root only
obc-pack schema --catalog-v2                                              # the v2 JSON Schema
```

That tree is self-describing too: `schema.json` is the packer config the cells were
baked with, plus a `_meta` block carrying the schema `revision` and the band table;
`skins/<id>.json` is one config per skin (same feature types, same style ids, different
values); `cells/<band>/<i>/<j>.obcm` is a cell with a `.obcm.json` sidecar recording its
revision, build time, source extracts and whether it is `partial`; and
`regions/<a>/<b>/region.json` names the region and **stores** its cell ids per band,
beside the `boundary.poly` (the region's Geofabrik `.poly`) that the outline is
simplified from. A cell carries no bbox anywhere — its id *is* its grid square, and the
generator verifies the artifact's own header against it.
### Cutting an extract into grid cells

The cell catalog ([`OBCA_Spec.md`](specs/OBCA_Spec.md)) bakes **grid cells** rather than
whole regions, so any selection becomes an assembly of cells instead of another bake.
`obc-pack cells` ingests the sources **once** and writes every cell of every band they
touch:

```sh
obc-pack cells germany-latest.osm.pbf builder/presets/default.json ~/cells \
    --source europe/germany@2026-07-01=5.8,47.2,15.1,55.1
# → ~/cells/cells/<band>/<i>/<j>.obcm   one valid .obcm per cell
# → ~/cells/cells.json                  the provenance sidecar, written last
```

Each cell's header bbox *is* its grid square, it carries the whole LOD ladder with the
levels outside its band written empty, and the nav graph and POIs live only in the band
that carries them — so band membership is never in the bytes. Useful flags:

- `--bands <bands.json>` — the schema's band table (which LODs and sections live at which
  cell size). Cell sizes are schema data; the default is the v1 table (`2^20` coarse /
  `2^19` mid / `2^18` fine + network).
- `--band <id>` / `--cell <log2/i/j>` — cut a subset. A cell is a function of the source,
  not of the run that asked for it, so a narrowed run writes byte-identical cells.
- `--source <id>[@<snapshot>][=W,S,E,N]` — what this run baked from. Without a coverage
  box nothing can be shown to be fully covered, so every cell is marked `partial`.
- `--bbox`, `--chunk-size`, `--no-land` — as for a normal pack.

### The bakery — filling that tree, and publishing it

`obc-bake` is what produces the tree above. It crosses a curated region list
([`host/obc-bake/regions.toml`](host/obc-bake/regions.toml) — Germany with all sixteen
Bundesländer, Austria, Switzerland; one line per region, so adding coverage is a
one-line PR) with the presets in `builder/presets/`:

```sh
cargo run --release -p obc-bake -- regions            # what would be baked
cargo run --release -p obc-bake -- bake --out ~/bake \
    --summary-json ~/bake/run.json                    # download, pack, verify, install
cargo run --release -p obc-bake -- publish ~/bake \
    --base-url https://maps.example.org --target r2   # artifacts first, manifest last
```

Per (region, preset) it downloads or reuses the Geofabrik extract, packs it with the
linked-in packer, and **opens the result with the real `obc-reader`** — every LOD, every
chunk, every feature — before renaming it into the tree. A corrupt artifact never gets a
name, so it can never reach the manifest. Re-running is cheap: the skip is keyed on the
SHA-256 of the extract and of the preset config plus the OBCM version, never on
timestamps, and a run prints the real per-artifact sizes and a total (which is what the
storage bill is actually made of). The sidecar-only facts — a region's display name, the
extract's date — are keyed separately, so a re-dated but byte-identical extract rewrites
the sidecar and packs nothing. A region that fails is loud — in the summary, and in the
exit status.

`publish` refuses to shrink the live catalog: before uploading anything it reads the
manifest already at the destination and stops if this tree would drop coverage that is
currently served, naming the artifacts that would disappear. That is the guard against
the easy mistake — publishing a `--region`-narrowed or CI-sized tree over the full one,
which succeeds atomically and un-offers everything it does not contain. `--allow-shrink`
proceeds anyway, loudly, for the deliberate case.

Useful flags: `--region <id>` / `--preset <id>` to narrow the matrix, `--source <dir>` to
bake from local extracts, `--force` to re-bake regardless, `--no-land` to skip the
~950 MB land dataset. `publish` defaults to a dry run; `--target dir:PATH` writes a
servable copy, `--target r2` uploads through `rclone` with credentials taken from
`OBC_R2_*` environment variables and never written to disk.

### Baking cells, region-scoped

`--cells` swaps the unit: the same curated regions, resolved to the **grid cells** their
coverage polygons touch and published as an [`OBCC_Spec.md` §11](specs/OBCC_Spec.md)
cell catalog. No planet extract is involved — the canonical testing flow is two
neighbours at once:

```sh
cargo run --release -p obc-bake -- bake --cells --out ~/cells \
    --base-url https://maps.example.org/cells \
    europe/germany europe/switzerland                 # regions may be positional
cargo run --release -p obc-bake -- verify ~/cells     # digests, headers, round-trips
cargo run --release -p obc-bake -- publish ~/cells --v2 \
    --base-url https://maps.example.org/cells --target r2
```

It downloads each region's extract **and its `.poly`**, resolves the polygon to a cell
set per band, groups every cell by the set of co-baked extracts whose polygon touches it,
and runs one cut per group — so the cells on the German/Swiss border are cut from *both*
extracts at once and come out complete rather than half-empty. A cell is published as
canonical only when its own sources cover its whole square; everything else is flagged
`partial` in the catalog, and a canonical cell is never replaced by a partial one (so
re-baking Switzerland alone afterwards keeps the joint border cells). Re-runs skip at
**plan** granularity — a group whose every cell is current is never ingested — and the
run prints the measured per-band byte density.

Flags on top of the region bake's: `--base-url` (required — every cell's `url` is it plus
the cell's path), `--schema-preset <id>` (the config the cells are cut with; default
`default`), `--schema-id` / `--schema-revision` (what the catalog publishes; a revision
bump invalidates the whole store), `--bands <file>` (the band table; default the
[`OBCA_Spec.md`](specs/OBCA_Spec.md) §1.5 v1 one), `--skin <id>` (repeatable; default the
schema preset).

`obc-bake verify <tree>` is the cell store's own gate: it runs the lockstep guard (one
schema revision, one OBCM version, no cell silently downgraded from canonical to
`partial`), checks every satellite against the digest the root pinned, checks **every**
cell's header bbox against its id, and opens one cell in fifty with the real reader
(`--sample 1` for all of them).

`obc-bake check-obcm-version` is the other half of the version law: it fetches the
*published* manifest (`--catalog-url`, or `OBC_CATALOG_URL`) and fails if what is being
served is not the OBCM version this build writes. Scheduled + dispatchable bakes and that
guard live in [`.github/workflows/bake.yml`](.github/workflows/bake.yml) — sized for small
regions, because a country-scale bake does not fit a GitHub runner.

### Web builder

For an interactive flow — select regions on a map, tweak styles against the live
device palette, watch build progress:

```sh
# from the repo root
.venv/bin/python -m builder.server        # http://localhost:8000
```

It drives the `obc-pack` binary you built above (override its path with the
`OBC_PACK_BIN` env var). The UI is a Svelte app compiled once with Node
(`cd builder/app && npm ci && npm run build`). Features:

- **Region picker** — click regions or search the Geofabrik tree; builds stream
  live progress over server-sent events and finish with a download link.
- **Bounding-box build mode** — draw a crop box on the map and the selected PBFs
  are cropped to it before packing, so you can target a small area precisely.
- **Style presets** — pick Bikepacking / Minimal / High detail on the main page
  (the files under `builder/presets/`); the **advanced editor** exposes every
  knob: per-feature colors / z-order / weights / per-LOD detail, LOD tiers, and
  output settings, with the color picker defaulting to the device's 64-color
  gamut (`palette.json`).
- **Your edits live in the browser** (localStorage) as "Custom — based on
  &lt;preset&gt;"; **Reset to preset** re-applies the shipped version. Nothing
  is stored server-side.
- **Export / Import** — the exported `.json` is a complete packer config,
  directly usable with the `obc-pack` CLI; old stylesheet exports import fine.
- Feature/category fields autocomplete from a curated OSM tag catalog; any
  freeform tag still works.

Downloads, caches, and the build queue are env-configurable (all optional):

| Variable | Default | Meaning |
| :-- | :-- | :-- |
| `OBCM_CACHE_DIR` | `~/.cache/obcm` | Geofabrik index, PBF downloads, land polygons. |
| `OBCM_OUTPUT_DIR` | `<cache>/builds` | Per-job build outputs, served by the download endpoint. |
| `OBC_PACK_BIN` | `target/{release,debug}/obc-pack` | Path to the packer binary. |
| `OBCM_MAX_CONCURRENT_JOBS` | `1` | Parallel packs (obc-pack is memory-hungry). |
| `OBCM_KEEP_JOBS` | `20` | Finished builds kept before the sweeper evicts by count/age. |

For frontend development, run the API (`.venv/bin/python -m builder.server
--no-browser`) and `npm run dev` in `builder/app/` side by side —
Vite proxies `/api` to port 8000.

The same Svelte source builds for three hosts, chosen by Vite mode (issue #895).
`npm run build` is the local FastAPI one described above and the only one the
Python server mounts:

| Script | Host | Output |
| :-- | :-- | :-- |
| `npm run build` | the local FastAPI dev server | `builder/server/static/dist/` |
| `npm run build:web` | the static hosted site (no backend) | `builder/app/dist/web/` |
| `npm run build:desktop` | the Tauri desktop app | `builder/app/dist/desktop/` |

The static host has no API to call, so it reads two files instead: `regions.json`
(the trimmed Geofabrik index the picker draws — the same document `/api/regions`
returns) and `catalog.json` (the [OBCC](specs/OBCC_Spec.md) manifest of pre-baked maps,
produced by `obc-pack catalog`). Both default to `./data/` beside the app;
`VITE_DATA_BASE` and `VITE_CATALOG_URL` move them. To run that host locally:

```sh
# regions.json — needs the packer venv (Geofabrik index + shapely)
.venv/bin/python -m builder.server.static_data \
    --out builder/app/public/data

# catalog.json — from a bake tree (OBCC_Spec.md §8), pointed at where it is served
target/release/obc-pack catalog <bake-tree> \
    --base-url http://localhost:5173/data

cd builder/app && npm run dev -- --mode web
```

`obc site [PORT]` does the built version of all of that in one command: builds the
web host, bakes `regions.json` beside it, and serves it on `:4173`
(`OBC_CATALOG_URL=…` points it at a real manifest).

**Published** by [`deploy-site.yml`](.github/workflows/deploy-site.yml) on every push
to `develop`, as one GitHub Pages artifact holding four surfaces: the landing page
and live demo at `/`, `/docs/`, `/blog/`, and this host at `/builder/` with its
`regions.json` beside it. Two things the workflow's header comment explains in full
and are worth knowing before changing anything there: every URL in the artifact is
**relative** (`trunk --public-url ./`, Vite `base: "./"`) so the site can move to a
custom domain without a rebuild — the deploy re-proves that each run by serving the
artifact from a deep sub-path; and Pages **cannot set response headers**, which is
why the catalog manifest is published with the artifacts rather than beside the site
(a manifest here could only change by redeploying the site) and why cross-origin
isolation — hence threaded wasm — is unavailable on this host.

The desktop host has no back end yet (D1, #906).

### Driving the device step without a device

The USB writes (map, route, firmware) need an OBC on the other end of a cable.
The device half ships in every firmware build (#889, `obc-fw-nrf54l/src/usb/`)
and is verified on hardware. `dev-harness/` is a second
entry point that mounts the whole app against the **simulated device** —
`lib/usb/loopback.ts`, the real protocol over an in-memory pipe, paced to the SD
card's measured **~0.5 MB/s write** ceiling so progress, throughput and the
remaining-time estimate behave the way they do on hardware. (Reads are faster;
uploads are write-bound — `sd_bench`'s `wr-*` shapes are the source of truth.) It lives outside `src/` because no build has it as
an input, which is what keeps the simulated device out of every shipped bundle.

```sh
cd builder/app
VITE_DATA_BASE=/data npm run dev -- --mode web   # then open /dev-harness/
```

`VITE_DATA_BASE` is root-relative here because the harness is served from a
sub-path, and the default `./data` would resolve under it.

All of them — and `npm run check` and `npm test` — need the three wasm bridges
built once first (`npm run build:wasm`, which wants a Rust toolchain and
`wasm-pack`): route conversion, the preset previews and cell assembly all run
client-side through the project's own code, so the TypeScript imports bindings
that don't exist until they're built. See
[`firmware/README.md`](firmware/README.md#build-the-web-builders-wasm-bridges-obc-web-convert-obc-web-preview-obc-web-assemble).

---

## Viewing & simulating

`obc-sim` renders `.obcm` maps through the exact code path the firmware runs.
Maps must be **v5**.

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
a screen stack (Home, Map with a pan mode, Menu, Route menu, Ride control,
Statistics) driven by the four-button input model, plus a control panel for
feeding it a simulated GPS.

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

The full app runs on the desktop simulator **and on the nRF54L15-DK** today: the
shared stack (`obc-map-scene`, `obc-reader`, `obc-route`, `obc-render`, `obc-app`) runs `no_std`
on the device, streaming maps/routes from a microSD card and driving the panel
over SPI.

**Working now:** OBCM v5 packing (CLI + web builder); the shared LOD-pyramid
renderer (quadtree query, polygon fill with holes, weighted lines, z-ordering,
RGB565 → RGB222 quantization); the on-device UI (screen stack + the four-button
input); route loading with live map-matching, ride logging, and ride saving; the
nRF54L firmware booting into the full load → ride → save loop on the DK (see
[`firmware/obc-fw-nrf54l`](firmware/obc-fw-nrf54l)); and the reflective
**LS021B7DD02** panel driver, its waveform backend running on the nRF54L's FLPR
coprocessor with partial / dirty-row updates.

**Next:** Settings / Shutdown screens and a Ride-control rework; and richer line
styling (dashed / two-color lines — a future OBCM v6 — and road casing).

> The packer was originally a Python pipeline; it has been ported to Rust
> (`host/obc-pack`) and the Python pipeline removed. The port's design notes
> live in `packer/docs/`.

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
