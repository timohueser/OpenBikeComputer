---
system: rendering-display
kind: system
area: Device
purpose: "Turns a map scene into pixels on the panel."
owns:
  - firmware/obc-render/**
  - firmware/obc-display/**
depends_on:
  - foundations
test_depends_on:
  - map-reading
status: draft
---

# Rendering & display

Phase 2 of the cairn plan writes the body of this document.
