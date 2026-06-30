//! Board-agnostic firmware glue between the shared [`obc_app`](../obc_app) and a
//! concrete board crate (`obc-fw-nrf54l`).
//!
//! `no_std`, over `embedded-hal` / `embedded-graphics`, so the board crate stays
//! thin (clocks, concrete pins, the main loop) and everything reusable lives here:
//! written once, ported to the next board by re-pointing the pins. Today that is the
//! framebuffer `DrawTarget`s, the button debouncer, the FatFs `ByteSource`/`Sink`
//! adapters (issue #36), and the transport-agnostic debug-sensor protocol (issue #38, behind
//! `debug-link`).
//!
//! Modules:
//! - [`framebuffer`] — the board-owned [`DrawTarget`](embedded_graphics::draw_target::DrawTarget)s
//!   the shared renderer draws into: the nRF's device-native RGB222 map plane ([`FbDevice64`], 1
//!   byte/px — the real target, issue #125) and the [`Framebuffer565`] RGB565 plane the banded
//!   [`Band`] scratch reuses.
//! - [`panel`] — the [`Band`] frame-absolute band/window view + the [`composite_overlay_window`]
//!   overlay helper, for boards that stream a frame to the panel over SPI/DMA a band at a time
//!   (issue #122). The banded present loop itself lives behind each board's `DisplayDriver`.
//! - [`button_input`] — a [`ButtonInput`] debouncer over four
//!   [`InputPin`](embedded_hal::digital::InputPin)s, feeding the shared gesture
//!   recognizer through [`InputSource`](obc_app::InputSource).
//! - [`sd`] — FatFs [`ByteSource`](obc_route::ByteSource)/[`ByteSink`](obc_route::ByteSink)
//!   and [`TrackSink`](obc_app::TrackSink) adapters over an [`embedded_sdmmc`] SD card, so
//!   maps/routes load and rides save against a real card (issue #36).
//! - [`synth`] — [`SynthLocation`], a board-agnostic synthetic moving
//!   [`LocationSource`](obc_app::LocationSource) — the `debug-link`-off fallback fake GPS that
//!   walks a slow square loop (always compiled, *not* behind `debug-link`, since it *is* the
//!   debug-link-off path).
//! - [`fuel`] — [`StubFuelGauge`], a fixed-level [`FuelGauge`](obc_app::FuelGauge) stand-in
//!   until the nPM1300 PMIC fuel gauge is wired in.
//!
//! ## Two-plane architecture — input/overlay vs. map (issue #48)
//!
//! Each board's main loop should run the device on **two planes across two executors**, so
//! input + the overlay stay responsive *while a map frame renders*. `render_map` is a
//! CPU-bound call (tens of ms) that never `.await`s, so it blocks
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
//! Whatever display resource the two planes genuinely share (the framebuffer / its transport on
//! a banded board, or a frame-flip register on a scan-out board) is guarded by a short critical
//! section in the board's present helper. The board's `main` may also offer a `single-executor`
//! fallback that drives both
//! planes inline through [`App::handle_input`](obc_app::App::handle_input) — proving the seam
//! composes — but the preemptive split is the shipping default. The concrete board wiring
//! lives in `obc-fw-nrf54l`'s `main`, which uses the embassy `InterruptExecutor` pattern.

#![no_std]

pub mod button_input;
// Transport-agnostic fake-sensor protocol + sources + telemetry (issue #38). The **pure codec**
// (line parser, `Telemetry`/fix encoders, `LineReader`) is always compiled so the host feeder
// reuses one canonical wire format; only the embassy-sync `Signal`/`Channel` plumbing + HAL-trait
// sources are gated *inside* the module behind `debug-link`, so the host workspace build never pulls
// embassy-sync. The board crate enables the feature and owns the actual transport driver (UART/VCOM
// on the nRF54L). The protocol + sources move to any board unchanged.
pub mod debug_link;
pub mod framebuffer;
// The LS021B7DD02 source-bus wire pack (issue #154) — the host-tested RGB222 → FLPR-wire transform
// the nRF's FLPR backend drains, the sibling of `framebuffer::device64_to_rgb565`. Pure integer
// math, so it always compiles and its unit tests run in the host workspace.
pub mod ls021_wire;
// The [`Band`] frame-absolute band/window view + the `composite_overlay_window` overlay helper
// (issue #122). The boards that ship (nRF54L and beyond) have no hardware scan-out, so they push a
// frame band-by-band over SPI/DMA; `Band` lets a whole-frame generator draw each band in absolute
// coordinates while each board's `DisplayDriver` owns the actual band push.
// Stand-in battery fuel gauge — a fixed level until the nPM1300 PMIC gauge is wired in.
pub mod fuel;
pub mod panel;
// The self-diffing present core (epic #199 / issue #200): a per-row framebuffer hash so the present
// path pushes only the rows that changed. Pure integer math over row bytes, so it always compiles
// and its unit tests run in the host workspace — and the simulator drives the same `diff_rows` core
// under its exact-diff oracle. The device present path adopts the `RowDiff` store in D2 (issue #201).
pub mod rowdiff;
pub mod sd;
// Always compiled — the synthetic GPS is the `debug-link`-OFF fallback, so it must exist without
// the `debug-link` feature.
pub mod synth;

pub use button_input::{ButtonInput, Timing};
pub use framebuffer::{device64_to_rgb565, FbDevice64, Framebuffer565};
pub use fuel::StubFuelGauge;
pub use ls021_wire::pack_row as ls021_pack_row;
pub use panel::{composite_overlay_window, Band};
pub use rowdiff::{clip_span, diff_rows, row_hash, spans_missed_changes, RowDiff};
pub use sd::{SdByteSink, SdByteSource, SdTrackSink};
pub use synth::SynthLocation;
