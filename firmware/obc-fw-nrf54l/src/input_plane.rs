//! The input plane: the high-priority half of the two-plane display machinery.
//!
//! It owns the input task, its executor static and the SWI01 pend vector, the gesture channel the
//! thread-mode map plane drains, and the input-liveness heartbeat. `main` owns bring-up: it starts
//! the executor and spawns [`input_task`] onto it, and spawns the COM task onto the same executor.
//!
//! Not to be confused with `obc_app`'s own `input_plane`, the board-agnostic gesture recogniser
//! ([`InputPlane`]) that this task drives under a lock.

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
use obc_app::{Chord, Gesture, InputPlane};
use obc_platform::ButtonInput;
use obc_ports::{InputClock, InputEvent, InputSource};

// The map render would block its executor for tens of milliseconds. To keep input and the hold
// bulge responsive during that, the device runs two planes. The map plane, in thread mode, drains
// the gesture channel, advances screen animations, re-renders the map when it is dirty, and pushes
// both the clean frame and the live bulge to glass. The input plane, on a high-priority
// `InterruptExecutor` pended from SWI01, samples the buttons and recognises gestures into the
// channel, so press-to-feedback latency and the auto-repeat cadence stay exact even while a deep map
// render holds thread mode; it never pushes to glass.
//
// The shared state is the lock-free gesture channel plus the `InputPlane` behind a brief blocking
// mutex, which neither side holds across an `.await`: the input plane advances the bulge under that
// lock and the map plane composites the same live state into its overlay push.

/// Bound of the input-to-map gesture channel. One frame yields a couple of gestures and the map
/// plane drains it each loop, so even across a slow push it never fills. `try_send` drops on the
/// unreachable overflow rather than block the high-priority plane.
pub(crate) const GESTURE_QUEUE: usize = 16;

/// Recognised gestures flowing from the input plane to the map plane: the only lock-free shared
/// state between the two.
pub(crate) static GESTURES: Channel<CriticalSectionRawMutex, Gesture, GESTURE_QUEUE> = Channel::new();

/// Device-wide chords, on their own lane beside the gestures.
///
/// A chord is not a gesture: it is resolved above the screen stack and its constituents were
/// swallowed, so putting it in [`GESTURES`] would mean a `Gesture` variant every screen must ignore.
/// Two slots: a rider cannot squeeze faster than the map plane drains, and a dropped duplicate
/// squeeze is a no-op.
pub(crate) static CHORDS: Channel<CriticalSectionRawMutex, Chord, 2> = Channel::new();

/// Wakes the event-driven map loop the moment a hold starts charging, and keeps it awake while the
/// bulge is live. Without it the loop has no wake source for a press: a button-down emits no
/// gesture, because `Press` fires on release and `Hold` at the threshold, so on a quiet screen the
/// map plane slept through the whole charge and the first thing to reach glass was the confirm pop.
/// A `Signal` is a coalescing level wake, which is what a repeated 8 ms nudge wants.
pub(crate) static INPUT_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The single high-priority executor. It free-runs the COM driver, which must keep alternating so
/// the panel never takes a DC bias whatever the map plane is doing, the gesture-input plane, so
/// button latency stays exact during a deep map render, and the buzzer, so note timing stays exact.
/// It is pended from the SWI01 vector at P3; SWI00 is MPSL's low-priority lane.
pub(crate) static EXECUTOR_HP: InterruptExecutor = InterruptExecutor::new();

/// SWI01 ISR: poll the high-priority executor. SWI01 has no peripheral; only its interrupt vector
/// is borrowed as the executor's pend line.
#[interrupt]
unsafe fn SWI01() {
    EXECUTOR_HP.on_interrupt();
}

/// Input-plane loop period in ms: buttons sampled, gestures recognised and the bulge animated this
/// often, on the executor that preempts the map render.
pub(crate) const LOOP_MS: u64 = 8;

/// Insurance re-poll cadence for the idle input plane. Once every button is released and settled
/// the plane sleeps on a button falling edge instead of polling, so a parked device burns no CPU.
/// This long guard wakes it occasionally regardless, so a missed edge can never strand the UI.
#[cfg(not(feature = "debug-uart"))]
const IDLE_REPOLL_MS: u64 = 30_000;

/// The input plane's liveness heartbeat: the millisecond stamp of its last recognizer pass or idle
/// wake, read by the ride loop's watchdog feed.
pub(crate) static INPUT_HB_MS: AtomicU32 = AtomicU32::new(0);

/// Chains two input sources for the gesture recogniser: it drains the physical buttons fully, then
/// the injected stream, so a host can drive the UI over the debug link interleaved with real
/// presses.
struct ChainedInput<'a> {
    a: &'a mut dyn InputSource,
    b: &'a mut dyn InputSource,
}
impl InputSource for ChainedInput<'_> {
    fn poll(&mut self) -> Option<InputEvent> {
        self.a.poll().or_else(|| self.b.poll())
    }
}

/// A never-yielding input source: the stand-in for the injected stream when `debug-uart` is off, so
/// the recogniser call site is one code path in both builds.
#[cfg(not(feature = "debug-uart"))]
struct NullInput;
#[cfg(not(feature = "debug-uart"))]
impl InputSource for NullInput {
    fn poll(&mut self) -> Option<InputEvent> {
        None
    }
}

/// The injected input stream to chain after the physical buttons, or [`NullInput`] when the feature
/// is off. One helper, so the input plane builds it the same way in both builds.
fn debug_input() -> impl InputSource {
    #[cfg(feature = "debug-uart")]
    return obc_platform::debug_link::DebugInput;
    #[cfg(not(feature = "debug-uart"))]
    NullInput
}

/// The input plane: it recognises gestures and animates the hold bulge. It runs on [`EXECUTOR_HP`]
/// beside COM, preempting the thread-mode map render, so press latency and the auto-repeat cadence
/// stay exact across a deep map render. Each [`LOOP_MS`] it samples the buttons and recognises
/// gestures into [`GESTURES`] under the shared [`InputPlane`] lock, so the live hold-bulge state it
/// advances is the same one the map plane composites into its overlay push.
///
/// This task does not push to glass, and the brief lock is never held across the `await`.
#[embassy_executor::task]
pub(crate) async fn input_task(
    mut buttons: ButtonInput<Input<'static>>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    gestures: Sender<'static, CriticalSectionRawMutex, Gesture, GESTURE_QUEUE>,
    chords: Sender<'static, CriticalSectionRawMutex, Chord, 2>,
) {
    loop {
        let now = Instant::now().as_millis() as u32;
        INPUT_HB_MS.store(now, Ordering::Relaxed); // liveness stamp the ride loop's WDT feed gates on
        buttons.update(now);
        // Recognise and animate the bulge under the shared lock, a brief critical section never
        // held across the await, so the bulge state the map plane composites is the one this
        // advanced. Also read whether the bulge is still live: the input plane must keep animating
        // it after the button is released, so it gates the idle sleep.
        let (chord, overlay_active, hold_charging) = input_plane.lock(|cell| {
            let plane = &mut *cell.borrow_mut();
            let mut dbg = debug_input();
            let mut input = ChainedInput { a: &mut buttons, b: &mut dbg };
            let chord = plane.recognize(InputClock(now), &mut input, |g| {
                if gestures.try_send(g).is_err() {
                    defmt::warn!("gesture channel full — dropped a gesture (map plane stalled?)");
                }
            });
            (chord, plane.overlay_active(), plane.select_hold_progress() > 0.0 || plane.back_hold_progress() > 0.0)
        });
        // A chord opens or closes a drawer, which is a whole-frame change the map plane owns, and it
        // emits no gesture, so nothing else would wake the sleeping loop for it.
        if let Some(chord) = chord {
            if chords.try_send(chord).is_err() {
                defmt::warn!("chord channel full — dropped a squeeze (map plane stalled?)");
            }
            INPUT_WAKE.signal(());
        }
        // Nudge the event-driven map loop for the whole hold lifecycle. The map plane owns every
        // bulge push and a press emits no gesture, so without this wake the loop sleeps through the
        // charge on a quiet screen and the first thing on glass is the confirm pop.
        if hold_charging || overlay_active {
            INPUT_WAKE.signal(());
        }
        // Event-driven sleep: once every button is released and settled and no bulge is animating,
        // sleep on a button falling edge instead of polling. While a button is down, debouncing or
        // repeating, or a bulge is live, keep the 8 ms poll so debounce, auto-repeat and the bulge
        // animation stay exact. The `debug-uart` build always polls, so injected input is prompt.
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
