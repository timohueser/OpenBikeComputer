//! The **elevation band** — the filled silhouette under a connected amber top stroke that every
//! elevation page draws: the Statistics chart, the Route overview's profile page, and the Ride
//! detail's recorded twin.
//!
//! One raster serves all three. [`ElevationBand`] owns the profile window, the elevation-to-y
//! mapping, the column sampling, the fill, the connected top stroke, and the peak label. A screen
//! composes its page from them in this order:
//!
//! 1. Build the raster from a profile, a window, and the band's rectangle.
//! 2. Draw the base fill.
//! 3. Draw any local overlay fill (Statistics' traveled shading is its own — see
//!    [`fill_column`](ElevationBand::fill_column)).
//! 4. Draw the connected top stroke.
//! 5. Draw the peak label, where the page wants one.
//!
//! Live layers stay with the screen that owns them: Statistics keeps its cursor, its progress bar,
//! and its waypoint ticks; `climb.rs` keeps its grade-striped renderer, which colours every column
//! by local gradient over a climb-local span — a different mechanism, not a copy of this one.
//!
//! The received-route card's mini sparkline is a different *raster*, for a reason the device
//! enforces: it interpolates the 64-byte min-max-normalized band the host builds once at commit
//! time ([`obc_route::elevation_sparkline`]), because no `Profile` can exist on that path —
//! building one costs tens of KB of stack, more than the device has. Its columns are its own; its
//! top line is not. Both rasters stroke through [`TopStroke`], so the connected amber rule has one
//! definition and cannot drift between the card and the overview.

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};
use obc_route::{Profile, Window};

use crate::screen::palette;
use crate::settings::Units;

/// Side inset (px) the over-the-peak label clamps to, so a peak at either end keeps its whole
/// centred string inside the band.
const PEAK_LABEL_INSET: i32 = 30;

/// How far above the apex (px) the over-the-peak label sits — clear of the top stroke, clamped so
/// it never rides above the band's own top edge.
const PEAK_LABEL_LIFT: i32 = 22;

/// Where a band's peak-elevation label sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeakLabel {
    /// Centred over the apex, clamped inside the band's ends — the Route overview's profile page,
    /// where the label gives the vertical scale meaning at the point it describes.
    OverPeak,
    /// In the band's top-right corner — the Ride detail, whose band is short enough that a label
    /// over the apex would collide with the stroke.
    TopRight,
}

/// One elevation band's raster: a profile sampled through a window into a rectangle.
pub(crate) struct ElevationBand<'a> {
    profile: &'a Profile,
    /// Pyramid level the window resolves to.
    level: usize,
    /// Route fraction at the band's left edge, and the fraction span it covers.
    lo_frac: f32,
    span: f32,
    /// The band's rectangle: left edge, width in columns, top row, and **inclusive** bottom row
    /// (the baseline every column fills down to).
    x: i32,
    w: i32,
    top: i32,
    bot: i32,
    /// The y-axis: the profile's lowest elevation maps to `bot`, `span_ele` metres above it to
    /// `top`. Guarded to at least 1 m, so a perfectly flat route still maps.
    min_ele_m: i16,
    span_ele: f32,
}

impl<'a> ElevationBand<'a> {
    /// The raster for `profile` seen through `win`, drawn into `area` (its height counts the
    /// baseline row, so a band from `top` to an inclusive `bot` is `bot - top + 1` tall).
    pub(crate) fn new(profile: &'a Profile, win: Window, area: Rectangle) -> Self {
        ElevationBand {
            profile,
            level: win.level,
            lo_frac: win.lo_frac,
            span: (win.hi_frac - win.lo_frac).max(1e-6),
            x: area.top_left.x,
            w: area.size.width as i32,
            top: area.top_left.y,
            bot: area.top_left.y + area.size.height as i32 - 1,
            min_ele_m: profile.min_ele_m,
            span_ele: (profile.max_ele_m - profile.min_ele_m).max(1) as f32,
        }
    }

    /// The whole-route raster: the non-interactive band the two detail pages draw — no cursor, no
    /// zoom, the full profile in `area`.
    pub(crate) fn whole_route(profile: &'a Profile, area: Rectangle) -> Self {
        let win = profile.window(0.5, 1.0, area.size.width.max(1));
        Self::new(profile, win, area)
    }

    /// The route fraction chart column `px` samples.
    pub(crate) fn frac(&self, px: i32) -> f32 {
        self.lo_frac + self.span * (px as f32 / self.w as f32)
    }

    /// The panel x route fraction `f` falls on — the inverse of [`frac`](Self::frac), for the
    /// live layers a screen draws over the raster.
    pub(crate) fn frac_to_x(&self, f: f32) -> i32 {
        self.x + ((f - self.lo_frac) / self.span * self.w as f32) as i32
    }

    /// The panel y elevation `e` maps to: the profile's floor sits on the baseline, its peak on
    /// the band's top row, everything outside that range clamps to them.
    pub(crate) fn ele_to_y(&self, e: i16) -> i32 {
        let t = ((e - self.min_ele_m) as f32 / self.span_ele).clamp(0.0, 1.0);
        self.bot - (t * (self.bot - self.top) as f32) as i32
    }

    /// The silhouette's top row at chart column `px`.
    fn column_top(&self, px: i32) -> i32 {
        self.ele_to_y(self.profile.sample(self.level, self.frac(px)).1)
    }

    /// Paint one chart column of silhouette, baseline up to the profile there. A screen shades
    /// part of the band by re-filling those columns over the base fill — Statistics' traveled
    /// half, which is its layer, not the raster's.
    pub(crate) fn fill_column(&self, cv: &mut impl Surface, px: i32, color: u16) {
        let top_y = self.column_top(px);
        cv.vline(self.x + px, top_y, self.bot - top_y + 1, 1, color);
    }

    /// Paint the whole silhouette in one colour.
    pub(crate) fn fill(&self, cv: &mut impl Surface, color: u16) {
        for px in 0..self.w {
            self.fill_column(cv, px, color);
        }
    }

    /// Stroke the silhouette's top line, column by column, through the shared [`TopStroke`] rule.
    pub(crate) fn stroke(&self, cv: &mut impl Surface, color: u16) {
        let mut stroke = TopStroke::default();
        for px in 0..self.w {
            stroke.column(cv, self.x + px, self.column_top(px), color);
        }
    }

    /// Draw the profile's peak elevation as a small label at `place`, in the rider's units. Both
    /// placements stay inside the band's ends.
    pub(crate) fn peak_label(&self, cv: &mut impl Surface, units: Units, place: PeakLabel) {
        let mut peak: heapless::String<10> = heapless::String::new();
        let _ = write!(peak, "{} {}", units.elev(self.profile.peak_ele_m() as f32) as i32, units.elev_label());
        let (at, align) = match place {
            PeakLabel::OverPeak => {
                let px = (self.x + (self.profile.peak_frac() * self.w as f32) as i32)
                    .clamp(self.x + PEAK_LABEL_INSET, self.x + self.w - PEAK_LABEL_INSET);
                let py = (self.ele_to_y(self.profile.peak_ele_m()) - PEAK_LABEL_LIFT).max(self.top - 2);
                (Point::new(px, py), TextAlign::Center)
            }
            PeakLabel::TopRight => (Point::new(self.x + self.w - 2, self.top - 2), TextAlign::Right),
        };
        cv.text(&peak, at, Font::Label, align, palette::SUBTEXT);
    }
}

/// The connected top stroke, one column at a time — the rule an elevation band's top line obeys
/// whatever produced its columns. Each column's span reaches back to the previous column's top, so
/// a steep section stays solid instead of stair-stepping into gaps; on a flat run it is the 2 px
/// cap. Stateful because "the previous column" is the whole rule.
///
/// Separate from [`ElevationBand`] because the received-route card strokes the same line over a
/// raster the band cannot produce: different columns, one top line.
#[derive(Default)]
pub(crate) struct TopStroke {
    prev_top: Option<i32>,
}

impl TopStroke {
    /// Stroke panel column `x`, whose silhouette top row is `top_y`.
    pub(crate) fn column(&mut self, cv: &mut impl Surface, x: i32, top_y: i32, color: u16) {
        let (y0, y1) = self.prev_top.map_or((top_y, top_y), |p| (p.min(top_y), p.max(top_y)));
        cv.vline(x, y0 - 1, (y1 - y0) + 2, 1, color);
        self.prev_top = Some(top_y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_render::rect;
    use std::vec::Vec;

    // The band under test: the Route overview's slot, 216 columns from x = 12.
    const X: i32 = 12;
    const W: i32 = 216;
    const TOP: i32 = 50;
    const BOT: i32 = 140;

    fn area() -> Rectangle {
        rect(X, TOP, W, BOT - TOP + 1)
    }

    /// Build a route profile whose points climb through `eles` (metres) along a straight eastward
    /// line — the same GPX → OBCR → `Profile` path the app itself takes.
    fn profile(eles: &[i32]) -> Profile {
        use obc_formats::io::{ByteSink, Error, SliceSource};
        #[derive(Default)]
        struct VecSink(Vec<u8>);
        impl ByteSink for VecSink {
            fn write(&mut self, b: &[u8]) -> Result<(), Error> {
                self.0.extend_from_slice(b);
                Ok(())
            }
            fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
                let o = off as usize;
                self.0[o..o + b.len()].copy_from_slice(b);
                Ok(())
            }
        }
        let mut gpx = std::string::String::from("<gpx><trk><trkseg>");
        for (i, e) in eles.iter().enumerate() {
            let _ = write!(gpx, "<trkpt lat=\"47.0000\" lon=\"8.{:04}\"><ele>{e}</ele></trkpt>", i * 200);
        }
        gpx.push_str("</trkseg></trk></gpx>");
        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Band", &mut sink).unwrap();
        let src = SliceSource(&sink.0);
        let idx = obc_route::RouteIndex::read(&src).unwrap();
        obc_route::RouteReader::new(&idx, &src).elevation_profile()
    }

    /// A [`Surface`] that records every filled rectangle and every text anchor.
    #[derive(Default)]
    struct Probe {
        fills: Vec<(i32, i32, i32, i32, u16)>,
        texts: Vec<(std::string::String, Point, TextAlign)>,
    }

    impl Surface for Probe {
        fn clear(&mut self, _color: u16) {}
        fn fill(&mut self, area: Rectangle, color: u16) {
            let (w, h) = (area.size.width as i32, area.size.height as i32);
            self.fills.push((area.top_left.x, area.top_left.y, w, h, color));
        }
        fn round(&mut self, area: Rectangle, _radius: u32, color: u16) {
            self.fill(area, color);
        }
        fn round_outline(&mut self, area: Rectangle, _radius: u32, color: u16) {
            self.fill(area, color);
        }
        fn line(&mut self, _a: Point, _b: Point, _color: u16) {}
        fn triangle(&mut self, _a: Point, _b: Point, _c: Point, _color: u16) {}
        fn disc(&mut self, _center: Point, _radius: u32, _color: u16) {}
        fn text(&mut self, s: &str, at: Point, _f: Font, align: TextAlign, _color: u16) -> Point {
            self.texts.push((s.into(), at, align));
            at
        }
    }

    /// The y mapping puts the route's floor on the baseline and its peak on the band's top row,
    /// whichever way the route runs — and a flat route, where the elevation span is zero, still
    /// maps every column onto the baseline instead of dividing by it.
    #[test]
    fn mapping_covers_rising_falling_and_flat_profiles() {
        for eles in [&[500, 600, 700, 800][..], &[800, 700, 600, 500][..]] {
            let p = profile(eles);
            let b = ElevationBand::whole_route(&p, area());
            assert_eq!(b.ele_to_y(p.min_ele_m), BOT, "the floor sits on the baseline ({eles:?})");
            assert_eq!(b.ele_to_y(p.max_ele_m), TOP, "the peak sits on the band's top row ({eles:?})");
            let mid = b.ele_to_y((p.min_ele_m + p.max_ele_m) / 2);
            assert!((mid - (TOP + BOT) / 2).abs() <= 1, "the midpoint maps to the band's middle ({eles:?})");
            // Out-of-range elevations clamp rather than escaping the band.
            assert_eq!(b.ele_to_y(p.min_ele_m - 500), BOT);
            assert_eq!(b.ele_to_y(p.max_ele_m + 500), TOP);
        }

        let flat = profile(&[600, 600, 600, 600]);
        let b = ElevationBand::whole_route(&flat, area());
        assert_eq!(b.ele_to_y(600), BOT, "a zero-span profile reads as a flat band on the baseline");
        for px in [0, W / 2, W - 1] {
            assert_eq!(b.column_top(px), BOT, "every column of a flat route is the baseline");
        }
    }

    /// A degenerate band — no columns, or a single row of height — draws nothing out of bounds and
    /// does not divide by its own zero extent.
    #[test]
    fn degenerate_bands_stay_inside_themselves() {
        let p = profile(&[500, 900, 500]);

        let mut probe = Probe::default();
        let empty = ElevationBand::new(&p, p.window(0.5, 1.0, 1), rect(X, TOP, 0, BOT - TOP + 1));
        empty.fill(&mut probe, 1);
        empty.stroke(&mut probe, 2);
        assert!(probe.fills.is_empty(), "a zero-width band has no columns to paint");

        // A one-row band: floor and peak both land on that row, so every column is a single pixel.
        let flatten = ElevationBand::new(&p, p.window(0.5, 1.0, W as u32), rect(X, TOP, W, 1));
        assert_eq!(flatten.ele_to_y(p.min_ele_m), TOP);
        assert_eq!(flatten.ele_to_y(p.max_ele_m), TOP);
        let mut probe = Probe::default();
        flatten.fill(&mut probe, 1);
        assert_eq!(probe.fills.len(), W as usize);
        assert!(probe.fills.iter().all(|f| f.1 == TOP && f.3 == 1), "every column is the band's one row");
    }

    /// The top stroke is **connected**: over a step steep enough to jump many rows in one column,
    /// each column's stroke still spans from its neighbour's top to its own, so the line has no
    /// gaps to fall through.
    #[test]
    fn stroke_bridges_a_steep_step() {
        // A short flat run, a cliff, then a flat run: adjacent columns differ by tens of rows.
        let p = profile(&[400, 400, 400, 1400, 1400, 1400]);
        let b = ElevationBand::whole_route(&p, area());
        let mut probe = Probe::default();
        b.stroke(&mut probe, 7);
        assert_eq!(probe.fills.len(), W as usize, "one stroke span per column");

        let mut steepest = 0;
        for px in 1..W {
            let (prev, cur) = (b.column_top(px - 1), b.column_top(px));
            steepest = steepest.max((cur - prev).abs());
            let (_, y, _, h, _) = probe.fills[px as usize];
            let (lo, hi) = (y, y + h - 1);
            assert!(
                lo <= prev.min(cur) && hi >= prev.max(cur),
                "column {px} leaves a gap: {lo}..{hi} misses {prev}/{cur}"
            );
        }
        assert!(steepest > 10, "the fixture must actually be steep (largest step {steepest} px)");
    }

    /// Both peak placements stay inside the band: over the apex the label is clamped away from the
    /// ends and never rides above the band's top edge; the corner placement anchors at the band's
    /// top-right, right-aligned.
    #[test]
    fn peak_labels_stay_within_the_band() {
        // A peak in the very first column is where the clamp has to work.
        for eles in [&[1400, 900, 400, 300][..], &[400, 900, 1400, 900][..]] {
            let p = profile(eles);
            let b = ElevationBand::whole_route(&p, area());

            let mut probe = Probe::default();
            b.peak_label(&mut probe, Units::Metric, PeakLabel::OverPeak);
            let (text, at, align) = probe.texts.pop().expect("the label draws");
            assert_eq!(text, "1400 m", "the peak reads in the rider's units");
            assert_eq!(align, TextAlign::Center);
            assert!(
                (X + PEAK_LABEL_INSET..=X + W - PEAK_LABEL_INSET).contains(&at.x),
                "the label is clamped inside the band ({eles:?}: x = {})",
                at.x
            );
            assert!(at.y >= TOP - 2 && at.y < BOT, "the label sits in the band ({eles:?}: y = {})", at.y);

            let mut probe = Probe::default();
            b.peak_label(&mut probe, Units::Imperial, PeakLabel::TopRight);
            let (text, at, align) = probe.texts.pop().expect("the label draws");
            assert_eq!(text, "4593 ft", "imperial reads in feet");
            assert_eq!((at, align), (Point::new(X + W - 2, TOP - 2), TextAlign::Right));
        }
    }
}
