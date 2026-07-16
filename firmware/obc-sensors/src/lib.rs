//! Pure `no_std` **sensor chip/protocol decoders** — the host-testable half of the OBC sensor stack.
//!
//! Split out of `obc-platform` (issue #807) so the decoders carry *only* byte/math needs
//! (`obc-ports` sample types + `libm`): no embassy signal plumbing, no I²C/bus transactions, no app
//! state. The board crate owns the concrete `Twim`/UART transactions and hands raw bytes/samples in;
//! everything here is a pure function of those bytes, so every module unit-tests on the host with no
//! transport or signal feature enabled (the #807 acceptance criterion).
//!
//! ## Responsibility / dependency table
//!
//! | Module | Owns | Depends on |
//! |---|---|---|
//! | [`ubx`] | UBX protocol decode for the u-blox **SAM-M10Q** GNSS receiver: NAV-PVT → [`Fix`](obc_ports::Fix), the `RXM-PMREQ`/`CFG-PM` low-power encodings | `obc-ports` |
//! | [`bmp581`] | BMP581 pressure/temperature register decode + the barometric pressure→altitude formula | `libm` |
//! | [`icm20948`] | ICM-20948 accel/gyro/mag register decode | (`obc-ports`) |
//! | [`compass`] | AK09916 magnetometer → tilt-compensated heading (clockwise from north) for [`CompassSource`](obc_ports::CompassSource) | `libm` |
//!
//! None of these depend on `obc-app`, embassy, or the SD stack. The board bridges parsed samples to
//! the app poll through the embassy-sync handoffs that live in `obc-platform` (behind its
//! `sensor-link`/`debug-link` features), keeping this crate feature- and transport-free.

#![no_std]

pub mod bmp581;
pub mod compass;
pub mod icm20948;
pub mod ubx;
