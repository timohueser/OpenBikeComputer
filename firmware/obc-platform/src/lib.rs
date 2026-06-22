//! Board-agnostic firmware glue between the shared [`obc_app`](../obc_app) and a
//! concrete board crate (`obc-fw-stm32f429`, a future `obc-fw-nrf54l`).
//!
//! `no_std`, over `embedded-hal` / `embedded-graphics`, so the board crates stay
//! thin (clocks, concrete pins, the main loop) and everything reusable lives here:
//! written once, ported to the next board by re-pointing the pins. Today that is the
//! framebuffer `DrawTarget`s, the button debouncer, the FatFs `ByteSource`/`Sink`
//! adapters (issue #36), and the USB-CDC debug-sensor protocol (issue #38, behind
//! `debug-usb`).
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
//!
//! ## Two-plane architecture — input/overlay vs. map (issue #48)
//!
//! Each board's main loop should run the device on **two planes across two executors**, so
//! input + the overlay stay responsive *while a map frame renders*. `render_map` is a
//! CPU-bound call (24–51 ms on the F429 prototype) that never `.await`s, so it blocks
//! whatever executor it runs on; dirty-tracking (issue #47) cuts how *often* it runs but not
//! the during-render case (panning re-renders rapidly while a button is held). The fix is
//! preemption:
//!
//! - **High-priority plane** — an embassy **`InterruptExecutor`** pended from an unused
//!   interrupt vector, at a priority *above* thread mode but *below* the embassy-time driver
//!   (so its `Timer`s still wake mid-render). It owns the [`ButtonInput`] debouncer, the
//!   shared [`InputPlane`](obc_app::InputPlane) (gesture recogniser + hold-hint overlay), and
//!   the **overlay** framebuffer/layer. Every few ms it *preempts the map render*, samples the
//!   buttons, recognises gestures — pushing each into a channel — and animates + repaints the
//!   hold bulge. Press-to-feedback latency and the auto-repeat cadence stay bounded regardless
//!   of map-render time.
//! - **Low-priority plane** — the thread-mode executor running the [`App`](obc_app::App): the
//!   screen stack, the camera, sensors, SD, and the **map** render. Each loop it drains the
//!   gesture channel → [`App::apply_gesture`](obc_app::App::apply_gesture),
//!   [`App::advance_animations`](obc_app::App::advance_animations) for timed screen content,
//!   polls sensors → [`App::tick`](obc_app::App::tick), and re-renders the map when
//!   [`Dirty::map`](obc_app::Dirty) — never the overlay (the high-priority plane owns it).
//!
//! The only shared state is a lock-free [`embassy_sync`](https://docs.rs/embassy-sync)
//! `Channel<Gesture>` plus the two **disjoint** framebuffers — so the long map render holds
//! no lock against the input plane. UX: the bulge confirms a press *instantly* on the overlay
//! layer; the screen transition lands a frame later when the map plane drains the channel.
//! The one piece of shared hardware (the LTDC's single vblank-reload bit, written by both
//! planes' framebuffer flips) is guarded by a short critical section in the board's flip
//! helper. The board's `main` may also offer a `single-executor` fallback that drives both
//! planes inline through [`App::handle_input`](obc_app::App::handle_input) — proving the seam
//! composes — but the preemptive split is the shipping default and the structure the future
//! `obc-fw-nrf54l` adopts unchanged (the nRF supports the identical embassy
//! `InterruptExecutor` pattern). The concrete F429 wiring lives in `obc-fw-stm32f429`'s `main`.

#![no_std]

pub mod button_input;
// USB-CDC fake-sensor protocol + sources + telemetry (issue #38). Behind `debug-usb` so the
// host workspace build never pulls embassy-sync; the board crate enables it and owns the actual
// embassy-usb CDC driver. The protocol + sources move to the nRF54L unchanged.
#[cfg(feature = "debug-usb")]
pub mod debug_usb;
pub mod framebuffer;
pub mod sd;

pub use button_input::{ButtonInput, Timing};
pub use framebuffer::{Framebuffer565, FramebufferArgb4444};
pub use sd::{SdByteSink, SdByteSource, SdTrackSink};
