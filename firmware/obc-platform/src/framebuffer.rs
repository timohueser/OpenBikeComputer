//! `DrawTarget`s the board owns and the shared renderer draws into: the nRF's device-native
//! RGB222 [`FbDevice64`] map plane (the real target) and the [`Framebuffer565`] RGB565 plane
//! (now the per-band scratch the [`Band`](crate::Band) view wraps).
//!
//! The on-device counterpart of the simulator's `obc-sim/src/framebuffer.rs`: the
//! shared [`obc_render::MapRenderer`](../../obc_render) (driven through
//! [`obc_app::App::render_frame`](../../obc_app)) runs the exact same rendering
//! code on the host and on the MCU, drawing into a buffer the board owns. On the nRF (no external
//! RAM, no scan-out engine) that buffer is a resident `.bss` frame the banded
//! [`Panel`](crate::Panel) push streams to the panel a band at a time over SPI/DMA; a board with a
//! hardware scan-out plane would instead let its display controller rescan the buffer directly,
//! with no explicit push.
//!
//! The `color_fn` the app is rendered with is the **identity** `RGB565 -> Rgb565`
//! (`|c| Rgb565::from(RawU16::new(c))`) on every board — the renderer stays `Rgb565`-typed. The
//! per-board pixel format is then the [`Pack`]'s business: a no-op on the RGB565 planes, and the
//! device-64 (RGB222) quantization on [`FbDevice64`]. So the gamut the simulator *previews* via
//! `obc_reader::rgb565_to_device64` is what the nRF actually stores and shows — not a host-only
//! concern any more.
//!
//! Every plane is the *same* `DrawTarget`: a borrowed `width * height` buffer, a clipped pixel
//! `put`, a scanline `fill_solid` and a `clear`. The planes differ by exactly two things — the
//! stored pixel *type* and how an [`Rgb565`] colour is packed into it: native RGB565 (`u16`) or
//! the nRF's device-native RGB222 (`u8`, [`FbDevice64`] — the real target, half the RAM). Both
//! are captured by the zero-sized [`Pack`] marker and its associated [`Pixel`](Pack::Pixel) type,
//! so the framebuffer body is written **once**, generic over `P: Pack`, and [`Framebuffer565`] /
//! [`FbDevice64`] are thin type aliases. `P::pack` is a static (monomorphized) call — no per-pixel
//! indirection in the hot render loop — and the markers are zero-sized, so a [`RawFb`] is the same
//! size as a bare `{ width, height, buf }` struct.

use core::marker::PhantomData;

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

/// How a [`RawFb`] packs an [`Rgb565`] colour into one stored pixel, and what type that
/// stored pixel is ([`Pixel`](Pack::Pixel)). This is the *only* per-plane difference between
/// the framebuffers; everything else (clip, scanline fill, clear) is shared. Impls are
/// zero-sized markers and `pack` is `#[inline]`, so a packed pixel costs a static call the
/// compiler folds in — never a `fn`-pointer / `dyn` indirection in the per-pixel render loop.
///
/// The stored pixel is associated, not fixed at `u16`, so a board's native byte width comes for
/// free: the RGB565 plane stores a `u16`, while the nRF's device-native RGB222 plane
/// ([`PackDevice64`]) stores a single `u8` — half the RAM for a frame the board has to fit in
/// on-chip SRAM (no external RAM).
pub trait Pack {
    /// The stored pixel type — `u16` for the RGB565 plane, `u8` for the 1-byte
    /// device-64 (RGB222) plane.
    type Pixel: Copy;
    /// Pack a rendered colour into its stored representation.
    fn pack(c: Rgb565) -> Self::Pixel;
}

/// Identity pack for the native-RGB565 plane: the stored `u16` is the colour's own storage word.
/// Backs the RGB565 [`Band`](crate::Band) scratch the [`Panel`](crate::Panel) backend reformats
/// per push (and would back any board with a native-RGB565 scan-out plane).
pub struct PackRgb565;
impl Pack for PackRgb565 {
    type Pixel = u16;
    #[inline]
    fn pack(c: Rgb565) -> u16 {
        c.into_storage()
    }
}

/// Device-64 (RGB222) pack for the nRF's device-native full-frame plane: the top 2 bits of each
/// RGB565 channel, packed into a single byte (`0b00_RR_GG_BB`) — one byte per pixel, so a 240×320
/// frame is 75 KB instead of RGB565's 150 KB, which is what lets it live in the nRF's 256 KB
/// on-chip SRAM (it has no external RAM). The 2-bit-per-channel quantization *is* the
/// LS021B7DD02's intended fidelity — the style colours are already tuned to this 64-colour gamut
/// (`obc_reader::rgb565_to_device64`), so storing it is the target format, not a loss.
///
/// The renderer stays `Rgb565`-typed throughout; the framebuffer quantizes on store here, and the
/// banded [`Panel`](crate::Panel) push expands each byte back to RGB565
/// ([`device64_to_rgb565`]) for the ST7789 — or, on the FLPR/LS021B7DD02, packs it to that
/// panel's wire bytes ([`ls021_wire::pack_row`](crate::ls021_wire::pack_row)). The byte value
/// `0..64` doubles as the device-64 palette index.
pub struct PackDevice64;
impl Pack for PackDevice64 {
    type Pixel = u8;
    #[inline]
    fn pack(c: Rgb565) -> u8 {
        rgb565_to_device64_byte(c)
    }
}

/// A `DrawTarget` wrapping a borrowed `width * height` buffer (one pixel per stored cell,
/// row-major), generic over how an [`Rgb565`] colour is packed into a stored pixel — and over
/// the *type* of that pixel ([`Pack::Pixel`]: `u16` for RGB565, `u8` for device-64).
/// The buffer is the board's — the nRF's resident RGB222 frame in `.bss`, or a per-band RGB565
/// scratch — so this owns nothing and only writes pixels.
///
/// Every display plane is this one type with a different [`Pack`]: the opaque RGB565 map plane
/// ([`Framebuffer565`], [`PackRgb565`]) and the nRF's device-native RGB222 plane ([`FbDevice64`],
/// [`PackDevice64`]). The `_pack` marker is zero-sized, so this is the same size as a bare
/// `{ width, height, buf }`.
pub struct RawFb<'a, P: Pack> {
    width: u32,
    height: u32,
    buf: &'a mut [P::Pixel],
    _pack: PhantomData<P>,
}

/// The native-RGB565 plane: every pixel stored as its own RGB565 word. On the shipping nRF this
/// backs the per-band [`Band`](crate::Band) scratch the [`Panel`] push reformats; a board with a
/// hardware scan-out plane would rescan a full-frame instance of it directly.
pub type Framebuffer565<'a> = RawFb<'a, PackRgb565>;

/// The nRF's device-native **RGB222** full-frame plane: one byte per pixel (top 2 bits/channel,
/// see [`PackDevice64`]), so the whole 240×320 frame is 75 KB and fits the nRF's on-chip SRAM.
/// The shared [`obc_app::App::render_map`](obc_app::App) draws into it exactly as it does the
/// RGB565 plane (the renderer is `Rgb565`-typed; the framebuffer quantizes on store), then the
/// banded [`Panel`](crate::Panel) push reads it back row by row, expanding each byte to RGB565
/// ([`device64_to_rgb565`]) for the ST7789 (issue #125). This is the device path the project
/// ships on; [`Framebuffer565`] survives only as the [`Band`](crate::Band) scratch interchange.
pub type FbDevice64<'a> = RawFb<'a, PackDevice64>;

impl<'a, P: Pack> RawFb<'a, P> {
    /// Wrap `buf` as a `width`×`height` target. `buf` must hold at least
    /// `width * height` pixels; a shorter slice is a board wiring bug, so it panics
    /// (this is bring-up code — better a clear panic over RTT than a silent
    /// out-of-bounds later).
    pub fn new(buf: &'a mut [P::Pixel], width: u32, height: u32) -> Self {
        assert!(buf.len() >= (width * height) as usize, "framebuffer slice smaller than width*height");
        RawFb { width, height, buf, _pack: PhantomData }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Write one already-packed pixel, clipping silently to the buffer bounds (the
    /// renderer projects geometry that can land off-screen).
    #[inline]
    fn put(&mut self, x: i32, y: i32, raw: P::Pixel) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.buf[y as usize * self.width as usize + x as usize] = raw;
    }
}

impl<P: Pack> OriginDimensions for RawFb<'_, P> {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl<P: Pack> DrawTarget for RawFb<'_, P> {
    type Color = Rgb565;
    // The buffer can't fail to accept a pixel; out-of-bounds writes are clipped.
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, P::pack(c));
        }
        Ok(())
    }

    /// Fast path for the renderer's scanline fills (it calls this per polygon row and
    /// to clear, and the overlay bulge is rasterized as edge-perpendicular strips):
    /// fill a clipped rectangle one **contiguous row-slice** at a time. Each row is a
    /// single `<[u16]>::fill`, so the inner loop carries no per-pixel bounds check and
    /// the compiler can coalesce the stores (word-wide where aligned) instead of the
    /// element-indexed `buf[row + x] = raw` it can't prove in-bounds — the polygon
    /// scanline fill is the renderer's dominant draw cost (issue #98 P3).
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clipped = area.intersection(&self.bounding_box());
        if let Some(br) = clipped.bottom_right() {
            let raw = P::pack(color);
            let w = self.width as usize;
            // Clipped to `bounding_box` (origin 0,0): `0 <= x0 <= x1 < width` and
            // `0 <= y <= height-1`, so `row + x0 ..= row + x1` is always in bounds.
            let (x0, x1) = (clipped.top_left.x as usize, br.x as usize);
            for y in clipped.top_left.y..=br.y {
                let row = y as usize * w;
                self.buf[row + x0..=row + x1].fill(raw);
            }
        }
        Ok(())
    }

    /// Fill the whole plane with `color` — the map redraw's clear.
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        // `fill` lowers to a burstable memset across the wait-stated FMC, far cheaper than a
        // per-pixel store on every map redraw.
        self.buf.fill(P::pack(color));
        Ok(())
    }
}

/// Pack an [`Rgb565`] colour into a **device-64 (RGB222)** byte: the top 2 bits of each channel
/// in `0b00_RR_GG_BB`. Keeping the top 2 bits is exactly the quantization
/// `obc_reader::rgb565_to_device64` applies (it keeps `channel8 >> 6`, and the top 2 bits of an
/// RGB565 channel *are* the top 2 of its RGB888 expansion), so this stores the same 64-colour
/// gamut the style table is tuned to — the byte `0..64` is that palette index. The inverse is
/// [`device64_to_rgb565`].
#[inline]
fn rgb565_to_device64_byte(c: Rgb565) -> u8 {
    let rgb = c.into_storage(); // RRRRR GGGGGG BBBBB
    let r = ((rgb >> 14) & 0x3) as u8; // top 2 of the 5-bit red
    let g = ((rgb >> 9) & 0x3) as u8; // top 2 of the 6-bit green
    let b = ((rgb >> 3) & 0x3) as u8; // top 2 of the 5-bit blue
    (r << 4) | (g << 2) | b
}

/// Expand a **device-64 (RGB222)** byte (`0b00_RR_GG_BB`, see [`rgb565_to_device64_byte`]) back to
/// an [`Rgb565`] storage word, for the banded [`Panel`](crate::Panel) push to feed an RGB565 panel
/// (the ST7789). Each 2-bit channel is bit-replicated up to its RGB565 width (5 or 6 bits) — which,
/// for these four levels, lands on exactly the values `round(level * max / 3)`, i.e. the same
/// `{0, ⅓, ⅔, 1}` ramp the simulator previews via `obc_reader::rgb565_to_device64`. Lossless on the
/// gamut: re-packing the result with [`rgb565_to_device64_byte`] yields the original byte.
#[inline]
pub fn device64_to_rgb565(byte: u8) -> u16 {
    let rq = ((byte >> 4) & 0x3) as u16;
    let gq = ((byte >> 2) & 0x3) as u16;
    let bq = (byte & 0x3) as u16;
    // Replicate the 2-bit value across the channel: `ab` → `ababa` (5-bit) / `ababab` (6-bit).
    let r5 = (rq << 3) | (rq << 1) | (rq >> 1);
    let g6 = (gq << 4) | (gq << 2) | gq;
    let b5 = (bq << 3) | (bq << 1) | (bq >> 1);
    (r5 << 11) | (g6 << 5) | b5
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::pixelcolor::raw::RawU16;

    fn fb(buf: &mut [u16], w: u32, h: u32) -> Framebuffer565<'_> {
        Framebuffer565::new(buf, w, h)
    }

    #[test]
    fn put_writes_native_rgb565_and_clips() {
        let mut buf = [0u16; 2 * 2];
        let mut fb = fb(&mut buf, 2, 2);
        let red = Rgb565::from(RawU16::new(0xF800));
        fb.draw_iter([
            Pixel(Point::new(0, 1), red),
            Pixel(Point::new(5, 5), red),  // off-screen: dropped
            Pixel(Point::new(-1, 0), red), // off-screen: dropped
        ])
        .unwrap();
        assert_eq!(buf, [0x0000, 0x0000, 0xF800, 0x0000]);
    }

    #[test]
    fn clear_sets_every_pixel() {
        let mut buf = [0u16; 4 * 3];
        let raw = 0x07E0; // green
        fb(&mut buf, 4, 3).clear(Rgb565::from(RawU16::new(raw))).unwrap();
        assert!(buf.iter().all(|&p| p == raw));
    }

    #[test]
    fn fill_solid_fills_subrect_and_clips() {
        let mut buf = [0u16; 4 * 4];
        {
            let mut fb = fb(&mut buf, 4, 4);
            // Rectangle straddling the right/bottom edge: only the in-bounds part fills.
            fb.fill_solid(&Rectangle::new(Point::new(2, 2), Size::new(10, 10)), Rgb565::from(RawU16::new(0x001F)))
                .unwrap();
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(1, 1), 0x0000); // outside
        assert_eq!(at(2, 2), 0x001F); // inside corner
        assert_eq!(at(3, 3), 0x001F); // last in-bounds pixel
    }

    /// Item 10 (negative top-left clip, `fill_solid` ~184 `area.intersection`): existing
    /// tests only overrun the right/bottom edge; a rectangle whose top-left is *negative*
    /// must be clipped by the intersection so the fill starts at (0,0), never indexing the
    /// buffer with a negative `x0`/`y0`. A rect at (-2,-2) sized 4×4 covers only (0,0)..(1,1).
    #[test]
    fn fill_solid_clips_a_negative_top_left() {
        let mut buf = [0u16; 4 * 4];
        {
            let mut fb = fb(&mut buf, 4, 4);
            fb.fill_solid(&Rectangle::new(Point::new(-2, -2), Size::new(4, 4)), Rgb565::from(RawU16::new(0x001F)))
                .unwrap();
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(0, 0), 0x001F); // clipped origin fills
        assert_eq!(at(1, 1), 0x001F); // last covered pixel (rect reaches x=y=1)
        assert_eq!(at(2, 2), 0x0000); // beyond the clipped rect: untouched
    }

    #[test]
    #[should_panic]
    fn too_small_buffer_panics() {
        let mut buf = [0u16; 3];
        let _ = Framebuffer565::new(&mut buf, 2, 2); // needs 4
    }

    // --- device-64 (RGB222) plane (issue #125) ---

    fn dev(buf: &mut [u8], w: u32, h: u32) -> FbDevice64<'_> {
        FbDevice64::new(buf, w, h)
    }

    /// The pack stores one byte/pixel: the top 2 bits of each channel in `0b00_RR_GG_BB`. Pure
    /// channels land in the expected bit positions, and an off-screen pixel is clipped.
    #[test]
    fn device64_packs_top_two_bits_per_channel_and_clips() {
        let mut buf = [0u8; 2 * 2];
        let mut fb = dev(&mut buf, 2, 2);
        let red = Rgb565::from(RawU16::new(0xF800)); // full red  → R=11
        let grn = Rgb565::from(RawU16::new(0x07E0)); // full green→ G=11
        let blu = Rgb565::from(RawU16::new(0x001F)); // full blue → B=11
        fb.draw_iter([
            Pixel(Point::new(0, 0), red),
            Pixel(Point::new(1, 0), grn),
            Pixel(Point::new(0, 1), blu),
            Pixel(Point::new(9, 9), red), // off-screen: dropped
        ])
        .unwrap();
        assert_eq!(buf, [0b00_11_00_00, 0b00_00_11_00, 0b00_00_00_11, 0x00]);
    }

    /// The byte value is the device-64 palette index `0..64` — white is the max (0x3F), black 0.
    #[test]
    fn device64_byte_is_the_palette_index() {
        assert_eq!(rgb565_to_device64_byte(Rgb565::from(RawU16::new(0xFFFF))), 0x3F); // white
        assert_eq!(rgb565_to_device64_byte(Rgb565::from(RawU16::new(0x0000))), 0x00);
        // black
    }

    /// The pack keeps exactly the bits `obc_reader::rgb565_to_device64` keeps: the top 2 of each
    /// channel's *RGB888* expansion (`channel8 >> 6`). Cross-checked here against the same RGB565→888
    /// math that helper uses, so the on-glass gamut matches the simulator preview's 64 colours.
    #[test]
    fn device64_matches_reader_gamut_indices() {
        // The exact channel expansion obc_reader::rgb565_to_rgb888 uses.
        let to888 = |c: u16| {
            let r5 = ((c >> 11) & 0x1F) as u8;
            let g6 = ((c >> 5) & 0x3F) as u8;
            let b5 = (c & 0x1F) as u8;
            ((r5 << 3) | (r5 >> 2), (g6 << 2) | (g6 >> 4), (b5 << 3) | (b5 >> 2))
        };
        for raw in [0x0000u16, 0xFFFF, 0xF800, 0x07E0, 0x001F, 0x1234, 0xABCD, 0x8410, 0x5555] {
            let byte = rgb565_to_device64_byte(Rgb565::from(RawU16::new(raw)));
            let (r8, g8, b8) = to888(raw);
            let want = ((r8 >> 6) << 4) | ((g8 >> 6) << 2) | (b8 >> 6); // rgb565_to_device64's indices
            assert_eq!(byte, want, "raw {raw:#06x}");
        }
    }

    /// Expansion is the exact inverse of the pack on the gamut: byte → RGB565 → byte round-trips,
    /// so the banded push reconstructs the stored colour losslessly. (The RGB565 it produces also
    /// re-quantizes to the same byte — the property the Panel push relies on.)
    #[test]
    fn device64_expand_roundtrips_every_byte() {
        for byte in 0u8..64 {
            let rgb = Rgb565::from(RawU16::new(device64_to_rgb565(byte)));
            assert_eq!(rgb565_to_device64_byte(rgb), byte, "byte {byte:#04x}");
        }
    }

    /// Expansion lands each 2-bit level on the `{0, ⅓, ⅔, 1}` ramp — the same levels the simulator
    /// shows via `obc_reader::rgb565_to_device64` (which expands to RGB888 by `level * 85`).
    #[test]
    fn device64_expand_levels_match_the_quarter_ramp() {
        // R channel sweep (top 2 bits) → expected 5-bit values round(level*31/3).
        let r5 = |byte: u8| (device64_to_rgb565(byte) >> 11) & 0x1F;
        assert_eq!([r5(0b00_0000), r5(0b01_0000), r5(0b10_0000), r5(0b11_0000)], [0, 10, 21, 31]);
        // G channel (6-bit) → round(level*63/3).
        let g6 = |byte: u8| (device64_to_rgb565(byte) >> 5) & 0x3F;
        assert_eq!([g6(0b00_0000), g6(0b00_0100), g6(0b00_1000), g6(0b00_1100)], [0, 21, 42, 63]);
    }

    #[test]
    fn device64_fill_solid_and_clear() {
        let mut buf = [0u8; 4 * 4];
        {
            let mut fb = dev(&mut buf, 4, 4);
            fb.clear(Rgb565::from(RawU16::new(0xFFFF))).unwrap(); // white → 0x3F everywhere
            fb.fill_solid(&Rectangle::new(Point::new(1, 1), Size::new(2, 2)), Rgb565::from(RawU16::new(0x0000)))
                .unwrap(); // black sub-rect
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(0, 0), 0x3F); // cleared white
        assert_eq!(at(1, 1), 0x00); // black sub-rect
        assert_eq!(at(2, 2), 0x00);
        assert_eq!(at(3, 3), 0x3F); // outside the sub-rect
    }
}
