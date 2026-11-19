---
system: data-engine
kind: system
area: Data and services
purpose: "Runs any product's steps: it fetches, stores, versions and publishes, with no knowledge of maps."
owns:
  - host/obc-data/**
  - tools/step_pool.py
  - tools/step_request.py
  - tools/data_registry.py
  - tools/check_data_sources.py
  - tools/check_obc_data_licence.py
depends_on: []
test_depends_on: []
status: draft
---

# Data engine

The body of this document is not written yet. Only the PMTiles reader is described.

## The PMTiles reader

Rust steps of other products read tiles of PMTiles v3 archives that earlier steps wrote: the
TrailView features read the terrain grid, and the route facts read the canopy archive.
[`pmtiles.rs`](../../host/obc-data/src/pmtiles.rs) is their one reader. It reads the root
directory when it opens a file, and a leaf directory and a tile only when a caller asks for the
tile. It is in this crate because every product crate already depends on it. The writer stays
with the TrailView features, its only user.
