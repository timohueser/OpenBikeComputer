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

The body of this document is not written yet.
