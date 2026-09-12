//! Observer metadata, named summits, and bounded runtime panorama generation.

/// An owned UTF-8 summit name with a fixed memory bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeakName {
    bytes: [u8; 24],
    len: u8,
}

impl PeakName {
    pub const fn new(name: &str) -> Self {
        let source = name.as_bytes();
        let mut len = if source.len() < 24 { source.len() } else { 24 };
        while len < source.len() && source[len] & 0xc0 == 0x80 {
            len -= 1;
        }
        let mut bytes = [0; 24];
        let mut at = 0;
        while at < len {
            bytes[at] = source[at];
            at += 1;
        }
        Self { bytes, len: len as u8 }
    }

    pub fn as_str(&self) -> &str {
        // SAFETY: construction copies only complete UTF-8 code points.
        unsafe { core::str::from_utf8_unchecked(&self.bytes[..self.len as usize]) }
    }
}

/// A named summit aligned to its visible DEM surface. Angles are quarter-degrees clockwise from
/// north and above the observer's horizontal plane. Height and distance retain catalogue values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeakViewPeak {
    pub name: PeakName,
    pub lat: i32,
    pub lon: i32,
    pub elevation_m: Option<i16>,
    pub distance_m: u32,
    pub azimuth_q4: u16,
    pub angle_q4: i16,
    /// True when the producer's sight line is not blocked by nearer terrain.
    pub visible: bool,
    /// Relative label importance. Only its ordering is significant.
    pub score: u32,
}

impl PeakViewPeak {
    pub fn project(&mut self, lat: i32, lon: i32) {
        let north = (i64::from(self.lat) - i64::from(lat)) as f32 * 0.11132;
        let east = (i64::from(self.lon) - i64::from(lon)) as f32
            * 0.11132
            * libm::cosf(((self.lat as f32 + lat as f32) / 2e6).to_radians());
        self.distance_m = libm::sqrtf(north * north + east * east) as u32;
        self.azimuth_q4 = (libm::roundf(libm::atan2f(east, north).to_degrees() * 4.0) as i32).rem_euclid(1440) as u16;
    }

    pub const EMPTY: Self = Self {
        name: PeakName::new(""),
        lat: 0,
        lon: 0,
        elevation_m: None,
        distance_m: 0,
        azimuth_q4: 0,
        angle_q4: 0,
        visible: false,
        score: 0,
    };
}

pub mod panorama;
pub mod surface;
pub mod terrain;
pub use panorama::Panorama;

/// Observer configuration and summit projections for a panorama.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeakViewProfile<'a> {
    /// Identity for a named test input; zero for a live observer.
    pub id: u8,
    pub name: &'static str,
    pub observer_lat: i32,
    pub observer_lon: i32,
    pub observer_elevation_m: i16,
    pub default_heading_q4: u16,
    /// Relief framing used to choose the horizontal window and vertical centre.
    pub angle_bottom_q4: i16,
    pub angle_top_q4: i16,
    /// Named summits sorted clockwise by [`PeakViewPeak::azimuth_q4`].
    pub peaks: &'a [PeakViewPeak],
}

impl PeakViewProfile<'_> {
    pub const fn at(lat: i32, lon: i32, elevation_m: i16) -> PeakViewProfile<'static> {
        PeakViewProfile {
            id: 0,
            name: "",
            observer_lat: lat,
            observer_lon: lon,
            observer_elevation_m: elevation_m,
            default_heading_q4: 0,
            angle_bottom_q4: -32,
            angle_top_q4: 92,
            peaks: &[],
        }
    }

    /// Copy framing and observer data without retaining a borrowed candidate list.
    pub fn detached(&self) -> PeakViewProfile<'static> {
        PeakViewProfile { peaks: &[], ..*self }
    }

    /// Set observer height and keep steep nearby summits inside the live view's vertical frame.
    pub fn set_ground(&mut self, ground_m: f32) {
        self.observer_elevation_m = libm::roundf(ground_m + 2.0) as i16;
        if self.id != 0 {
            return;
        }
        let highest = self
            .peaks
            .iter()
            .filter_map(|peak| {
                let height = peak.elevation_m?;
                (peak.distance_m >= 30).then(|| {
                    libm::atan2f(f32::from(height) - f32::from(self.observer_elevation_m), peak.distance_m as f32)
                        .to_degrees()
                        * 4.0
                })
            })
            .fold(96.0f32, f32::max);
        if highest <= 96.0 {
            return;
        }
        let top = (libm::ceilf(highest) as i32 + 40).min(340);
        let bottom = -60;
        let centre = (top + bottom) / 2;
        let span = ((top - bottom) * 50 + 71) / 72;
        self.angle_bottom_q4 = (centre - span / 2) as i16;
        self.angle_top_q4 = self.angle_bottom_q4 + span as i16;
    }

    pub fn horizontal_fov_q4(&self) -> i32 {
        (i32::from(self.angle_top_q4) - i32::from(self.angle_bottom_q4)).max(1) * 72 / 37
    }

    /// Terrain and labels share 1.25× vertical exaggeration on the 240×222 chart.
    pub fn vertical_bounds_q4(&self) -> (i32, i32) {
        let span = (self.horizontal_fov_q4() * 37 / 50).max(1);
        let centre = (i32::from(self.angle_top_q4) + i32::from(self.angle_bottom_q4)) / 2;
        let bottom = centre - span / 2;
        (bottom, bottom + span)
    }
}

/// Keep two strong candidates per bearing sector; DEM visibility is resolved by the renderer.
pub fn collect_summits(
    reader: &obc_reader::Reader<'_>,
    position: (i32, i32),
    out: &mut heapless::Vec<PeakViewPeak, 32>,
) -> Result<(), obc_reader::Error> {
    out.clear();
    reader.visit_summits_within((position.1, position.0), 100_000, |summit| {
        let mut peak = PeakViewPeak {
            name: PeakName::new(summit.name.as_str()),
            lat: summit.lat,
            lon: summit.lon,
            elevation_m: summit.elevation_m,
            ..PeakViewPeak::EMPTY
        };
        peak.project(position.0, position.1);
        if peak.distance_m < 30 || peak.distance_m > 100_000 {
            return;
        }
        peak.score =
            (u32::from(peak.elevation_m.unwrap_or(0).max(0) as u16) + 100) * 20_000 / (peak.distance_m + 20_000);
        let sector = peak.azimuth_q4 / 90;
        let mut count = 0;
        let mut weakest = None;
        for (i, previous) in out.iter().enumerate() {
            if previous.azimuth_q4 / 90 == sector {
                count += 1;
                if weakest.is_none_or(|old: usize| previous.score < out[old].score) {
                    weakest = Some(i);
                }
            }
        }
        if count < 2 {
            let _ = out.push(peak);
        } else if let Some(index) = weakest {
            if peak.score > out[index].score {
                out[index] = peak;
            }
        }
    })?;
    out.sort_unstable_by_key(|peak| peak.azimuth_q4);
    Ok(())
}

/// Position changes do not cancel a panorama already being generated.
pub fn moved(from: (i32, i32), to: (i32, i32)) -> bool {
    let north = (i64::from(to.0) - i64::from(from.0)) as f32 * 0.11132;
    let east = (i64::from(to.1) - i64::from(from.1)) as f32 * 0.11132 * libm::cosf((to.0 as f32 / 1e6).to_radians());
    north * north + east * east > 400.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_end_at_a_utf8_boundary_and_projection_uses_exact_coordinates() {
        assert_eq!(PeakName::new("abcdefghijklmnopqrstuvwä").as_str(), "abcdefghijklmnopqrstuvw");
        let mut peak = PeakViewPeak { lat: 46_010_000, lon: 8_000_000, ..PeakViewPeak::EMPTY };
        peak.project(46_000_000, 8_000_000);
        assert_eq!(peak.azimuth_q4, 0);
        assert!((1112..=1114).contains(&peak.distance_m));
        peak.project(46_010_000, 7_990_000);
        assert_eq!(peak.azimuth_q4, 360);
        assert!((772..=774).contains(&peak.distance_m));
        let peaks = [PeakViewPeak { distance_m: 1000, elevation_m: Some(2000), ..peak }];
        let mut profile = PeakViewProfile { peaks: &peaks, ..PeakViewProfile::at(46_000_000, 8_000_000, 0) };
        profile.set_ground(1000.0);
        let (bottom, top) = profile.vertical_bounds_q4();
        assert!(bottom <= -56 && top >= 200, "nearby 45-degree terrain fits with headroom");
        assert_eq!(profile.observer_elevation_m, 1002);
        let mut flat = PeakViewProfile::at(46_000_000, 8_000_000, 0);
        flat.set_ground(1000.0);
        assert_eq!(flat.horizontal_fov_q4(), 241, "the ordinary frame keeps its 60-degree window");
        assert!(!moved((46_000_000, 8_000_000), (46_000_050, 8_000_000)));
        assert!(moved((46_000_000, 8_000_000), (46_000_200, 8_000_000)));
    }
}
