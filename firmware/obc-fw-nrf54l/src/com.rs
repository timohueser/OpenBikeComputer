//! The LS021B7DD02 COM driver: the free-running `VCOM`, `VB` and `VA` square wave.
//!
//! COM must run whoever clocks the pixels, so it stays on the M33 while the FLPR drives frames.
//!
//! The Memory-in-Pixel cells must never see a DC bias, so the three lines have to alternate
//! forever, about 60 Hz at 50 % duty, the whole time the panel is powered and driven, even on a
//! static image. `VB` is in phase with `VCOM`, and `VA` is its exact inverse.
//!
//! It is a GPIO toggle on a timer rather than a PWM peripheral: on the bring-up routing PWM would
//! not drive the COM pins at all, while plain GPIO toggled them cleanly. [`com_task`] flips the
//! lines and awaits half a period, forever.
//!
//! To keep it free-running while the M33 is busy elsewhere, spawn `com_task` on a high-priority
//! `InterruptExecutor`: the timer wakeup pends that executor and preempts thread mode, so COM never
//! stalls behind a long thread-mode loop. The three crossings are back-to-back register writes, tens
//! of nanoseconds apart and far below the 100 µs edge spec, so there is no meaningful overlap.
//!
//! The pins move into the task and toggle for the life of the program. They boot `Output(Lo)` and
//! stay low until the task runs, which is what "hold COM low during init, then start" means.
//!
//! Each COM line is a 56 to 77 nF load, so the caller configures the three as high-drive GPIO to
//! slew inside the datasheet's 100 µs rise and fall time.

use embassy_nrf::gpio::Output;
use embassy_time::Timer;

/// Half of the 60 Hz COM period, which is inside the datasheet's 54 to 66 Hz and 48 to 52 % window.
pub const COM_HALF_PERIOD_US: u64 = 8333;

/// The free-running COM driver: a 60 Hz square wave with `vcom` and `vb` in phase and `va` their
/// exact inverse. It runs forever, so spawn it on a high-priority `InterruptExecutor`.
///
/// The task owns the three pins for the life of the program. Pass them already configured as
/// high-drive outputs, booted low, and the first half period below raises `va` to its inverse
/// phase.
#[embassy_executor::task]
pub async fn com_task(mut vcom: Output<'static>, mut vb: Output<'static>, mut va: Output<'static>) {
    loop {
        vcom.set_high();
        vb.set_high();
        va.set_low();
        Timer::after_micros(COM_HALF_PERIOD_US).await;
        vcom.set_low();
        vb.set_low();
        va.set_high();
        Timer::after_micros(COM_HALF_PERIOD_US).await;
    }
}
