//! Shared helpers for the `obc-render` integration tests.
//!
//! Two recording targets for different pixel stores: [`Buf`] keeps the full `Rgb888` colour (so
//! colour-counting tests can tell features apart), [`BitBuf`] a single coverage bit per pixel (the
//! stroke tests only ask "was this pixel painted?"). `#[allow(dead_code)]` covers accessors unused
//! in some binaries.

#![allow(dead_code)]

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_route::{ByteSink, Error};

/// A `w`×`h` `Rgb888` buffer implementing `DrawTarget`, with clipped writes. Records
/// the full colour so tests can count and locate distinctly-coloured features.
pub struct Buf {
    pub w: i32,
    pub h: i32,
    pub px: Vec<Rgb888>,
}

impl Buf {
    pub fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    pub fn get(&self, x: i32, y: i32) -> Rgb888 {
        self.px[(y * self.w + x) as usize]
    }
    pub fn count(&self, c: Rgb888) -> usize {
        self.px.iter().filter(|&&p| p == c).count()
    }
    pub fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
    /// Inclusive `(min_x, min_y, max_x, max_y)` bounding box of pixels of color `c`, or
    /// `None` if the color is absent.
    pub fn bbox(&self, c: Rgb888) -> Option<(i32, i32, i32, i32)> {
        let (mut minx, mut miny, mut maxx, mut maxy) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.get(x, y) == c {
                    minx = minx.min(x);
                    miny = miny.min(y);
                    maxx = maxx.max(x);
                    maxy = maxy.max(y);
                }
            }
        }
        (maxx >= minx).then_some((minx, miny, maxx, maxy))
    }
}

impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}

impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }
}

/// A `w`×`h` 1-bit coverage buffer implementing `DrawTarget`: it records only whether
/// each pixel was painted (any colour), which is all the stroke tests probe.
pub struct BitBuf {
    pub w: i32,
    pub h: i32,
    pub px: Vec<bool>,
}

impl BitBuf {
    pub fn new(w: i32, h: i32) -> Self {
        BitBuf { w, h, px: vec![false; (w * h) as usize] }
    }
    pub fn put(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = true;
        }
    }
    pub fn on(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h && self.px[(y * self.w + x) as usize]
    }
}

impl OriginDimensions for BitBuf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}

impl DrawTarget for BitBuf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, _) in pixels {
            self.put(p.x, p.y);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, _c: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y);
                }
            }
        }
        Ok(())
    }
}

/// A `ByteSink` over a growable `Vec` — the host's "whole file to RAM" backing (the
/// device uses a FatFs-backed sink instead).
#[derive(Default)]
pub struct VecSink {
    pub buf: Vec<u8>,
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.buf[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}
