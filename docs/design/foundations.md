---
system: foundations
kind: system
area: Device
purpose: "Holds the policy-free building blocks that every crate uses: byte codecs, CRC, host interfaces, the map-scene contract."
owns:
  - firmware/obc-formats/**
  - firmware/obc-crc/**
  - firmware/obc-ports/**
  - firmware/obc-map-scene/**
  - host/obc-file-source/**
depends_on: []
test_depends_on:
  - map-reading
status: draft
---

# Foundations

Phase 2 of the cairn plan writes the body of this document.
