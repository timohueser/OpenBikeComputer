//! Host-side GPX **replay** — the desktop stand-in for the device's GPS + barometer.
//!
//! The real device has a GPS chip and a pressure altimeter; it never parses a GPX file.
//! Replaying a recorded ride is purely a host convenience, so it lives here (it needs `std`)
//! and produces nothing the shared crates know about: [`GpxPlayer`] turns a parsed [`Track`]
//! into the same [`Fix`](obc_app::Fix)es a GPS driver emits, and [`BaroSensor`] feeds the
//! track's elevation as a barometer would. Both implement the `obc-app` HAL traits, so the
//! shared [`App`](obc_app::App) can't tell a replay from live hardware.
//!
//! Two hosts share this one crate: the desktop simulator ([`obc-sim`]) replays straight into
//! the in-process app, and the USB feeder ([`obc-usb-host`]) replays out over a serial port to
//! the real prototype — both deriving course/speed the same way, so a recorded ride drives
//! either identically.
//!
//! [`obc-sim`]: https://docs.rs/obc-sim
//! [`obc-usb-host`]: https://docs.rs/obc-usb-host

pub mod baro;
pub mod effort;
pub mod gpx;
pub mod gpx_player;

pub use baro::BaroSensor;
pub use effort::{effort_from_speed, Effort};
pub use gpx::{Track, TrackPoint};
pub use gpx_player::GpxPlayer;
