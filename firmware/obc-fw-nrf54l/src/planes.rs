//! The two-plane display machinery every build shares — split out of `main.rs` (issue #351).
//!
//! Owns the high-priority input plane (the task, its executor static + the SWI01 pend vector, the
//! gesture channel) and the [`MapDisplay`] handle the thread-mode plane
//! ([`run_app`](crate::ride::run_app)) drives the panel through. `main` still owns
//! bring-up: it constructs the panel + `MapDisplay`, starts the executor, and spawns the tasks.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::InterruptExecutor;
#[cfg(not(feature = "debug-uart"))]
use embassy_futures::select::select;
use embassy_nrf::gpio::Input;
// The FLPR `MapDisplay` parks the gate/source GPIO lines it must keep driven for the program's life.
use embassy_nrf::gpio::Output;
use embassy_nrf::interrupt;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Sender};
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_app::{Gesture, InputClock, InputEvent, InputPlane, InputSource};
// `Band` is the frame-absolute draw view the map plane's `present_overlay` drawer paints the
// hold bulge into.
use obc_platform::{Band, ButtonInput, FbDevice64};
use obc_render::RenderStats;

#[cfg(feature = "com-hw")]
use crate::com_hw::HwCom;
use crate::display::{DisplayDriver, OverlayRegion, FRAME_H, FRAME_W};
use crate::ls021_flpr::{relaunch_flpr, Ls021Flpr};

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
/// from SWI01).
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

// The hold-bulge's right-edge overlay **columns**. Both bulges erupt from the right screen edge ≤12 px
// deep, so this fixed 16-px column band bounds them with margin. The map plane re-presents the bulge
// through `DisplayDriver::present_overlay` over the clean framebuffer, addressing only the live bulge's
// *rows* (`InputPlane::overlay_rows`: encoder ≈ 59–171, Back ≈ 182–246) — the FLPR the full-width rows
// of that span (it has its own `MAX_OVERLAY_*` bound in `Ls021Flpr::push_overlay`).
/// First overlay column: the rightmost 16 px (bulge depth ≤12 + margin).
const OVL_X0: u16 = (FRAME_W - 16) as u16;
/// Overlay window width (columns).
const OVL_W: u16 = 16;

// The live-bulge "present the rows *around* it" discipline lives **inside** the self-diffing present:
// the map plane passes the bulge's row span to the seam's `DisplayDriver::present(exclude)`, which
// clips it out of the changed-row spans it pushes (`obc_platform::RowDiff::diff_clipped`), leaving
// those rows for the map plane's own `MapDisplay::present_bulge`.

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
/// drains host-injected turns/edges (`K` lines), or [`NullInput`] when the feature is off. One
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
            (plane.overlay_active(), plane.encoder_hold_progress() > 0.0 || plane.back_hold_progress() > 0.0)
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

// ============================ The map plane ============================
// The ride loop drives the screen through the [`MapDisplay`] handle, so [`run_app`] stays free of the
// panel's transport details. `MapDisplay` owns the `Ls021Flpr` panel and exposes the methods the loop
// calls:
//   - `poll_overlay`     — this frame's hold-bulge state (dirty edge + live row span);
//   - `render_present`   — render the clean frame into the framebuffer + push it to glass;
//   - `present_bulge`    — re-present the hold bulge over the clean map.
// The FLPR owns the panel outright (whole-frame scan per push → no shared bus), so the map plane pushes
// both the clean frame and the bulge itself; the input plane only recognises gestures. The seam it goes
// through, [`DisplayDriver`], is the deliberate panel-swap point (a follow-up PR moves it into
// obc-platform and makes the simulator the second backend).

/// What [`MapDisplay::render_present`] reports for one map frame: whether the push reached glass
/// (`false` → a transport fault to retry, #66), the render's [`RenderStats`], and the render / push
/// timings (µs) the RTT log + the VCOM telemetry carry.
pub(crate) struct FramePresent {
    pub(crate) ok: bool,
    // Read by the ride loop's telemetry/log lines only — the status build presents text frames
    // whose stats are all `default()`, so it never looks.
    #[cfg_attr(not(has_map), allow(dead_code))]
    pub(crate) stats: RenderStats,
    pub(crate) render_us: u64,
    pub(crate) push_us: u64,
}

/// Draw a full-screen [boot fault](obc_app::BootFault) to glass and return — the **undismissable**
/// storage-failure screen (no card / no map file / unreadable map). `main` brings the display up
/// first, then calls this at the fatal SD/map sites before dropping to the heartbeat idle, so the
/// rider sees *what's wrong* instead of a silently dark panel. Reuses the map plane's
/// [`render_present`](MapDisplay::render_present) so the fault frame lands through the same backend
/// push (and the same self-diffing FLPR scan) as any other frame; one push holds, since the message
/// never changes. Free-standing (not tied to an [`App`]) because at boot there may be no map to
/// build one around. Backend-agnostic: the one concrete `MapDisplay` this build compiled.
pub(crate) async fn show_boot_fault(display: &mut MapDisplay, fault: obc_app::BootFault) {
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
    display
        .render_present(None, |d| {
            let mut fbdev = FbDevice64::new(d.fb_mut(), FRAME_W as u32, FRAME_H as u32);
            obc_app::draw_boot_fault(&mut fbdev, FRAME_W as i32, FRAME_H as i32, color_fn, fault);
            RenderStats::default()
        })
        .await;
}

/// Consecutive failed presents that trigger one FLPR relaunch (#349): each failure already costs a
/// full frame-deadline spin inside the transport (250 ms), so three in a row (~0.75 s) is far past any
/// transient — the FLPR is wedged, escalate.
const PUSH_FAILS_PER_RELAUNCH: u8 = 3;
/// Consecutive relaunches that may fail (the launch erroring, or the presents after it still timing
/// out) before the device stops touching the FLPR and degrades to the heartbeat idle (#349).
const MAX_CONSEC_RELAUNCHES: u8 = 3;

/// The map plane's display handle: the `Ls021Flpr` panel owned outright (whole-frame scan per push →
/// no shared bus), plus the shared `InputPlane` it composites the bulge from and the gate/source GPIO
/// lines it must keep driven for the program's life.
pub(crate) struct MapDisplay {
    pub(crate) panel: Ls021Flpr<'static>,
    pub(crate) input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    /// The last live bulge's rows, so the trailing clear wipes exactly them, not the whole hint band.
    pub(crate) last_overlay_span: Option<(u16, u16)>,
    /// Consecutive failed pushes (map presents **and** bulge pushes — a bulge-only wedge must
    /// escalate too) since the last success; [`PUSH_FAILS_PER_RELAUNCH`] of them fire a relaunch.
    pub(crate) push_fails: u8,
    /// Relaunches run without a successful push in between; [`MAX_CONSEC_RELAUNCHES`] of them
    /// degrade the device. Cleared by any push that reaches glass.
    pub(crate) consec_relaunches: u8,
    /// A relaunch landed → the ride loop must fold in a full map repaint (`take_relaunch_repaint`).
    pub(crate) relaunch_repaint: bool,
    /// Terminal (until power-cycle): the FLPR would not come back after [`MAX_CONSEC_RELAUNCHES`]
    /// attempts. All pushes become no-ops (each would cost a frame-deadline spin against a dead
    /// core); the ride loop drops to the heartbeat idle. COM + the M33-held panel GPIOs keep the
    /// glass DC-bias-safe throughout — see [`relaunch_flpr`]'s doc.
    pub(crate) degraded: bool,
    /// The gate + source lines the FLPR drives — held only to keep them configured as outputs for the
    /// program's life (never touched after launch); dropping them would float the panel.
    pub(crate) _gate_bus: [Output<'static>; 4],
    pub(crate) _src_bus: [Output<'static>; 8],
    /// The zero-CPU hardware COM generator (`com-hw` build): held for the program's life like the
    /// gate/source buses — dropping it would stop the toggle and let the panel DC-bias. The default DK
    /// build has no field here (the M33 `com_task` owns the COM pins instead).
    #[cfg(feature = "com-hw")]
    pub(crate) _com_hw: HwCom,
}

impl MapDisplay {
    /// Sample the shared `InputPlane` once per frame (the map plane is the sole owner of the FLPR
    /// overlay bookkeeping): the dirty edge (live while the bulge animates, plus one trailing clear)
    /// and the live bulge's **row span** (`None` when quiet), so the map present can go *around* it and
    /// `present_bulge` can re-present it.
    #[inline(always)]
    pub(crate) fn poll_overlay(&mut self) -> (bool, Option<(u16, u16)>) {
        self.input_plane.lock(|c| {
            let p = &mut *c.borrow_mut();
            (p.take_overlay_dirty(), p.overlay_rows(FRAME_W as i32, FRAME_H as i32))
        })
    }

    /// The live encoder hold-progress from the shared input plane (0.0–1.0). Fed to the map render
    /// so the in-screen confirm fills (the factory-Reset bar) track the hold — `App`'s own input
    /// plane isn't driven on the two-plane firmware, so without this the bar never fills. (The
    /// status build has no in-screen fills, so only the ride loop calls it.)
    #[cfg_attr(not(has_map), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn hold_progress(&self) -> f32 {
        self.input_plane.lock(|c| c.borrow().encoder_hold_progress())
    }

    /// Whether a hold is **charging** right now — either button down, its long-press not yet fired.
    /// The pre-fire window the ride loop defers expensive map redraws in, so the bulge keeps its
    /// cadence instead of waiting out a 150–300 ms map frame mid-charge.
    #[cfg_attr(not(has_map), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn hold_charging(&self) -> bool {
        self.input_plane.lock(|c| {
            let p = c.borrow();
            p.encoder_hold_progress() > 0.0 || p.back_hold_progress() > 0.0
        })
    }

    /// Cancel any in-flight hold on the shared input plane — rung by the ride loop after a gesture
    /// changed the screen stack ([`App::take_hold_cancel`](obc_app::App::take_hold_cancel)), so a
    /// long-press charging over the *old* top can't complete onto the new one (issue #480).
    #[cfg_attr(not(has_map), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn cancel_holds(&self) {
        self.input_plane.lock(|c| c.borrow_mut().cancel_holds());
    }

    /// Render the clean frame into the owned panel and **self-diff** it to glass: push only the rows
    /// that changed since the last present. With a live bulge, the seam's `present(exclude)` clips its
    /// rows out (`overlay_span`) and leaves them for `present_bulge` — the FLPR's ~44 ms full-frame
    /// scan would otherwise blank the bulge for that whole scan (the pop-flicker), and even a partial
    /// clean push would flash it off. No shared bus: the map plane owns every push here. Marked
    /// `#[inline(always)]` with a generic (non-`dyn`) `render` so the deep render folds into the
    /// caller's frame rather than nesting another (the stack regression).
    #[inline(always)]
    pub(crate) async fn render_present(
        &mut self,
        overlay_span: Option<(u16, u16)>,
        mut render: impl FnMut(&mut dyn DisplayDriver) -> RenderStats,
    ) -> FramePresent {
        let t_render = Instant::now();
        let stats = render(&mut self.panel);
        let render_us = t_render.elapsed().as_micros();
        if self.degraded {
            // Terminal FLPR-down mode (#349): don't spin a frame deadline against a dead core —
            // drop the frame, reporting `ok` so the caller doesn't latch an endless retry. The
            // ride loop has already dropped (or is about to drop) to the heartbeat idle; the `ble`
            // status build keeps its radio useful with the glass frozen on the last good frame.
            return FramePresent { ok: true, stats, render_us, push_us: 0 };
        }
        let t_push = Instant::now();
        // Self-diffing present through the seam, clipped around a live bulge's rows so
        // `present_bulge` owns them (issue #163/#201/#345). The await frees the M33 for the whole
        // scan (#347) — and suspending the map plane here is exactly what guarantees the
        // framebuffer stays untouched while the FLPR reads it.
        let ok = self.panel.present(overlay_span).await;
        if !ok {
            // The push didn't reach glass (a stalled FLPR), but the self-diffing present already
            // advanced its row-hash store to this frame — so the caller's latched `pending_map_redraw`
            // retry would diff the identical `fb` against an up-to-date store and re-push *nothing*,
            // stranding the rows that missed glass. Re-arm a full push so the retry re-seeds the store
            // and repaints every row.
            self.panel.reset_diff();
        }
        let push_us = t_push.elapsed().as_micros();
        self.note_push(ok).await;
        FramePresent { ok, stats, render_us, push_us }
    }

    /// Present the hold bulge over the clean map (the FLPR bulge rides this map plane — no shared SPI
    /// bus to serialise against). While the bulge is live this re-composites its rows every frame (the
    /// map present clipped them out via its `exclude`, so the fresh backdrop + bulge land here — no
    /// mid-pop flash). Only the active bulge's rows are touched (the FLPR fast-forwards the gate to them
    /// + early-stops).
    ///
    /// The trailing clear (bulge just went quiet) wipes **the same rows** the last bulge used, because
    /// the self-diffing map present no longer guarantees it touched those rows: the bulge composited
    /// glass content the row-hash diff can't see (the store tracks the clean `fb`), so if the map
    /// content there is unchanged the diff skips it and the stale bulge would strand without this clear.
    /// The clear re-pushes the clean `fb` rows, which the store already agrees with, so the next present
    /// stays quiet there. It is driven off [`last_overlay_span`](Self#) (cleared only on a **successful**
    /// push), not the one-shot `overlay_dirty` edge — so a one-frame FLPR stall during the clear is
    /// retried on the next frame rather than stranding the bulge with no edge left to re-fire it.
    #[inline(always)]
    pub(crate) async fn present_bulge(&mut self, overlay_span: Option<(u16, u16)>, overlay_dirty: bool) {
        let _ = overlay_dirty; // `last_overlay_span` drives the clear so a stalled clear retries — see the doc.
        if self.degraded {
            return; // FLPR down for good (#349) — no push to retry against.
        }
        if let Some((y0, rows)) = overlay_span {
            let t_push = Instant::now();
            let ok = Self::composite_push(&mut self.panel, self.input_plane, y0, rows).await;
            let push_us = t_push.elapsed().as_micros();
            self.last_overlay_span = Some((y0, rows));
            if ok {
                // Per-tick during a hold — `debug` so it doesn't flood the default log.
                defmt::debug!("overlay frame: bulge push {=u64} us ({=u16} rows @ y{=u16})", push_us, rows, y0);
            } else {
                defmt::warn!("overlay frame: bulge push failed (FLPR stalled?) — retrying next overlay tick");
            }
            self.note_push(ok).await;
        } else if let Some((y0, rows)) = self.last_overlay_span {
            // Trailing clear: re-present just the last bulge's rows with nothing composited = the clean
            // map restored under the just-gone bulge (the self-diffing map present may have skipped
            // them, so this is what actually wipes the bulge — see the method docs). Drop
            // `last_overlay_span` only when the push lands, so a stalled FLPR retries next frame.
            let ok = Self::composite_push(&mut self.panel, self.input_plane, y0, rows).await;
            if ok {
                self.last_overlay_span = None;
            } else {
                defmt::warn!("overlay frame: trailing clear failed (FLPR stalled?) — retrying next frame");
            }
            self.note_push(ok).await;
        }
    }

    /// One overlay composite + push of the bulge band's rows `[y0, y0+rows)` through the seam —
    /// shared by the live-bulge repaint and the trailing clear above. An associated fn (not a
    /// closure — closures can't await) taking the panel + plane apart so `present_bulge` can call
    /// it around its `&mut self` borrows.
    #[inline(always)]
    async fn composite_push(
        panel: &mut Ls021Flpr<'static>,
        input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
        y0: u16,
        rows: u16,
    ) -> bool {
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        panel
            .present_overlay(OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows }, &mut |band: &mut Band| {
                input_plane.lock(|cell| cell.borrow().render_overlay(band, FRAME_W as f32, FRAME_H as f32, color_fn));
            })
            .await
    }

    /// Fold one push outcome into the **relaunch escalation** (#349) — every FLPR push (map present,
    /// bulge, trailing clear) reports here. A success clears both counters; the
    /// [`PUSH_FAILS_PER_RELAUNCH`]th consecutive failure runs a full [`relaunch_flpr`] (the failing
    /// push already logged its `dump_flpr_state` snapshot — hung vs reset vs corrupted shared RAM).
    /// When [`MAX_CONSEC_RELAUNCHES`] relaunches pass without a single successful push in between,
    /// the escalation stops for good: `degraded` latches, every later push becomes a no-op, and the
    /// ride loop drops to the heartbeat idle. **COM never stops either way** — it runs on the M33
    /// (`com_task` / `HwCom`), so the panel stays DC-bias-safe through a dead FLPR, a relaunch, and
    /// the degraded idle alike (see [`relaunch_flpr`]'s doc; that property is load-bearing).
    async fn note_push(&mut self, ok: bool) {
        if ok {
            self.push_fails = 0;
            self.consec_relaunches = 0;
            return;
        }
        self.push_fails += 1;
        if self.push_fails < PUSH_FAILS_PER_RELAUNCH {
            return;
        }
        self.push_fails = 0;
        if self.consec_relaunches >= MAX_CONSEC_RELAUNCHES {
            // The last K relaunches all failed to restore service (each proven by the next
            // N failed pushes, or by erroring outright) — stop pounding a dead core.
            self.degraded = true;
            defmt::error!(
                "FLPR: {=u8} consecutive relaunches failed — degrading to heartbeat idle (COM keeps the panel DC-bias-safe; power-cycle to retry)",
                MAX_CONSEC_RELAUNCHES
            );
            return;
        }
        self.consec_relaunches += 1;
        defmt::error!(
            "FLPR: {=u8} consecutive failed pushes — full relaunch (attempt {=u8}/{=u8})",
            PUSH_FAILS_PER_RELAUNCH,
            self.consec_relaunches,
            MAX_CONSEC_RELAUNCHES
        );
        match relaunch_flpr().await {
            Ok(()) => {
                // Fresh core, no frame history: the diff store may believe rows are on glass that
                // never landed — force the next present to repaint every row, and tell the ride
                // loop to schedule that present even if nothing else dirtied the map.
                self.panel.reset_diff();
                self.relaunch_repaint = true;
                defmt::info!("FLPR: relaunch OK — alive again, full repaint armed");
            }
            Err(e) => defmt::error!("FLPR: relaunch failed ({}) — escalating on the next failed pushes", e),
        }
    }

    /// One-shot: a relaunch landed since the last call, so the ride loop must fold in a full map
    /// repaint (the fresh FLPR has no frame history; the diff store was reset).
    #[inline(always)]
    pub(crate) fn take_relaunch_repaint(&mut self) -> bool {
        core::mem::take(&mut self.relaunch_repaint)
    }

    /// Terminal FLPR-down state (#349): [`MAX_CONSEC_RELAUNCHES`] relaunches failed. The ride loop
    /// checks this each pass and drops to the heartbeat idle. (The status build never calls it —
    /// there, a degraded display just freezes the glass while BLE keeps serving.)
    #[cfg_attr(not(has_map), allow(dead_code))]
    #[inline(always)]
    pub(crate) fn degraded(&self) -> bool {
        self.degraded
    }
}
