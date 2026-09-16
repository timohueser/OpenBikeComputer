//! `obc-ble`'s integration tests. One binary; each module below was a
//! top-level `tests/*.rs` file and keeps its own name, helpers and assertions.

#[path = "cases/dfu.rs"]
mod dfu;
#[path = "cases/list.rs"]
mod list;
#[path = "cases/sensors.rs"]
mod sensors;
#[path = "cases/transfer.rs"]
mod transfer;
#[path = "cases/vectors.rs"]
mod vectors;
