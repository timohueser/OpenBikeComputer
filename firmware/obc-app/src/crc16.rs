//! CRC-16 shared by the settings and DFU marker codecs.

/// CRC-16/CCITT-FALSE (poly `0x1021`, init `0xFFFF`) over `data`.
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}
