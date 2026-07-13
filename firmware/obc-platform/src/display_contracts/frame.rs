//! [`NativeFrame`] — the frame specification/storage contract — and [`Device64Frame`], the shipping
//! RGB222 implementation of it.
//!
//! A frame type answers, at compile time, everything the rest of the stack must never hard-code:
//! geometry, the device-native storage cell, the stride/backing-length invariant, and how drawing
//! code writes it (a [`DrawTarget`] view straight into the backing — no staging buffer, no format
//! conversion). Presenters are *paired* with a frame type; anything beyond this contract (raw byte
//! access for a row-hash diff, a wire pack) is the pairing's private business on the concrete type,
//! not a generic requirement.

use core::convert::Infallible;

use embedded_graphics::pixelcolor::{PixelColor, Rgb565};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use crate::framebuffer::FbDevice64;

/// A device-native frame: geometry + storage + a direct-write draw view.
///
/// **Backing invariant**: `backing().len() == BACKING_CELLS` (`STRIDE * HEIGHT` storage cells),
/// validated at construction — a presenter paired with this frame may index rows as
/// `y * STRIDE .. y * STRIDE + …` without re-checking. `STRIDE` is in **storage cells**, not
/// visual pixels, and may exceed the cells `WIDTH` pixels need (a padded backing); the shipping
/// frame packs one pixel per cell with `STRIDE == WIDTH`.
///
/// **Quantization invariant**: [`draw_target`](Self::draw_target) packs [`Color`](Self::Color)
/// into the native cell **on store, exactly once** — there is no readback-convert-rewrite anywhere
/// in the contract, so the hot render loop's per-pixel cost is one packed store.
///
/// **Borrowing**: drawing needs `&mut self`; a base present borrows the frame shared. Holding the
/// present's `&self` across its `await` is what statically keeps a render from racing a scan-out
/// of the same bytes — see the [module docs](super).
pub trait NativeFrame {
    /// The colour drawing code supplies — the renderer's colour space (RGB565 on the shipping
    /// path), *not* the storage format. Packing to the native cell happens on store.
    type Color: PixelColor;
    /// One storage cell of the backing — `u8` for the RGB222 device-64 plane, `u16` for a
    /// native-RGB565 plane. `PartialEq` so pairings and tests can compare backings without
    /// knowing the format.
    type Pixel: Copy + PartialEq;
    /// Frame width in visual pixels.
    const WIDTH: usize;
    /// Frame height in rows.
    const HEIGHT: usize;
    /// Storage cells per row (≥ the cells `WIDTH` pixels occupy; the shipping frame's is `WIDTH`).
    const STRIDE: usize;
    /// The validated backing length, in cells.
    const BACKING_CELLS: usize = Self::STRIDE * Self::HEIGHT;

    /// The backing storage, row-major at [`STRIDE`](Self::STRIDE) cells per row.
    fn backing(&self) -> &[Self::Pixel];
    /// Mutable backing access — the escape hatch a *board's* composition edge may need (the FLPR
    /// backend transiently composites into the resident bytes); generic render/UI code draws
    /// through [`draw_target`](Self::draw_target) instead.
    fn backing_mut(&mut self) -> &mut [Self::Pixel];
    /// A [`DrawTarget`] writing **directly into the backing** — the render path's view. Infallible:
    /// out-of-frame writes clip, they don't error.
    fn draw_target(&mut self) -> impl DrawTarget<Color = Self::Color, Error = Infallible> + '_;
    /// [`draw_target`](Self::draw_target) restricted to `area`: writes outside it are discarded, so
    /// a region-scoped repaint replays a full draw sequence and pays only for the region — **no
    /// second frame** is ever allocated for the view.
    fn clipped(&mut self, area: Rectangle) -> impl DrawTarget<Color = Self::Color, Error = Infallible> + '_;
}

/// The shipping frame: one resident **RGB222 / device-64** plane over a borrowed byte backing —
/// `W × H` cells of one byte per pixel (`0b00_RR_GG_BB`), exactly the buffer the board keeps in
/// `.bss` and the simulator in its backend. The draw view is the existing [`FbDevice64`], so the
/// quantize-on-store path (and its codegen) is byte-for-byte the one the renderer always used.
///
/// Borrowing the backing (rather than owning an array) is deliberate: the board's frame is a
/// placement-initialized static at the composition edge, and this type must wrap it without a copy
/// or a second allocation.
pub struct Device64Frame<'b, const W: usize, const H: usize> {
    buf: &'b mut [u8],
}

impl<'b, const W: usize, const H: usize> Device64Frame<'b, W, H> {
    /// Wrap `buf` as the `W × H` device-64 frame. Panics unless `buf.len() == W * H` — the
    /// contract's validated-backing invariant, checked once here so presenters never re-check.
    pub fn new(buf: &'b mut [u8]) -> Self {
        assert!(buf.len() == W * H, "device-64 backing must be exactly WIDTH * HEIGHT bytes");
        Self { buf }
    }

    /// The backing as raw bytes — what a row-diff/wire-pack pairing reads (`W` bytes per row).
    /// Same slice as [`NativeFrame::backing`]; named for call sites that want bytes, not cells.
    pub fn bytes(&self) -> &[u8] {
        self.buf
    }
}

impl<const W: usize, const H: usize> NativeFrame for Device64Frame<'_, W, H> {
    type Color = Rgb565;
    type Pixel = u8;
    const WIDTH: usize = W;
    const HEIGHT: usize = H;
    const STRIDE: usize = W;

    fn backing(&self) -> &[u8] {
        self.buf
    }

    fn backing_mut(&mut self) -> &mut [u8] {
        self.buf
    }

    fn draw_target(&mut self) -> impl DrawTarget<Color = Rgb565, Error = Infallible> + '_ {
        FbDevice64::new(self.buf, W as u32, H as u32)
    }

    fn clipped(&mut self, area: Rectangle) -> impl DrawTarget<Color = Rgb565, Error = Infallible> + '_ {
        let mut fb = FbDevice64::new(self.buf, W as u32, H as u32);
        fb.set_clip(area);
        fb
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::raw::RawU16;

    use super::*;

    fn rgb(raw: u16) -> Rgb565 {
        Rgb565::from(RawU16::new(raw))
    }

    /// The frame's draw view is byte-for-byte the existing `FbDevice64` path — same quantization,
    /// same writes — so wrapping the resident plane in the contract changes no pixel.
    #[test]
    fn draw_target_matches_fbdevice64_exactly() {
        let mut via_contract = [0u8; 4 * 4];
        let mut direct = [0u8; 4 * 4];
        {
            let mut frame = Device64Frame::<4, 4>::new(&mut via_contract);
            let mut t = frame.draw_target();
            t.clear(rgb(0xFFFF)).unwrap();
            t.fill_solid(&Rectangle::new(Point::new(1, 1), Size::new(2, 2)), rgb(0xF800)).unwrap();
            t.draw_iter([Pixel(Point::new(0, 3), rgb(0x001F)), Pixel(Point::new(9, 9), rgb(0x001F))]).unwrap();
        }
        {
            let mut fb = FbDevice64::new(&mut direct, 4, 4);
            fb.clear(rgb(0xFFFF)).unwrap();
            fb.fill_solid(&Rectangle::new(Point::new(1, 1), Size::new(2, 2)), rgb(0xF800)).unwrap();
            fb.draw_iter([Pixel(Point::new(0, 3), rgb(0x001F)), Pixel(Point::new(9, 9), rgb(0x001F))]).unwrap();
        }
        assert_eq!(via_contract, direct, "the contract view must be the same store path");
    }

    /// The clip view discards writes outside `area` without allocating anything: the backing is the
    /// one buffer throughout.
    #[test]
    fn clipped_view_discards_outside_writes() {
        let mut buf = [0u8; 4 * 4];
        {
            let mut frame = Device64Frame::<4, 4>::new(&mut buf);
            let mut t = frame.clipped(Rectangle::new(Point::new(2, 2), Size::new(2, 2)));
            t.clear(rgb(0xFFFF)).unwrap(); // clipped clear: only the 2×2 window whitens
        }
        let at = |x: usize, y: usize| buf[y * 4 + x];
        assert_eq!(at(0, 0), 0x00, "outside the clip: untouched");
        assert_eq!(at(2, 2), 0x3F, "inside the clip: cleared white");
        assert_eq!(at(3, 3), 0x3F);
    }

    #[test]
    fn geometry_and_backing_len_are_the_contract() {
        let mut buf = [0u8; 6 * 5];
        let frame = Device64Frame::<6, 5>::new(&mut buf);
        assert_eq!(<Device64Frame<6, 5> as NativeFrame>::WIDTH, 6);
        assert_eq!(<Device64Frame<6, 5> as NativeFrame>::HEIGHT, 5);
        assert_eq!(<Device64Frame<6, 5> as NativeFrame>::STRIDE, 6);
        assert_eq!(<Device64Frame<6, 5> as NativeFrame>::BACKING_CELLS, 30);
        assert_eq!(frame.backing().len(), 30);
    }

    #[test]
    #[should_panic(expected = "exactly WIDTH * HEIGHT")]
    fn short_backing_panics() {
        let mut buf = [0u8; 3];
        let _ = Device64Frame::<2, 2>::new(&mut buf);
    }
}
