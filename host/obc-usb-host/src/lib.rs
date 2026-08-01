//! The bench host's protocol half — the parts that are *not* a window.
//!
//! `obc-usb-host` started as a GPX feeder over the debug UART (issue #38) and its binary still is
//! one. This library is the other thing a host has to be able to do to a device, and the reason it
//! is a library rather than more of the binary: the map builder's browser and desktop tiers already
//! own the byte pipes (`builder/app/src/lib/usb/` over WebUSB, `apps/obc-desktop/src/usb/` over
//! nusb), and #894's whole architecture is **one protocol implementation, not two drifting ones**.
//! What was missing is not another cable — it is the *rules* for putting a volume set on a device,
//! which are the same rules whichever cable carries them.
//!
//! So [`set_transfer`] is transport-generic on purpose: a plan a caller can verify without a device
//! attached, and a driver over a one-method [`set_transfer::SetLink`] trait. A real link implements
//! that trait; so does the device's own receive logic in a test, which is what makes the round trip
//! in `tests/volume_set.rs` possible with no hardware at all.

pub mod set_transfer;
