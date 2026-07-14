//! [`RenderResources`] — the render-only allocation the [`App`](crate::App) façade owns.
//!
//! One component makes the allocation/placement ownership of the render path obvious: the reusable
//! [`MapRenderer`] and its large render scratch (the dominant slice of `App`'s resident size) live
//! here, separated from the pure domain state so nothing but the frame render ever touches them.
//! The renderer clears — never frees — its scratch each frame, so steady-state rendering does no
//! allocation (important on the MCU).
//!
//! Placement matters more than code here: an `App` is placement-initialized straight into its
//! reserved `.bss` region on the board, and this scratch is the field that must never be
//! materialized on the ~36 KB device stack. [`init_zeroed`](RenderResources::init_zeroed) zeroes it
//! in place (an empty renderer *is* the all-zero bit pattern).

use obc_render::MapRenderer;

/// The render-only resources: the reusable renderer + its per-frame scratch. See the module docs.
pub(crate) struct RenderResources {
    /// Reused renderer; clears (not frees) its scratch each frame, so steady-state rendering does
    /// no allocation — important on the MCU.
    pub(crate) renderer: MapRenderer,
}

impl RenderResources {
    /// A fresh renderer, by value — the host/test constructor path (return-value optimization
    /// keeps it off the stack in practice; the board uses [`init_zeroed`](Self::init_zeroed)).
    pub(crate) fn new() -> Self {
        RenderResources { renderer: MapRenderer::new() }
    }

    /// Zero the renderer **in place** at `slot` — the placement path. The scratch is the one
    /// KB-scale field a by-value constructor could spill onto the stack, so it is zeroed straight
    /// into the slot via [`MapRenderer::init_zeroed`]; no by-value `RenderResources` is ever
    /// formed.
    ///
    /// # Safety
    /// `slot` must be valid, aligned, exclusively owned, and writable for a full
    /// `RenderResources`.
    pub(crate) unsafe fn init_zeroed(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        // SAFETY: caller's contract; the single field is fully initialized in place.
        unsafe {
            MapRenderer::init_zeroed(addr_of_mut!((*slot).renderer));
            // Exhaustiveness guard: a field added to `RenderResources` fails to compile here until
            // it is initialized above (see `App::init_idle`).
            let RenderResources { renderer: _ } = &*slot;
        }
    }
}
