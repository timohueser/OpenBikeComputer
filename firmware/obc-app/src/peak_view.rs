//! Peak View's small, renderer-ready scene contract.
//!
//! The prototype keeps terrain acquisition outside the device map format. A host supplies one
//! immutable [`PeakViewProfile`]: three full-circle horizon layers plus the named summits that can
//! be selected. The screen does not know whether those samples came from a simulator fixture, a
//! future OBCM section, or another store. This keeps the first UI iteration independent from the
//! storage decision.

/// One named summit in a [`PeakViewProfile`]. Angles are quarter-degrees clockwise from north;
/// elevation angles are quarter-degrees above the observer's horizontal plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeakViewPeak {
    pub name: &'static str,
    pub elevation_m: u16,
    pub distance_m: u32,
    pub azimuth_q4: u16,
    pub angle_q4: i16,
    /// Terrain distance band: 0 near, 1 middle, 2 far.
    pub layer: u8,
    /// Relative label importance. Only its ordering is significant.
    pub score: u32,
}

/// A complete 360-degree panorama at one observer location.
#[derive(Debug)]
pub struct PeakViewProfile {
    /// Stable fixture/store identity. Equality uses this value instead of walking the sample arrays.
    pub id: u8,
    pub name: &'static str,
    pub observer_lat: i32,
    pub observer_lon: i32,
    pub observer_elevation_m: u16,
    pub default_heading_q4: u16,
    /// Uniform angular distance between adjacent samples, in quarter-degrees.
    pub sample_step_q4: u16,
    /// Shared vertical scale for every heading, with deliberate sky/ground padding.
    pub angle_bottom_q4: i16,
    pub angle_top_q4: i16,
    /// Near, middle, and far horizon layers. Each slice covers 360 degrees and has equal length.
    pub layers_q4: [&'static [i16]; 3],
    /// Named summits sorted clockwise by [`PeakViewPeak::azimuth_q4`].
    pub peaks: &'static [PeakViewPeak],
}

impl PartialEq for PeakViewProfile {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for PeakViewProfile {}
