//! The generic display contracts: the compile-time seam between what a frame is and what a panel
//! can do with it, so a display with different geometry, native pixel storage or presentation
//! grain is a new (frame, presenter) pairing at the board's composition edge, never a change to
//! the rendering stack and never a runtime pixel conversion.
//!
//! Four concerns, four owners:
//!
//! 1. Frame specification and storage, [`NativeFrame`](frame::NativeFrame): geometry, the
//!    device-native storage cells, stride and validated backing length, a `DrawTarget` view
//!    writing directly into the backing, and a clip view that needs no second frame.
//! 2. Base-frame presentation, [`Presenter`](presenter::Presenter): put the clean resident frame
//!    on glass, accept damage at a neutral level or through the presenter's own associated type,
//!    and report the transport outcome.
//! 3. Transient overlay presentation, [`OverlayPresenter`](presenter::OverlayPresenter): read the
//!    clean frame as the backdrop, composite a bounded region, coordinate exclusion and the
//!    trailing clear with the base present, and stay callable from the preemptive overlay plane
//!    with no map render in the path.
//! 4. Damage strategy, deliberately not a contract. How a presenter decides what changed is the
//!    pairing's own business, owned next to that pairing's transport, and there is no universal
//!    fallback the generic layer substitutes.
//!
//! Borrowing model: the frame is a value the host owns next to its presenter, not inside it.
//! Rendering needs `&mut F`, and a base present borrows the frame shared for the whole push, so
//! the type system rejects a render into a frame a present is still scanning. An overlay present
//! takes `&mut F`, because a backend may transiently composite into the resident frame for the
//! scan and restore the clean bytes before returning. Either way the frame is always the clean
//! base image when a presenter method returns.
//!
//! Everything here is compile-time: const-generic geometry, monomorphized `impl Trait` draw
//! targets, `async fn` in traits with no `Send` bound and no boxed futures, and no trait objects.
//! A pairing the board never names is never monomorphized.
//!
//! [`conformance`] holds the reusable, backend-agnostic checks of the mandatory invariants. Both
//! shipping backends run the same suite, alongside a tile-grained proof pairing that keeps the
//! contracts honest about geometry, storage and grain.

#[cfg(feature = "conformance")]
pub mod conformance;
pub mod frame;
pub mod presenter;

pub use frame::{Device64Frame, NativeFrame};
pub use presenter::{OverlayPresenter, PresentStats, Presenter};
