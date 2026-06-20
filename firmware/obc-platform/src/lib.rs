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
//! - [`framebuffer`] — a [`DrawTarget`](embedded_graphics::draw_target::DrawTarget)
//!   over a raw `&mut [u16]` RGB565 buffer (the LTDC-scanned SDRAM framebuffer).
//! - [`button_input`] — a [`ButtonInput`] debouncer over four
//!   [`InputPin`](embedded_hal::digital::InputPin)s, feeding the shared gesture
//!   recognizer through [`InputSource`](obc_app::InputSource).

#![no_std]

pub mod button_input;
pub mod framebuffer;

pub use button_input::{ButtonInput, Timing};
pub use framebuffer::Framebuffer565;
