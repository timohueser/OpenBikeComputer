//! **The display-driver seam** — the single, board-agnostic interface the map/ride app writes the
//! screen through, so the panel is a *swappable part* and the rendering stack never couples to it.
//!
//! The whole shared rendering stack (`obc-render` / `obc-app` / `obc-reader` / `obc-route`,
//! [`FbDevice64`](crate::FbDevice64), the [`Dirty`](obc_app::Dirty) model) is panel-agnostic:
//! it renders a frame into one **RGB222 / device-64 framebuffer** (the 64-colour gamut is the cap, by
//! design — one byte per pixel, `0b00_RR_GG_BB`). A concrete display is then nothing more than a
//! [`DisplayDriver`]: hand it that framebuffer and it puts it on glass. Adding or swapping a panel is
//! one new `impl DisplayDriver` — no change to the rendering stack.
//!
//! **Two backends keep the seam honest.** The **LS021B7DD02** reflective MIP panel driven by the FLPR
//! coprocessor (the real target) lives in the board crate: a row-addressed panel where `present`
//! drives a masked full-frame scan and [`present_overlay`](DisplayDriver::present_overlay)
//! fast-forwards the gate to the dirty rows. The **simulator** (`obc-sim`) is the second backend, a
//! host `impl DisplayDriver` over an egui texture — and, unlike a second on-board panel, it is
//! **compiled and tested in the workspace on every CI run**, driving the *same* self-diffing
//! [`diff_rows`](crate::diff_rows) core and the *same* [`composite_overlay_window`](crate::composite_overlay_window)
//! as the device under an exact-diff oracle ([`spans_missed_changes`](crate::spans_missed_changes)).
//! Two live implementations are what keep the abstraction from silently growing a dependency on one
//! panel's quirks.
//!
//! ## Why the overlay is a *separate* seam method, not part of the framebuffer
//!
//! The transient chrome (the hold bulge, a future clock/status field) is **never** written into the
//! resident framebuffer: that would force a full map re-render to clear it again. Instead the
//! framebuffer stays the clean map (the source of truth) and [`present_overlay`](DisplayDriver)
//! composites the overlay over just the rows it touches and re-pushes only those — a few ms, no map
//! redraw. So the seam has two write paths: `present` (the clean frame, self-diffed, going *around*
//! a live overlay's rows via its `exclude` parameter so a map redraw never blanks the bulge) and
//! `present_overlay` (a dirty region with the overlay drawn on top).
//!
//! ## Async present
//!
//! The write paths are **async** (`false` = a transport fault the caller may retry): the FLPR
//! backend `await`s its coprocessor's EGU20 frame ack (issue #347 — the M33 is freed for the whole
//! ~44 ms scan; a deadline turns a stalled FLPR into a clean `false`). The simulator's paths complete
//! synchronously (a host texture upload never faults) but honour the same signature, so the app loop
//! is written once. Both async methods carry `where Self: Sized`, so the trait stays object-safe for
//! the one thing the render path needs through `&mut dyn DisplayDriver` —
//! [`fb_mut`](DisplayDriver::fb_mut); presents are always called on the concrete backend. The
//! two-plane concurrency (which executor drives each path) lives in each board's `main` — *outside*
//! the seam.

use crate::Band;

/// **The frame geometry — the single authority.** The frame the app renders and every backend
/// presents: `FRAME_W × FRAME_H` device-64 bytes. Everything frame-sized derives from these two
/// constants (`FB_BYTES`, the `RowDiff` height, the overlay-window columns, every render-call
/// viewport, the simulator's default window); each backend statically asserts its panel-native
/// geometry equals them, so a panel change can't silently desynchronize the framebuffer the app
/// renders from the frame a backend scans.
pub const FRAME_W: usize = 240;
/// Frame height in rows — see [`FRAME_W`].
pub const FRAME_H: usize = 320;

// The wire pack ([`ls021_wire`](crate::ls021_wire)) consumes exactly one `WIDTH`-pixel row per
// framebuffer row, so the panel-native row width *is* the frame width. Pin them together here at the
// single authority (the LS021/FLPR backend also re-asserts it against its FLPR gate-scan height).
const _: () = assert!(crate::ls021_wire::WIDTH == FRAME_W, "ls021_wire::WIDTH diverged from display::FRAME_W");

/// A dirty rectangle of the frame to re-present with the overlay composited over it — today the hold
/// bulge's right-edge window. The row-addressed LS021 panel widens it to full-width rows internally
/// (it can't latch a sub-span of columns) but still only touches rows `[y0, y0 + rows)`.
pub struct OverlayRegion {
    pub x0: u16,
    pub y0: u16,
    pub w: u16,
    pub rows: u16,
}

/// The swappable display backend — see the module docs. The map plane renders the frame into
/// [`fb_mut`](Self::fb_mut), then [`present`](Self::present)s it; the overlay plane re-pushes a dirty
/// region with the bulge composited via [`present_overlay`](Self::present_overlay).
#[allow(async_fn_in_trait)] // single-core executors on-device, a synchronous host backend — no Send bound wanted
pub trait DisplayDriver {
    /// The resident **RGB222 / device-64** framebuffer (`WIDTH × HEIGHT` bytes, `0b00_RR_GG_BB`) the
    /// renderer draws the whole frame into through an [`FbDevice64`](crate::FbDevice64), then
    /// [`present`](Self::present) puts on glass. Owned by the driver; this is how the app reaches it.
    fn fb_mut(&mut self) -> &mut [u8];

    /// Push the resident framebuffer to glass, self-diffed (only the rows that changed since the
    /// last present), optionally going **around** a live overlay: `exclude = Some((y0, rows))` means
    /// the rows `[y0, y0+rows)` belong to the overlay plane this frame — the diff store is still
    /// updated for them (it tracks the clean framebuffer, so no stale entry survives the overlay),
    /// but they are **not** pushed; the overlay's own re-present / trailing clear owns repainting
    /// them (≤ one overlay tick away). `None` ⇒ the whole frame is eligible. Returns `false` on a
    /// transport fault (a stalled FLPR, an SPI error) so the caller keeps the last frame and
    /// retries, rather than faulting. Async: the FLPR awaits its frame ack (the M33 runs other
    /// futures for the whole scan); the framebuffer must not be written until it returns.
    async fn present(&mut self, exclude: Option<(u16, u16)>) -> bool
    where
        Self: Sized;

    /// Re-present `region` with `draw_overlay` composited over the **clean framebuffer backdrop** — no
    /// map re-render. `draw_overlay` paints the transient chrome (the hold bulge)
    /// frame-absolute into the [`Band`] window the driver hands it, over the backdrop the driver reads
    /// from the framebuffer. The driver calls `draw_overlay` **once** (over the whole region) — never
    /// per row — so the caller's brief `InputPlane` lock inside it is taken once per overlay frame, and
    /// the framebuffer stays the clean map (the FLPR backend composites into it transiently for the
    /// push and restores the clean bytes before returning). Returns `false` on a transport fault.
    async fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool
    where
        Self: Sized;
}
