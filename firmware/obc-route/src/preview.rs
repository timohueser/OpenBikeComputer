//! Route shape previews: the polyline decimated to a fixed number of points, the first and last
//! always kept. Call them once per plan, never per frame.

use heapless::Vec;
use obc_formats::io::Error;

use crate::reader::{RoutePoint, RouteReader};

/// A uniform pick of `keep` of `count` items by ordinal: the j-th kept item is item
/// `j * (count - 1) / (keep - 1)`, so the endpoints are exact and the rest an even stride.
pub struct Pick {
    count: usize,
    keep: usize,
    seen: usize,
    kept: usize,
    /// The ordinal of the next kept item.
    at: usize,
}

impl Pick {
    pub fn new(count: usize, limit: usize) -> Self {
        Pick { count, keep: limit.min(count), seen: 0, kept: 0, at: 0 }
    }

    /// Whether the next item is kept.
    pub fn keep_next(&mut self) -> bool {
        let keep = self.kept < self.keep && self.seen == self.at;
        self.seen += 1;
        if keep {
            self.kept += 1;
            self.at = if self.keep > 1 { self.kept * (self.count - 1) / (self.keep - 1) } else { 0 };
        }
        keep
    }

    fn done(&self) -> bool {
        self.kept == self.keep
    }
}

impl RouteReader<'_> {
    /// The route's polyline decimated to at most `N` points, uniform by point index: the
    /// overview's shape preview.
    ///
    /// It streams the chunks once in route order, skipping each seam's shared point, so the pick
    /// is over distinct points. A chunk that fails to decode is skipped; the preview is a sketch,
    /// not navigation data.
    pub fn preview_polyline<const N: usize>(&self) -> Vec<(i32, i32), N> {
        let mut out = Vec::new();
        if self.chunks().is_empty() || N == 0 {
            return out;
        }
        let mut pick = Pick::new(self.segment_count() as usize + 1, N);
        for k in 0..self.chunks().len() {
            let _ = self.with_chunk(k, |points| {
                points.skip(usize::from(k > 0)).for_each(|p| {
                    if pick.keep_next() {
                        let _ = out.push((p.lon, p.lat));
                    }
                })
            });
            if pick.done() {
                break;
            }
        }
        out
    }

    /// Assistant Visit shape from departure through rejoin; other reviews show the full route.
    /// The stored continuation stays intact and ordinary route overviews still use `preview_polyline`.
    pub fn assistant_preview_polyline<const N: usize>(&self) -> Result<Vec<(i32, i32), N>, Error> {
        let Some(visit) = self.visit_descriptor()? else {
            let shape = self.preview_polyline::<N>();
            let count = if self.chunks().is_empty() { 0 } else { self.segment_count() as usize + 1 };
            return if shape.len() == count.min(N) { Ok(shape) } else { Err(Error::BadOffset) };
        };
        let [lo, stop, hi] = visit.accepted_anchors_m;
        let mut shape = Vec::new();
        if N >= 3 {
            self.append_preview_span(lo, stop, N / 2 + 1, &mut shape)?;
            self.append_preview_span(stop, hi, N, &mut shape)?;
        } else if N > 0 {
            self.append_preview_span(lo, hi, N, &mut shape)?;
        }
        Ok(shape)
    }

    /// Append `[lo, hi]` picked down to `limit` points. The pick needs the span's point count up
    /// front, so the span is walked twice.
    fn append_preview_span<const N: usize>(
        &self,
        lo: u32,
        hi: u32,
        limit: usize,
        shape: &mut Vec<(i32, i32), N>,
    ) -> Result<(), Error> {
        let mut count = 0;
        self.preview_span(lo, hi, |_| count += 1)?;
        let continues = !shape.is_empty();
        let mut pick = Pick::new(count, limit.min(N - shape.len() + usize::from(continues)));
        let mut first = continues;
        self.preview_span(lo, hi, |point| {
            if pick.keep_next() {
                // Both spans share the stop occurrence. Keep the first representation even when a
                // chunk boundary quantizes the second span's start to another coordinate.
                if !core::mem::take(&mut first) && shape.last() != Some(&point) {
                    let _ = shape.push(point);
                }
            }
        })
    }

    /// The distinct consecutive coordinates of `[lo, hi]`. A span that ends at the route end keeps
    /// the final stored point, not a sub-metre clip of it.
    fn preview_span(&self, lo: u32, hi: u32, mut visit: impl FnMut((i32, i32))) -> Result<(), Error> {
        let upper = if hi == self.total_distance_m { u32::MAX } else { hi };
        let mut previous = None;
        let mut emit = |p: RoutePoint| -> Result<(), Error> {
            let p = (p.lon, p.lat);
            if previous != Some(p) {
                visit(p);
                previous = Some(p);
            }
            Ok(())
        };
        for (k, chunk) in self.chunks().iter().enumerate() {
            if chunk.cum_distance_m > hi {
                break;
            }
            if self.chunk_end_m(k) < lo {
                continue;
            }
            if chunk.point_count == 1 {
                self.with_chunk(k, |mut points| points.try_for_each(&mut emit))??;
            } else {
                self.clip_chunk(k, lo, upper, &mut emit)?;
            }
        }
        Ok(())
    }
}
