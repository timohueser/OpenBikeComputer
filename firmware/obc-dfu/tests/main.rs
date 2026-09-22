//! `obc-dfu`'s integration tests: one binary, one module per case file.

#[path = "cases/armer.rs"]
mod armer;
#[path = "cases/blobstage.rs"]
mod blobstage;
#[path = "cases/boot_state.rs"]
mod boot_state;
#[path = "cases/engine.rs"]
mod engine;
#[path = "cases/layout.rs"]
mod layout;
#[path = "cases/signature.rs"]
mod signature;
#[path = "cases/vectors.rs"]
mod vectors;
