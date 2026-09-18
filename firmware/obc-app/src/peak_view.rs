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
    pub source: obc_formats::obcm::SourceId,
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
        source: obc_formats::obcm::SourceId(0),
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

mod articles;
pub mod panorama;
pub mod runtime;
pub mod surface;
pub mod terrain;
pub use panorama::Panorama;

/// Observer configuration and summit projections for a panorama.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeakViewProfile<'a> {
    pub observer_lat: i32,
    pub observer_lon: i32,
    pub observer_elevation_m: i16,
    pub default_heading_q4: u16,
    /// Fixed framing for the whole panorama, in quarter degrees.
    pub fov_q4: u16,
    pub vertical_centre_q4: i16,
    pub vertical_span_q4: u16,
    /// Named summits sorted clockwise by [`PeakViewPeak::azimuth_q4`].
    pub peaks: &'a [PeakViewPeak],
}

impl PeakViewProfile<'_> {
    pub const fn at(lat: i32, lon: i32, elevation_m: i16) -> PeakViewProfile<'static> {
        PeakViewProfile {
            observer_lat: lat,
            observer_lon: lon,
            observer_elevation_m: elevation_m,
            default_heading_q4: 0,
            fov_q4: 360,
            vertical_centre_q4: 30,
            vertical_span_q4: 266,
            peaks: &[],
        }
    }

    /// Copy framing and observer data without retaining a borrowed candidate list.
    pub fn detached(&self) -> PeakViewProfile<'static> {
        PeakViewProfile { peaks: &[], ..*self }
    }

    /// Frame the observer's relief once; turning keeps the same scale and horizon position.
    pub fn set_ground(&mut self, ground_m: f32) {
        self.observer_elevation_m = libm::roundf(ground_m + 2.0) as i16;
        let (highest, relief) = self
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
            .fold((0.0f32, None::<f32>), |(highest, relief), angle| {
                (highest.max(angle), Some(relief.unwrap_or(0.0).max(angle.abs())))
            });
        // Boost shallow relief by at most 2.4×; steep views use the base 1.25× scale.
        let boost = relief.map(|angle| (56.0 / angle.max(1.0)).clamp(1.0, 2.4)).unwrap_or(1.0);
        let (fov, centre) = if highest > 96.0 {
            let top = (libm::ceilf(highest) as i32 + 40).min(340);
            ((((top + 60) * 50 + 71) / 72 * 72 / 37).max(360), (top - 60) / 2)
        } else {
            (360, 30)
        };
        self.fov_q4 = fov as u16;
        self.vertical_centre_q4 = (centre as f32 / boost) as i16;
        // 222 / 240 chart aspect, with the base 1.25× vertical scale.
        self.vertical_span_q4 = (fov as f32 * 0.74 / boost).max(1.0) as u16;
    }

    pub fn horizontal_fov_q4(&self) -> i32 {
        i32::from(self.fov_q4)
    }

    pub fn vertical_bounds_q4(&self) -> (i32, i32) {
        let span = i32::from(self.vertical_span_q4.max(1));
        let bottom = i32::from(self.vertical_centre_q4) - span / 2;
        (bottom, bottom + span)
    }
}

pub const MAX_PEAKS: usize = 64;
pub type Candidates = heapless::Vec<PeakViewPeak, MAX_PEAKS>;

/// Apparent elevation above the observer, with a positive offset for unsigned ranking.
pub fn apparent_size(peak: &PeakViewPeak, observer_height: i16) -> u32 {
    peak.elevation_m
        .map(|height| {
            ((libm::atan2f(f32::from(height) - f32::from(observer_height), peak.distance_m as f32).to_degrees() + 90.0)
                * 10_000.0) as u32
        })
        .unwrap_or(0)
}

/// Reserve each sector's tallest landmark, then fill the remaining slots by apparent size.
/// Existing visible peaks survive a refill. Already tested names are excluded from both passes.
pub fn collect_summits(
    reader: &obc_reader::Reader<'_>,
    position: (i32, i32),
    observer_height: i16,
    tested: &[PeakName],
    out: &mut Candidates,
) -> Result<(), obc_reader::Error> {
    let mut landmarks = [None::<PeakViewPeak>; 16];
    reader.visit_summits_within((position.1, position.0), 100_000, |summit| {
        let mut peak = PeakViewPeak {
            source: summit.source,
            name: PeakName::new(summit.name.as_str()),
            lat: summit.lat,
            lon: summit.lon,
            elevation_m: summit.elevation_m,
            ..PeakViewPeak::EMPTY
        };
        if tested.contains(&peak.name) || out.iter().any(|old| old.name == peak.name) {
            return;
        }
        peak.project(position.0, position.1);
        if !(30..=100_000).contains(&peak.distance_m) {
            return;
        }
        peak.score = apparent_size(&peak, observer_height);
        let slot = &mut landmarks[usize::from(peak.azimuth_q4 / 90)];
        if slot.is_none_or(|old| (peak.elevation_m, rank(&peak)) > (old.elevation_m, rank(&old))) {
            *slot = Some(peak);
        }
    })?;
    for peak in landmarks.iter().flatten() {
        if !out.is_full() && !out.iter().any(|old| old.name == peak.name) {
            let _ = out.push(*peak);
        }
    }
    // A second bounded scan fills all remaining slots, regardless of sector density.
    reader.visit_summits_within((position.1, position.0), 100_000, |summit| {
        let mut peak = PeakViewPeak {
            source: summit.source,
            name: PeakName::new(summit.name.as_str()),
            lat: summit.lat,
            lon: summit.lon,
            elevation_m: summit.elevation_m,
            ..PeakViewPeak::EMPTY
        };
        if tested.contains(&peak.name) || out.iter().any(|old| old.name == peak.name) {
            return;
        }
        peak.project(position.0, position.1);
        if !(30..=100_000).contains(&peak.distance_m) {
            return;
        }
        peak.score = apparent_size(&peak, observer_height);
        if !out.is_full() {
            let _ = out.push(peak);
            return;
        }
        let replace = out
            .iter()
            .enumerate()
            .filter(|(_, old)| !old.visible && !landmarks.iter().flatten().any(|landmark| landmark.name == old.name))
            .min_by_key(|(_, old)| rank(old))
            .map(|(i, _)| i);
        if let Some(i) = replace {
            if rank(&peak) > rank(&out[i]) {
                out[i] = peak;
            }
        }
    })?;
    out.sort_unstable_by_key(|peak| peak.azimuth_q4);
    Ok(())
}

fn rank(peak: &PeakViewPeak) -> (u32, core::cmp::Reverse<u32>, i32, i32) {
    (peak.score, core::cmp::Reverse(peak.distance_m), peak.lat, peak.lon)
}

/// At most two label-only passes, with exact bounded name history.
#[derive(Default)]
pub struct SummitSearch {
    tested: heapless::Vec<PeakName, 128>,
    rounds: u8,
}
impl SummitSearch {
    pub fn refill(
        &mut self,
        builder: &mut surface::Builder,
        reader: &obc_reader::Reader<'_>,
    ) -> Result<bool, obc_reader::Error> {
        if self.rounds == 2 || builder.peaks.iter().all(|peak| peak.visible) {
            return Ok(false);
        }
        self.rounds += 1;
        for peak in &builder.peaks {
            if !self.tested.contains(&peak.name) {
                self.tested.push(peak.name).expect("two bounded candidate passes");
            }
        }
        let mut candidates = Candidates::new();
        candidates.extend(builder.peaks.iter().filter(|peak| peak.visible).copied());
        let retained = candidates.len();
        let profile = builder.profile();
        collect_summits(
            reader,
            (profile.observer_lat, profile.observer_lon),
            profile.observer_elevation_m,
            &self.tested,
            &mut candidates,
        )?;
        builder.refill(&candidates);
        Ok(candidates.len() > retained)
    }
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
        assert_eq!(flat.horizontal_fov_q4(), 360, "the ordinary frame keeps its 90-degree window");
        assert!(!moved((46_000_000, 8_000_000), (46_000_050, 8_000_000)));
        assert!(moved((46_000_000, 8_000_000), (46_000_200, 8_000_000)));
    }

    #[test]
    fn shallow_relief_gets_a_capped_boost_without_changing_horizontal_bearings() {
        let peaks = [PeakViewPeak { elevation_m: Some(1250), distance_m: 16_500, ..PeakViewPeak::EMPTY }];
        let mut profile = PeakViewProfile { peaks: &peaks, ..PeakViewProfile::at(0, 0, 0) };
        profile.set_ground(200.0);
        assert_eq!(profile.vertical_span_q4, 110);
        assert_eq!(profile.horizontal_fov_q4(), 360);
        let bounds = profile.vertical_bounds_q4();
        assert!(bounds.0 < 0 && bounds.1 >= 48, "retain ground below and label space above the horizon");
        profile.default_heading_q4 = 720;
        profile.peaks = &[];
        assert_eq!(profile.detached().vertical_bounds_q4(), bounds, "heading and visibility do not rescale it");
        profile.set_ground(200.0);
        assert_eq!(profile.vertical_span_q4, 266, "missing height metadata does not imply flat terrain");
        let steep = [PeakViewPeak { elevation_m: Some(-500), distance_m: 1000, ..peaks[0] }];
        profile.peaks = &steep;
        profile.set_ground(200.0);
        assert_eq!(profile.vertical_span_q4, 266, "steep terrain below the observer also needs vertical room");
    }

    #[test]
    fn query_fills_dense_sectors_reserves_landmarks_and_skips_tested_names() {
        use obc_formats::io::SliceSource;
        use obcm_testkit::{build_poi_map, PoiSpec};
        let mut records: std::vec::Vec<_> = (0..200)
            .map(|i| PoiSpec {
                lat: 10000 + i * 100,
                lon: 0,
                subtype: 19,
                name: std::format!("Peak {i}"),
                payload: 3200,
            })
            .collect();
        records.push(PoiSpec { lat: 500000, lon: 0, subtype: 19, name: "Landmark".into(), payload: 4634 });
        let bytes = build_poi_map((-100000, -100000, 100000, 600000), 4096, &[(7, records)]);
        let source = SliceSource(&bytes);
        let tables = obc_reader::MapTables::parse(&source).unwrap();
        let cache = std::boxed::Box::new(obc_reader::MapCache::new());
        let reader = obc_reader::Reader::new(&source, &tables, &cache);
        let mut selected = Candidates::new();
        collect_summits(&reader, (0, 0), 3100, &[], &mut selected).unwrap();
        assert_eq!(selected.len(), 64);
        assert!(selected.iter().any(|p| p.name.as_str() == "Landmark"));
        assert!(selected.iter().any(|p| p.name.as_str() == "Peak 0"));
        let mut tested = heapless::Vec::<_, 128>::new();
        tested.extend(selected.iter().map(|p| p.name));
        let mut survivor = selected[0];
        survivor.visible = true;
        selected.clear();
        selected.push(survivor).unwrap();
        collect_summits(&reader, (0, 0), 3100, &tested, &mut selected).unwrap();
        assert_eq!(selected.len(), 64);
        assert!(selected.contains(&survivor));
        assert!(selected.iter().all(|p| p.visible || !tested.contains(&p.name)));
    }

    #[test]
    fn apparent_size_uses_the_observer_height() {
        let hump = PeakViewPeak { elevation_m: Some(3200), distance_m: 2000, ..PeakViewPeak::EMPTY };
        let landmark = PeakViewPeak { elevation_m: Some(4634), distance_m: 10000, ..hump };
        assert!(apparent_size(&landmark, 3100) > apparent_size(&hump, 3100));
        assert!(apparent_size(&hump, 3100) > apparent_size(&hump, 3300));
    }
}
