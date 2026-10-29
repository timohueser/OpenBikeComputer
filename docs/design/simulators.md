---
system: simulators
kind: shell
area: Shells
purpose: "Runs the device core away from the board, for development and demonstration."
owns:
  - sim/desktop/**
  - sim/web-demo/**
  - sim/phone/**
  - sim/feeder/**
  - sim/bench/**
depends_on:
  - board-inputs
  - card-storage
  - data-engine
  - device-link
  - device-runtime
  - firmware-update
  - foundations
  - host-runtime
  - map-reading
  - navigation
  - rendering-display
  - repository-tooling
test_depends_on: []
status: draft
---

# Simulators

Phase 2 of the cairn plan writes the body of this document.
