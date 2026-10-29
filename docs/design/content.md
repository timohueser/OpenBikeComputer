---
system: content
kind: system
area: Data and services
purpose: "Collects facts about places (names, articles, photos) for the device and the planner."
owns:
  - host/obc-places/**
  - tools/wiki/**
  - tools/landmark_capture.py
  - host/obc-data-steps/src/wiki*
  - host/obc-data-steps/src/wiki/**
depends_on:
  - foundations
  - rendering-display
test_depends_on: []
status: draft
---

# Content

Phase 2 of the cairn plan writes the body of this document.
