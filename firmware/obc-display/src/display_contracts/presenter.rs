//! [`Presenter`] and [`OverlayPresenter`]: how a presenter puts the resident [`Device64Frame`] on
//! the LS021.
//!
//! The methods are `async` with no `Send` bound and no boxed futures: the board awaits the FLPR's
//! frame ack while the executor runs other futures, and a host presenter completes synchronously
//! under the same signature.

use core::fmt::Debug;

use super::frame::Device64Frame;
use crate::ls021::RowWindow;
use crate::panel::Band;

/// What one present cost, in rows. `pushed_units == 0` is the "a spurious redraw is free" outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentStats {
    /// Rows actually pushed to glass this present.
    pub pushed_units: u32,
    /// The frame's row count, for context.
    pub total_units: u32,
    /// Contiguous row spans pushed.
    pub regions: u32,
}

/// Base-frame presentation: self-diff the clean resident frame against the rows on glass and push
/// the rows that changed.
///
/// The frame is borrowed shared for the whole present, which makes rendering into it impossible
/// until the transport is done with the bytes. On the board the FLPR scans the resident frame
/// directly, so that borrow is load-bearing.
///
/// Recovery rule: after a failed [`present`](Self::present) or
/// [`present_overlay`](OverlayPresenter::present_overlay), the next `present` pushes every row
/// outside its `exclude`. The first `present` of a new presenter also pushes every row. A presenter
/// that advances its row store before the push must reset the store when the push fails, or a
/// retry diffs against rows that never reached glass and pushes nothing.
#[allow(async_fn_in_trait)] // single-core executors on-device + a synchronous host backend — no Send bound wanted
pub trait Presenter<const W: usize, const H: usize> {
    /// A transport fault: the push did not reach glass; the caller keeps the frame and may retry.
    type Error: Debug;

    /// Push the rows of `frame` that changed. `exclude` is `(first row, row count)` of a live
    /// overlay: those rows are not pushed, because clean bytes under a live bulge would flash it
    /// off, but the row store still tracks the clean frame for them, so the trailing clear leaves
    /// nothing stale.
    async fn present(
        &mut self,
        frame: &Device64Frame<'_, W, H>,
        exclude: Option<(u16, u16)>,
    ) -> Result<PresentStats, Self::Error>;
}

/// Transient overlay presentation over a [`Presenter`]'s clean frame: the hold bulge.
///
/// The caller never renders the overlay into the frame. The presenter reads the clean frame as
/// the backdrop, hands `draw` a frame-absolute [`Band`] clipped to the window, and pushes the
/// window's full-width rows. It may stage the composite transiently in the resident frame, but the
/// frame's backing is byte-identical when the call returns, on a fault too. That is why the frame
/// is borrowed `&mut`.
///
/// Each composite starts from the clean frame, so a shrinking bulge needs no undo pass, and a
/// `draw` that paints nothing is the trailing clear.
#[allow(async_fn_in_trait)]
pub trait OverlayPresenter<const W: usize, const H: usize>: Presenter<W, H> {
    /// Composite `draw` over the clean-frame backdrop within `window` and push only its rows.
    /// `draw` is called once per overlay present, so a caller's brief input-plane lock inside it is
    /// taken once per overlay frame.
    async fn present_overlay(
        &mut self,
        frame: &mut Device64Frame<'_, W, H>,
        window: RowWindow,
        draw: impl FnOnce(&mut Band<'_>),
    ) -> Result<PresentStats, Self::Error>;
}
