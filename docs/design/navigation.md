---
system: navigation
kind: system
area: Device
purpose: "Guides the rider along a route."
owns:
  - firmware/obc-route/**
  - firmware/obc-app/src/navigator*
  - firmware/obc-app/src/navigator/**
  - firmware/obc-app/src/route.rs
  - firmware/obc-app/src/corridor*
  - firmware/obc-app/src/next_ahead*
  - firmware/obc-app/src/whats_next*
  - firmware/obc-app/src/easier*
  - firmware/obc-app/src/breadcrumb*
depends_on:
  - foundations
  - map-reading
test_depends_on:
  - device-map-baking
  - repository-tooling
status: draft
---

# Navigation

The body of this document is not written yet.
