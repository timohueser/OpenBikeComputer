//! The integration tests' half of the shared test support. The helpers themselves live in the
//! crate (`src/harness/support.rs`), because the in-crate staging harnesses need exactly the same
//! `Buf` / `.obcm` builder / scripted-hardware set. Pulled in by path rather than duplicated, so
//! there is one source of truth.

#[path = "../../src/harness/support.rs"]
mod support;

// Not every test binary uses every helper (`ble.rs` pulls in none of them).
#[allow(unused_imports)]
pub use support::*;
