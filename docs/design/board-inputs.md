---
system: board-inputs
kind: system
area: Device
purpose: "Turns raw sensor bytes and button edges into the values the core reads."
owns:
  - firmware/obc-sensors/**
  - firmware/obc-platform/**
  - firmware/obc-sensor-sim/**
  - firmware/obc-app/src/sensors*
depends_on:
  - foundations
test_depends_on:
  - device-link
status: draft
---

# Board inputs

Phase 2 of the cairn plan writes the body of this document.
