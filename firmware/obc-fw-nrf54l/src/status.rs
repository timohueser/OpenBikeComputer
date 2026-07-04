//! The `ble` status build's thread-mode plane (`not(has_map)`) — split out of `main.rs`
//! (issue #351): [`run_status`], `run_app`'s deliberately dumb sibling.

// The status loop tells a BLE status edge from a gesture/animation wake by the select arm (the
// edge `Signal` is consumed by the await, so the arm is the information).
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::gpio::Output;
use embassy_time::{Instant, Timer};
// The status loop polls the fuel gauge itself (`run_app` polls it through `Sensors`).
use obc_app::FuelGauge;
use obc_platform::StubFuelGauge;
use obc_render::RenderStats;

use crate::display::DisplayDriver;
use crate::planes::{MapDisplay, GESTURES, LOOP_MS};
use crate::{ble, stackmeter};

/// The `ble` status build's thread-mode plane — [`run_app`]'s deliberately dumb sibling: no map, no
/// ride, no SD reconcile. It paints the BLE status screen ([`ble::draw_status_screen`]) into the
/// resident framebuffer and presents it through the same [`MapDisplay`] seam, keeps the hold bulge
/// working (the input plane recognises + animates it exactly as on the map build), and sleeps
/// event-driven: a recognised gesture, a BLE link edge ([`ble::wait_status_change`]), or the short tick
/// while a bulge animates. Joined against [`ble::run`] on the thread-mode executor in `main`.
pub(crate) async fn run_status(
    mut display: MapDisplay,
    // Whether a card mounted at boot — a status line, never a fault. The card itself (and the RRAM
    // settings store) live in the BLE plane's `ObjectStore`.
    sd_ok: bool,
    led: &mut Output<'static>,
) -> ! {
    let mut fuel = StubFuelGauge::new(75);
    // The on-screen input counter — the dumb UI's visible ack that buttons + the input plane run beside
    // the radio (every recognised gesture bumps it; nothing navigates anywhere).
    let mut inputs: u32 = 0;
    let mut stack_hw = 0usize;
    let mut last_led = 0u32;
    let mut redraw = true; // boot: paint the first frame + seed the RowDiff store
    loop {
        let now = Instant::now().as_millis() as u32;
        let hw = stackmeter::used();
        if hw > stack_hw {
            stack_hw = hw;
            defmt::info!("stack high-water {=usize} / {=usize} B (new peak)", hw, stackmeter::total());
        }

        while GESTURES.try_receive().is_ok() {
            inputs += 1;
            redraw = true;
        }
        // A FLPR relaunch landed (#349): repaint the status screen in full (diff store was reset).
        // If instead the display *degraded*, presents become silent no-ops — the status build keeps
        // its radio useful with the glass frozen, rather than idling out a working BLE link.
        redraw |= display.take_relaunch_repaint();

        // This frame's hold-bulge state, exactly as the ride loop samples it — the status present
        // goes around a live bulge's rows and `present_bulge` re-composites them. Bulge pushes
        // FIRST, as in the ride loop (#348 follow-up): a fired hold's confirm pop must not queue
        // behind the status redraw it triggered.
        let (overlay_dirty, overlay_span) = display.poll_overlay();
        display.present_bulge(overlay_span, overlay_dirty).await;

        if redraw {
            let battery = fuel.poll().unwrap_or(0);
            ble::publish_battery(battery); // feed the BAS characteristic (A4) from the FuelGauge seam
            let render = |d: &mut dyn DisplayDriver| {
                ble::draw_status_screen(d.fb_mut(), battery, sd_ok, inputs);
                RenderStats::default()
            };
            let fp = display.render_present(overlay_span, render).await;
            redraw = !fp.ok; // a transport fault latches a retry, like the ride loop
            defmt::info!("status frame: render {=u64} us + push {=u64} us", fp.render_us, fp.push_us);
            // Re-composite the bulge rows the present's `exclude` skipped (see the ride loop's note).
            if overlay_span.is_some() {
                display.present_bulge(overlay_span, false).await;
            }
        }

        if now.wrapping_sub(last_led) >= 500 {
            led.toggle();
            last_led = now;
        }

        // Event-driven sleep: a gesture, a BLE link edge, or — while a bulge animates / a failed present
        // wants its retry — the short tick. The link-edge `Signal` is consumed by the await, so *which
        // arm fired* is the redraw signal.
        if overlay_dirty || overlay_span.is_some() || redraw {
            match select3(GESTURES.ready_to_receive(), ble::wait_status_change(), Timer::after_millis(LOOP_MS)).await {
                Either3::Second(_) => redraw = true,
                Either3::First(_) | Either3::Third(_) => {}
            }
        } else {
            match select(GESTURES.ready_to_receive(), ble::wait_status_change()).await {
                Either::Second(_) => redraw = true,
                Either::First(_) => {}
            }
        }
    }
}
