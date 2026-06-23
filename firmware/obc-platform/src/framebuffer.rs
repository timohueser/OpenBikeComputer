//! `DrawTarget`s over raw SDRAM framebuffers the LTDC scans: the opaque
//! [`Framebuffer565`] map plane and the transparent [`FramebufferArgb4444`] overlay
//! plane.
//!
//! The on-device counterpart of the simulator's `obc-sim/src/framebuffer.rs`: the
//! shared [`obc_render::MapRenderer`](../../obc_render) (driven through
//! [`obc_app::App::render_frame`](../../obc_app)) runs the exact same rendering
//! code on the host and on the MCU, drawing into a buffer the board owns. Here the
//! buffer is the LTDC-scanned SDRAM framebuffer: a flat `width * height` array of
//! native pixels, hardware-rescanned to the panel every frame, so a written pixel
//! is on glass with no blit.
//!
//! The panel is native RGB565, so the `color_fn` the app is rendered with is the
//! **identity** `RGB565 -> Rgb565` (`|c| Rgb565::from(RawU16::new(c))`). The
//! device-64 quantization (`obc_reader::rgb565_to_device64`) the simulator applies
//! is a host concern — a preview of the final LS021B7DD02's RGB222 gamut — and has
//! no place on the device path.
//!
//! [`FramebufferArgb4444`] is the second, per-pixel-alpha-blended LTDC layer (issue
//! #46): the UI overlay (the hold-bulge / confirm ring) renders into it so the LTDC
//! composites it over the map in hardware, and the overlay repaints without ever
//! touching the map plane. It shares the *same* `Color` and `color_fn` as
//! [`Framebuffer565`] — every drawn pixel is stored opaque, transparency is only the
//! cleared state — so the shared overlay renderer is board- and layer-agnostic.
//!
//! Both planes are the *same* `DrawTarget`: a borrowed `width * height` `u16` buffer,
//! a clipped pixel `put`, a scanline `fill_solid` and a `clear`. The two layers differ
//! by exactly one thing — how an [`Rgb565`] colour is packed into the stored `u16`
//! (native RGB565 vs. opaque ARGB4444). That single difference is captured by the
//! zero-sized [`Pack`] marker, so the framebuffer body is written **once**, generic
//! over `P: Pack`, and [`Framebuffer565`] / [`FramebufferArgb4444`] are thin type
//! aliases. `P::pack` is a static (monomorphized) call — no per-pixel indirection in
//! the hot render loop — and the markers are zero-sized, so a [`RawFb`] is the same
//! size as a bare `{ width, height, buf }` struct. A third panel (the nRF54's) only
//! has to add a third `Pack` impl, not a third copy of the body.

use core::marker::PhantomData;

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

/// How a [`RawFb`] packs an [`Rgb565`] colour into the stored `u16` pixel. This is the
/// *only* per-layer difference between the map and overlay framebuffers; everything
/// else (clip, scanline fill, clear) is shared. Impls are zero-sized markers and
/// `pack` is `#[inline]`, so a packed pixel costs a static call the compiler folds in —
/// never a `fn`-pointer / `dyn` indirection in the per-pixel render loop.
pub trait Pack {
    /// Pack a rendered colour into its native stored representation.
    fn pack(c: Rgb565) -> u16;
}

/// Identity pack for the native-RGB565 map plane: the panel *is* RGB565, so the
/// stored `u16` is the colour's own storage word.
pub struct PackRgb565;
impl Pack for PackRgb565 {
    #[inline]
    fn pack(c: Rgb565) -> u16 {
        c.into_storage()
    }
}

/// Opaque-ARGB4444 pack for the overlay plane: every drawn pixel is forced opaque
/// (alpha `0xF`) via [`opaque_argb4444`]; transparency is only ever the cleared state.
pub struct PackOpaqueArgb4444;
impl Pack for PackOpaqueArgb4444 {
    #[inline]
    fn pack(c: Rgb565) -> u16 {
        opaque_argb4444(c)
    }
}

/// A `DrawTarget` wrapping a borrowed `width * height` `u16` buffer (one pixel per
/// `u16`, row-major), generic over how an [`Rgb565`] colour is packed into a stored
/// pixel ([`Pack`]). The buffer is the board's — typically a slice over the SDRAM
/// framebuffer the LTDC scans — so this owns nothing and only writes pixels.
///
/// Both display planes are this one type with a different [`Pack`]: the opaque map
/// plane ([`Framebuffer565`], [`PackRgb565`]) and the transparent overlay plane
/// ([`FramebufferArgb4444`], [`PackOpaqueArgb4444`]). The `_pack` marker is zero-sized,
/// so this is the same size as a bare `{ width, height, buf }`.
pub struct RawFb<'a, P: Pack> {
    width: u32,
    height: u32,
    buf: &'a mut [u16],
    _pack: PhantomData<P>,
}

/// The native-RGB565 map plane: the LTDC's opaque bottom layer. Every pixel is stored
/// as its own RGB565 word, hardware-rescanned to the panel each frame.
pub type Framebuffer565<'a> = RawFb<'a, PackRgb565>;

/// The transparent ARGB4444 overlay plane (issue #46) — the dual-layer display's
/// second layer. Where [`Framebuffer565`] backs the LTDC's opaque map layer, this
/// backs the per-pixel-alpha-blended layer above it: the LTDC composites the two in
/// hardware (`BC = α·overlay + (1−α)·map`), so the overlay (the hold-bulge / confirm
/// ring) repaints without re-rendering — or even reading — the map plane.
///
/// The [`Color`](DrawTarget::Color) is [`Rgb565`], **identical** to [`Framebuffer565`],
/// so the same `color_fn` drives both layers and the shared overlay renderer is
/// layer-agnostic. The overlay only ever draws *opaque* pixels (the near-black bulge),
/// so every write is packed opaque ([`opaque_argb4444`]); the only transparency is the
/// cleared background, reset by
/// [`clear_transparent`](RawFb::<PackOpaqueArgb4444>::clear_transparent). 4-bit channels
/// are ample for the flat near-black bulge, and at 2 bytes/px the buffer is the same
/// 150 KB as the RGB565 map buffer.
pub type FramebufferArgb4444<'a> = RawFb<'a, PackOpaqueArgb4444>;

impl<'a, P: Pack> RawFb<'a, P> {
    /// Wrap `buf` as a `width`×`height` target. `buf` must hold at least
    /// `width * height` pixels; a shorter slice is a board wiring bug, so it panics
    /// (this is bring-up code — better a clear panic over RTT than a silent
    /// out-of-bounds later).
    pub fn new(buf: &'a mut [u16], width: u32, height: u32) -> Self {
        assert!(buf.len() >= (width * height) as usize, "framebuffer slice smaller than width*height");
        RawFb { width, height, buf, _pack: PhantomData }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw stored pixels — `width * height` long. Handy for tests and for a
    /// board that wants to DMA the buffer somewhere itself.
    pub fn as_u16(&self) -> &[u16] {
        self.buf
    }

    /// Write one already-packed pixel, clipping silently to the buffer bounds (the
    /// renderer projects geometry that can land off-screen).
    #[inline]
    fn put(&mut self, x: i32, y: i32, raw: u16) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.buf[y as usize * self.width as usize + x as usize] = raw;
    }
}

/// The overlay plane's transparent reset — inherent to the ARGB4444 alias only, since
/// transparency is meaningless on the opaque RGB565 plane.
impl<'a> RawFb<'a, PackOpaqueArgb4444> {
    /// Reset the whole plane to **fully transparent** (alpha `0x0` everywhere), so the
    /// map on the layer below shows through wherever the next `render_overlay` doesn't
    /// draw. Called once at the start of each overlay frame, before the bulge is drawn.
    /// (This is the transparent reset; the `DrawTarget` `clear` fills an *opaque* field
    /// of a colour instead.)
    pub fn clear_transparent(&mut self) {
        self.buf.fill(0x0000);
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

    /// Fill the whole plane with `color`. For RGB565 this is the map redraw's clear; for
    /// ARGB4444 it fills an *opaque* field of the colour (the overlay path never calls
    /// this — it uses [`clear_transparent`](RawFb::<PackOpaqueArgb4444>::clear_transparent)
    /// for the transparent reset — but it honours the trait).
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        // `fill` lowers to a burstable memset across the wait-stated FMC, far cheaper than a
        // per-pixel store on every map redraw.
        self.buf.fill(P::pack(color));
        Ok(())
    }
}

/// Pack an [`Rgb565`] colour into a **fully-opaque** ARGB4444 pixel: the alpha nibble
/// is forced to `0xF` and the top four bits of each RGB565 channel are kept. Every
/// pixel the overlay renderer writes is opaque; transparency is only ever the cleared
/// state ([`clear_transparent`](RawFb::<PackOpaqueArgb4444>::clear_transparent), alpha
/// `0x0`), so a single opaque-pack covers every draw. (`0x0000` is transparent; opaque
/// black is `0xF000` — the distinction the whole layer turns on.)
#[inline]
fn opaque_argb4444(c: Rgb565) -> u16 {
    let rgb = c.into_storage(); // RRRRR GGGGGG BBBBB
    let r = (rgb >> 12) & 0xF; // top 4 of the 5-bit red
    let g = (rgb >> 7) & 0xF; // top 4 of the 6-bit green
    let b = (rgb >> 1) & 0xF; // top 4 of the 5-bit blue
    0xF000 | (r << 8) | (g << 4) | b
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

    // --- ARGB4444 overlay plane ---

    fn ovl(buf: &mut [u16], w: u32, h: u32) -> FramebufferArgb4444<'_> {
        FramebufferArgb4444::new(buf, w, h)
    }

    #[test]
    fn opaque_pack_keeps_top_bits_and_forces_alpha() {
        // Pure channels keep their top nibble; alpha is always 0xF.
        assert_eq!(opaque_argb4444(Rgb565::from(RawU16::new(0xF800))), 0xFF00); // red
        assert_eq!(opaque_argb4444(Rgb565::from(RawU16::new(0x07E0))), 0xF0F0); // green
        assert_eq!(opaque_argb4444(Rgb565::from(RawU16::new(0x001F))), 0xF00F); // blue
        assert_eq!(opaque_argb4444(Rgb565::from(RawU16::new(0xFFFF))), 0xFFFF); // white
                                                                                // The load-bearing case: black packs to *opaque* black (0xF000), never the
                                                                                // transparent 0x0000 that clear_transparent paints.
        assert_eq!(opaque_argb4444(Rgb565::from(RawU16::new(0x0000))), 0xF000);
    }

    #[test]
    fn draw_writes_opaque_argb4444_and_clips() {
        let mut buf = [0xABCDu16; 2 * 2]; // garbage start state
        let mut fb = ovl(&mut buf, 2, 2);
        let red = Rgb565::from(RawU16::new(0xF800));
        fb.draw_iter([
            Pixel(Point::new(0, 1), red),
            Pixel(Point::new(5, 5), red),  // off-screen: dropped
            Pixel(Point::new(-1, 0), red), // off-screen: dropped
        ])
        .unwrap();
        assert_eq!(buf, [0xABCD, 0xABCD, 0xFF00, 0xABCD]); // only (0,1) written, opaque red
    }

    #[test]
    fn clear_transparent_zeros_alpha_everywhere() {
        let mut buf = [0xFF00u16; 4 * 3]; // opaque red
        ovl(&mut buf, 4, 3).clear_transparent();
        assert!(buf.iter().all(|&p| p == 0x0000), "every pixel fully transparent");
    }

    #[test]
    fn trait_clear_fills_opaque_field() {
        // The DrawTarget `clear` is the opaque sibling of clear_transparent.
        let mut buf = [0u16; 2 * 2];
        ovl(&mut buf, 2, 2).clear(Rgb565::from(RawU16::new(0x0000))).unwrap();
        assert!(buf.iter().all(|&p| p == 0xF000), "opaque black, not transparent");
    }

    #[test]
    fn overlay_fill_solid_fills_subrect_opaque_and_clips() {
        let mut buf = [0x0000u16; 4 * 4]; // transparent start
        {
            let mut fb = ovl(&mut buf, 4, 4);
            fb.fill_solid(
                &Rectangle::new(Point::new(2, 2), Size::new(10, 10)),
                Rgb565::from(RawU16::new(0x001F)), // blue
            )
            .unwrap();
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(1, 1), 0x0000); // untouched → still transparent
        assert_eq!(at(2, 2), 0xF00F); // opaque blue
        assert_eq!(at(3, 3), 0xF00F); // last in-bounds pixel
    }

    /// Item 10 on the overlay plane: a negative top-left must clip the same way (the
    /// intersection clamps to the bounding box) and pack the clipped fill opaque, never
    /// indexing the buffer with a negative coordinate.
    #[test]
    fn overlay_fill_solid_clips_a_negative_top_left() {
        let mut buf = [0x0000u16; 4 * 4]; // transparent start
        {
            let mut fb = ovl(&mut buf, 4, 4);
            fb.fill_solid(&Rectangle::new(Point::new(-2, -2), Size::new(4, 4)), Rgb565::from(RawU16::new(0x001F)))
                .unwrap();
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(0, 0), 0xF00F); // clipped origin, opaque blue
        assert_eq!(at(1, 1), 0xF00F); // last covered pixel
        assert_eq!(at(2, 2), 0x0000); // beyond the rect: still transparent
    }

    #[test]
    #[should_panic]
    fn overlay_too_small_buffer_panics() {
        let mut buf = [0u16; 3];
        let _ = FramebufferArgb4444::new(&mut buf, 2, 2); // needs 4
    }
}
