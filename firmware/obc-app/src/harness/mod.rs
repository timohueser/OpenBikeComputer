//! The two staging harnesses that reach crate-private domain state directly: the screen-stack
//! transitions (`screens`) and the route-upload popups (`upload`), plus [`quick_drawer`], whose
//! chord plane and drawer owner are `App`'s rather than a screen's. In-crate, they use
//! `pub(crate)` access, so no public accessor exists purely for tests.
//!
//! [`support`] holds the shared test helpers. It lives here because in-crate code cannot reach a
//! `tests/` module; the integration tests get the same file through a `#[path]` include in
//! `tests/common/mod.rs`, so there is one copy.

pub(crate) mod support;

mod quick_drawer;
mod screens;
mod upload;

mod marquee;
mod nav;
