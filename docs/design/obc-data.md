---
system: obc-data
kind: shell
purpose: "Runs the `obc data` command and its operator page."
owns:
  - host/obc-data-steps/**
  - builder/web/src/data.ts
depends_on:
  - data-engine
  - device-map-baking
  - foundations
  - routing
  - search
  - source-preparation
  - trailview
test_depends_on:
  - map-reading
  - repository-tooling
status: draft
---

# obc data

The body of this document is not written yet.
