//! The **input plane** — the high-priority half of the two-plane display machinery. It was first
//! split out of `main.rs` (issue #351); the machinery then lived in one `planes.rs`, and this file is
//! the input half of that final split, so gesture/input handling no longer shares a file with the
//! display machinery (the [map plane](crate::map_plane)).
//!
//! Owns the input task, its executor static + the SWI01 pend vector, the gesture channel the
//! thread-mode map plane drains, and the input-liveness heartbeat. `main` still owns bring-up: it
//! starts the executor and spawns [`input_task`] onto it (the COM task is spawned onto the same
//! executor by `main` too — see [`EXECUTOR_HP`]).
//!
//! Not to be confused with `obc_app`'s own `input_plane` — the board-agnostic gesture recogniser
//! ([`InputPlane`]) that this task *drives* under a lock. Here `crate::input_plane` is the board's
//! high-priority plane that hosts it.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::InterruptExecutor;
#[cfg(not(feature = "debug-uart"))]
use embassy_futures::select::select;
use embassy_nrf::gpio::Input;
use embassy_nrf::interrupt;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Sender};
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use obc_app::{Gesture, InputPlane};
use obc_platform::ButtonInput;
use obc_ports::{InputClock, InputEvent, InputSource};

// ============================ Two-plane input + overlay ============================
// The map render (`render_map` + the FLPR frame scan the M33 awaits) would block its executor for
// tens of ms. To keep input + the hold bulge responsive *during* that, the device runs two planes:
//   - Map plane (thread mode, the `main` loop): drains the gesture channel → `apply_gesture`,
//     advances screen animations, re-renders the map only on `dirty.map`, and — since it owns the
//     panel — pushes both the clean frame and the live bulge to glass.
//   - Input plane (`input_task`, on a high-priority `InterruptExecutor` pended from SWI01): samples
//     the buttons and recognises gestures (into the channel), so press-to-feedback latency + the
//     auto-repeat cadence stay exact even while a deep map render holds thread mode. It does **not**
//     push to glass — the FLPR scans whole frames, so the map plane owns every push.
// The shared state between them is the gesture `Channel` (lock-free) plus the `InputPlane` (behind a
// brief blocking mutex the recognizer + the bulge composite each take, never across an `.await`): the
// input plane advances the bulge under that lock and the map plane composites the same live state into
// its partial overlay push.

/// Bound of the input→map gesture channel. One frame yields a couple of gestures and the map plane
/// drains it each loop, so even across a slow map push it never fills; `try_send` drops on the
/// (unreachable) overflow rather than block the high-priority plane.
const GESTURE_QUEUE: usize = 16;

/// Recognised gestures flowing from the input plane (high priority) to the map plane (thread mode) —
/// the only lock-free shared state between the two planes.
pub(crate) static GESTURES: Channel<CriticalSectionRawMutex, Gesture, GESTURE_QUEUE> = Channel::new();

/// Wakes the event-driven map loop the moment a hold starts **charging** (and keeps it awake while
/// the bulge is live). Without it the loop has no wake source for a press: a button-*down* emits no
/// gesture (`Press` fires on release, `Hold` at the 500 ms threshold), so on a quiet screen the map
/// plane slept through the whole charge and the first thing to reach glass was the confirm pop —
/// the "nothing, then the bulge jumps out" bug. The input plane signals this every recognizer tick
/// while a hold is in flight or the overlay is animating; a `Signal` (a coalescing level wake, not
/// a queue) is exactly the semantics a repeated 8 ms nudge wants.
pub(crate) static INPUT_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The single high-priority executor: it free-runs **both** the COM driver (which must keep
/// alternating `VCOM`/`VB`/`VA` so the panel never DC-biases, whatever the map plane is doing)
/// **and** the gesture-input plane (so button latency stays exact during a ~44 ms full-frame scan —
/// the M33 now *awaits* that scan (#347), but a deep map render still occupies thread mode). Pended
/// from the SWI01 vector @ P3 (SWI00 is MPSL's low-prio lane on `ble` builds, so every build pends
/// from SWI01). Lives with the input plane because [`input_task`] is spawned onto it here; `main`
/// starts it and spawns the COM task onto the same executor.
pub(crate) static EXECUTOR_HP: InterruptExecutor = InterruptExecutor::new();

/// SWI01 ISR → poll the high-priority executor. SWI01 has no peripheral; we only borrow its interrupt
/// vector as the executor's pend line.
#[interrupt]
unsafe fn SWI01() {
    EXECUTOR_HP.on_interrupt();
}

/// Input-plane loop period (ms): buttons sampled + gestures recognised + the bulge animated this
/// often, on the high-priority executor that preempts the map render — so press-to-feedback latency
/// and the auto-repeat cadence stay exact regardless of how long a map frame takes.
pub(crate) const LOOP_MS: u64 = 8;

/// Insurance re-poll cadence (ms) for the **idle** input plane: once every button is released +
/// settled, the plane sleeps on a button falling edge ([`ButtonInput::wait_for_any_press`]) instead of
/// polling at [`LOOP_MS`], so a parked device burns no CPU sampling unchanging pins. This long guard
/// wakes it occasionally regardless, so a missed edge can never strand the UI.
#[cfg(not(feature = "debug-uart"))]
const IDLE_REPOLL_MS: u64 = 30_000;

/// The input plane's liveness heartbeat: `Instant` millis of its last recognizer pass / idle wake,
/// stamped by [`input_task`] and read by the ride loop's watchdog feed.
pub(crate) static INPUT_HB_MS: AtomicU32 = AtomicU32::new(0);

/// Chains two input sources for the gesture recogniser: drains `a` (the physical buttons) fully,
/// then `b` (the VCOM-injected `K` events with `debug-uart`, else [`NullInput`]). So a host can
/// drive the UI (taps/holds) over the same VCOM link, interleaved with real presses.
struct ChainedInput<'a> {
    a: &'a mut dyn InputSource,
    b: &'a mut dyn InputSource,
}
impl InputSource for ChainedInput<'_> {
    fn poll(&mut self) -> Option<InputEvent> {
        self.a.poll().or_else(|| self.b.poll())
    }
}

/// A never-yielding input source — the `debug-uart`-off stand-in for the VCOM-injected stream, so
/// the recogniser call site is one code path in both builds.
#[cfg(not(feature = "debug-uart"))]
struct NullInput;
#[cfg(not(feature = "debug-uart"))]
impl InputSource for NullInput {
    fn poll(&mut self) -> Option<InputEvent> {
        None
    }
}

/// The VCOM-injected input stream to chain after the physical buttons: the `debug-uart` source that
/// drains host-injected steps/edges (`K` lines), or [`NullInput`] when the feature is off. One
/// helper so the input plane builds it the same `cfg` way regardless.
fn debug_input() -> impl InputSource {
    #[cfg(feature = "debug-uart")]
    return obc_platform::debug_link::DebugInput;
    #[cfg(not(feature = "debug-uart"))]
    NullInput
}

/// The input plane: recognises gestures + animates the hold bulge. Runs on [`EXECUTOR_HP`]
/// beside COM, preempting the thread-mode map render, so press latency + the auto-repeat cadence stay
/// exact across a deep map render. Each [`LOOP_MS`] it samples the buttons + (with
/// `debug-uart`) the VCOM-injected `K` events and recognises gestures into [`GESTURES`] for the map
/// plane to apply — **under the shared [`InputPlane`] lock**, so the live hold-bulge state it advances
/// is the same one the map plane composites into its partial overlay push.
///
/// This task does **not** push to glass: the FLPR scans whole frames, so the *map plane* owns every
/// push. This task is purely the recogniser; the brief lock is never held across the `await`.
#[embassy_executor::task]
pub(crate) async fn input_task(
    mut buttons: ButtonInput<Input<'static>>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    gestures: Sender<'static, CriticalSectionRawMutex, Gesture, GESTURE_QUEUE>,
) {
    loop {
        let now = Instant::now().as_millis() as u32;
        INPUT_HB_MS.store(now, Ordering::Relaxed); // liveness stamp the ride loop's WDT feed gates on
        buttons.update(now);
        // Recognise + animate the bulge under the shared lock (a brief critical section, never held
        // across the await), so the bulge state the map plane composites is the one this advanced.
        // Physical buttons + (with `debug-uart`) the VCOM-injected `K` events, one recogniser pass.
        // Also read whether the hold bulge is still live (charging / popping / retracting): the input
        // plane must keep animating it even after the button is released, so it gates the idle sleep.
        let (overlay_active, hold_charging) = input_plane.lock(|cell| {
            let plane = &mut *cell.borrow_mut();
            let mut dbg = debug_input();
            let mut input = ChainedInput { a: &mut buttons, b: &mut dbg };
            plane.recognize(InputClock(now), &mut input, |g| {
                if gestures.try_send(g).is_err() {
                    defmt::warn!("gesture channel full — dropped a gesture (map plane stalled?)");
                }
            });
            (plane.overlay_active(), plane.select_hold_progress() > 0.0 || plane.back_hold_progress() > 0.0)
        });
        // Nudge the event-driven map loop for the whole hold lifecycle (charge → pop/retract). On
        // this backend the *map plane* owns every bulge push, and a press emits no gesture — so
        // without this wake the loop sleeps through the charge on a quiet screen and the first
        // thing on glass is the confirm pop (see [`INPUT_WAKE`]).
        if hold_charging || overlay_active {
            INPUT_WAKE.signal(());
        }
        // Event-driven sleep (issue #219): once every button is released + settled and no bulge is
        // animating, sleep on a button falling edge instead of polling — a parked device burns no CPU
        // here. While a button is down / debouncing / repeating, or a bulge is live, keep the 8 ms poll
        // so debounce + auto-repeat + the bulge animation stay exact. The `debug-uart` dev build always
        // polls so host-injected `K` input is seen promptly (power isn't the concern there).
        #[cfg(feature = "debug-uart")]
        {
            let _ = overlay_active;
            Timer::after_millis(LOOP_MS).await;
        }
        #[cfg(not(feature = "debug-uart"))]
        if buttons.is_idle() && !overlay_active {
            let _ = select(buttons.wait_for_any_press(), Timer::after_millis(IDLE_REPOLL_MS)).await;
        } else {
            Timer::after_millis(LOOP_MS).await;
        }
    }
}
