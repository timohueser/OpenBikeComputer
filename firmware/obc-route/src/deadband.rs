//! Elevation dead-band integrator.
//!
//! Ascent/descent totals must ignore the small up-and-down wiggle of sampled or sensed
//! elevation, or GPS/barometric noise inflates them. This hysteresis integrator only
//! books a change once it exceeds [`ELE_DEADBAND_M`] from the last booked reference, then
//! re-anchors there.
//!
//! **Why it's shared.** Three elevation paths must agree on this dead-band or their numbers
//! silently diverge:
//! - the [converter](crate::convert) precomputes a route's total ascent/descent,
//! - the [elevation profile](crate::profile) integrates the same climb per column so the
//!   "to climb" stat reaches 0 exactly at the route's end, and
//! - the app's actually-ridden barometric climb must land near that precomputed ascent
//!   when the rider follows the route.
//!
//! One definition, so tuning the dead-band can't leave one copy behind. Generic over the
//! sample type so the converter's `f64` and the profile/app `f32` share the code.

use core::ops::{Add, Neg, Sub};

/// Elevation dead-band (m): a move smaller than this is treated as noise — it neither
/// counts toward ascent/descent nor moves the reference. The single source of truth for
/// every [`DeadBand`]; the per-sample-type threshold is this value cast in [`Elev`].
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
    ascent: T,
    descent: T,
}

impl<T: Elev> DeadBand<T> {
    /// A fresh integrator: no reference, zero totals.
    pub fn new() -> Self {
        DeadBand { ref_ele: None, ascent: T::ZERO, descent: T::ZERO }
    }

    /// Integrate one elevation sample. A move of at least [`Elev::DEADBAND`] from the
    /// reference books the whole delta as ascent (up) or descent (down) and re-anchors
    /// the reference there; a smaller move is ignored (neither booked nor re-anchored).
    pub fn push(&mut self, e: T) {
        match self.ref_ele {
            None => self.ref_ele = Some(e),
            Some(r) => {
                let d = e - r;
                if d >= T::DEADBAND {
                    self.ascent = self.ascent + d;
                    self.ref_ele = Some(e);
                } else if d <= -T::DEADBAND {
                    self.descent = self.descent + (-d);
                    self.ref_ele = Some(e);
                }
            }
        }
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
