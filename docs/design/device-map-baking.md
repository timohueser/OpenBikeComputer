---
system: device-map-baking
kind: system
area: Data and services
purpose: "Turns prepared sources into the device's map cells."
owns:
  - host/obc-bake/**
  - host/obc-pack/**
  - host/obc-draw/**
  - host/obc-network/**
  - host/obc-map-core/**
  - host/obc-pbf/**
  - host/obc-tree-walk/**
  - host/obcm-assemble/**
  - host/obc-data-steps/src/device*
  - host/obc-data-steps/src/device/**
  - tools/basemap_tool.py
depends_on:
  - data-engine
  - foundations
  - rendering-display
test_depends_on:
  - device-link
  - device-runtime
  - host-runtime
  - navigation
  - repository-tooling
status: draft
---

# Device map baking

Phase 2 of the cairn plan writes the body of this document.
