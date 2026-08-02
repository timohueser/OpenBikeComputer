//! Elevation dead-band integrator.
//!
//! Ascent/descent totals must ignore the small up-and-down wiggle of sampled or sensed
//! elevation, or GPS/barometric noise inflates them. This hysteresis integrator only
//! books a change once it exceeds a threshold from the last booked reference, then
//! re-anchors there.
//!
//! **Why it's shared.** Every elevation path must agree on this dead-band or their numbers
//! silently diverge:
//! - the GPX converter (`obc_route::convert`) precomputes a route's total ascent/descent,
//! - the elevation profile (`obc_route::profile`) integrates the same climb per column so the
//!   "to climb" stat reaches 0 exactly at the route's end,
//! - the app's actually-ridden barometric climb must land near that precomputed ascent
//!   when the rider follows the route,
//! - and (epic #1068) the packer's per-edge ascent and the device's emit-time profile must agree
//!   with all three, or a climb-aware route costs one number and displays another.
//!
//! One definition, so tuning the dead-band can't leave one copy behind. Generic over the
//! sample type so the converter's `f64` and the profile/app `f32` share the code.
//!
//! **The threshold is a parameter, not only a constant.** [`ELE_DEADBAND_M`] stays the default and
//! is what every rider-facing total uses, but a DEM resample and a barometer are different error
//! models — a raster's jitter is a function of posting and slope, a baro's of weather and vibration
//! — so a caller that has measured its own may hand it in with [`DeadBand::with_threshold`]. A
//! consumer that does MUST state the value it pinned and why (epic #1068's named risk).

use core::ops::{Add, Neg, Sub};

/// Default elevation dead-band (m): a move smaller than this is treated as noise — it neither
/// counts toward ascent/descent nor moves the reference. The single source of truth for
/// every [`DeadBand::new`]; the per-sample-type value is this constant cast in [`Elev`].
pub const ELE_DEADBAND_M: f64 = 3.0;

/// A float usable as an elevation sample. Implemented for the converter's `f64` and the
/// profile/app `f32`; both take the same [`ELE_DEADBAND_M`] dead-band, cast to the sample
/// type via [`Elev::DEADBAND`].
pub trait Elev: Copy + PartialOrd + Add<Output = Self> + Sub<Output = Self> + Neg<Output = Self> {
    /// The additive identity for this type (`0.0`).
    const ZERO: Self;
    /// [`ELE_DEADBAND_M`] in this sample type.
    const DEADBAND: Self;
}

impl Elev for f32 {
    const ZERO: f32 = 0.0;
    const DEADBAND: f32 = ELE_DEADBAND_M as f32;
}

impl Elev for f64 {
    const ZERO: f64 = 0.0;
    const DEADBAND: f64 = ELE_DEADBAND_M;
}

/// Hysteresis integrator over a stream of elevations. Feed samples in route/time order
/// with [`push`](DeadBand::push) and read the running [`ascent`](DeadBand::ascent) /
/// [`descent`](DeadBand::descent) at any point. A caller wanting only climb reads
/// `ascent` and ignores `descent` (it still tracks, harmlessly).
#[derive(Debug, Clone, Copy)]
pub struct DeadBand<T: Elev> {
    /// Reference the next sample is measured against; `None` until the first sample.
    ref_ele: Option<T>,
    /// The hysteresis threshold this integrator books against — [`Elev::DEADBAND`] unless the
    /// caller supplied its own.
    threshold: T,
    ascent: T,
    descent: T,
}

impl<T: Elev> DeadBand<T> {
    /// A fresh integrator at the shared [`ELE_DEADBAND_M`] threshold: no reference, zero totals.
    pub fn new() -> Self {
        Self::with_threshold(T::DEADBAND)
    }

    /// A fresh integrator at a caller-chosen threshold (m). See the module docs: use this only with
    /// a measured error model, and say so where it is pinned — a total booked at a different
    /// dead-band is not comparable with the rider-facing ones.
    pub fn with_threshold(threshold: T) -> Self {
        DeadBand { ref_ele: None, threshold, ascent: T::ZERO, descent: T::ZERO }
    }

    /// Integrate one elevation sample. A move of at least the threshold from the reference books
    /// the whole delta as ascent (up) or descent (down) and re-anchors the reference there; a
    /// smaller move is ignored (neither booked nor re-anchored).
    pub fn push(&mut self, e: T) {
        match self.ref_ele {
            None => self.ref_ele = Some(e),
            Some(r) => {
                let d = e - r;
                if d >= self.threshold {
                    self.ascent = self.ascent + d;
                    self.ref_ele = Some(e);
                } else if d <= -self.threshold {
                    self.descent = self.descent + (-d);
                    self.ref_ele = Some(e);
                }
            }
        }
    }

    /// The current smoothed elevation: the reference the next sample is measured against, i.e.
    /// the last elevation that moved at least the threshold. `None` until the first
    /// [`push`](DeadBand::push). This is the dead-band's staircase view of the signal — the same
    /// hysteresis that filters ascent/descent, exposed for callers (e.g. `obc_route::climb`) that
    /// segment on the *smoothed* height rather than the raw noisy samples, so a sub-band wiggle
    /// can't spuriously open or close a segment.
    #[inline]
    pub fn smoothed(&self) -> Option<T> {
        self.ref_ele
    }

    /// Cumulative climb (m) booked so far.
    #[inline]
    pub fn ascent(&self) -> T {
        self.ascent
    }

    /// Cumulative drop (m) booked so far, as a positive quantity.
    #[inline]
    pub fn descent(&self) -> T {
        self.descent
    }

    /// The threshold this integrator books against (m).
    #[inline]
    pub fn threshold(&self) -> T {
        self.threshold
    }

    /// Drop the reference but keep the accumulated totals — for a tracking pause, so an
    /// elevation change *during* the gap isn't booked when sampling resumes. The next
    /// [`push`](DeadBand::push) re-anchors instead of measuring across the hole.
    #[inline]
    pub fn pause(&mut self) {
        self.ref_ele = None;
    }
}

impl<T: Elev> Default for DeadBand<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the hysteresis: sub-band wiggle books nothing and does not re-anchor, so a
    /// staircase of noise cannot accumulate; a clear move books its *whole* delta.
    #[test]
    fn sub_band_noise_books_nothing_and_a_clear_move_books_all_of_it() {
        let mut db = DeadBand::<f32>::new();
        for e in [100.0, 101.0, 99.5, 100.5, 102.0] {
            db.push(e);
        }
        assert_eq!(db.ascent(), 0.0, "every move stayed inside the 3 m band");
        assert_eq!(db.descent(), 0.0);
        assert_eq!(db.smoothed(), Some(100.0), "…and none of them re-anchored");

        db.push(104.0);
        assert_eq!(db.ascent(), 4.0, "the whole delta from the reference, not just the excess");
        assert_eq!(db.smoothed(), Some(104.0));
        db.push(100.0);
        assert_eq!(db.descent(), 4.0);
        assert_eq!(db.ascent(), 4.0, "descent never touches the ascent total");
    }

    /// A pause drops the reference without touching the totals, so a gap in sampling cannot be
    /// booked as one giant climb when tracking resumes.
    #[test]
    fn a_pause_re_anchors_instead_of_measuring_across_the_hole() {
        let mut db = DeadBand::<f64>::new();
        db.push(500.0);
        db.push(510.0);
        assert_eq!(db.ascent(), 10.0);
        db.pause();
        assert_eq!(db.smoothed(), None);
        db.push(1500.0); // the rider drove up a pass with tracking off
        assert_eq!(db.ascent(), 10.0, "the gap is not climb");
        db.push(1510.0);
        assert_eq!(db.ascent(), 20.0, "…and integration resumes from the new anchor");
    }

    #[test]
    fn the_default_threshold_is_the_shared_constant_in_both_sample_types() {
        assert_eq!(DeadBand::<f32>::new().threshold(), ELE_DEADBAND_M as f32);
        assert_eq!(DeadBand::<f64>::new().threshold(), ELE_DEADBAND_M);
    }

    /// A caller-supplied threshold changes what is booked and nothing else.
    #[test]
    fn a_custom_threshold_replaces_only_the_hysteresis() {
        let mut tight = DeadBand::<f32>::with_threshold(0.5);
        let mut loose = DeadBand::<f32>::with_threshold(20.0);
        for e in [100.0, 101.0, 99.5, 110.0] {
            tight.push(e);
            loose.push(e);
        }
        assert_eq!((tight.ascent(), tight.descent()), (11.5, 1.5));
        assert_eq!((loose.ascent(), loose.descent()), (0.0, 0.0));
        assert_eq!(loose.smoothed(), Some(100.0));
    }
}
