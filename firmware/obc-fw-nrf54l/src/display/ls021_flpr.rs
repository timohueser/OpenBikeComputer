//! **The LS021/FLPR `DisplayDriver` backend** (issue #174) — the default reflective MIP panel behind
//! the board-agnostic [`DisplayDriver`](super::DisplayDriver) seam.
//!
//! A thin adapter over the [`Ls021Flpr`](crate::ls021_flpr::Ls021Flpr) coprocessor backend (the FLPR
//! launch + ping-pong transport stay at the crate root, shared with the bring-up bin). The map plane
//! renders the whole frame into the resident RGB222 plane, then [`present`](DisplayDriver::present)
//! drives it in one masked full-frame scan; [`present_overlay`](DisplayDriver::present_overlay)
//! re-presents the bulge's rows via [`push_overlay`](crate::ls021_flpr::Ls021Flpr::push_overlay) (issue
//! #163), whose composite step is the shared `obc_platform::composite_overlay_window` the ST7789 backend
//! also runs. The only LS021-specific code is the device-64 → 6-line wire-pack inside those pushes.
//!
//! The hold bulge is **not** composited in `present`: it rides `present_overlay` on its own plane, so
//! the framebuffer stays the clean map. A stalled FLPR returns `false` so the caller keeps the last
//! frame and retries rather than faulting.

use obc_platform::Band;

use super::{DisplayDriver, OverlayRegion};
use crate::ls021_flpr::Ls021Flpr;

impl DisplayDriver for Ls021Flpr<'_> {
    fn fb_mut(&mut self) -> &mut [u8] {
        Ls021Flpr::fb_mut(self)
    }

    fn present(&mut self) -> bool {
        // Self-diffing whole-frame present (issue #201): push only the rows that changed since the last
        // present. No bulge to clip — the map plane passes the live span via `present_within` directly.
        self.present_within(None)
    }

    /// Re-present the overlay rectangle's **rows** with the bulge composited (issue #163): the LS021
    /// can't latch a sub-span of columns, so the FLPR rewrites the full-width rows `[y0, y0+rows)`
    /// (only `[x0, x0+w)` carry the overlay) and fast-forwards the gate over the rest — see
    /// [`push_overlay`](Ls021Flpr::push_overlay) for the stack-frugal, lock-once composite.
    fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool {
        self.push_overlay(region.x0, region.y0, region.w, region.rows, draw_overlay)
    }
}
