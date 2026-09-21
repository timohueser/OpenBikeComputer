//! Board-agnostic `no_std` display seam: the generic frame and presentation contracts every
//! backend implements, plus the shipping LS021B7DD02/FLPR pairing that satisfies them.
//!
//! The display path carries only `embedded-graphics`: no embassy plumbing, no SD stack, no board
//! transport, no app or UI state. A second board reuses this crate by pointing its own presenter
//! backend at the same contracts.
//!
//! [`framebuffer`] owns the board-owned draw targets the renderer draws into, [`panel`] the
//! [`Band`] frame-absolute band view and the overlay composite helper, [`display_contracts`] the
//! generic native-frame and presentation contracts with their conformance suite, and [`ls021`]
//! everything specific to the shipping panel: its geometry, the row-hash damage strategy, the
//! source-bus wire pack and the mutate-and-restore overlay engine.

#![no_std]

pub mod framebuffer;
pub mod panel;
// The generic display contracts, deliberately namespaced rather than re-exported at the root: a
// bounded module another board could reuse without the LS021 pairing.
pub mod display_contracts;
// The LS021B7DD02 pairing owner. Everything LS021-specific lives here, off the generic contracts.
pub mod ls021;

pub use framebuffer::{device64_to_rgb565, FbDevice64, Framebuffer565};
pub use panel::{composite_overlay_window, Band};
