//! **The display-driver seam** — the single, board-agnostic interface the map/ride app writes the
//! screen through, so the panel is a *swappable part* and the rendering stack never couples to it.
//!
//! The whole shared rendering stack (`obc-render` / `obc-app` / `obc-reader` / `obc-route`,
//! [`FbDevice64`](obc_platform::FbDevice64), the [`Dirty`](obc_app::Dirty) model) is panel-agnostic:
//! it renders a frame into one **RGB222 / device-64 framebuffer** (the 64-colour gamut is the cap, by
//! design — one byte per pixel, `0b00_RR_GG_BB`). A concrete display is then nothing more than a
//! [`DisplayDriver`]: hand it that framebuffer and it puts it on glass. Adding or swapping a panel is
//! one new `impl DisplayDriver` in this crate — no change to the rendering stack.
//!
//! Two backends implement it today, which is what keeps the seam honest (the core can't silently grow
//! a dependency on one panel's quirks):
//!   - the **ST7789** EYESPI TFT (the bring-up stand-in): a random-access GRAM panel — `present` bands
//!     the framebuffer over SPI-DMA, [`present_overlay`] addresses a column window directly;
//!   - the **LS021B7DD02** reflective MIP panel driven by the FLPR coprocessor (the real target): a
//!     row-addressed panel — `present` drives a masked full-frame scan, [`present_overlay`]
//!     fast-forwards the gate to the dirty rows (issue #163).
//!
//! ## Why the overlay is a *separate* seam method, not part of the framebuffer
//!
//! The transient chrome (the hold bulge, a future clock/status field) is **never** written into the
//! resident framebuffer: that would force a full map re-render to clear it again. Instead the
//! framebuffer stays the clean map (the source of truth) and [`present_overlay`](DisplayDriver) (issue
//! #163, added on the partial-update mechanism) composites the overlay over just the rows it touches
//! and re-pushes only those — a few ms, no map redraw. So the seam has two write paths: `present` (the
//! whole clean frame) and `present_overlay` (a dirty region with the overlay drawn on top).
//!
//! ## Sync, blocking
//!
//! Both backends present **synchronously** (ST7789 blocks on the SPI-DMA write, the FLPR busy-polls
//! its coprocessor), so the seam is plain `&mut self` methods returning `bool` (`false` = a transport
//! fault the caller may retry) — no async-in-trait. The two-plane concurrency (which executor drives
//! each path, the bus mutex) lives in `main.rs`, *outside* the seam.

use obc_platform::Band;

/// A dirty rectangle of the frame to re-present with the overlay composited over it — today the hold
/// bulge's right-edge window (issue #126/#163). A column-addressable panel (ST7789) re-pushes exactly
/// this rectangle; a row-addressed panel (LS021) widens it to full-width rows internally (it can't
/// latch a sub-span of columns) but still only touches rows `[y0, y0 + rows)`.
pub struct OverlayRegion {
    pub x0: u16,
    pub y0: u16,
    pub w: u16,
    pub rows: u16,
}

/// The board's swappable display backend — see the module docs. The map plane renders the frame into
/// [`fb_mut`](Self::fb_mut), then [`present`](Self::present)s it; the overlay plane re-pushes a dirty
/// region with the bulge composited via [`present_overlay`](Self::present_overlay) (issue #163).
pub trait DisplayDriver {
    /// The resident **RGB222 / device-64** framebuffer (`WIDTH × HEIGHT` bytes, `0b00_RR_GG_BB`) the
    /// renderer draws the whole frame into through an [`FbDevice64`](obc_platform::FbDevice64), then
    /// [`present`](Self::present) puts on glass. Owned by the driver; this is how the app reaches it.
    fn fb_mut(&mut self) -> &mut [u8];

    /// Push the whole resident framebuffer to glass. Returns `false` on a transport fault (a stalled
    /// FLPR, an SPI error) so the caller keeps the last frame and retries, rather than faulting.
    fn present(&mut self) -> bool;

    /// Re-present `region` with `draw_overlay` composited over the **clean framebuffer backdrop** — no
    /// map re-render (issue #163). `draw_overlay` paints the transient chrome (the hold bulge)
    /// frame-absolute into the [`Band`] window the driver hands it, over the backdrop the driver reads
    /// from the framebuffer. The driver calls `draw_overlay` **once** (over the whole region) — never
    /// per row — so the caller's brief `InputPlane` lock inside it is taken once per overlay frame, and
    /// the framebuffer stays the clean map (the overlay is never written into it). Returns `false` on a
    /// transport fault.
    fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool;
}
