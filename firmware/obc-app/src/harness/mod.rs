//! In-crate relocation (FAR-19, #812) of the two staging harnesses that reach `Activity`'s now
//! `pub(crate)` fields directly — the screen-stack transitions (`screens`) and the route-upload
//! popups (`upload`). They were integration tests under `tests/`, but staging `Activity::mode` /
//! `active_route` and the three match readouts (`progress_m` / `off_route` / `dist_to_route_m`)
//! from outside the crate is exactly what kept those fields `pub`. Relocated here, they access the
//! fields at `pub(crate)` visibility — so no public accessor exists purely for tests, and the
//! fields drop out of the crate's public surface.
//!
//! [`support`] is the in-crate copy of the shared integration-test helpers these two harnesses
//! need (`tests/common/mod.rs` stays for the remaining integration tests).

mod support;

mod screens;
mod upload;
