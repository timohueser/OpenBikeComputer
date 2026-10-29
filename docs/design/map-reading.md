---
system: map-reading
kind: system
area: Device
purpose: "Reads the device's map content from the card: OBCM maps, OBCT terrain."
owns:
  - firmware/obc-reader/**
  - firmware/obc-elevation/**
  - host/obcm-testkit/**
depends_on:
  - foundations
test_depends_on:
  - rendering-display
  - repository-tooling
status: draft
---

# Map reading

The body of this document is not written yet.
