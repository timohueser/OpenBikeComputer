---
system: device-runtime
kind: system
area: Device
purpose: "Drives each frame of the device core."
owns:
  - firmware/obc-app/src/app.rs
  - firmware/obc-app/src/app/**
  - firmware/obc-app/src/device_core*
  - firmware/obc-app/src/device_core/**
  - firmware/obc-app/src/arena_gate*
  - firmware/obc-app/src/placement*
  - firmware/obc-app/src/host.rs
  - firmware/obc-app/src/wall_clock*
  - firmware/obc-app/src/settings*
  - firmware/obc-app/src/harness*
  - firmware/obc-app/src/harness/**
  - firmware/obc-app/src/lib.rs
  - firmware/obc-app/src/crc16*
  - firmware/obc-app/tests/**
  - firmware/obc-app/benches/**
  - firmware/obc-app/build.rs
  - firmware/obc-app/examples/**
depends_on:
  - foundations
  - map-reading
  - navigation
  - rendering-display
test_depends_on:
  - data-engine
  - repository-tooling
status: draft
---

# Device runtime

Phase 2 of the cairn plan writes the body of this document.
