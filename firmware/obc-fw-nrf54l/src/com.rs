//! **LS021B7DD02 COM (common-electrode) driver** — the free-running `VCOM`/`VB`/`VA` square wave.
//!
//! COM is panel-board-agnostic infrastructure that must run no matter who clocks the pixels, so it
//! stays on the M33 here (the FLPR drives frames).
//!
//! ## Why COM has to free-run
//!
//! The Memory-in-Pixel cells must never see a **DC bias**, so `VCOM`/`VB`/`VA` have to alternate
//! forever (~60 Hz, ~50 % duty) the *whole* time the panel is powered and driven — even on a perfectly
//! static image. `VB` is **in phase** with `VCOM`; `VA` is its **exact inverse**.
//!
//! ### Why a GPIO toggle on a timer, not a PWM peripheral
//!
//! On the retired P2.07/P2.08/P2.10 bring-up routing, **PWM20 would not drive the then-COM pins**:
//! the analyzer showed the lines dead `Lo`, while plain GPIO toggled them cleanly. The current board
//! contract is P1.22/P1.23/P1.24; the shipping path remains a **GRTC/timer-backed GPIO square wave**
//! until the opt-in GPIOTE driver is verified. [`com_task`] flips the lines and `await`s half a
//! period, forever.
//!
//! To keep it free-running **while the M33 is busy elsewhere**, spawn `com_task` on a **high-priority
//! `InterruptExecutor`** (see the callers): the GRTC wakeup pends that executor and preempts
//! thread-mode, so COM never stalls behind a long-running thread-mode loop. The crossings are
//! effectively simultaneous — three back-to-back register writes, tens of ns apart, far below the
//! ~100 µs edge spec — so there is no meaningful overlap glitch.
//!
//! Built as a task so the COM pins move into it and toggle for the life of the program. "Hold COM `Lo`
//! during init, then start" is just *when* it is spawned: the pins boot `Output(Lo)` and stay `Lo`
//! until the task runs.
//!
//! Each COM line is a real **56–77 nF** load, so the caller configures the three as **high-drive
//! (H0H1)** GPIO to slew it inside the datasheet ≤100 µs rise/fall (~2.5 mA). If the analyzer shows
//! rounded edges into the real load, external buffering is the documented fallback.

use embassy_nrf::gpio::Output;
use embassy_time::Timer;

/// Half of the ~60 Hz COM period: `1 / 60 / 2 ≈ 8333 µs` → 60.0 Hz, 50 % duty. Inside the
/// datasheet `f_VCOM` 54–66 Hz / 48–52 % window.
pub const COM_HALF_PERIOD_US: u64 = 8333;

/// The free-running COM driver: a ~60 Hz square wave with `vcom`/`vb` in phase and `va` the
/// exact inverse. Runs forever — **spawn it on a high-priority `InterruptExecutor`** so it
/// keeps toggling while the thread-mode CPU is busy (see the module docs).
///
/// The three pins are owned by the task for the life of the program; pass them already
/// configured as high-drive outputs (they boot `Lo` = the COM-held-`Lo` init state, and the
/// first half-period below raises `va` to its inverse phase).
#[embassy_executor::task]
pub async fn com_task(mut vcom: Output<'static>, mut vb: Output<'static>, mut va: Output<'static>) {
    loop {
        // First half: VCOM/VB high, VA low.
        vcom.set_high();
        vb.set_high();
        va.set_low();
        Timer::after_micros(COM_HALF_PERIOD_US).await;
        // Second half: VCOM/VB low, VA high — the exact inverse crossing.
        vcom.set_low();
        vb.set_low();
        va.set_high();
        Timer::after_micros(COM_HALF_PERIOD_US).await;
    }
}
