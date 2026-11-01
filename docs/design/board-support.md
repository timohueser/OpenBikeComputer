---
system: board-support
kind: system
area: Device
purpose: "Drives the nRF54L hardware: radio, USB, microSD, display panel, sensors, buzzer."
owns:
  - firmware/obc-fw-nrf54l/vendor/**
  - firmware/obc-fw-nrf54l/src/ble/**
  - firmware/obc-fw-nrf54l/src/usb/**
  - firmware/obc-fw-nrf54l/src/flpr/**
  - firmware/obc-fw-nrf54l/src/semmc.rs
  - firmware/obc-fw-nrf54l/src/ls021_flpr.rs
  - firmware/obc-fw-nrf54l/src/com.rs
  - firmware/obc-fw-nrf54l/src/sensors.rs
  - firmware/obc-fw-nrf54l/src/buzzer.rs
  - firmware/obc-fw-nrf54l/src/panel_power.rs
  - firmware/obc-fw-nrf54l/src/flpr_mux.rs
  - firmware/obc-fw-nrf54l/src/card_io.rs
  - firmware/obc-fw-nrf54l/src/board.rs
depends_on: []
test_depends_on: []
status: draft
---

# Board support

The body of this document is not written yet.
