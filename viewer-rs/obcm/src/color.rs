//! RGB565 (file/style format) → display color conversions.
//!
//! The style table stores device-independent RGB565; the renderer quantizes to
//! the target panel once per color. These helpers are host- and MCU-agnostic.

/// Expand an RGB565 color to RGB888 components.
#[inline]
pub fn rgb565_to_rgb888(c: u16) -> (u8, u8, u8) {
    let r5 = ((c >> 11) & 0x1F) as u8;
    let g6 = ((c >> 5) & 0x3F) as u8;
    let b5 = (c & 0x1F) as u8;
    ((r5 << 3) | (r5 >> 2), (g6 << 2) | (g6 >> 4), (b5 << 3) | (b5 >> 2))
}

/// Quantize an RGB565 color to the LS021B7DD02's 64-color (RGB222) palette,
/// returned expanded to RGB888 so it can be shown on a full-color preview while
/// matching what the device will actually display.
#[inline]
pub fn rgb565_to_device64(c: u16) -> (u8, u8, u8) {
    let (r, g, b) = rgb565_to_rgb888(c);
    // keep the top 2 bits of each channel, expand back (each step = 85)
    let q = |v: u8| (v >> 6) * 85;
    (q(r), q(g), q(b))
}
