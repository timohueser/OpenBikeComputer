# OpenBikeComputer — notes for Claude

Open-source bikepacking GPS computer: an nRF54L MCU driving a reflective
memory-LCD, developed firmware-first on an STM32F429 prototype and a desktop/wasm
simulator.

## Keep the docs in sync with the code

The public docs are markdown in `docs/content/**.md`, rendered to HTML by
`docs/build_docs.py` (a field-guide theme with hand-built SVG diagrams). They cover
the rendering pipeline, system architecture, data formats, the UI system, and
packer & routing — at a high bar (concepts + diagrams + a few well-chosen snippets,
not API dumps).

**On every PR you open or edit:** briefly check whether the change touches anything
the docs describe — a rendering algorithm or optimization, a binary format / byte
layout, a UI or navigation behavior, a packer stage, or a cross-page link / `src:`
target. If it does, **tell me what's now stale** and land the fix as a **separate
`docs:` commit** in the same PR, so it reviews on its own. If nothing doc-relevant
changed, say so in one line. Don't let the docs silently drift.

When editing docs: read the actual source (the `.md` and spec files can lag the
code), rebuild with `python3 docs/build_docs.py`, and verify every cross-page
`../page/#anchor` resolves to a real heading id.

## Layout

- `firmware/` — all Rust. `no_std` crates share one render path:
  obc-reader → obc-route → obc-render → obc-app → hosts (obc-sim,
  obc-fw-stm32f429). `obc-pack` is the std-host map packer (OSM `.osm.pbf` →
  `.obcm`). `cargo fmt` is safe (rustfmt config is committed).
- `docs/` — the public docs site (above): `docs/index.html` is the marketing
  landing, `docs/content/` the docs source. `packer/` — the `web_builder` UI that
  drives `obc-pack` (the former Python packer is gone).
- `OBCM_Spec.md` / `OBCR_Spec.md` (repo root) — the normative byte-level format
  specs; the docs' data-formats page is the readable tour and links to them. Keep
  both current.
