# OpenBikeComputer — notes for Claude

Open-source bikepacking GPS computer: an **nRF54L** MCU driving a reflective
Sharp memory-LCD (LS021B7DD02). The nRF54L is THE target (an STM32F429 was the
original bring-up bridge, now removed) — keep the *shared* crates board-agnostic
and the simulator + tests first-class.

## Layout

- `firmware/` — all Rust. Shared `no_std` render path, hosts at the edges:
  obc-reader → obc-route → obc-render → obc-app → hosts (obc-sim, the
  obc-fw-nrf54l board crate). `obc-pack` is the std-host map packer (OSM `.osm.pbf`
  → `.obcm`); it also owns the config's JSON Schema (`obc-pack schema` — a config
  parser change must extend `schema/config.schema.json` + the `schema_*` pinning
  tests, or the web builder's editor lies). Per-crate roles + build/run:
  [firmware/README.md](firmware/README.md).
- `docs/` — the public docs site (below), published at
  <https://timohueser.github.io/OpenBikeComputer/>: it's the **conceptual**
  reference (architecture, formats, rendering, UI, display protocol). `docs/
  index.html` is the marketing landing, `docs/content/` the source. `packer/` —
  the web builder (FastAPI `web_builder/` + Svelte `web_builder/frontend/`,
  built into `static/dist/` — gitignored, `npm run build`; CI runs the `web`
  job) and the style presets in `packer/presets/` (each a complete, CLI-usable
  packer config). The user's working config lives in the browser, not on disk.

Division of labor: **concepts** live in the docs site; **build / run / flash**
specifics live in the READMEs (root + `firmware/` + the board crate). Keep each
where it belongs — don't re-explain the architecture in a README.
- `OBCM_Spec.md` / `OBCR_Spec.md` / `OBCU_Spec.md` (repo root) — the normative
  byte-level format specs (map / route / firmware-update image); the docs'
  data-formats + firmware-updates pages are the readable tours and link to them.
  DFU crates: `obc-dfu` (shared `no_std` container + boot-state codec + install
  engine/armer), `obc-mkimage` (host tool: `UPDATE.BIN` wrap/inspect), `obc-boot`
  (the 32 KB bootloader — see Build & verify).

## Build & verify

- Host crates + sim: `cargo build --release` and `cargo test` from `firmware/`.
- The `firmware/` workspace **excludes** the board crate `obc-fw-nrf54l` and the
  bootloader `obc-boot` — both standalone (own target + `.cargo/config.toml`), so
  workspace `cargo test`/`build` does **not** touch them. Build each on its own.
  nRF specifics + on-glass gotchas:
  [firmware/obc-fw-nrf54l/README.md](firmware/obc-fw-nrf54l/README.md); the
  boot-chain layout + flash-once workflow:
  [firmware/obc-boot/README.md](firmware/obc-boot/README.md).
- `cargo fmt` is a **three-step**: `cargo fmt --all` for the workspace **plus** a
  separate `cargo fmt` inside each excluded crate (board crate + `obc-boot`), or
  the fmt CI guard fails. (rustfmt config is committed — let it do style; don't
  hand-format.)
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
