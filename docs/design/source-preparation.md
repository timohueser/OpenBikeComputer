---
system: source-preparation
kind: system
area: Data and services
purpose: "Turns raw external datasets into the prepared sources that products build from."
owns:
  - host/obc-dem/**
  - host/obc-osm/**
  - tools/forest_reference.py
  - tools/clms.py
  - tools/region_osm.py
  - host/obc-data-steps/src/forest_reference*
  - host/obc-data-steps/src/climate_reference*
  - host/obc-data-steps/src/terrain_reference*
  - host/obc-data-steps/src/terrain_bias*
  - host/obc-data-steps/src/region_sources*
  - host/obc-data-steps/src/region_sources/**
  - host/obc-data-steps/src/prepared_tiles*
depends_on:
  - data-engine
  - foundations
test_depends_on:
  - repository-tooling
status: draft
---

# Source preparation

The body of this document is not written yet.
