//! `no_std` display code shared by the board and the simulator: the draw targets the renderer
//! draws into, the presenter contracts, and the LS021B7DD02/FLPR pairing that implements them.
//!
//! The display path carries only `embedded-graphics`: no embassy plumbing, no SD stack, no board
//! transport, no app or UI state.
//!
//! [`framebuffer`] owns the board-owned draw targets the renderer draws into, [`panel`] the
//! [`Band`] frame-absolute window view and the overlay composite helper, [`display_contracts`] the
//! resident frame and the presenter traits with their conformance suite, and [`ls021`] everything
//! specific to the shipping panel: its geometry, the row-hash diff, the source-bus wire pack and
//! the mutate-and-restore overlay engine.

#![no_std]

pub mod display_contracts;
pub mod framebuffer;
pub mod ls021;
pub mod panel;

pub use framebuffer::{device64_to_rgb565, FbDevice64, Framebuffer565};
pub use panel::{composite_overlay_window, Band};
