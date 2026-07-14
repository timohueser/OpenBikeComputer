//! Board-agnostic `no_std` **display seam** — the generic frame/presentation contracts every
//! backend implements, plus the shipping LS021B7DD02/FLPR pairing that satisfies them.
//!
//! Split out of `obc-platform` (issue #807) so the display path carries *only* `embedded-graphics`:
//! no embassy plumbing, no SD stack, no board transport, no app/UI state. A second board reuses this
//! crate by pointing its own presenter backend at the same contracts.
//!
//! ## Responsibility / dependency table
//!
//! | Module | Owns | Depends on |
//! |---|---|---|
//! | [`framebuffer`] | the board-owned [`DrawTarget`](embedded_graphics::draw_target::DrawTarget)s the renderer draws into — the device-native RGB222 map plane ([`FbDevice64`], 1 byte/px, the real target) and the [`Framebuffer565`] RGB565 plane the banded [`Band`] scratch reuses; the shared [`device64_to_rgb565`] unpack | `embedded-graphics` |
//! | [`panel`] | the [`Band`] frame-absolute band/window view + the [`composite_overlay_window`] overlay helper, for boards that stream a frame to the panel a band at a time | `embedded-graphics`, [`framebuffer`] |
//! | [`display_contracts`] | the generic native-frame + presentation capability contracts (#805/#806): `NativeFrame`/`Device64Frame`, `Presenter`/`OverlayPresenter`, and the backend-agnostic conformance suite — the seam both shipping backends implement, LS021-free by construction | `embedded-graphics`, [`framebuffer`] |
//! | [`ls021`] | the LS021B7DD02 pairing owner: the shipping panel geometry ([`ls021::FRAME_W`]/[`ls021::FRAME_H`]), the row-hash/span damage strategy ([`ls021::rowdiff`]), the source-bus wire pack ([`ls021::wire`]), the shared damage/region vocabulary, and the mutate-and-restore [`composite_into_resident`](ls021::composite_into_resident) overlay engine | `embedded-graphics`, [`framebuffer`], [`panel`], [`display_contracts`] |
//!
//! Nothing here depends on `obc-app`, `obc-ports`, embassy, or the SD stack — the display seam is
//! generic over frame geometry/storage/pixel policy at compile time (the epic's native-pixels rule)
//! and the board supplies the concrete panel transport at composition.

#![no_std]

pub mod framebuffer;
pub mod panel;
// The generic native-frame + presentation capability contracts (#805/#806) — the display seam both
// backends implement. Deliberately namespaced (not re-exported at the root): a clearly-bounded
// module that another board could reuse without the LS021 pairing.
pub mod display_contracts;
// The LS021B7DD02 pairing owner: the shipping panel's frame geometry (`ls021::FRAME_W/H`), the
// row-hash/span damage strategy (`ls021::rowdiff`), the source-bus wire pack (`ls021::wire`), the
// shared damage/region vocabulary, and the mutate-and-restore overlay composite. Everything
// LS021-specific lives here, off the generic display contracts.
pub mod ls021;

pub use framebuffer::{device64_to_rgb565, FbDevice64, Framebuffer565};
pub use panel::{composite_overlay_window, Band};
