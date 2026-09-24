//! Board-agnostic firmware source and handoff adapters: the seam that bridges a concrete board's
//! tasks to the shared app's [`obc_ports`] semantic sources.
//!
//! `no_std`, over `embedded-hal` and `obc-ports`, so the board crate stays thin (clocks, concrete
//! pins, the main loop) and the reusable bridges live here, ported to the next board by
//! re-pointing the pins: the button debouncer, the transport-agnostic debug-sensor protocol, the
//! real-sensor and BLE-sensor cross-task hand-offs, and the synthetic fallback sources.
//!
//! What remains here is exactly the adapter role: the pieces that turn a board-side event (a GPIO
//! edge, a decoded UBX sample, a BLE notification, a debug-UART line) into an [`obc_ports`]
//! `*Source` the app polls, plus the embassy plumbing that carries them across tasks. The display
//! contracts live in `obc-display`, the chip decoders in `obc-sensors`, and the SD adapters in
//! `obc-storage`.
//!
//! [`backlight`] owns the level-to-duty ladder, [`sound`] the cue-to-pattern table,
//! [`button_input`] the debouncer and its edge-wake, [`debug_link`] the fake-sensor protocol,
//! [`sensor_hub`] the instance-owned cross-task sensor streams, [`synth`] the synthetic moving
//! location, and [`fuel`] a fixed-level fuel gauge.
//!
//! Two-plane architecture: each board's main loop runs the device on two planes across two
//! executors, so input and the overlay stay responsive while a map frame renders. `render_map` is
//! CPU-bound and never awaits, so it blocks its executor, and dirty-tracking cuts how often it
//! runs but not the during-render case.
//!
//! - The high-priority plane is an embassy `InterruptExecutor` above thread mode but below the
//!   embassy-time driver, so its timers still wake mid-render. It owns the [`ButtonInput`]
//!   debouncer, the app input plane and the overlay framebuffer. Every few ms it preempts the map
//!   render, samples the buttons, recognises gestures into a channel and repaints the hold bulge,
//!   so press-to-feedback latency stays bounded whatever the map render costs.
//! - The low-priority plane is the thread-mode executor running the app: screen stack, camera,
//!   sensors, SD and the map render.
//!
//! The only shared state is a lock-free `Channel<Gesture>` plus the two disjoint framebuffers, so
//! the long map render holds no lock against the input plane. Whatever display resource the two
//! planes genuinely share is guarded by a short critical section in the board's present helper.
//! The single-loop hosts run the same two planes inline, fused by the app's `handle_input`.

#![no_std]

// The level-to-duty ladder every real `Backlight` drives. Board-agnostic on purpose: the PWM the
// board wires today and a later constant-current driver want the same five steps.
pub mod backlight;
pub mod button_input;
// Transport-agnostic fake-sensor protocol, sources and telemetry. The pure codec is always
// compiled; only the embassy-sync plumbing is gated behind `debug-link`, so the host workspace
// build never pulls embassy-sync.
pub mod debug_link;
// Stand-in battery fuel gauge until the nPM1300 PMIC gauge is wired in.
pub mod fuel;
// Always compiled: the synthetic GPS is the `synth`-feature fallback, so it must exist without
// the real-sensor features.
pub mod synth;
// The instance-owned sensor hub: one `SensorHub` owns every cross-task sensor stream, constructed
// once in static storage by the board and split into typed producer, consumer and control handles.
// Gated behind `sensor-link`, which pulls embassy-sync.
#[cfg(feature = "sensor-link")]
pub mod sensor_hub;
// The cue-to-pattern table every real `Sounder` plays.
pub mod sound;

pub use button_input::ButtonInput;
pub use fuel::StubFuelGauge;
#[cfg(feature = "sensor-link")]
pub use sensor_hub::SensorHub;
pub use synth::SynthLocation;

#[cfg(test)]
mod deleted_knobs {
    /// All four buttons forward debounced edges and the step cadence lives in `obc_app::input`.
    /// This reads `button_input.rs` as text, so a second timing model cannot quietly grow back
    /// beside the shared one. The needle is assembled here rather than written literally, so the
    /// guard does not match its own source, and the haystack is lower-cased first, so a
    /// case-sensitive match cannot miss the names it guards against.
    #[test]
    fn button_input_grows_no_second_repeat_timing() {
        let source = include_str!("button_input.rs").to_ascii_lowercase();
        for needle in [concat!("auto_", "repeat"), concat!("repeat_", "delay"), concat!("repeat_", "interval")] {
            assert!(
                !source.contains(needle),
                "`{needle}` is back in obc-platform's button_input.rs; repeat timing belongs to the \
                 shared recogniser in obc-app's `input.rs`, not to a board adapter"
            );
        }
    }
}
