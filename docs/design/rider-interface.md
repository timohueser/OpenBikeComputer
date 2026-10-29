---
system: rider-interface
kind: system
area: Device
purpose: "Connects the rider to the device: screens, gestures, sound."
owns:
  - firmware/obc-app/src/screen/**
  - firmware/obc-app/src/screen.rs
  - firmware/obc-app/src/ui_runtime*
  - firmware/obc-app/src/input_plane*
  - firmware/obc-app/src/input.rs
  - firmware/obc-app/src/input/**
  - firmware/obc-app/src/dirty*
  - firmware/obc-app/src/render_key*
  - firmware/obc-app/src/hold_hint*
  - firmware/obc-app/src/map_icons*
  - firmware/obc-app/src/stat_fields*
  - firmware/obc-app/src/stat_fields/**
  - firmware/obc-app/src/i18n*
  - firmware/obc-app/src/i18n/**
  - firmware/obc-app/src/cues*
  - firmware/obc-app/src/distress*
  - firmware/obc-app/src/alert*
  - firmware/obc-app/src/fault*
  - firmware/obc-app/src/device_status*
  - firmware/tools/ui_frames/**
  - firmware/tools/tests/test_ui_frames.py
depends_on: []
test_depends_on: []
status: draft
---

# Rider interface

Phase 2 of the cairn plan writes the body of this document.
