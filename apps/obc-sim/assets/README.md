# Simulator-owned assets

This directory contains the shipped web-demo map and reviewable renderer
goldens. It is not the developer-fixture store.

- `grimsel-demo.obcm` is a shipped wasm demo payload.
- `weather-icons/` contains renderer goldens pinned by tests.

Shared authored tracks/routes live under `fixtures/sources/`; keeping them
there prevents firmware, host, iOS, and app tests from reaching into one
another's asset directories.

Large maps, terrain, captured provider data, and coherent runnable scenarios
are declared in [`../../../fixtures/catalog.toml`](../../../fixtures/catalog.toml)
and documented in [`../../../fixtures/README.md`](../../../fixtures/README.md).
Use `obc fixtures list` and `obc sim SCENARIO`; do not add generated review
screenshots or realistic binary fixtures here.

## Repacked at OBCM v14 (FS7.5b, #1420)

`grimsel-demo.obcm` is now a **v14** file — a v14 reader refuses a v13 one
outright, so the landing demo would not have mounted otherwise, and no test
catches that (`apps/obc-web-demo` only `include_bytes!`s the payload, so the
failure would have surfaced in a browser).

Regenerated through the sanctioned path, `fixtures/build-map-package.sh
grimsel-demo`, with the current Geofabrik Switzerland snapshot
(**`2026-08-17`**) — the pinned `2026-08-08` one is no longer hosted. The demo
bbox is unchanged and canonical; content moves with the snapshot as it does on
any deliberate refresh.

Size: 664 064 B → **689 664 B** (+3.9%), of which v14's §1.2 unit filler is the
larger part — roughly half a percent of the geometry chunk bytes plus a gap at
each of the file's ~50 region boundaries — and a fortnight of OSM edits the
rest.
