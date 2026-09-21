//! CRC-32/IEEE: the whole-object, end-to-end integrity check. The BLE Link Layer already CRCs each
//! packet, so this covers what the link cannot — encode bugs and storage write errors, from the
//! phone's encode to the device's flash and back. One CRC per object, never per chunk, and
//! byte-identical to the app's Swift `CRC32.Hasher`.

pub use obc_crc::Crc32;
