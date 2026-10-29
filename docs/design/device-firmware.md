---
system: device-firmware
kind: shell
area: Shells
purpose: "Builds the nRF54L board image from the device systems."
owns:
  - firmware/obc-fw-nrf54l/**
depends_on:
  - board-inputs
  - card-storage
  - device-link
  - device-runtime
  - firmware-update
  - foundations
  - map-reading
  - navigation
  - rendering-display
test_depends_on: []
status: draft
---

# Device firmware

Phase 2 of the cairn plan writes the body of this document.
