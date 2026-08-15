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
