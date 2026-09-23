//! The map plane: the thread-mode half of the two-plane display machinery.
//!
//! The ride loop drives the screen through the [`MapDisplay`] handle, so
//! [`run_app`](crate::ride::run_app) stays free of the panel's transport details. `MapDisplay` owns
//! the `Ls021Flpr` panel and exposes `poll_overlay` (this frame's hold-bulge state), `render_frame`
//! (render into the resident framebuffer, no push), `present_frame` (push it to glass) and
//! `present_bulge`.
//!
//! Render and present are separate calls: the ride loop renders while its store guard is live,
//! because the render closure borrows the open reader, and presents after the guard is gone, so
//! object operations never queue behind the 44 ms panel scan. Framebuffer ownership makes the split
//! safe: both halves borrow the same owned `frame` field, so nothing can render into it while a
//! present's shared borrow is scanning it.
//!
//! The FLPR owns the panel outright — a whole-frame scan per push, so no shared bus — and the map
//! plane pushes both the clean frame and the bulge. The input plane only recognises gestures. The
//! one piece shared with it is the `&'static BlockingMutex<…, RefCell<InputPlane>>` both take as a
//! parameter: the input plane advances the hold bulge under that lock, and the map plane composites
//! the same live state into its overlay push.

use core::cell::RefCell;

use embassy_nrf::gpio::Output;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_time::Instant;
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_app::InputPlane;
// `Band` is the frame-absolute draw view the overlay drawer paints the hold bulge into.
use obc_display::display_contracts::{OverlayPresenter, Presenter};
use obc_display::ls021::{RowDamage, RowWindow, FRAME_H, FRAME_W};
use obc_display::{Band, FbDevice64};
use obc_render::RenderStats;

#[cfg(feature = "com-hw")]
use crate::com_hw::HwCom;
use crate::ls021_flpr::{relaunch_flpr, Frame64, Ls021Flpr};

// A full-width strip keeps both edge hints in the same panel scan. Twelve rows fit the presenter's
// bounded scratch without adding a full-width overlay buffer.
const OVL_ROWS: u16 = 12;

// The "present the rows around a live bulge" discipline lives inside the self-diffing present: the
// map plane presents with `damage_around(bulge window)`, which clips the bulge's rows out of the
// spans it pushes and leaves them for `MapDisplay::present_bulge`.

/// Draw a static boot fault when card bring-up fails before USB recovery is available.
pub(crate) async fn show_boot_fault(display: &mut MapDisplay, fault: obc_app::BootFault) {
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
    display.render_frame(|f| {
        let mut fbdev = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
        obc_app::draw_boot_fault(&mut fbdev, FRAME_W as i32, FRAME_H as i32, color_fn, fault);
        RenderStats::default()
    });
    let _ = display.present_frame(None).await;
}

/// Keep the recovery UI and its restart control live while USB replaces an unreadable map.
pub(crate) async fn run_map_recovery(
    display: &mut MapDisplay,
    led: &mut Output<'static>,
    fault: obc_app::BootFault,
) -> ! {
    let mut recovery = obc_app::fault::BootRecovery::new(fault);
    let mut redraw = true;
    loop {
        let transfer = crate::link::map_transfer_state();
        // A queued press cannot accept a success page that has not reached the display.
        let changed = recovery.update(transfer);
        while let Ok(gesture) = crate::input_plane::GESTURES.try_receive() {
            if !redraw && !changed && recovery.restart_requested(gesture) && !crate::flat_store::transfer_active() {
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
        while crate::input_plane::CHORDS.try_receive().is_ok() {}
        if redraw || changed {
            display.render_frame(|f| {
                let mut fbdev = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
                recovery.draw(&mut fbdev, FRAME_W as i32, FRAME_H as i32, |c| Rgb565::from(RawU16::new(c)));
                RenderStats::default()
            });
            redraw = !display.present_frame(None).await.0;
        }
        led.toggle();
        embassy_time::Timer::after_millis(500).await;
    }
}

/// Consecutive failed presents that trigger one FLPR relaunch. Each failure already costs a full
/// frame-deadline spin inside the transport, so three in a row is far past any transient.
const PUSH_FAILS_PER_RELAUNCH: u8 = 3;
/// Consecutive relaunches that may fail — the launch erroring, or the presents after it still
/// timing out — before the device stops touching the FLPR and degrades to the heartbeat idle.
const MAX_CONSEC_RELAUNCHES: u8 = 3;

/// The map plane's display handle: the (`Frame64`, `Ls021Flpr`) pairing owned outright, with the
/// resident frame next to its presenter, plus the shared `InputPlane` it composites the bulge from
/// and the gate and source lines it must keep driven for the program's life.
pub(crate) struct MapDisplay {
    /// The resident device-64 frame. Rendering borrows it mutably; a base present shares it with
    /// the FLPR for the whole scan.
    pub(crate) frame: Frame64,
    pub(crate) panel: Ls021Flpr<'static>,
    pub(crate) input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    /// The last live bulge's rows, so the trailing clear wipes exactly them.
    pub(crate) last_overlay_span: Option<(u16, u16)>,
    /// Consecutive failed pushes, map presents and bulge pushes alike, since the last success.
    /// [`PUSH_FAILS_PER_RELAUNCH`] of them fire a relaunch.
    pub(crate) push_fails: u8,
    /// Relaunches run without a successful push in between. [`MAX_CONSEC_RELAUNCHES`] of them
    /// degrade the device. Any push that reaches glass clears it.
    pub(crate) consec_relaunches: u8,
    /// A relaunch landed, so the ride loop must fold in a full map repaint.
    pub(crate) relaunch_repaint: bool,
    /// Terminal until a power cycle: the FLPR would not come back after [`MAX_CONSEC_RELAUNCHES`]
    /// attempts. Every push becomes a no-op, because each would cost a frame-deadline spin against a
    /// dead core, and the ride loop drops to the heartbeat idle. COM and the M33-held panel GPIOs
    /// keep the glass DC-bias-safe throughout.
    pub(crate) degraded: bool,
    /// The gate and source lines the FLPR drives, held only to keep them configured as outputs for
    /// the program's life. Dropping them would float the panel.
    pub(crate) _gate_bus: [Output<'static>; 4],
    pub(crate) _src_bus: [Output<'static>; 8],
    /// The zero-CPU hardware COM generator, held for the program's life like the buses above:
    /// dropping it would stop the toggle and let the panel take a DC bias.
    #[cfg(feature = "com-hw")]
    pub(crate) _com_hw: HwCom,
}

impl MapDisplay {
    /// Sample the shared `InputPlane` once per frame: the live bulge's row span, or `None` when it
    /// is quiet, so the map present can go around it and `present_bulge` can re-present it.
    #[inline(always)]
    pub(crate) fn poll_overlay(&self) -> Option<(u16, u16)> {
        self.input_plane.lock(|c| c.borrow().overlay_rows(FRAME_W as i32, FRAME_H as i32))
    }

    /// Whether the overlay plane still owes glass a push: a live bulge, or a trailing clear that has
    /// not landed. `last_overlay_span` is `Some` from the first bulge push until the clear succeeds,
    /// which is exactly "the layer is not clean". It is what keeps the ride loop on the short
    /// animation cadence while a bulge is on screen.
    #[inline(always)]
    pub(crate) fn overlay_owed(&self) -> bool {
        self.last_overlay_span.is_some()
    }

    /// The live `(Select, Back)` hold progress from the shared input plane. It is fed to the app so
    /// the in-screen confirm bars track the Select hold and a Back hold defers a landing card:
    /// `App`'s own input plane is not driven on this firmware, so without it the bar never fills
    /// and no hold is seen at all.
    #[inline(always)]
    pub(crate) fn hold_progress(&self) -> (f32, f32) {
        self.input_plane.lock(|c| {
            let p = c.borrow();
            (p.select_hold_progress(), p.back_hold_progress())
        })
    }

    /// Whether an ordinary hold or the Assistant chord is charging right now. This is the pre-fire
    /// window in which the ride loop defers expensive map redraws, so the bulge keeps its cadence.
    #[inline(always)]
    pub(crate) fn hold_charging(&self) -> bool {
        self.input_plane.lock(|c| c.borrow().hold_charging())
    }

    /// Cancel any in-flight hold on the shared input plane. The ride loop rings it after a gesture
    /// changed the screen stack, so a long press charging over the old top cannot complete onto the
    /// new one.
    #[inline(always)]
    pub(crate) fn cancel_holds(&self) {
        self.input_plane.lock(|c| c.borrow_mut().cancel_holds());
    }

    /// Render the clean frame into the owned `Frame64`: the sync half of the render and present
    /// split. No push and no await, so the ride loop can call it while its store guard is live and
    /// push the result with [`present_frame`](Self::present_frame) after the guard is gone. It is
    /// `#[inline(always)]` with a generic, non-`dyn` `render`, so the deep render folds into the
    /// caller's frame rather than nesting another.
    #[inline(always)]
    pub(crate) fn render_frame(&mut self, mut render: impl FnMut(&mut Frame64) -> RenderStats) -> (RenderStats, u64) {
        let t_render = Instant::now();
        let stats = render(&mut self.frame);
        (stats, t_render.elapsed().as_micros())
    }

    /// Self-diff the already-rendered resident frame to glass: the async half of the split, pushing
    /// only the rows that changed since the last present. With a live bulge, presenting with
    /// `damage_around(bulge window)` clips its rows out and leaves them for `present_bulge`,
    /// because the full-frame scan would otherwise blank the bulge for its whole duration. Returns
    /// `(reached_glass, push_us)`; `false` is a transport fault the caller retries. Rendering
    /// between a [`render_frame`](Self::render_frame) and this push is impossible for anyone but the
    /// caller: both halves borrow the same owned `frame` field through `&mut self`.
    #[inline(always)]
    pub(crate) async fn present_frame(&mut self, overlay_span: Option<(u16, u16)>) -> (bool, u64) {
        if self.degraded {
            // Terminal FLPR-down mode: do not spin a frame deadline against a dead core. Drop the
            // frame and report `ok`, so the caller does not latch an endless retry. The ride loop
            // has already dropped to the heartbeat idle.
            return (true, 0);
        }
        let t_push = Instant::now();
        // Self-diffing present through the contracts, clipped around a live bulge's rows so
        // `present_bulge` owns them. The await frees the M33 for the whole scan, and the shared
        // `&self.frame` borrow held across it is what guarantees the framebuffer stays untouched
        // while the FLPR reads it.
        let ok = self.panel.present(&self.frame, RowDamage::SelfDiff { exclude: overlay_span }).await.is_ok();
        if !ok {
            // The push did not reach glass, but the self-diffing present already advanced its
            // row-hash store to this frame, so the caller's latched retry would diff an identical
            // frame against an up-to-date store and push nothing, stranding the rows that missed
            // glass. Re-arm a full push.
            self.panel.reset_diff();
        }
        let push_us = t_push.elapsed().as_micros();
        self.note_push(ok).await;
        (ok, push_us)
    }

    /// Present the hold bulge over the clean map. While the bulge is live this re-composites its
    /// rows every frame: the map present clipped them out through its `exclude`, so the fresh
    /// backdrop and the bulge land here together. Only the active bulge's rows are touched.
    ///
    /// The trailing clear, when the bulge has just gone quiet, wipes the same rows the last bulge
    /// used, because the self-diffing map present no longer guarantees it touched them: the bulge
    /// composited glass content the row-hash diff cannot see, so if the map content there is
    /// unchanged the diff skips it and the stale bulge would strand. The clear re-pushes the clean
    /// rows, which the store already agrees with. It is driven off `last_overlay_span`, cleared only
    /// on a successful push, so a one-frame stall during the clear is retried next frame.
    #[inline(always)]
    pub(crate) async fn present_bulge(&mut self, overlay_span: Option<(u16, u16)>) {
        if self.degraded {
            return; // FLPR down for good (#349) — no push to retry against.
        }
        if let Some((y0, rows)) = overlay_span {
            let t_push = Instant::now();
            let ok = Self::composite_push(&mut self.panel, &mut self.frame, self.input_plane, y0, rows).await;
            let push_us = t_push.elapsed().as_micros();
            self.last_overlay_span = Some((y0, rows));
            if ok {
                // Per-tick during a hold, so `debug` keeps it out of the default log.
                defmt::debug!("overlay frame: bulge push {=u64} us ({=u16} rows @ y{=u16})", push_us, rows, y0);
            } else {
                defmt::warn!("overlay frame: bulge push failed (FLPR stalled?) — retrying next overlay tick");
            }
            self.note_push(ok).await;
        } else if let Some((y0, rows)) = self.last_overlay_span {
            // Trailing clear: re-present just the last bulge's rows with nothing composited, which
            // restores the clean map under the bulge that has just gone. Drop `last_overlay_span`
            // only when the push lands, so a stalled FLPR retries next frame.
            let ok = Self::composite_push(&mut self.panel, &mut self.frame, self.input_plane, y0, rows).await;
            if ok {
                self.last_overlay_span = None;
            } else {
                defmt::warn!("overlay frame: trailing clear failed (FLPR stalled?) — retrying next frame");
            }
            self.note_push(ok).await;
        }
    }

    /// One overlay composite and push of the bulge band's rows, shared by the live-bulge repaint and
    /// the trailing clear above. It is an associated function rather than a closure, because
    /// closures cannot await, and it takes the frame, panel and plane apart so `present_bulge` can
    /// call it around its `&mut self` borrows.
    #[inline(always)]
    async fn composite_push(
        panel: &mut Ls021Flpr<'static>,
        frame: &mut Frame64,
        input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
        y0: u16,
        rows: u16,
    ) -> bool {
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        let end = y0 + rows;
        let mut y = y0;
        while y < end {
            let rows = (end - y).min(OVL_ROWS);
            let region = RowWindow { x0: 0, y0: y, w: FRAME_W as u16, rows };
            if panel
                .present_overlay(frame, region, |band: &mut Band| {
                    input_plane
                        .lock(|cell| cell.borrow().render_overlay(band, FRAME_W as f32, FRAME_H as f32, color_fn));
                })
                .await
                .is_err()
            {
                return false;
            }
            y += rows;
        }
        true
    }

    /// Fold one push outcome into the relaunch escalation: every push reports here. A success clears
    /// both counters, and the [`PUSH_FAILS_PER_RELAUNCH`]th consecutive failure runs a full
    /// relaunch. When [`MAX_CONSEC_RELAUNCHES`] relaunches pass without a single successful push in
    /// between, the escalation stops for good: `degraded` latches, every later push becomes a
    /// no-op, and the ride loop drops to the heartbeat idle. COM never stops either way, so the
    /// panel stays DC-bias-safe through a dead FLPR, a relaunch and the degraded idle alike.
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
            // The last relaunches all failed to restore service, so stop pounding a dead core.
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
                // never landed. Force the next present to repaint every row, and tell the ride loop
                // to schedule that present even if nothing else dirtied the map.
                self.panel.reset_diff();
                self.relaunch_repaint = true;
                defmt::info!("FLPR: relaunch OK — alive again, full repaint armed");
            }
            Err(e) => defmt::error!("FLPR: relaunch failed ({}) — escalating on the next failed pushes", e),
        }
    }

    /// One-shot: a relaunch landed since the last call, so the ride loop must fold in a full map
    /// repaint.
    #[inline(always)]
    pub(crate) fn take_relaunch_repaint(&mut self) -> bool {
        core::mem::take(&mut self.relaunch_repaint)
    }

    /// Terminal FLPR-down state: [`MAX_CONSEC_RELAUNCHES`] relaunches failed. The ride loop checks
    /// this each pass and drops to the heartbeat idle.
    #[inline(always)]
    pub(crate) fn degraded(&self) -> bool {
        self.degraded
    }
}
