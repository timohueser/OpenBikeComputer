//! The **map plane** — the thread-mode half of the two-plane display machinery. It was first split
//! out of `main.rs` (issue #351); the machinery then lived in one `planes.rs`, and this file is the
//! display half of that final split, so the panel machinery no longer shares a file with the
//! gesture/input handling (the [input plane](crate::input_plane)).
//!
//! The ride loop drives the screen through the [`MapDisplay`] handle, so
//! [`run_app`](crate::ride::run_app) stays free of the panel's transport details. `MapDisplay` owns
//! the `Ls021Flpr` panel and exposes the methods the loop calls:
//!   - `poll_overlay`     — this frame's hold-bulge state (dirty edge + live row span);
//!   - `render_present`   — render the clean frame into the framebuffer + push it to glass;
//!   - `present_bulge`    — re-present the hold bulge over the clean map.
//!
//! The FLPR owns the panel outright (whole-frame scan per push → no shared bus), so the map plane
//! pushes both the clean frame and the bulge itself; the input plane only recognises gestures. The
//! seam it goes through, [`DisplayDriver`](obc_platform::DisplayDriver), lives in obc-platform and is
//! the deliberate panel-swap point — the simulator is its second backend, so the abstraction stays
//! honest off-device too.
//!
//! The one piece shared with the input plane is the `&'static BlockingMutex<…, RefCell<InputPlane>>`
//! handle both take as a parameter (constructed and owned by `main`): the input plane advances the
//! hold bulge under that lock and the map plane composites the same live state into its overlay push.

use core::cell::RefCell;

use embassy_nrf::gpio::Output;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_time::Instant;
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_app::InputPlane;
// `Band` is the frame-absolute draw view the map plane's `present_overlay` drawer paints the
// hold bulge into.
use obc_platform::ls021::{FRAME_H, FRAME_W};
use obc_platform::{Band, DisplayDriver, FbDevice64, OverlayRegion};
use obc_render::RenderStats;

#[cfg(feature = "com-hw")]
use crate::com_hw::HwCom;
use crate::ls021_flpr::{relaunch_flpr, Ls021Flpr};

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
// clips it out of the changed-row spans it pushes (`obc_platform::ls021::RowDiff::diff_clipped`), leaving
// those rows for the map plane's own `MapDisplay::present_bulge`.

/// What [`MapDisplay::render_present`] reports for one map frame: whether the push reached glass
/// (`false` → a transport fault to retry, #66), the render's [`RenderStats`], and the render / push
/// timings (µs) the RTT log + the VCOM telemetry carry.
pub(crate) struct FramePresent {
    pub(crate) ok: bool,
    // Read by the ride loop's telemetry/log lines only.
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
    /// plane isn't driven on the two-plane firmware, so without this the bar never fills.
    #[inline(always)]
    pub(crate) fn hold_progress(&self) -> f32 {
        self.input_plane.lock(|c| c.borrow().encoder_hold_progress())
    }

    /// Whether a hold is **charging** right now — either button down, its long-press not yet fired.
    /// The pre-fire window the ride loop defers expensive map redraws in, so the bulge keeps its
    /// cadence instead of waiting out a 150–300 ms map frame mid-charge.
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
    /// checks this each pass and drops to the heartbeat idle.
    #[inline(always)]
    pub(crate) fn degraded(&self) -> bool {
        self.degraded
    }
}
