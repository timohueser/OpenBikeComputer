//! CRC-32/IEEE — the integrity check for both DFU byte formats.
//!
//! The implementation lives in [`obc_crc`]; this module is the DFU-side name for it, so
//! `crate::crc32::{crc32, Crc32}` keeps reading the way the image, boot-state and engine code
//! already spells it. What matters here is *what* it covers: the image header, the boot-state
//! page, and the staged-image verify the bootloader folds extent-by-extent (§S3) instead of
//! buffering ~900 KB. The one property that must never change is the standard check value
//! (`crc32("123456789") == 0xCBF43926`) — `obc-crc` pins it.

pub use obc_crc::{crc32, Crc32};
