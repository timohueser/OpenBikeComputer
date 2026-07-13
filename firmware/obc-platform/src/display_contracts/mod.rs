//! **The generic display contracts** (FAR-12, issue #805) — the compile-time seam that separates
//! *what a frame is* from *what a panel can do with it*, so a display with different geometry,
//! native pixel storage, or presentation grain is a new (frame, presenter) pairing at the board's
//! composition edge — never a change to the rendering stack, and never a runtime pixel conversion.
//!
//! Four concerns, four owners:
//!
//! 1. **Frame specification/storage** — [`NativeFrame`](frame::NativeFrame): geometry, the
//!    device-native storage cells, stride + validated backing length, a `DrawTarget` view writing
//!    *directly* into the backing, and a clip view that needs no second frame. The shipping frame is
//!    [`Device64Frame`](frame::Device64Frame)`<240, 320>` — the one resident RGB222 plane, quantized
//!    on store exactly once through the same [`PackDevice64`](crate::framebuffer::PackDevice64) path
//!    the renderer always used.
//! 2. **Base-frame presentation** — [`Presenter`](presenter::Presenter): put the clean resident
//!    frame on glass, accept damage at a neutral level (the [`damage_full`](presenter::Presenter::damage_full)
//!    / [`damage_unknown`](presenter::Presenter::damage_unknown) constructors) or through the
//!    presenter's own [`Damage`](presenter::Presenter::Damage) associated type, and report the
//!    transport outcome — no render or domain policy.
//! 3. **Transient overlay presentation** — [`OverlayPresenter`](presenter::OverlayPresenter): read
//!    the clean frame as the backdrop, composite a bounded region (into bounded scratch, or
//!    transiently into hardware/the frame with a byte-identical restore), coordinate
//!    exclusion/trailing clear with the base present
//!    ([`damage_around`](presenter::OverlayPresenter::damage_around)), and stay callable from the
//!    preemptive overlay plane with **no map render** in the path.
//! 4. **Damage strategy** — deliberately *not* a contract. Row hashing and span masking
//!    ([`RowDiff`](crate::rowdiff::RowDiff) and friends in [`rowdiff`](crate::rowdiff)) are the
//!    LS021 backend's own strategy; a different presenter may diff tiles, track controller dirty
//!    windows, or always present fully. There is no universal fallback the generic layer silently
//!    substitutes — the shipping board *statically* pairs its frame with a presenter whose damage
//!    and overlay grain meet its responsiveness contract.
//!
//! ## Borrowing model: render and present cannot race
//!
//! The frame is a value the host owns *next to* its presenter, not inside it. Rendering needs
//! `&mut F` (through [`draw_target`](frame::NativeFrame::draw_target)); a base present borrows the
//! frame shared (`&F`) for the whole push — so the type system rejects a render into a frame a
//! present is still scanning (on the LS021/FLPR that scan is a multi-ms coprocessor read of the
//! resident bytes). An overlay present takes `&mut F`: the shipping backend transiently composites
//! the bulge *into* the resident frame for the scan and restores the clean bytes before returning
//! (the no-second-frame design), and exclusive access is exactly what makes that sound. The
//! invariant either way: **the frame is always the clean base image when a presenter method
//! returns** — transient overlays never persist into it.
//!
//! The two-plane board split (preemptive input/overlay plane vs. thread-mode map plane) stays
//! expressible exactly as today: the map plane owns the (frame, presenter) pair and drives both
//! present paths; the input plane shares only its own gesture/bulge state, sampled inside the
//! overlay draw closure once per overlay frame.
//!
//! ## Zero-cost by construction
//!
//! Everything here is compile-time: const-generic geometry, monomorphized `impl Trait` draw
//! targets, `async fn` in traits with no `Send` bound and no boxed futures, and no trait objects
//! anywhere in the contracts (the [`bridge`]'s `&mut dyn FnMut` overlay drawer is the *existing*
//! seam's shape, one indirect call per overlay frame — never per pixel). A pairing the board never
//! names is never monomorphized, so alternate frames/presenters (the test-only proof backend) cost
//! the shipping image nothing.
//!
//! ## Migration status
//!
//! This module is **additive** (issue #805): the operational seam both backends implement is still
//! [`DisplayDriver`](crate::display::DisplayDriver), and [`bridge::BridgedDriver`] proves the old
//! seam is expressible over a (frame, presenter) pairing. Issue #806 migrates the LS021/FLPR
//! backend, the simulator, and the hosts onto these contracts; issue #807 may lift this
//! module — deliberately self-contained, depending only on `embedded-graphics` and the crate's
//! existing framebuffer/band types — into its own crate if the split pays.

pub mod bridge;
pub mod frame;
pub mod presenter;

pub use bridge::BridgedDriver;
pub use frame::{Device64Frame, NativeFrame};
pub use presenter::{OverlayPresenter, PresentStats, Presenter};
