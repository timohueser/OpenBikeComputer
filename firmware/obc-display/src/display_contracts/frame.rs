//! [`NativeFrame`], the frame specification and storage contract, and [`Device64Frame`], the
//! shipping RGB222 implementation of it.
//!
//! A frame type answers, at compile time, everything the rest of the stack must never hard-code:
//! geometry, the device-native storage cell, the stride and backing-length invariant, and how
//! drawing code writes it, through a [`DrawTarget`] view straight into the backing with no
//! staging buffer and no format conversion. Anything beyond this contract, such as raw byte
//! access for a damage diff or a wire pack, is the pairing's private business.

use core::convert::Infallible;

use embedded_graphics::pixelcolor::{PixelColor, Rgb565};
use embedded_graphics::prelude::*;

use crate::framebuffer::FbDevice64;

/// A device-native frame: geometry, storage and a direct-write draw view.
///
/// Backing invariant: `backing().len() == STRIDE * HEIGHT`, validated at construction, so a paired
/// presenter may index rows without re-checking. `STRIDE` is in storage cells, not visual pixels,
/// and may exceed the cells `WIDTH` pixels need; the shipping frame packs one pixel per cell.
///
/// Quantization invariant: [`draw_target`](Self::draw_target) packs [`Color`](Self::Color) into
/// the native cell on store, exactly once, so the hot render loop's per-pixel cost is one packed
/// store.
///
/// Borrowing: drawing needs `&mut self` and a base present borrows the frame shared, which is
/// what statically keeps a render from racing a scan-out of the same bytes.
pub trait NativeFrame {
    /// The colour drawing code supplies: the renderer's colour space, not the storage format.
    /// Packing to the native cell happens on store.
    type Color: PixelColor;
    /// One storage cell of the backing: `u8` for the RGB222 device-64 plane, `u16` for a
    /// native-RGB565 plane. `PartialEq` so pairings and tests can compare backings.
    type Pixel: Copy + PartialEq;
    /// Frame width in visual pixels.
    const WIDTH: usize;
    /// Frame height in rows.
    const HEIGHT: usize;
    /// Storage cells per row, at least the cells `WIDTH` pixels occupy.
    const STRIDE: usize;

    /// The backing storage, row-major at [`STRIDE`](Self::STRIDE) cells per row.
    fn backing(&self) -> &[Self::Pixel];
    /// Mutable backing access, the escape hatch a board's composition edge may need when its
    /// transport scans the resident frame and composites into the backing. Generic render and UI
    /// code draws through [`draw_target`](Self::draw_target) instead.
    fn backing_mut(&mut self) -> &mut [Self::Pixel];
    /// A [`DrawTarget`] writing directly into the backing: the render path's view. Infallible,
    /// because out-of-frame writes clip rather than error.
    fn draw_target(&mut self) -> impl DrawTarget<Color = Self::Color, Error = Infallible> + '_;
}

/// The shipping frame: one resident RGB222 device-64 plane over a borrowed byte backing, `W × H`
/// cells of one byte per pixel, exactly the buffer the board keeps in `.bss`. The draw view is
/// [`FbDevice64`], so the quantize-on-store path is the one the renderer always used.
///
/// Borrowing the backing rather than owning an array is deliberate: the board's frame is a
/// placement-initialized static, and this type must wrap it without a copy.
pub struct Device64Frame<'b, const W: usize, const H: usize> {
    buf: &'b mut [u8],
}

impl<'b, const W: usize, const H: usize> Device64Frame<'b, W, H> {
    /// Wrap `buf` as the `W × H` device-64 frame. Panics unless `buf.len() == W * H`, the
    /// validated-backing invariant, checked once here so presenters never re-check.
    pub fn new(buf: &'b mut [u8]) -> Self {
        assert!(buf.len() == W * H, "device-64 backing must be exactly WIDTH * HEIGHT bytes");
        Self { buf }
    }

    /// The backing as raw bytes, `W` bytes per row: what a byte-plane pairing's damage diff and
    /// wire pack read. The same slice as [`NativeFrame::backing`].
    pub fn bytes(&self) -> &[u8] {
        self.buf
    }

    /// [`bytes`](Self::bytes), mutable, for the board composition edge's render entry. The same
    /// slice as [`NativeFrame::backing_mut`].
    pub fn bytes_mut(&mut self) -> &mut [u8] {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "exactly WIDTH * HEIGHT")]
    fn short_backing_panics() {
        let mut buf = [0u8; 3];
        let _ = Device64Frame::<2, 2>::new(&mut buf);
    }
}
