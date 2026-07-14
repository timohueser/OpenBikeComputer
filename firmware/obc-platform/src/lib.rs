//! Board-agnostic firmware **source/handoff adapters** — the focused seam that bridges a concrete
//! board's tasks to the shared app's [`obc_ports`] semantic sources.
//!
//! `no_std`, over `embedded-hal` / `obc-ports`, so the board crate stays thin (clocks, concrete
//! pins, the main loop) and the reusable port bridges live here, ported to the next board by
//! re-pointing the pins: the button debouncer, the transport-agnostic debug-sensor protocol
//! (behind `debug-link`), the real-sensor and BLE-sensor cross-task hand-offs (behind
//! `sensor-link`), and the synthetic/stub fallback sources.
//!
//! Issue #807 split the former junk-drawer into coherent crates; what remains here is exactly the
//! **adapter/handoff** role — the pieces that turn board-side events (a GPIO edge, a decoded UBX
//! sample, a BLE HR notification, a debug-UART line) into an [`obc_ports`] `*Source` the app polls,
//! plus the embassy-sync/embassy-time plumbing that carries them across tasks. The sibling crates
//! own the rest:
//!
//! - [`obc-display`](https://docs.rs/obc-display) — the generic frame/presentation contracts, the
//!   framebuffer DrawTargets, the banded panel view, and the LS021/FLPR pairing.
//! - [`obc-sensors`](https://docs.rs/obc-sensors) — the pure chip/protocol decoders (UBX, BMP581,
//!   ICM-20948, compass) this crate's `sensor-link` handoff carries.
//! - [`obc-storage`](https://docs.rs/obc-storage) — the FatFs/SD `ByteSource`/`Sink`/`TrackSink`
//!   adapters and the FAT-extent map fast path.
//!
//! ## Responsibility / dependency table
//!
//! | Module | Owns | Depends on |
//! |---|---|---|
//! | [`button_input`] | a [`ButtonInput`] debouncer over four [`InputPin`](embedded_hal::digital::InputPin)s, feeding the shared gesture recognizer through [`InputSource`](obc_ports::InputSource); the `input-wait` edge-wake | `obc-ports`, `embedded-hal`, `heapless` (+ `embedded-hal-async`/`embassy-futures` behind `input-wait`) |
//! | [`debug_link`] | the transport-agnostic fake-sensor debug protocol (#38): the always-compiled line codec + telemetry/fix encoders, and — behind `debug-link` — the embassy-sync `Signal`/`Channel` hand-off + `LocationSource`/`AltimeterSource`/`CompassSource` impls | `obc-ports`, `heapless` (+ `embassy-sync` behind `debug-link`) |
//! | [`sensor_link`] | the embassy-sync `Signal` hand-off bridging the board's I²C sensor task ([`obc-sensors`](https://docs.rs/obc-sensors) decodes) to the app poll (the fresh-fix latch) — behind `sensor-link` | `obc-ports`, `embassy-sync` (`sensor-link`) |
//! | [`sensor_values`] | the BLE-sensor (HR / power / cadence) value mailboxes, fed by both the BLE central manager and the `debug-uart` injection path — behind `sensor-link` | `obc-ports`, `embassy-sync` (`sensor-link`) |
//! | [`synth`] | [`SynthLocation`], the board-agnostic synthetic moving [`LocationSource`](obc_ports::LocationSource) — the `debug-link`-off fallback fake GPS (always compiled) | `obc-ports`, `embassy-time` |
//! | [`fuel`] | [`StubFuelGauge`], a fixed-level [`FuelGauge`](obc_ports::FuelGauge) stand-in until the nPM1300 PMIC fuel gauge is wired in | `obc-ports` |
//!
//! Nothing here depends on `obc-app` (the FAR-00 upward edge is gone), on the display seam, or on
//! the SD stack — those moved to the sibling crates above. `sensor_link`/`sensor_values` keep the
//! global mailbox handoff unchanged pending issue #808's instance-owned rework.
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
// Stand-in battery fuel gauge — a fixed level until the nPM1300 PMIC gauge is wired in.
pub mod fuel;
// Always compiled: the synthetic GPS is the `synth`-feature fallback, so it must exist without the
// real-sensor / `sensor-link` features.
pub mod synth;
// The embassy-sync `Signal` hand-off bridging the board's I²C sensor task (decoded by the
// `obc-sensors` crate) to the app's HAL poll — the real-hardware sibling of `debug_link`'s handoff.
// Gated behind `sensor-link` (it pulls embassy-sync); the pure decode lives in `obc-sensors`.
#[cfg(feature = "sensor-link")]
pub mod sensor_link;
// The BLE-sensor (HR / power / cadence) value mailboxes — the raw-value sibling of `sensor_link`,
// fed by both the board's BLE central manager (SE6) and the `debug-uart` injection path (SE8). Same
// `sensor-link` gate (it pulls embassy-sync); the pure profile decode lives in `obc-ble`.
#[cfg(feature = "sensor-link")]
pub mod sensor_values;

pub use button_input::{ButtonInput, Timing};
pub use fuel::StubFuelGauge;
pub use synth::SynthLocation;
