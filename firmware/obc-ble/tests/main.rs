//! `obc-ble`'s integration tests: one binary, one module per area.

#[path = "cases/dfu.rs"]
mod dfu;
#[path = "cases/radio_policy.rs"]
mod radio_policy;
#[path = "cases/sensors.rs"]
mod sensors;
#[path = "cases/vectors.rs"]
mod vectors;
