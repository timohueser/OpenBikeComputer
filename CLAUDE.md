We are building an open source bikepacking GPS computer. The brain of the OpenBikeComputer is the **NRF54LM20**.  


## Coding Preferences
The goal for this codebase is to make this a robust and extendable open source mono-repo, with modern coding practices and clear architectural seams and boundaries. We put a great deal of thought and effort into building our features and modules in ways that makes them easy to understand and reason about.

- **Keep things simple, adhere to YAGNI and DRY principles** This is a large codebase and we don't want it to grow uncontrollably. So make sure large LOC additions you make are well justified. If you think an architectural choice we made earlier forces you to write "patchworky" code or use a lot of workarounds do flag this with me and don't be afraid to suggest a larger restructuring.
- Make sure individual files do not grow into huge Monoliths, instead split them into more digestible units if possible. 
- **Feel free to propose any ideas you have to me**, but never go off and implement features, or additions to features without explicit consent. Only build what we agree upon, or what is written in the issue you're following.
- Test are good! But avoid endless smoke thests and too many "regression tests". Write focused, high quality tests, never add tests just for the sake of adding them.
- **Use comments to clearly describe how a function or class is supposed to be used, but keep them concise**. Keep revision history and references to PRs, issues etc. out of the comments. They shoul n only ever describe the current state of the codebase, never give a history lesson or justify why changes were made.
- Make sure that comments stay up to date and remove/rewrite stale comments if you stumble across them

## Pull Requests and issues
- Keep all PRs and issues simple and concise, use **ASD-STE100 Simplified Technical English**.
- **Open real PRs, never drafts**.
- Open the PRs description with a simple explanation of the problem this PR solves, or the feature it implements. 
- Do not over-explain your solution in the PRs body, stick to the most important information, highlight potential pitfalls or areas where you diverged from the plan/issue.

## Codebase Layout

- The Rust is in **three trees**, one cargo workspace rooted at `Cargo.toml` (so
  one `Cargo.lock`, one `target/`). The rule is mechanical: a crate is in
  `firmware/` if the device image reaches it through *normal* deps.
  - `firmware/` — the shared `no_std` render path and the device: the
    dependency-free foundations (`obc-crc`, `obc-formats`, `obc-ports`,
    `obc-map-scene`), the render path (obc-reader → obc-elevation → obc-route → obc-render →
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
    `obc-wx-pack`, which freezes a real past storm into a
    replayable event pack in the fixture registry and its
    counterpart `obc-wx-client` (manifest + OBCG corridor Range reads + MET → one
    OBCW bundle — the Rust twin of the phone's client, driving `--weather live`).
  - `apps/` — the shells: `obc-sim`, `obc-web-demo`, `obc-web-convert`,
    `obc-web-assemble`, `obc-skin-preview`, `obc-desktop`.

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
  job) Style documents live in `builder/presets/`: **one** `schema.json` (the complete,
  CLI-usable packer config everything is baked with) plus `skins/<id>.json`
  (presentation only, stamped onto an assembled map).
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
- Use the test levels, fast hermetic tier, suite-granularity rule, and exception language from
[`docs/testing.md`](docs/testing.md). Run `obc suites check` when test sources, validation commands,
workflows, registries, or test policy change.

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
