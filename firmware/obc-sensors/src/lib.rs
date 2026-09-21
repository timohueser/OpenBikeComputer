//! Pure `no_std` sensor chip and protocol decoders: the host-testable half of the sensor stack.
//!
//! They carry only byte and math needs, with no embassy plumbing, no bus transactions and no app
//! state. The board crate owns the concrete transactions and hands raw bytes in, so every module
//! unit-tests on the host with no transport feature enabled.
//!
//! [`ubx`] decodes the u-blox SAM-M10Q's UBX messages, [`bmp581`] the BMP581's pressure and
//! temperature registers with the pressure-to-altitude formula, [`icm20948`] the ICM-20948's
//! registers, and [`compass`] a magnetometer sample into a heading.

#![no_std]

pub mod bmp581;
pub mod compass;
pub mod icm20948;
pub mod ubx;
