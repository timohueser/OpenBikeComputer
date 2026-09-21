use core::ops::{Add, Neg, Sub};

/// Default elevation dead-band in metres: a smaller move is noise, so it neither counts toward
/// ascent or descent nor moves the reference. The single source of truth for every
/// [`DeadBand::new`].
pub const ELE_DEADBAND_M: f64 = 3.0;

/// A float usable as an elevation sample, implemented for `f64` and `f32`. Both take the same
/// [`ELE_DEADBAND_M`], cast to the sample type.
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

/// Hysteresis integrator over a stream of elevations. Feed samples in route or time order and read
/// the running totals at any point. A caller wanting only climb ignores `descent`.
#[derive(Debug, Clone, Copy)]
pub struct DeadBand<T: Elev> {
    /// Reference the next sample is measured against; `None` until the first sample.
    ref_ele: Option<T>,
    /// The hysteresis threshold this integrator books against.
    threshold: T,
    ascent: T,
    descent: T,
}

impl<T: Elev> DeadBand<T> {
    /// A fresh integrator at the shared [`ELE_DEADBAND_M`] threshold: no reference, zero totals.
    pub fn new() -> Self {
        Self::with_threshold(T::DEADBAND)
    }

    /// A fresh integrator at a caller-chosen threshold in metres. Use it only with a measured
    /// error model: a total booked at a different dead-band is not comparable with the
    /// rider-facing ones.
    pub fn with_threshold(threshold: T) -> Self {
        DeadBand { ref_ele: None, threshold, ascent: T::ZERO, descent: T::ZERO }
    }

    /// Restore already-booked totals after a persistence boundary, deliberately without restoring
    /// the elevation reference. The next sample re-anchors, so an altitude change while the device
    /// was off is never booked as one giant climb.
    pub fn from_totals(ascent: T, descent: T) -> Self {
        DeadBand { ref_ele: None, threshold: T::DEADBAND, ascent, descent }
    }

    /// Integrate one elevation sample. A move of at least the threshold books the whole delta and
    /// re-anchors the reference; a smaller move is ignored.
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

    /// The current smoothed elevation: the last sample that moved at least the threshold. `None`
    /// until the first push. This staircase view is what callers segment on, so a sub-band wiggle
    /// cannot spuriously open or close a segment.
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

    /// Drop the reference but keep the accumulated totals, for a tracking pause, so an elevation
    /// change during the gap is not booked when sampling resumes.
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

    /// The shape of the hysteresis: a sub-band wiggle books nothing and does not re-anchor, so a
    /// staircase of noise cannot accumulate, while a clear move books its whole delta.
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
    /// booked as one giant climb.
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
    fn restored_totals_keep_the_total_and_reanchor_after_the_gap() {
        let mut db = DeadBand::<f32>::from_totals(42.0, 7.0);
        assert_eq!(db.ascent(), 42.0);
        assert_eq!(db.descent(), 7.0);
        assert_eq!(db.smoothed(), None);
        db.push(1_000.0);
        assert_eq!(db.ascent(), 42.0, "the first post-restore height is only an anchor");
        db.push(1_004.0);
        assert_eq!(db.ascent(), 46.0);
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
