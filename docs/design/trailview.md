---
system: trailview
kind: system
area: Data and services
purpose: "Renders a region as an explorable 3-D landscape in the browser."
owns:
  - host/obc-trailview-features/**
  - host/obc-ground-plan/**
  - builder/ground-plan-wasm/**
  - builder/web/src/lib/trailview/**
  - builder/web/src/plan-timing.ts
  - builder/web/plan-timing.html
  - tools/trailview/**
  - tools/planner_trailview_canopy.py
depends_on:
  - data-engine
test_depends_on: []
status: draft
---

# TrailView

The body of this document is not written yet.
