---
system: search
kind: system
area: Data and services
purpose: "Finds places by name."
owns:
  - planner/search/**
  - host/obc-search-bake/**
  - tools/planner_grid_search.py
  - tools/planner_places.py
depends_on:
  - content
  - data-engine
test_depends_on:
  - device-map-baking
status: draft
---

# Search

Phase 2 of the cairn plan writes the body of this document.
