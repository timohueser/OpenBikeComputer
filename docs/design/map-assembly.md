---
system: map-assembly
kind: system
area: Apps
purpose: "Assembles a device map from published cells."
owns:
  - builder/wasm/**
  - builder/tests/**
  - builder/web/src/lib/catalog/**
  - builder/web/src/lib/cells/**
  - builder/web/src/lib/assemble/**
  - builder/web/src/lib/convert/**
  - builder/web/src/lib/coverage/**
  - builder/web/src/lib/map/**
  - builder/web/src/components/coverage/**
depends_on:
  - card-storage
  - device-link
  - device-map-baking
  - foundations
  - host-runtime
  - map-reading
  - navigation
  - rendering-display
test_depends_on:
  - content
status: draft
---

# Map assembly

Phase 2 of the cairn plan writes the body of this document.
