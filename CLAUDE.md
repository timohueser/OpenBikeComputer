# OpenBikeComputer — notes for Claude

Open-source bikepacking GPS computer: an **nRF54L** MCU driving a reflective
Sharp memory-LCD (LS021B7DD02). The nRF54L is THE target; the STM32F429 was a
bring-up bridge and is **not** a CI gate — optimize freely for nRF even if it
breaks the STM32 crate, but keep the *shared* crates board-agnostic and the
simulator + tests first-class.

## Layout

- `firmware/` — all Rust. Shared `no_std` render path, hosts at the edges:
  obc-reader → obc-route → obc-render → obc-app → hosts (obc-sim, the two
  obc-fw-* board crates). `obc-pack` is the std-host map packer (OSM `.osm.pbf`
  → `.obcm`). Per-crate roles + build/run: [firmware/README.md](firmware/README.md).
- `docs/` — the public docs site (below), published at
  <https://timohueser.github.io/OpenBikeComputer/>: it's the **conceptual**
  reference (architecture, formats, rendering, UI, display protocol). `docs/
  index.html` is the marketing landing, `docs/content/` the source. `packer/` —
  the `web_builder` UI that drives `obc-pack` (the former Python packer is gone).

Division of labor: **concepts** live in the docs site; **build / run / flash**
specifics live in the READMEs (root + `firmware/` + the board crate). Keep each
where it belongs — don't re-explain the architecture in a README.
- `OBCM_Spec.md` / `OBCR_Spec.md` (repo root) — the normative byte-level format
  specs; the docs' data-formats page is the readable tour and links to them.

## Build & verify

- Host crates + sim: `cargo build --release` and `cargo test` from `firmware/`.
- The `firmware/` workspace **excludes** the board crates `obc-fw-stm32f429`
  and `obc-fw-nrf54l` — they're standalone (own target + `.cargo/config.toml`),
  so workspace `cargo test`/`build` does **not** touch them. Build each on its
  own. nRF specifics + on-glass gotchas: [firmware/obc-fw-nrf54l/README.md](firmware/obc-fw-nrf54l/README.md).
- `cargo fmt` is a **two-step**: `cargo fmt --all` for the workspace **plus** a
  separate `cargo fmt` inside *each* excluded board crate, or the fmt CI guard
  fails. (rustfmt config is committed — let it do style; don't hand-format.)
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
