//! The integration tests' half of the shared test support. The helpers themselves live in the crate
//! (`src/harness/support.rs`) because the relocated staging harnesses (FAR-19, #812) need exactly
//! the same `Buf` / `.obcm` builder / scripted-hardware set, and two copies of 300 lines drift.
//! Pulled in by path rather than duplicated, so there is one source of truth; it names `App` as
//! `obc_app::App`, which resolves here as the extern crate and in-crate through lib.rs's
//! `cfg(test)` self-alias.

#[path = "../../src/harness/support.rs"]
mod support;

// Not every test binary uses every helper (`ble.rs` pulls in none of them).
#[allow(unused_imports)]
pub use support::*;
