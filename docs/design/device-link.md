---
system: device-link
kind: system
area: Device
purpose: "Moves objects between the device and a phone or computer."
owns:
  - firmware/obc-link/**
  - firmware/obc-ble/**
  - builder/usb/**
  - sim/flat-device/**
  - host/obc-vectors/**
  - builder/web/src/lib/usb/**
  - builder/web/src/lib/device/**
  - companion-ios/Packages/OBCKit/Sources/OBCTransport/**
  - companion-ios/Packages/OBCKit/Sources/OBCProtocolV4/**
  - companion-ios/Packages/OBCKit/Tests/OBCTransportTests/**
  - companion-ios/Packages/OBCKit/Tests/OBCProtocolV4Tests/**
  - firmware/obc-app/src/ble*
depends_on:
  - firmware-update
  - foundations
  - navigation
test_depends_on:
  - map-reading
status: draft
---

# Device link

The body of this document is not written yet.
