---
system: host-runtime
kind: system
area: Apps
purpose: "Runs the device core on a computer or phone instead of the board."
owns:
  - sim/host-core/**
  - sim/replay/**
depends_on:
  - card-storage
  - device-runtime
  - foundations
  - map-reading
  - navigation
  - rendering-display
test_depends_on:
  - content
  - device-link
  - device-map-baking
  - repository-tooling
status: draft
---

# Host runtime

Phase 2 of the cairn plan writes the body of this document.
