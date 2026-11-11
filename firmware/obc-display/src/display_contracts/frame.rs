//! [`Device64Frame`], the resident device-64 frame every presenter puts on glass.

use crate::framebuffer::FbDevice64;

/// The resident RGB222 device-64 plane over a borrowed byte backing, `W × H` cells of one byte per
/// pixel, exactly the buffer the board keeps in `.bss`.
///
/// Borrowing the backing rather than owning an array is deliberate: the board's frame is a
/// placement-initialized static, and this type must wrap it without a copy.
pub struct Device64Frame<'b, const W: usize, const H: usize> {
    buf: &'b mut [u8],
}

impl<'b, const W: usize, const H: usize> Device64Frame<'b, W, H> {
    /// Wrap `buf` as the `W × H` device-64 frame. Panics unless `buf.len() == W * H`, checked once
    /// here so presenters index rows without re-checking.
    pub fn new(buf: &'b mut [u8]) -> Self {
        assert!(buf.len() == W * H, "device-64 backing must be exactly WIDTH * HEIGHT bytes");
        Self { buf }
    }

    /// The backing as raw bytes, `W` bytes per row: what the row diff and the wire pack read.
    pub fn bytes(&self) -> &[u8] {
        self.buf
    }

    /// [`bytes`](Self::bytes), mutable, for the overlay composite that writes into the frame.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        self.buf
    }

    /// The render view: an [`FbDevice64`] that quantizes each pixel to device-64 on store, straight
    /// into the backing.
    pub fn draw_target(&mut self) -> FbDevice64<'_> {
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
