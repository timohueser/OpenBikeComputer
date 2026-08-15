//! CRC-32/IEEE — the whole-object, end-to-end integrity check.
//!
//! **Not the on-air check.** The BLE Link Layer already CRCs every packet, so the CoC is a reliable,
//! ordered stream. This CRC covers what the link can't: encode bugs, storage write errors — end to
//! end from the phone's encode to the device's flash and back. One CRC per **object**, never per
//! chunk, and byte-identical to the app's Swift `CRC32.Hasher`.
//!
//! The implementation lives in [`obc_crc`] — a leaf with no dependencies of its own, so sharing it
//! with the DFU side costs this crate nothing it was protecting.

pub use obc_crc::Crc32;
