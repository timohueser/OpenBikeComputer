//! OBCM device application layer + hardware-abstraction traits.
//!
//! `no_std` so the **same** application logic runs in the desktop simulator and
//! on the nRF5340 firmware, exactly like [`obcm`]'s reader and renderer already
//! do. This crate adds no allocations of its own; the only heap use is inside the
//! [`MapRenderer`](obcm::MapRenderer) scratch that [`App`] drives (see [`obcm`]),
//! which clears-not-frees each frame. This crate sits one level above the
//! renderer: it owns *what the device is doing* — where the user is, where the
//! camera looks, which mode it's in — and leaves *how pixels reach a screen* to
//! the host.
//!
//! The boundary is a small hardware-abstraction layer (HAL): the app reads the
//! user's position from a [`LocationSource`] and (later) buttons from an
//! [`InputSource`], never caring whether those come from a real GPS chip and
//! GPIO pins (firmware) or from the simulator's control panel and a GPX replay
//! (host). The host injects an implementation; the app stays identical.
//!
//! Modules:
//! - [`hal`] — the injected-hardware traits ([`LocationSource`], [`InputSource`])
//!   and their data types ([`Fix`], [`Button`], [`ButtonEvent`]).
//! - [`app`] — [`AppState`]: the camera + mode + last-known-fix, advanced one
//!   tick at a time from the HAL and projected into an [`obcm::Viewport`].

#![no_std]

pub mod app;
pub mod hal;

pub use app::{App, AppState, CameraMode};
pub use hal::{Button, ButtonEvent, Fix, InputSource, LocationSource};
