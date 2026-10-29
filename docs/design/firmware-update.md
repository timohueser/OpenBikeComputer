---
system: firmware-update
kind: system
area: Device
purpose: "Installs a verified firmware image on the device, with rollback."
owns:
  - firmware/obc-dfu/**
  - firmware/obc-boot/**
  - host/obc-mkimage/**
  - firmware/obc-app/src/dfu*
  - builder/web/src/lib/firmware/**
depends_on:
  - foundations
test_depends_on:
  - map-reading
status: draft
---

# Firmware update

Phase 2 of the cairn plan writes the body of this document.
