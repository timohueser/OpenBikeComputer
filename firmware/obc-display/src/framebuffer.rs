//! `DrawTarget`s the board owns and the shared renderer draws into: the nRF's device-native
//! RGB222 [`FbDevice64`] map plane, the real target, and the [`Framebuffer565`] RGB565 plane, the
//! per-band scratch the [`Band`](crate::Band) view wraps.
//!
//! The renderer stays `Rgb565`-typed everywhere and the per-board pixel format is the [`Pack`]'s
//! business, so the gamut the simulator previews is what the nRF stores and shows. Both planes are
//! the same `DrawTarget` over a borrowed buffer, differing only in the stored pixel type, so the
//! body is written once over `P: Pack` and `P::pack` is monomorphized.

use core::marker::PhantomData;

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

/// How a [`RawFb`] packs an [`Rgb565`] colour into one stored pixel, and what type that pixel is:
/// the only per-plane difference. Impls are zero-sized markers and `pack` is `#[inline]`, so a
/// packed pixel is a static call in the per-pixel render loop, and the associated type gives a
/// board's native byte width for free.
pub trait Pack {
    /// The stored pixel type: `u16` for RGB565, `u8` for the device-64 (RGB222) plane.
    type Pixel: Copy;
    fn pack(c: Rgb565) -> Self::Pixel;
}

/// Identity pack for the native-RGB565 plane: the stored `u16` is the colour's own storage word.
pub struct PackRgb565;
impl Pack for PackRgb565 {
    type Pixel = u16;
    #[inline]
    fn pack(c: Rgb565) -> u16 {
        c.into_storage()
    }
}

/// Device-64 (RGB222) pack for the nRF's full-frame plane: the top 2 bits of each RGB565 channel
/// in one byte, so a 240×320 frame is 75 KB and fits on-chip SRAM. The quantization is the
/// LS021B7DD02's intended fidelity and the style colours are tuned to this gamut, so it is the
/// target format, not a loss. The byte value doubles as the palette index.
pub struct PackDevice64;
impl Pack for PackDevice64 {
    type Pixel = u8;
    #[inline]
    fn pack(c: Rgb565) -> u8 {
        rgb565_to_device64_byte(c.into_storage())
    }
}

/// A `DrawTarget` wrapping a borrowed `width * height` buffer, row-major, generic over the
/// [`Pack`]. The buffer is the board's, so this owns nothing and only writes pixels.
///
/// Writes outside the clip rect are discarded, so a host that knows this frame's change is
/// contained in a region can replay the full draw sequence and pay only for that region. The clip
/// is the bounds check and defaults to the whole frame.
pub struct RawFb<'a, P: Pack> {
    width: u32,
    height: u32,
    // Clip bounds, half-open and always inside the frame, so the default adds no per-pixel cost.
    cx0: i32,
    cy0: i32,
    cx1: i32,
    cy1: i32,
    buf: &'a mut [P::Pixel],
    _pack: PhantomData<P>,
}

/// The native-RGB565 plane: every pixel stored as its own RGB565 word.
pub type Framebuffer565<'a> = RawFb<'a, PackRgb565>;

/// The nRF's device-native RGB222 full-frame plane, one byte per pixel. The overlay composite
/// reads it back row by row, expanding each byte to RGB565, and the simulator does the same.
pub type FbDevice64<'a> = RawFb<'a, PackDevice64>;

impl<'a, P: Pack> RawFb<'a, P> {
    /// Wrap `buf` as a `width`×`height` target. A shorter slice is a wiring bug and panics.
    pub fn new(buf: &'a mut [P::Pixel], width: u32, height: u32) -> Self {
        assert!(buf.len() >= (width * height) as usize, "framebuffer slice smaller than width*height");
        RawFb { width, height, cx0: 0, cy0: 0, cx1: width as i32, cy1: height as i32, buf, _pack: PhantomData }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Restrict every subsequent write to `area`, intersected with the frame. The caller's
    /// contract is that everything it wants changed lies inside `area`, because writes outside are
    /// dropped and not deferred. A disjoint `area` empties the clip.
    pub fn set_clip(&mut self, area: Rectangle) {
        let c = area.intersection(&Rectangle::new(Point::zero(), Size::new(self.width, self.height)));
        self.cx0 = c.top_left.x;
        self.cy0 = c.top_left.y;
        self.cx1 = c.top_left.x + c.size.width as i32;
        self.cy1 = c.top_left.y + c.size.height as i32;
    }

    /// The current clip as a `Rectangle`, the rect the fill paths intersect against.
    fn clip_rect(&self) -> Rectangle {
        Rectangle::new(
            Point::new(self.cx0, self.cy0),
            Size::new((self.cx1 - self.cx0) as u32, (self.cy1 - self.cy0) as u32),
        )
    }

    /// Write one already-packed pixel, clipped silently: the renderer projects geometry that can
    /// land off-screen, and [`set_clip`](RawFb::set_clip) narrows it further.
    #[inline]
    fn put(&mut self, x: i32, y: i32, raw: P::Pixel) {
        if x < self.cx0 || y < self.cy0 || x >= self.cx1 || y >= self.cy1 {
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

    /// Fast path for the renderer's scanline fills: fill a clipped rectangle one contiguous row
    /// slice at a time, so the inner loop carries no per-pixel bounds check and the compiler can
    /// coalesce the stores. The polygon scanline fill is the renderer's dominant draw cost.
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clipped = area.intersection(&self.clip_rect());
        if let Some(br) = clipped.bottom_right() {
            let raw = P::pack(color);
            let w = self.width as usize;
            // Clipped to `clip_rect`, so the row slice is always in bounds.
            let (x0, x1) = (clipped.top_left.x as usize, br.x as usize);
            for y in clipped.top_left.y..=br.y {
                let row = y as usize * w;
                self.buf[row + x0..=row + x1].fill(raw);
            }
        }
        Ok(())
    }

    /// The default `fill_contiguous` is `draw_iter` over the area's points, with a lazy colors
    /// iterator such as the glyph decode. Rejecting a whole area that misses the clip skips that
    /// decode for one rect test.
    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        if area.intersection(&self.clip_rect()).is_zero_sized() {
            return Ok(());
        }
        // The default impl's body: pair the area's row-major points with the colors.
        self.draw_iter(area.points().zip(colors).map(|(p, c)| Pixel(p, c)))
    }

    /// Fill the whole plane with `color`, clipped like every other write.
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        if self.cx0 == 0 && self.cy0 == 0 && self.cx1 == self.width as i32 && self.cy1 == self.height as i32 {
            // `fill` lowers to a bulk memset, far cheaper than a per-pixel store on every map redraw.
            self.buf.fill(P::pack(color));
        } else {
            self.fill_solid(&self.clip_rect(), color)?;
        }
        Ok(())
    }
}

/// Pack an RGB565 storage word into a device-64 (RGB222) byte: the top 2 bits of each channel.
/// That is the same quantization `obc_reader::rgb565_to_device64` applies, so it stores the same
/// 64-colour gamut the style table is tuned to, and [`device64_to_rgb565`] is the inverse.
///
/// Public because it is the one place this formula lives: every re-derivation is a chance for
/// on-glass colour to fork from the simulator preview.
#[inline]
pub fn rgb565_to_device64_byte(rgb: u16) -> u8 {
    // rgb is RRRRR GGGGGG BBBBB.
    let r = ((rgb >> 14) & 0x3) as u8; // top 2 of the 5-bit red
    let g = ((rgb >> 9) & 0x3) as u8; // top 2 of the 6-bit green
    let b = ((rgb >> 3) & 0x3) as u8; // top 2 of the 5-bit blue
    (r << 4) | (g << 2) | b
}

/// Expand a device-64 byte back to an [`Rgb565`] storage word for the banded push. Each 2-bit
/// channel is bit-replicated, landing on `round(level * max / 3)`, and re-packing the result
/// yields the original byte.
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

    /// A rectangle with a negative top-left is clipped by the intersection, so the fill starts at (0,0).
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

    fn dev(buf: &mut [u8], w: u32, h: u32) -> FbDevice64<'_> {
        FbDevice64::new(buf, w, h)
    }

    /// The pack stores one byte per pixel, and an off-screen pixel is clipped.
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

    /// The byte value is the device-64 palette index: white is the max, black 0.
    #[test]
    fn device64_byte_is_the_palette_index() {
        assert_eq!(rgb565_to_device64_byte(0xFFFF), 0x3F); // white
        assert_eq!(rgb565_to_device64_byte(0x0000), 0x00);
        // black
    }

    /// The pack keeps exactly the bits `obc_reader::rgb565_to_device64` keeps, cross-checked
    /// against the same expansion that helper uses, so the on-glass gamut matches the preview.
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
            let byte = rgb565_to_device64_byte(raw);
            let (r8, g8, b8) = to888(raw);
            let want = ((r8 >> 6) << 4) | ((g8 >> 6) << 2) | (b8 >> 6); // rgb565_to_device64's indices
            assert_eq!(byte, want, "raw {raw:#06x}");
        }
    }

    /// Exhaustive pin over all 65,536 RGB565 values: this pack must agree with the simulator
    /// preview's quantizer at the palette index. The two crates implement it independently, so a
    /// change to one would otherwise silently fork on-glass colour from the preview. Only the
    /// quantization is pinned, not the expansion, which differs by at most 3/255 per channel.
    #[test]
    fn device64_matches_reader_quantization_exhaustively() {
        for raw in 0u16..=u16::MAX {
            let byte = rgb565_to_device64_byte(raw);
            // The reader expands on the `level * 85` ramp, so `/ 85` recovers the 2-bit index.
            let (r, g, b) = obc_reader::rgb565_to_device64(raw);
            let want = ((r / 85) << 4) | ((g / 85) << 2) | (b / 85);
            assert_eq!(byte, want, "raw {raw:#06x}");
        }
    }

    /// Expansion is the exact inverse of the pack on the gamut, so the banded push is lossless.
    #[test]
    fn device64_expand_roundtrips_every_byte() {
        for byte in 0u8..64 {
            assert_eq!(rgb565_to_device64_byte(device64_to_rgb565(byte)), byte, "byte {byte:#04x}");
        }
    }

    /// Expansion lands each 2-bit level on the `{0, ⅓, ⅔, 1}` ramp the simulator shows.
    #[test]
    fn device64_expand_levels_match_the_quarter_ramp() {
        // R channel sweep (top 2 bits) → expected 5-bit values round(level*31/3).
        let r5 = |byte: u8| (device64_to_rgb565(byte) >> 11) & 0x1F;
        assert_eq!([r5(0b00_0000), r5(0b01_0000), r5(0b10_0000), r5(0b11_0000)], [0, 10, 21, 31]);
        // G channel (6-bit) → round(level*63/3).
        let g6 = |byte: u8| (device64_to_rgb565(byte) >> 5) & 0x3F;
        assert_eq!([g6(0b00_0000), g6(0b00_0100), g6(0b00_1000), g6(0b00_1100)], [0, 21, 42, 63]);
    }

    /// `set_clip` discards `put`s outside the clip, the silent-drop contract off-frame writes had.
    #[test]
    fn clip_discards_pixel_writes_outside() {
        let mut buf = [0u16; 4 * 4];
        let mut fb = fb(&mut buf, 4, 4);
        fb.set_clip(Rectangle::new(Point::new(1, 1), Size::new(2, 2)));
        let red = Rgb565::from(RawU16::new(0xF800));
        fb.draw_iter([
            Pixel(Point::new(1, 1), red), // inside: lands
            Pixel(Point::new(0, 0), red), // outside (before the clip): dropped
            Pixel(Point::new(3, 2), red), // outside (past the clip's right edge): dropped
        ])
        .unwrap();
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(1, 1), 0xF800);
        assert_eq!(at(0, 0), 0x0000);
        assert_eq!(at(3, 2), 0x0000);
    }

    /// `fill_solid` and `clear` both restrict to the clip, so a clipped `clear` leaves the rest of
    /// the frame byte-identical, which is what lets a region repaint replay a full screen draw.
    #[test]
    fn clip_restricts_fill_solid_and_clear() {
        let mut buf = [0u16; 4 * 4];
        let mut fb = fb(&mut buf, 4, 4);
        fb.set_clip(Rectangle::new(Point::new(2, 2), Size::new(2, 2)));
        fb.clear(Rgb565::from(RawU16::new(0x07E0))).unwrap();
        fb.fill_solid(&Rectangle::new(Point::new(0, 0), Size::new(4, 3)), Rgb565::from(RawU16::new(0x001F))).unwrap();
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(0, 0), 0x0000, "outside the clip: clear + fill both discarded");
        assert_eq!(at(2, 2), 0x001F, "inside the clip and the fill rect");
        assert_eq!(at(2, 3), 0x07E0, "inside the clip, below the fill rect: keeps the clear");
    }

    /// A `fill_contiguous` area disjoint from the clip is skipped whole; a straddling one clips.
    #[test]
    fn clip_rejects_and_straddles_fill_contiguous() {
        let red = Rgb565::from(RawU16::new(0xF800));
        let mut buf = [0u16; 4 * 4];
        {
            let mut fb = fb(&mut buf, 4, 4);
            fb.set_clip(Rectangle::new(Point::new(0, 2), Size::new(4, 2)));
            // Disjoint (rows 0–1): consumed nothing, wrote nothing.
            fb.fill_contiguous(&Rectangle::new(Point::new(0, 0), Size::new(2, 2)), [red; 4]).unwrap();
            // Straddling rows 1–2: only row 2 (in-clip) lands, from the iterator's correct offsets.
            fb.fill_contiguous(&Rectangle::new(Point::new(0, 1), Size::new(2, 2)), [red; 4]).unwrap();
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(0, 0), 0x0000, "a disjoint contiguous fill is rejected whole");
        assert_eq!(at(1, 1), 0x0000, "…and so is the straddler's out-of-clip row");
        assert_eq!(at(0, 1), 0x0000, "the out-of-clip half is discarded");
        assert_eq!(at(0, 2), 0xF800, "the in-clip half lands");
        assert_eq!(at(1, 2), 0xF800);
    }

    /// A clip that misses the frame entirely empties: every write discards, nothing panics.
    #[test]
    fn clip_disjoint_from_frame_discards_everything() {
        let mut buf = [0u16; 2 * 2];
        let mut fb = fb(&mut buf, 2, 2);
        fb.set_clip(Rectangle::new(Point::new(10, 10), Size::new(4, 4)));
        let red = Rgb565::from(RawU16::new(0xF800));
        fb.draw_iter([Pixel(Point::new(0, 0), red)]).unwrap();
        fb.clear(red).unwrap();
        fb.fill_solid(&Rectangle::new(Point::zero(), Size::new(2, 2)), red).unwrap();
        assert!(buf.iter().all(|&p| p == 0));
    }

    /// A full-frame `set_clip` behaves exactly like no clip.
    #[test]
    fn full_frame_clip_is_identity() {
        let red = Rgb565::from(RawU16::new(0xF800));
        let mut plain = [0u16; 3 * 3];
        let mut clipped = [0u16; 3 * 3];
        let paint = |fb: &mut Framebuffer565| {
            fb.clear(Rgb565::from(RawU16::new(0x07E0))).unwrap();
            fb.fill_solid(&Rectangle::new(Point::new(1, 1), Size::new(5, 1)), red).unwrap();
            fb.draw_iter([Pixel(Point::new(2, 2), red), Pixel(Point::new(-1, 5), red)]).unwrap();
        };
        paint(&mut fb(&mut plain, 3, 3));
        let mut fbc = fb(&mut clipped, 3, 3);
        fbc.set_clip(Rectangle::new(Point::zero(), Size::new(3, 3)));
        paint(&mut fbc);
        assert_eq!(plain, clipped);
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
