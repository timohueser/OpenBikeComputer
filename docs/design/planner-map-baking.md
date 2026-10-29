---
system: planner-map-baking
kind: system
area: Data and services
purpose: "Turns prepared sources into the planner's map releases."
owns:
  - tools/planner_assets.py
  - tools/planner_basemap.py
  - tools/planner_climate.py
  - tools/planner_downloads.py
  - tools/planner_geo.py
  - tools/planner_grid.py
  - tools/planner_grid_fonts.py
  - tools/planner_grid_index.py
  - tools/planner_grid_maps.py
  - tools/planner_grid_pack.py
  - tools/planner_map_archive.py
  - tools/planner_mvt.py
  - tools/planner_offline.py
  - tools/planner_runtime.py
  - tools/planner_snow.py
  - tools/planner_verify.py
  - tools/planner_mvt_verify.mjs
  - host/obc-data-steps/src/planner*
  - host/obc-data-steps/src/planner/**
depends_on: []
test_depends_on: []
status: draft
---

# Planner map baking

Phase 2 of the cairn plan writes the body of this document.
