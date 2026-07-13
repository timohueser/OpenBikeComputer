//! [`BridgedDriver`] — the compatibility adapter that runs the existing
//! [`DisplayDriver`](crate::display::DisplayDriver) seam **on top of** a (frame, presenter)
//! pairing, so every current call site (the board's map plane, the simulator's GUI loop) keeps
//! compiling unchanged while issue #806 migrates the backends underneath it.
//!
//! The bridge is the proof that the old seam is a *special case* of the new contracts:
//!
//! - `fb_mut` ⇒ the frame's byte backing (`Pixel = u8`, the device-64 plane);
//! - `present(exclude)` ⇒ [`damage_unknown`](Presenter::damage_unknown), or
//!   [`damage_around`](OverlayPresenter::damage_around) the excluded rows widened to a full-width
//!   region — exactly the seam's "go around the live bulge" discipline;
//! - `present_overlay(region, drawer)` ⇒ [`OverlayPresenter::present_overlay`] with the old seam's
//!   [`Band`]-typed drawer, which is why the bound pins `OverlayTarget<'t> = Band<'t>`: the old
//!   seam hard-codes the RGB565 window composite, so only pairings with that composite strategy
//!   can stand behind it. That restriction is the old seam's, not the contracts' — it is the
//!   reason the seam is being split.
//!
//! Zero-cost: the bridge is a plain struct over `F` and `P`, fully monomorphized; the only dynamic
//! dispatch is the `&mut dyn FnMut(&mut Band)` drawer the *old* seam's signature already carries
//! (one indirect call per overlay frame).

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::frame::NativeFrame;
use super::presenter::OverlayPresenter;
use crate::display::{DisplayDriver, OverlayRegion};
use crate::panel::Band;

/// A (frame, presenter) pairing exposed through the legacy [`DisplayDriver`] seam. Owns both —
/// like the old drivers owned their framebuffer — and hands the pieces back out through
/// [`frame`](Self::frame)/[`presenter`](Self::presenter) so hosts can migrate call sites
/// incrementally during #806.
pub struct BridgedDriver<F, P> {
    frame: F,
    presenter: P,
}

impl<F, P> BridgedDriver<F, P> {
    pub fn new(frame: F, presenter: P) -> Self {
        Self { frame, presenter }
    }

    pub fn frame(&self) -> &F {
        &self.frame
    }

    pub fn frame_mut(&mut self) -> &mut F {
        &mut self.frame
    }

    pub fn presenter(&self) -> &P {
        &self.presenter
    }

    pub fn presenter_mut(&mut self) -> &mut P {
        &mut self.presenter
    }

    /// Unbundle for a call site that has migrated to the split contracts.
    pub fn into_parts(self) -> (F, P) {
        (self.frame, self.presenter)
    }
}

impl<F, P> DisplayDriver for BridgedDriver<F, P>
where
    F: NativeFrame<Color = Rgb565, Pixel = u8>,
    P: for<'t> OverlayPresenter<F, OverlayTarget<'t> = Band<'t>>,
{
    fn fb_mut(&mut self) -> &mut [u8] {
        self.frame.backing_mut()
    }

    async fn present(&mut self, exclude: Option<(u16, u16)>) -> bool {
        let damage = match exclude {
            None => P::damage_unknown(),
            // The old seam's exclude is a row span; widen it to the presenter's region over the
            // full frame width — the same full-width-rows shape the LS021 clip always used.
            Some((y0, rows)) => P::damage_around(P::region(Rectangle::new(
                Point::new(0, y0 as i32),
                Size::new(F::WIDTH as u32, rows as u32),
            ))),
        };
        self.presenter.present(&self.frame, damage).await.is_ok()
    }

    async fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool {
        let r = P::region(Rectangle::new(
            Point::new(region.x0 as i32, region.y0 as i32),
            Size::new(region.w as u32, region.rows as u32),
        ));
        self.presenter.present_overlay(&mut self.frame, r, |band: &mut Band| draw_overlay(band)).await.is_ok()
    }
}
