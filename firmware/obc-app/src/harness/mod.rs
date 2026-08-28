//! In-crate relocation (FAR-19, #812) of the two staging harnesses that reach crate-private domain
//! state directly: the screen-stack transitions (`screens`) and the route-upload popups (`upload`).
//! They were integration tests under `tests/`, but staging `Activity::mode` with Navigator's active
//! route and match readouts (`progress_m` / `off_route` / `dist_to_route_m`) from outside the crate
//! would require public test access. Relocated here, they use `pub(crate)` access, so no public
//! accessor exists purely for tests.
//!
//! [`quick_drawer`] joined them for #1515 D2: the chord plane and the drawer owner are `App`'s,
//! not a screen's, and the harness reaches `Screen` variants and the settings handshake the way
//! the other two reach Activity and Navigator state.
//!
//! [`support`] holds the shared test helpers these harnesses need. It lives here rather than
//! under `tests/` because in-crate code can't reach a `tests/` module — the integration tests get
//! the same file through a `#[path]` include in `tests/common/mod.rs`, so there is one copy.

pub(crate) mod support;

mod quick_drawer;
mod screens;
mod upload;
