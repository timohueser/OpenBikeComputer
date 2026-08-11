# OpenBikeComputer — notes for Claude

Open-source bikepacking GPS computer: an **nRF54L** MCU driving a reflective
Sharp memory-LCD (LS021B7DD02). The nRF54L is THE target (an STM32F429 was the
original bring-up bridge, now removed) — keep the *shared* crates board-agnostic
and the simulator + tests first-class.

## Layout

- The Rust is in **three trees**, one cargo workspace rooted at `Cargo.toml` (so
  one `Cargo.lock`, one `target/`). The rule is mechanical: a crate is in
  `firmware/` iff the device image reaches it through *normal* deps.
  - `firmware/` — the shared `no_std` render path and the device: the
    dependency-free foundations (`obc-crc`, `obc-formats`, `obc-ports`,
    `obc-map-scene`),
    the render path (obc-reader → obc-elevation → obc-route → obc-render →
    obc-app), the platform adapters (`obc-platform`, `obc-display`,
    `obc-sensors`, `obc-storage`), `obc-ble`, `obc-dfu`, the `obc-fw-nrf54l`
    board crate and `obc-boot`, plus the allocation-free `obc-weather` OBCW
    reader seam (storage/cache policy comes later).
  - `host/` — tools and oracles, never on the device: `obc-pack` (the std-host
    map packer, OSM `.osm.pbf` → `.obcm`), `obc-bake` (the bakery: the curated
    region list → a catalog tree → published objects), `obc-dem` (the terrain
    baker, GLO-30 → `.obcd`), `obcm-assemble` (the OBCA cell assembler),
    `obc-mkimage`, `obc-bench`, `obcm-testkit`, `obc-vectors`, `obc-host-core`,
    `obc-replay`, `obc-usb-host`, the weather bakery `obc-wx-bake` (upstream
    radar/model products → OBCG frames + manifest; it also ships a second binary
    `obc-wx-pack`, which freezes a real past storm — raw archive bytes, the tree
    the real baker makes of them, and the observed frames that followed — into a
    replayable event pack in the fixture registry, with reviewable manifests under
    `fixtures/sources/weather-events/`) and its
    counterpart `obc-wx-client` (manifest + OBCG corridor Range reads + MET → one
    OBCW bundle — the Rust twin of the phone's client, driving `--weather live`).
  - `apps/` — the shells: `obc-sim`, `obc-web-demo`, `obc-web-convert`,
    `obc-web-assemble`, `obc-skin-preview`, `obc-desktop`.

  Dev-deps deliberately cross the boundary (obc-render → obcm-testkit,
  obc-route → obc-pack); they never touch the `no_std` build. `obc-pack` also
  owns the config's JSON Schema (`obc-pack schema` — a config parser change must
  extend `schema/config.schema.json` + the `schema_*` pinning tests, or the
  builder's editor lies). Per-crate roles + build/run:
  [firmware/README.md](firmware/README.md).
- `docs/` — the public docs site (below), published at
  <https://openbikecomputer.com/>: it's the **conceptual**
  reference (architecture, formats, rendering, UI, display protocol). `docs/
  index.html` is the marketing landing, `docs/content/` the source. The blog
  ("expedition log", `/blog/`) lives in `docs/content/blog/<slug>/` — one folder
  per post, rendered by the same `build_docs.py`; authoring guide + the
  ```compare / ```model directives: [docs/BLOG.md](docs/BLOG.md).
- `builder/` — the map builder: **one** Svelte app (`app/`) with three hosts
  selected at build time by vite's `$host` alias (static web, Tauri desktop,
  and the FastAPI dev server in `server/`; `npm run build`, CI runs the `web`
  job). Nothing here packs anything — all three drive `host/obc-pack`. Style
  documents live in `builder/presets/`: **one** `schema.json` (the complete,
  CLI-usable packer config everything is baked with) plus `skins/<id>.json`
  (presentation only, stamped onto an assembled map — never handed to the
  packer). The user's working config lives in the browser, not on disk.
- `tools/` — the dev scripts: `justfile` (behind `obc <task>`), the GEOS and
  RISC-V installers, shell completion.

- `specs/` — the normative contracts, one directory, referenced from all three
  languages. `OBCM_Spec.md` / `OBCR_Spec.md` / `OBCU_Spec.md` are the normative
  byte-level format specs (map / route / firmware-update image), plus `OBCA` /
  `OBCC` (cells + catalog), `OBCT` (terrain) and the BLE wire contract; the docs'
  data-formats + firmware-updates pages are the readable tours and link to them.
  DFU crates: `obc-dfu` (shared `no_std` container + boot-state codec + install
  engine/armer), `obc-mkimage` (host tool: `UPDATE.BIN` wrap/inspect), `obc-boot`
  (the 32 KB bootloader — see Build & verify).

Division of labor: **concepts** live in the docs site; **build / run / flash**
specifics live in the READMEs (root + `firmware/` + the board crate). Keep each
where it belongs — don't re-explain the architecture in a README.

## Build & verify

- Follow the proportional verification policy in [CONTRIBUTING.md](CONTRIBUTING.md).
  During development and before handoff, run tests and clippy for the packages and
  surfaces actually changed. Do not default to the full workspace suite merely
  because a task is ending; `obc test full` and `obc check full` are explicit
  cross-cutting gates.
- Host crates + sim build from the **repo root** (that's where the workspace is
  rooted; Cargo walks up, so running from a subdirectory works too). Use
  `obc test -p <crate>` for the normal focused loop and `obc test fixtures -p
  <crate>` only when captured external data is part of the behavior.
- The workspace **excludes** three standalone crates, so workspace
  `cargo test`/`build` does **not** touch them — build each on its own: the board
  crate `obc-fw-nrf54l` and the bootloader `obc-boot` (own MCU target +
  `.cargo/config.toml`), and the Tauri desktop app `obc-desktop` (own webview
  toolchain, and it embeds a built frontend). nRF specifics + on-glass gotchas:
  [firmware/obc-fw-nrf54l/README.md](firmware/obc-fw-nrf54l/README.md); the
  boot-chain layout + flash-once workflow:
  [firmware/obc-boot/README.md](firmware/obc-boot/README.md); building and running
  the app: [apps/obc-desktop/README.md](apps/obc-desktop/README.md).
- `cargo fmt` is a **four-step**: `cargo fmt --all` for the workspace **plus** a
  separate `cargo fmt` inside each excluded crate (board crate, `obc-boot`,
  `obc-desktop`), or the fmt CI guard fails. (rustfmt config is committed — let it
  do style; don't hand-format.)
- Required CI check is the `ci` job in `.github/workflows/ci.yml`.

## Keep the docs in sync with the code

The public docs are markdown in `docs/content/**.md`, rendered to HTML by
`docs/build_docs.py` (a field-guide theme with hand-built SVG diagrams). They
cover the rendering pipeline, system architecture, data formats, the UI system,
and packer & routing — at a high bar (concepts + diagrams + a few well-chosen
snippets, not API dumps).

**On every PR you open or edit:** briefly check whether the change touches
anything the docs describe — a rendering algorithm or optimization, a binary
format / byte layout, a UI or navigation behavior, a packer stage, or a
cross-page link / `src:` target. If it does, **tell me what's now stale** and
land the fix as a **separate `docs:` commit** in the same PR. If nothing
doc-relevant changed, say so in one line. Don't let the docs silently drift.

When editing docs: read the actual source (the `.md` and spec files can lag the
code), then rebuild + check cross-page links in one step with
`python3 docs/build_docs.py --check-links` (CI runs the same check).
