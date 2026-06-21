//! Board-agnostic firmware glue between the shared [`obc_app`](../obc_app) and a
//! concrete board crate (`obc-fw-stm32f429`, a future `obc-fw-nrf54l`).
//!
//! `no_std`, over `embedded-hal` / `embedded-graphics`, so the board crates stay
//! thin (clocks, concrete pins, the main loop) and everything reusable lives here:
//! written once, ported to the next board by re-pointing the pins. The epic
//! (issue #32) grows this into FatFs `ByteSource`/`Sink` adapters and the USB-CDC
//! debug protocol; today it is the framebuffer `DrawTarget` and the button debouncer.
//!
//! Modules:
//! - [`framebuffer`] — [`DrawTarget`](embedded_graphics::draw_target::DrawTarget)s
//!   over the LTDC-scanned SDRAM framebuffers: the opaque RGB565 map plane
//!   ([`Framebuffer565`]) and the transparent ARGB4444 overlay plane
//!   ([`FramebufferArgb4444`], the dual-layer display's second layer — issue #46).
//! - [`button_input`] — a [`ButtonInput`] debouncer over four
//!   [`InputPin`](embedded_hal::digital::InputPin)s, feeding the shared gesture
//!   recognizer through [`InputSource`](obc_app::InputSource).
//! - [`sd`] — FatFs [`ByteSource`](obc_route::ByteSource)/[`ByteSink`](obc_route::ByteSink)
//!   and [`TrackSink`](obc_app::TrackSink) adapters over an [`embedded_sdmmc`] SD card, so
//!   maps/routes load and rides save against a real card (issue #36).

#![no_std]

pub mod button_input;
pub mod framebuffer;
pub mod sd;

pub use button_input::{ButtonInput, Timing};
pub use framebuffer::{Framebuffer565, FramebufferArgb4444};
pub use sd::{SdByteSink, SdByteSource, SdTrackSink};
