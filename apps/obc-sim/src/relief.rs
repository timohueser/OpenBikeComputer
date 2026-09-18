//! Opt-in simulator-only relief fill. The prototype map reserves one RGB565 value per shade
//! density. Each must be a colour nothing else draws, after the panel quantises to 64.
use embedded_graphics::{prelude::*, primitives::Rectangle};

/// Reserved fills, lightest first: the marker, and the tile offsets its level stamps. Repeating
/// pattern F at another offset steps the density without a second table, and every dot stays one
/// pixel wide and irregularly placed. Dot counts per 1,024: 64, 126, 224.
const LEVELS: [(u16, &[(i32, i32)]); 3] =
    [(0xFABF, &[(0, 0)]), (0xFAB5, &[(0, 0), (16, 16)]), (0xA815, &[(0, 0), (16, 16), (8, 24), (24, 8)])];

pub struct Target<'a, D: DrawTarget> {
    inner: &'a mut D,
    enabled: bool,
    markers: [D::Color; LEVELS.len()],
    light: D::Color,
    dark: D::Color,
    pattern: Pattern,
}
impl<'a, D: DrawTarget> Target<'a, D> {
    pub fn new(inner: &'a mut D, color: impl Fn(u16) -> D::Color, viewport: obc_render::Viewport) -> Self {
        Self {
            inner,
            enabled: std::env::var_os("OBC_SIM_RELIEF").is_some(),
            markers: LEVELS.map(|(c, _)| color(c)),
            light: color(0xAD55),
            dark: color(0x52AA),
            pattern: Pattern::new(viewport),
        }
    }
}
impl<D: DrawTarget> Dimensions for Target<'_, D> {
    fn bounding_box(&self) -> Rectangle {
        self.inner.bounding_box()
    }
}
impl<D: DrawTarget> DrawTarget for Target<'_, D> {
    type Color = D::Color;
    type Error = D::Error;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        self.inner.draw_iter(pixels)
    }
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.inner.clear(color)
    }
    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        self.inner.fill_contiguous(area, colors)
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let level = self.enabled.then(|| self.markers.iter().position(|m| *m == color)).flatten();
        let Some(offsets) = level.map(|i| LEVELS[i].1) else {
            return self.inner.fill_solid(area, color);
        };
        let clipped = area.intersection(&self.bounding_box());
        let (light, dark) = (self.light, self.dark);
        let pattern = self.pattern;
        // Every polygon uses the same map anchor and orientation.
        self.inner.draw_iter(clipped.points().map(|p| {
            let dot = pattern.dot(p, offsets);
            Pixel(p, if dot { dark } else { light })
        }))
    }
}

/// Keep one-pixel dots, but anchor their phase to a fixed map coordinate.
/// Inverse camera rotation makes the texture turn with heading-up views too.
#[derive(Clone, Copy)]
struct Pattern {
    origin: [f64; 2],
    cos: f64,
    sin: f64,
    north_phase: [i32; 2],
}
impl Pattern {
    fn new(vp: obc_render::Viewport) -> Self {
        let (sin, cos) = (vp.course_rad as f64).sin_cos();
        let x = (vp.cam_lon as f64 - 8_425_000.0) * vp.aspect as f64 * vp.zoom as f64;
        let y = (46_800_000.0 - vp.cam_lat as f64) * vp.zoom as f64;
        let origin = [
            (x - cos * vp.w as f64 / 2.0 + sin * vp.h as f64 / 2.0).rem_euclid(32.0),
            (y - sin * vp.w as f64 / 2.0 - cos * vp.h as f64 / 2.0).rem_euclid(32.0),
        ];
        Self { origin, cos, sin, north_phase: [(origin[0] + 0.5).floor() as i32, (origin[1] + 0.5).floor() as i32] }
    }
    /// The tile cell a screen pixel lands in, in map-anchored tile coordinates.
    fn cell(self, p: Point) -> (i32, i32) {
        if self.sin == 0.0 {
            return (p.x + self.north_phase[0], p.y + self.north_phase[1]);
        }
        let x = p.x as f64 + 0.5;
        let y = p.y as f64 + 0.5;
        (
            (self.origin[0] + self.cos * x - self.sin * y).floor() as i32,
            (self.origin[1] + self.sin * x + self.cos * y).floor() as i32,
        )
    }
    fn dot(self, p: Point, offsets: &[(i32, i32)]) -> bool {
        let (u, v) = self.cell(p);
        offsets.iter().any(|(du, dv)| set(u + du, v + dv))
    }
}
fn set(u: i32, v: i32) -> bool {
    ROWS[(v & 31) as usize] & (1 << (u & 31)) != 0
}

const ROWS: [u32; 32] = [
    0x00400020, 0x00010200, 0x00081001, 0x08000000, 0x00800040, 0x00000402, 0x00080000, 0x20000000, 0x02008108,
    0x00000000, 0x00200000, 0x08040220, 0x40009000, 0x01000004, 0x00000040, 0x00220000, 0x09001000, 0x00000204,
    0x00040040, 0x42400000, 0x00000002, 0x00009210, 0x04080000, 0x00000000, 0x40408000, 0x00001008, 0x00000080,
    0x02110000, 0x00000002, 0x10000820, 0x00020100, 0x80000000,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::Framebuffer;
    use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
    use obc_host_core::frame::device_rgb888;

    const PLAIN: &[(i32, i32)] = LEVELS[0].1;

    #[test]
    fn texture_follows_camera_pan_and_quarter_turn() {
        let mut vp = obc_render::Viewport::new(240.0, 320.0, 8_425_000, 46_800_000, 1.0);
        vp.zoom = 0.5 / vp.aspect;
        let original = Pattern::new(vp);
        let mut panned = vp;
        panned.cam_lon += 32; // Ground moves 16 pixels left.
        let moved = Pattern::new(panned);
        for y in 0..320 {
            for x in 16..240 {
                assert_eq!(original.dot(Point::new(x, y), PLAIN), moved.dot(Point::new(x - 16, y), PLAIN));
            }
        }
        let mut rotated = vp;
        rotated.course_rad = core::f32::consts::FRAC_PI_2;
        let turned = Pattern::new(rotated);
        for y in 80..240 {
            for x in 40..200 {
                assert_eq!(original.dot(Point::new(x, y), PLAIN), turned.dot(Point::new(y - 40, 279 - x), PLAIN));
            }
        }
    }
    #[test]
    fn clipped_split_fills_match_headless_and_device_colour_paths() {
        let mut rgb = Framebuffer::new(32, 32);
        let mut target =
            Target::new(&mut rgb, device_rgb888, obc_render::Viewport::new(32.0, 32.0, 8_425_000, 46_800_000, 1.0));
        target.enabled = true;
        target.fill_solid(&Rectangle::new(Point::new(-3, -4), Size::new(40, 40)), device_rgb888(0xFABF)).unwrap();
        let mut bytes = [0u8; 1024];
        let mut device = obc_display::FbDevice64::new(&mut bytes, 32, 32);
        let mut target = Target::new(
            &mut device,
            |c| Rgb565::from(RawU16::new(c)),
            obc_render::Viewport::new(32.0, 32.0, 8_425_000, 46_800_000, 1.0),
        );
        target.enabled = true;
        for (x, width) in [(0, 13), (13, 19)] {
            target
                .fill_solid(&Rectangle::new(Point::new(x, 0), Size::new(width, 32)), Rgb565::from(RawU16::new(0xFABF)))
                .unwrap();
        }
        assert_eq!(bytes.iter().filter(|&&v| v == 0b01_01_01).count(), 64);
        for (rgb, &device) in rgb.as_rgb888().chunks_exact(3).zip(&bytes) {
            assert_eq!(rgb, if device == 0b01_01_01 { &[85; 3] } else { &[170; 3] });
        }
    }
    /// A reserved marker is only safe while nothing else draws that colour. The panel shows 64
    /// colours, so a marker can collide with a palette entry it differs from in RGB565: `0xFA9F`
    /// collapsed onto the shade marker, and `0xF81F` *is* the magenta route line.
    #[test]
    fn the_markers_are_colours_the_device_never_draws() {
        use obc_app::screen::palette;
        let device = obc_reader::rgb565_to_device64;
        let used = [
            ("PARCHMENT", palette::PARCHMENT),
            ("PARCHMENT_SHADE", palette::PARCHMENT_SHADE),
            ("HUD", palette::HUD),
            ("WOOD", palette::WOOD),
            ("WOOD_LIGHT", palette::WOOD_LIGHT),
            ("INK", palette::INK),
            ("SUBTEXT", palette::SUBTEXT),
            ("RULE", palette::RULE),
            ("AMBER", palette::AMBER),
            ("WARNING", palette::WARNING),
            ("ON", palette::ON),
            ("YELLOW", palette::YELLOW),
            ("RED", palette::RED),
            ("CLIMB_TILE", palette::CLIMB_TILE),
            ("ROUTE", palette::ROUTE),
            ("BREADCRUMB", palette::BREADCRUMB),
            ("CONTOUR", palette::CONTOUR),
        ];
        for (marker, _) in LEVELS {
            for (name, other) in used {
                assert_ne!(device(marker), device(other), "marker {marker:#06X} collides with {name}");
            }
            for (second, _) in LEVELS {
                assert!(marker == second || device(marker) != device(second), "two markers quantise alike");
            }
        }
    }
    #[test]
    fn each_density_level_adds_dots_without_moving_the_ones_below() {
        let mut rgb = Framebuffer::new(32, 32);
        let target =
            Target::new(&mut rgb, device_rgb888, obc_render::Viewport::new(32.0, 32.0, 8_425_000, 46_800_000, 1.0));
        let pattern = target.pattern;
        let counts: Vec<usize> = LEVELS
            .iter()
            .map(|(_, offsets)| {
                (0..32)
                    .flat_map(|y| (0..32).map(move |x| Point::new(x, y)))
                    .filter(|&p| {
                        // A denser level never clears a dot a lighter one set.
                        assert!(pattern.dot(p, offsets) || !pattern.dot(p, LEVELS[0].1));
                        pattern.dot(p, offsets)
                    })
                    .count()
            })
            .collect();
        assert_eq!(counts, vec![64, 126, 224], "6.25%, 12.3% and 21.9% of the tile");
    }
}
