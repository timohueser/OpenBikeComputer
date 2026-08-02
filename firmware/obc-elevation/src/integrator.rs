//! [`ProfileIntegrator`] — the dead-banded ascent/descent accumulator over a `(distance, elevation)`
//! stream: the one place a polyline turns into the two numbers everything downstream displays.
//!
//! [`DeadBand`] alone answers "how much did this signal climb". A profile also needs "over what
//! length", and every consumer that wants one wants the other:
//!
//! - the packer (EL5) integrates ascent **along an edge polyline** and stores it per direction next
//!   to that edge's length — the pair *is* the routing cost input;
//! - route emit (EL7) fills per-point elevation and re-derives the same totals the imported-GPX
//!   path derives, so a device-planned route and the identical Komoot import agree;
//! - the profile and ride ledger already carry both.
//!
//! Sampling ascent is **not** the same as differencing endpoints: a pass road between two 500 m
//! junctions has enormous climb and zero net. That is why the integrator is a stream fold and why
//! the dead-band lives inside it — the noise floor has to be applied to the *samples*, not to the
//! result.
//!
//! The distance channel is also the ordering guard: distances must not go backwards, and a repeat
//! at the same distance (a chunk seam's shared point, a duplicated vertex) contributes nothing.

use crate::deadband::{DeadBand, Elev};

/// Running ascent, descent and length over a `(distance_m, elevation_m)` stream.
///
/// Feed samples in travel order with [`push`](ProfileIntegrator::push); read the totals at any
/// point. Copy-cheap and allocation-free: it holds four numbers, so a caller integrating thousands
/// of edges keeps one on the stack per edge without a thought.
#[derive(Debug, Clone, Copy)]
pub struct ProfileIntegrator<T: Elev> {
    band: DeadBand<T>,
    /// Distance of the last accepted sample; `None` until the first push.
    last_dist_m: Option<f32>,
    /// Distance from the first accepted sample to the last, m.
    length_m: f32,
}

impl<T: Elev> ProfileIntegrator<T> {
    /// A fresh integrator at the shared [`ELE_DEADBAND_M`](crate::ELE_DEADBAND_M) dead-band.
    pub fn new() -> Self {
        Self::with_band(DeadBand::new())
    }

    /// A fresh integrator over a caller-configured [`DeadBand`] — the seam for a consumer that has
    /// measured its own noise floor (see [`DeadBand::with_threshold`]).
    pub fn with_band(band: DeadBand<T>) -> Self {
        ProfileIntegrator { band, last_dist_m: None, length_m: 0.0 }
    }

    /// Integrate one sample. `dist_m` is the **cumulative** distance along the line, not a step.
    ///
    /// A sample at or before the previous distance is not an error and is not rejected: its
    /// elevation still integrates (a doubled vertex is common in real geometry and its height is
    /// real), but it cannot shorten the line. Non-monotone input therefore degrades to "no length
    /// added", never to a negative length that would silently poison an edge cost.
    pub fn push(&mut self, dist_m: f32, elevation_m: T) {
        match self.last_dist_m {
            None => self.last_dist_m = Some(dist_m),
            Some(prev) if dist_m > prev => {
                self.length_m += dist_m - prev;
                self.last_dist_m = Some(dist_m);
            }
            Some(_) => {}
        }
        self.band.push(elevation_m);
    }

    /// Cumulative dead-banded climb (m).
    #[inline]
    pub fn ascent(&self) -> T {
        self.band.ascent()
    }

    /// Cumulative dead-banded drop (m), as a positive quantity.
    #[inline]
    pub fn descent(&self) -> T {
        self.band.descent()
    }

    /// Length of the integrated line (m) — the distance spanned by the accepted samples.
    #[inline]
    pub fn length_m(&self) -> f32 {
        self.length_m
    }

    /// The dead-band underneath, for a caller that also wants the smoothed height
    /// ([`DeadBand::smoothed`]) or has to pause it across a gap.
    #[inline]
    pub fn band(&mut self) -> &mut DeadBand<T> {
        &mut self.band
    }
}

impl<T: Elev> Default for ProfileIntegrator<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileIntegrator<f32> {
    /// Ascent as the `uint16` metres the OBCM v12 neighbour entry carries (EL5), saturating.
    /// Saturation rather than wrap is the safe direction for a router: an absurd edge becomes
    /// maximally expensive instead of free. 65 535 m of climb on one edge is not reachable on
    /// Earth, so this only ever fires on corrupt input.
    #[inline]
    pub fn ascent_u16(&self) -> u16 {
        let a = self.ascent();
        if a <= 0.0 {
            0
        } else if a >= u16::MAX as f32 {
            u16::MAX
        } else {
            a as u16
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The headline property: ascent is an **integral**, not an endpoint difference. A pass with
    /// equal start and end heights has zero net change and hundreds of metres of climb.
    #[test]
    fn a_pass_has_no_net_change_and_a_great_deal_of_climb() {
        let mut it = ProfileIntegrator::<f32>::new();
        let profile = [(0.0, 500.0), (2_000.0, 900.0), (4_000.0, 1_200.0), (7_000.0, 500.0)];
        for (d, e) in profile {
            it.push(d, e);
        }
        assert_eq!(it.ascent(), 700.0);
        assert_eq!(it.descent(), 700.0);
        assert_eq!(it.length_m(), 7_000.0);
        assert_eq!(it.ascent_u16(), 700);
    }

    /// The dead-band is inside the integrator, so a jittery DEM resample along a flat valley road
    /// books nothing.
    #[test]
    fn sub_band_jitter_along_a_flat_road_books_no_climb() {
        let mut it = ProfileIntegrator::<f32>::new();
        for (k, e) in [220.0f32, 221.5, 219.0, 221.0, 220.5, 222.0].into_iter().enumerate() {
            it.push(k as f32 * 100.0, e);
        }
        assert_eq!(it.ascent(), 0.0);
        assert_eq!(it.descent(), 0.0);
        assert_eq!(it.length_m(), 500.0);
    }

    /// Non-monotone distance cannot shorten the line, and a repeated vertex is free.
    #[test]
    fn a_repeated_or_backward_sample_never_shortens_the_line() {
        let mut it = ProfileIntegrator::<f32>::new();
        it.push(0.0, 100.0);
        it.push(100.0, 110.0);
        it.push(100.0, 110.0); // a chunk seam's shared point
        it.push(80.0, 110.0); // a backward step in bad input
        it.push(200.0, 120.0);
        assert_eq!(it.length_m(), 200.0);
        assert_eq!(it.ascent(), 20.0);
    }

    #[test]
    fn an_empty_or_single_sample_line_has_no_length_and_no_climb() {
        let empty = ProfileIntegrator::<f64>::new();
        assert_eq!((empty.ascent(), empty.descent(), empty.length_m()), (0.0, 0.0, 0.0));
        let mut one = ProfileIntegrator::<f64>::new();
        one.push(1_234.0, 800.0);
        assert_eq!((one.ascent(), one.length_m()), (0.0, 0.0));
    }

    #[test]
    fn the_u16_ascent_saturates_instead_of_wrapping() {
        let mut it = ProfileIntegrator::<f32>::new();
        it.push(0.0, 0.0);
        it.push(1.0, 90_000.0);
        assert_eq!(it.ascent_u16(), u16::MAX);
    }

    /// A custom band flows through the integrator untouched.
    #[test]
    fn the_integrator_books_at_the_bands_threshold() {
        let mut it = ProfileIntegrator::with_band(DeadBand::<f32>::with_threshold(10.0));
        for (k, e) in [100.0f32, 105.0, 100.0, 115.0].into_iter().enumerate() {
            it.push(k as f32 * 50.0, e);
        }
        assert_eq!(it.ascent(), 15.0, "only the move past 10 m booked");
        assert_eq!(it.descent(), 0.0);
        assert_eq!(it.band().smoothed(), Some(115.0));
    }
}
