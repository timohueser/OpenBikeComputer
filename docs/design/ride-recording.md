---
system: ride-recording
kind: system
area: Device
purpose: "Records the ride: its track, its measurements, its archive on the card."
owns:
  - firmware/obc-app/src/recorder*
  - firmware/obc-app/src/recorder/**
  - firmware/obc-app/src/metadata*
  - firmware/obc-app/src/upload_facts*
  - firmware/obc-app/src/effort*
  - firmware/obc-app/src/altitude*
  - firmware/obc-app/src/activity*
depends_on: []
test_depends_on: []
status: draft
---

# Ride recording

Phase 2 of the cairn plan writes the body of this document.
