//! The presenter contracts: how a presenter puts the one resident [`Device64Frame`] on the LS021.
//!
//! Borrowing rule: the frame is a value the host owns next to its presenter. Rendering needs
//! `&mut` access, and [`Presenter::present`] borrows the frame shared for the whole push, so the
//! type system rejects a render into a frame a present is still scanning.
//! [`OverlayPresenter::present_overlay`] takes `&mut`, because the board composites into the
//! resident frame for the scan and restores the clean bytes before it returns. Either way the
//! frame is the clean base image when a presenter method returns.
//!
//! Recovery rule: after a failed `present` or `present_overlay`, the next `present` pushes every
//! row outside its `exclude`. The presenter repairs itself; the caller only presents again.
//!
//! [`conformance`] holds the checks the host double and the simulator presenter run. The board's
//! presenter runs on hardware only, and shares the [`ls021`](crate::ls021) diff and overlay code
//! the double exercises.

#[cfg(feature = "conformance")]
pub mod conformance;
pub mod frame;
pub mod presenter;

pub use frame::Device64Frame;
pub use presenter::{OverlayPresenter, PresentStats, Presenter};
