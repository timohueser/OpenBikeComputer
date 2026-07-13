//! Board-agnostic firmware glue between the shared app and a concrete board
//! crate (`obc-fw-nrf54l`).
//!
//! `no_std`, over `embedded-hal` / `embedded-graphics`, so the board crate stays thin (clocks,
//! concrete pins, the main loop) and everything reusable lives here, ported to the next board by
//! re-pointing the pins: the framebuffer `DrawTarget`s, the button debouncer, the FatFs
//! `ByteSource`/`Sink` adapters, and the transport-agnostic debug-sensor protocol (behind
//! `debug-link`).
//!
//! Modules:
//! - [`framebuffer`] — the board-owned [`DrawTarget`](embedded_graphics::draw_target::DrawTarget)s
//!   the shared renderer draws into: the nRF's device-native RGB222 map plane ([`FbDevice64`], 1
//!   byte/px — the real target) and the [`Framebuffer565`] RGB565 plane the banded [`Band`] scratch
//!   reuses.
//! - [`panel`] — the [`Band`] frame-absolute band/window view + the [`composite_overlay_window`]
//!   overlay helper, for boards that stream a frame to the panel over SPI/DMA a band at a time. The
//!   banded present loop itself lives behind each board's `DisplayDriver`.
//! - [`button_input`] — a [`ButtonInput`] debouncer over four
//!   [`InputPin`](embedded_hal::digital::InputPin)s, feeding the shared gesture recognizer through
//!   [`InputSource`](obc_ports::InputSource).
//! - [`sd`] — FatFs [`ByteSource`](obc_route::ByteSource)/[`ByteSink`](obc_route::ByteSink) and
//!   [`TrackSink`](obc_ports::TrackSink) adapters over an [`embedded_sdmmc`] SD card.
//! - [`synth`] — [`SynthLocation`], a board-agnostic synthetic moving
//!   [`LocationSource`](obc_ports::LocationSource) — the `debug-link`-off fallback fake GPS that walks
//!   a slow square loop (always compiled, *not* behind `debug-link`, since it *is* the
//!   debug-link-off path).
//! - [`fuel`] — [`StubFuelGauge`], a fixed-level [`FuelGauge`](obc_ports::FuelGauge) stand-in until
//!   the nPM1300 PMIC fuel gauge is wired in.
//!
//! ## Two-plane architecture — input/overlay vs. map (issue #48)
//!
//! Each board's main loop runs the device on **two planes across two executors**, so input + the
//! overlay stay responsive *while a map frame renders*. `render_map` is CPU-bound (tens of ms) and
//! never `.await`s, so it blocks its executor; dirty-tracking cuts how *often* it runs but not the
//! during-render case (panning re-renders rapidly while a button is held). The fix is preemption:
//!
//! - **High-priority plane** — an embassy **`InterruptExecutor`** at a priority *above* thread mode
//!   but *below* the embassy-time driver (so its `Timer`s still wake mid-render). It owns the
//!   [`ButtonInput`] debouncer, the shared app input plane, and the **overlay**
//!   framebuffer. Every few ms it *preempts the map render*, samples the buttons, recognises
//!   gestures into a channel, and repaints the hold bulge — so press-to-feedback latency stays
//!   bounded regardless of map-render time.
//! - **Low-priority plane** — the thread-mode executor running the app: screen
//!   stack, camera, sensors, SD, and the **map** render. Each loop it drains the gesture channel,
//!   advances animations, polls sensors through [`obc_ports`], and re-renders the map on the app's
//!   dirty signal — never the overlay.
//!
//! The only shared state is a lock-free `Channel<Gesture>` plus the two **disjoint** framebuffers,
//! so the long map render holds no lock against the input plane. Whatever display resource the two
//! planes genuinely share (the framebuffer/transport on a banded board, a frame-flip register on a
//! scan-out board) is guarded by a short critical section in the board's present helper. A
//! `single-executor` fallback drives both planes inline through
//! the app's input handler; the preemptive split is the shipping default.

#![no_std]

pub mod button_input;
// Transport-agnostic fake-sensor protocol + sources + telemetry. The pure codec is always compiled;
// only the embassy-sync `Signal`/`Channel` plumbing + HAL-trait sources are gated inside the module
// behind `debug-link`, so the host workspace build never pulls embassy-sync.
pub mod debug_link;
// The map file's FAT chain resolved once into extent runs → direct-block `read_at` (issue #500):
// the seek-per-read `sd` adapters stay the general path; this is the fast path for the one big
// read-only file whose scattered reads dominate (the `.obcm` map).
pub mod fat_extents;
pub mod framebuffer;
pub mod ls021_wire;
// The board-agnostic display-driver seam (`DisplayDriver`, `OverlayRegion`, the frame geometry) both
// backends implement — the on-device LS021/FLPR panel and the host simulator. No new deps: the trait
// is dependency-free, so it stays compiled into every host workspace build.
pub mod display;
// Stand-in battery fuel gauge — a fixed level until the nPM1300 PMIC gauge is wired in.
pub mod fuel;
pub mod panel;
pub mod rowdiff;
pub mod sd;
// Always compiled: the synthetic GPS is the `synth`-feature fallback, so it must exist without the
// real-sensor / `sensor-link` features.
pub mod bmp581;
pub mod compass;
pub mod icm20948;
pub mod synth;
pub mod ubx;
// The embassy-sync `Signal` hand-off bridging the board's I²C sensor task to the app's HAL poll —
// the real-hardware sibling of `debug_link`'s handoff. Gated behind `sensor-link` (it pulls
// embassy-sync); the pure `ubx`/`bmp581` decode above is not.
#[cfg(feature = "sensor-link")]
pub mod sensor_link;
// The BLE-sensor (HR / power / cadence) value mailboxes — the raw-value sibling of `sensor_link`,
// fed by both the board's BLE central manager (SE6) and the `debug-uart` injection path (SE8). Same
// `sensor-link` gate (it pulls embassy-sync); the pure profile decode lives in `obc-ble`.
#[cfg(feature = "sensor-link")]
pub mod sensor_values;

pub use button_input::{ButtonInput, Timing};
pub use display::{DisplayDriver, OverlayRegion, FRAME_H, FRAME_W};
pub use fat_extents::{ExtentSource, ExtentTable};
pub use framebuffer::{device64_to_rgb565, FbDevice64, Framebuffer565};
pub use fuel::StubFuelGauge;
pub use ls021_wire::pack_row as ls021_pack_row;
pub use panel::{composite_overlay_window, Band};
pub use rowdiff::{clip_span, diff_rows, row_hash, spans_missed_changes, RowDiff};
pub use sd::{SdByteSink, SdByteSource, SdTrackSink};
pub use synth::SynthLocation;
