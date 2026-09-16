//! `obc-dfu`'s integration tests. One binary; each module below was a
//! top-level `tests/*.rs` file and keeps its own name, helpers and assertions.

#[path = "cases/armer.rs"]
mod armer;
#[path = "cases/blobstage.rs"]
mod blobstage;
#[path = "cases/boot_state.rs"]
mod boot_state;
#[path = "cases/engine.rs"]
mod engine;
#[path = "cases/signature.rs"]
mod signature;
#[path = "cases/vectors.rs"]
mod vectors;
