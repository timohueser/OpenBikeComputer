//! Board-agnostic firmware glue between the shared [`obc_app`](../obc_app) and a
//! concrete board crate (`obc-fw-stm32f429`, a future `obc-fw-nrf54l`).
//!
//! `no_std`, over `embedded-hal` / `embedded-graphics`, so the board crates stay
//! thin (clocks, concrete pins, the main loop) and everything reusable lives here:
//! written once, ported to the next board by re-pointing the pins. The epic
//! (issue #32) grows this into FatFs `ByteSource`/`Sink` adapters, the input
//! debouncer and the USB-CDC debug protocol; for now it is just the framebuffer
//! `DrawTarget`.
//!
//! Modules:
//! - [`framebuffer`] — a [`DrawTarget`](embedded_graphics::draw_target::DrawTarget)
//!   over a raw `&mut [u16]` RGB565 buffer (the LTDC-scanned SDRAM framebuffer).

#![no_std]

pub mod framebuffer;

pub use framebuffer::Framebuffer565;
