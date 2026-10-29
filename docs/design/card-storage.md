---
system: card-storage
kind: system
area: Device
purpose: "Keeps the device's objects on the SD card."
owns:
  - firmware/obc-storage/**
  - firmware/obc-app/src/catalog_state*
  - firmware/obc-app/src/catalog_state/**
  - firmware/obc-app/src/trip.rs
  - firmware/obc-app/src/ride.rs
  - firmware/obc-app/src/card_scheduler*
  - firmware/obc-app/src/card_scheduler/**
  - firmware/obc-app/src/map_catalog*
depends_on:
  - foundations
test_depends_on:
  - map-reading
  - navigation
status: draft
---

# Card storage

Phase 2 of the cairn plan writes the body of this document.
