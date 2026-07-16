//! **The LS021B7DD02 pairing** — everything specific to the shipping Sharp memory-LCD panel and the
//! row-span presentation strategy built around it, owned in one place so the generic
//! [`display_contracts`](crate::display_contracts) stay free of any panel's width, wire format, or
//! damage model.
//!
//! The shipping display pairing is `Device64Frame<FRAME_W, FRAME_H>` + a row-span presenter (the
//! board's LS021/FLPR backend; the simulator's host backend). This module owns the pairing's shared
//! substance:
//!
//! - [`FRAME_W`] / [`FRAME_H`] — the panel-native frame geometry, the single authority every
//!   frame-sized thing derives from (the board's resident plane, the simulator's default window,
//!   the render-call viewports). Pinned against the wire pack's row width below.
//! - [`rowdiff`] — the **damage strategy**: the per-row hash store ([`RowDiff`]), the span-emitting
//!   self-diff ([`diff_rows`]), the live-overlay span clip ([`clip_span`]), and the exact-diff
//!   host oracle ([`spans_missed_changes`]). Row hashing and span masking are *this pairing's*
//!   choice — the generic contracts impose no damage model on other panels.
//! - [`wire`] — the LS021 source-bus **wire pack** (device-64 row → the 6-line DDR words the FLPR
//!   clocks out), host-tested here as the normative reference the C blob ports line-for-line.
//! - [`RowDamage`] / [`RowWindow`] — the pairing's damage/region vocabulary behind the contracts'
//!   `Presenter::Damage` / `OverlayPresenter::Region` associated types, shared by the board and the
//!   simulator so the two backends speak (and test) one strategy.
//! - [`composite_into_resident`] — the **mutate-and-restore overlay composite** the FLPR transport
//!   needs (the coprocessor scans the resident frame directly, and a second full frame is banned):
//!   save the clean window bytes, write the composited window in, push, restore byte-identically.
//!   Transport-generic so the host conformance tests drive the *same* engine the board runs.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use crate::panel::{composite_overlay_window, Band};

pub mod rowdiff;
pub mod wire;

pub use rowdiff::{clip_span, diff_rows, row_hash, spans_missed_changes, RowDiff};

/// **The frame geometry — the single authority.** The frame the app renders and the LS021 pairing
/// presents: `FRAME_W × FRAME_H` device-64 bytes. Everything frame-sized derives from these two
/// constants (the board's `FB` plane, the `RowDiff` height, the overlay-window columns, every
/// render-call viewport, the simulator's default window); the board backend statically asserts its
/// panel-native geometry equals them, so a panel change can't silently desynchronize the
/// framebuffer the app renders from the frame the backend scans.
pub const FRAME_W: usize = 240;
/// Frame height in rows — see [`FRAME_W`].
pub const FRAME_H: usize = 320;

// The wire pack ([`wire`]) consumes exactly one `WIDTH`-pixel row per framebuffer row, so the
// panel-native row width *is* the frame width. Pin them together here at the single authority (the
// LS021/FLPR backend also re-asserts it against its FLPR gate-scan height).
const _: () = assert!(wire::WIDTH == FRAME_W, "ls021::wire::WIDTH diverged from ls021::FRAME_W");

/// The LS021 pairing's damage description — the `Presenter::Damage` type both the board and the
/// simulator presenters use. Callers construct it only through the contracts' neutral constructors
/// (`damage_full` / `damage_unknown` / `damage_around`), so rows never leak into generic code.
pub enum RowDamage {
    /// Forced full repaint: re-seed the row-hash store and push every row — the panel-reinit /
    /// transport-recovery damage, collapsing the old `reset_diff()` + full-present pair.
    Full,
    /// Self-diff against the row-hash store ([`RowDiff::diff_clipped`]), optionally going *around*
    /// a live overlay's rows: the half-open row span `exclude = Some((y0, rows))` is clipped out of
    /// the pushed spans (the store still tracks the clean frame for them) so a map redraw never
    /// flashes a live bulge off.
    SelfDiff {
        /// A live overlay's rows `(y0, rows)`, owned by the overlay plane this present.
        exclude: Option<(u16, u16)>,
    },
}

/// The LS021 pairing's overlay region — the `OverlayPresenter::Region` type: a bounded column
/// window on full-width rows. The row-addressed panel re-latches all columns of a touched row, so
/// exclusion and the row push widen to full-width rows `[y0, y0 + rows)` while the composite only
/// repaints the `[x0, x0 + w)` columns.
#[derive(Clone, Copy)]
pub struct RowWindow {
    /// First frame column of the overlay window.
    pub x0: u16,
    /// First frame row.
    pub y0: u16,
    /// Window width in columns.
    pub w: u16,
    /// Window height in rows — also the widened region's row span.
    pub rows: u16,
}

impl RowWindow {
    /// The smallest window covering `rect`, clamped to a `frame_w × frame_h` frame — the pairing's
    /// `OverlayPresenter::region` widening rule (shared by both backends).
    pub fn from_rect(rect: Rectangle, frame_w: u32, frame_h: u32) -> Self {
        let c = rect.intersection(&Rectangle::new(Point::zero(), Size::new(frame_w, frame_h)));
        Self { x0: c.top_left.x as u16, y0: c.top_left.y as u16, w: c.size.width as u16, rows: c.size.height as u16 }
    }

    /// The full-width row span `(y0, rows)` this window's rows occupy — what a base present
    /// excludes while the overlay is live ([`RowDamage::SelfDiff`]).
    pub fn exclude_span(&self) -> (u16, u16) {
        (self.y0, self.rows)
    }
}

/// The paired scratch buffers one [`composite_into_resident`] call borrows — kept together because
/// they describe the *same* window: `win` is the RGB565 composite the drawer paints
/// ([`composite_overlay_window`]'s target), `save` the clean device-64 window bytes the engine
/// restores after the push. Both are call-scoped transients on the caller's stack (the board's
/// `MAX_OVERLAY_*`-sized arrays), never resident state.
pub struct OverlayScratch<'a> {
    /// RGB565 composite window, ≥ `w × rows` pixels.
    pub win: &'a mut [u16],
    /// Clean device-64 window bytes for the restore, ≥ `w × rows` bytes.
    pub save: &'a mut [u8],
}

/// **The mutate-and-restore overlay composite** (#347): present `draw_overlay` composited over the
/// clean resident `fb` backdrop within `window`, for a transport that scans the resident frame
/// directly — so the composited window must transiently *be* in the frame. The clean window bytes
/// are saved (≤ the window's area), the composited window written in (each RGB565 scratch pixel
/// re-quantized to a device-64 byte through `quantize`), `push` drives the full-width rows
/// `[y0, y0 + rows)` to glass, and the clean bytes are restored — **byte-identically, push fault or
/// not** (a mid-scan transport reading restored clean bytes just paints clean rows; the caller
/// retries). The row-hash store keeps tracking the clean frame throughout: after the restore the
/// frame is byte-identical to before, so the store needs no touch-up.
///
/// The composite itself is the shared [`composite_overlay_window`] (backdrop fill + one
/// `draw_overlay` call over a frame-absolute [`Band`] — the caller's brief input-plane lock inside
/// it is taken once per overlay frame, never per row). `quantize` is the caller's RGB565 →
/// device-64 packer, passed in so this crate stays free of the reader's quantizer dependency —
/// both the board and the host tests pass the same `rgb565_to_device64`-based closure.
///
/// Both [`OverlayScratch`] slices must hold at least `w × rows` entries (panics otherwise — a
/// backend wiring bug, caught loudly). Transport-generic: the board's `push` is the blocking FLPR
/// span push; the host conformance double's copies the pushed rows to its glass — so the *same*
/// save/composite/push/restore engine is what the clean-frame postcondition tests.
pub fn composite_into_resident<E>(
    fb: &mut [u8],
    frame: Size,
    window: RowWindow,
    scratch: OverlayScratch<'_>,
    quantize: impl Fn(u16) -> u8,
    draw_overlay: &mut dyn FnMut(&mut Band),
    push: impl FnOnce(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    let OverlayScratch { win: win_scratch, save: save_scratch } = scratch;
    let (x0, y0, w, rows) = (window.x0 as usize, window.y0 as usize, window.w as usize, window.rows as usize);
    let fw = frame.width as usize;
    assert!(win_scratch.len() >= w * rows && save_scratch.len() >= w * rows, "overlay scratch smaller than the window");

    // 1. Composite the overlay ONCE into the window scratch over the clean `fb` backdrop: the
    //    shared helper fills the window from `fb` (device-64 → RGB565) and lets `draw_overlay`
    //    paint over it through a frame-absolute `Band`. `fb` is untouched so far.
    let rect = Rectangle::new(Point::new(x0 as i32, y0 as i32), Size::new(w as u32, rows as u32));
    composite_overlay_window(fb, frame, rect, win_scratch, draw_overlay);

    // 2. Save the clean window bytes, then write the composited window into the fb (re-quantized
    //    to device-64) — the transport scans the fb, so the overlay must transiently live there.
    for r in 0..rows {
        for c in 0..w {
            let idx = (y0 + r) * fw + x0 + c;
            save_scratch[r * w + c] = fb[idx];
            fb[idx] = quantize(win_scratch[r * w + c]);
        }
    }

    // 3. Push the full-width rows `[y0, y0+rows)` — the transport packs them from the fb, overlay
    //    included.
    let result = push(fb);

    // 4. Restore the clean map under the overlay — the fb is byte-identical to before, so the
    //    row-hash store (which tracks the clean fb) needs no touch-up. Runs on the fault path too.
    for r in 0..rows {
        for c in 0..w {
            fb[(y0 + r) * fw + x0 + c] = save_scratch[r * w + c];
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::Rgb565;

    use super::*;

    extern crate std;
    use std::vec;
    use std::vec::Vec;

    /// The real RGB565 → device-64 byte packer — the exact closure the board backend passes
    /// (`obc_reader::rgb565_to_device64` returns 0/85/170/255 per channel; `/85` recovers the
    /// 2-bit level), so the roundtrip through the backdrop expansion is exact.
    fn quant(px: u16) -> u8 {
        let (r, g, b) = obc_reader::rgb565_to_device64(px);
        ((r / 85) << 4) | ((g / 85) << 2) | (b / 85)
    }

    fn white() -> Rgb565 {
        Rgb565::new(31, 63, 31)
    }

    #[test]
    fn window_clamps_to_the_frame() {
        let w = RowWindow::from_rect(Rectangle::new(Point::new(-2, 6), Size::new(8, 8)), 10, 10);
        assert_eq!((w.x0, w.y0, w.w, w.rows), (0, 6, 6, 4));
        assert_eq!(w.exclude_span(), (6, 4));
    }

    /// The engine's contract: the composited window is what the transport scans, and the frame is
    /// byte-identical afterwards — on the success path *and* the fault path.
    #[test]
    fn composite_pushes_the_window_and_restores_the_frame() {
        let (fw, fh) = (8usize, 6usize);
        let mut fb: Vec<u8> = (0..fw * fh).map(|i| (i as u8) & 0x3F).collect();
        let clean = fb.clone();
        let window = RowWindow { x0: 4, y0: 2, w: 3, rows: 2 };
        let mut win = vec![0u16; 3 * 2];
        let mut save = vec![0u8; 3 * 2];
        let mut scanned: Vec<u8> = Vec::new();
        let r: Result<(), ()> = composite_into_resident(
            &mut fb,
            Size::new(fw as u32, fh as u32),
            window,
            OverlayScratch { win: &mut win, save: &mut save },
            quant,
            &mut |band| {
                // Paint frame-absolute (5, 3) into the window.
                band.fill_solid(&Rectangle::new(Point::new(5, 3), Size::new(1, 1)), white()).ok();
            },
            |fb_now| {
                scanned.extend_from_slice(fb_now);
                Ok(())
            },
        );
        assert!(r.is_ok());
        assert_eq!(fb, clean, "the frame is byte-identical after the push");
        // The transport saw the composite: (5,3) carries the overlay, its neighbours the backdrop.
        assert_eq!(scanned[3 * fw + 5], 0x3F, "the overlay pixel was in the scanned frame");
        assert_eq!(scanned[3 * fw + 4], clean[3 * fw + 4], "un-drawn window pixels scanned as the clean backdrop");
        assert_eq!(scanned[0], clean[0], "outside the window the scan is the clean frame");
    }

    #[test]
    fn a_push_fault_still_restores_the_frame() {
        let (fw, fh) = (6usize, 4usize);
        let mut fb: Vec<u8> = (0..fw * fh).map(|i| (i as u8) & 0x3F).collect();
        let clean = fb.clone();
        let window = RowWindow { x0: 2, y0: 1, w: 2, rows: 2 };
        let mut win = vec![0u16; 2 * 2];
        let mut save = vec![0u8; 2 * 2];
        let r = composite_into_resident(
            &mut fb,
            Size::new(fw as u32, fh as u32),
            window,
            OverlayScratch { win: &mut win, save: &mut save },
            quant,
            &mut |band| {
                band.fill_solid(&Rectangle::new(Point::new(2, 1), Size::new(2, 2)), white()).ok();
            },
            |_fb| Err("stalled"),
        );
        assert_eq!(r, Err("stalled"));
        assert_eq!(fb, clean, "the fault path restores the clean frame too");
    }
}
