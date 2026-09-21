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

/// The chart the screen draws is this many pixels wide; [`panorama::ROWS`] is its height.
const CHART_WIDTH: i32 = 240;
const ROWS: i32 = panorama::ROWS as i32;
/// The bottom of every frame. Below about -12 degrees an eye 2 m up sees only its own wheel.
const GROUND_Q4: i32 = -48;
/// A 60-degree arc at the chart's aspect. Wider reads as a map; narrower magnifies the panorama.
const BASE_SPAN_Q4: i32 = 60 * 4 * ROWS / CHART_WIDTH;
/// Room above the highest summit for its label.
const LABEL_HEADROOM_Q4: i32 = 24;
/// A summit almost overhead still has to leave a drawable window.
const MAX_TOP_Q4: i32 = 340;

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
            fov_q4: (BASE_SPAN_Q4 * CHART_WIDTH / ROWS) as u16,
            vertical_centre_q4: (GROUND_Q4 + BASE_SPAN_Q4 / 2) as i16,
            vertical_span_q4: BASE_SPAN_Q4 as u16,
            peaks: &[],
        }
    }

    /// Copy framing and observer data without retaining a borrowed candidate list.
    pub fn detached(&self) -> PeakViewProfile<'static> {
        PeakViewProfile { peaks: &[], ..*self }
    }

    /// Frame the observer's relief once; turning keeps the same scale and horizon position.
    ///
    /// The window is anchored at [`GROUND_Q4`] and grows upwards until the highest summit and its
    /// label fit. The horizontal field follows the vertical span at the chart's own aspect, so a
    /// degree is the same number of pixels on both axes and a summit keeps its shape.
    pub fn set_ground(&mut self, ground_m: f32) {
        self.observer_elevation_m = libm::roundf(ground_m + 2.0) as i16;
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
            .fold(0.0f32, f32::max);
        let wanted = libm::ceilf(highest) as i32 + LABEL_HEADROOM_Q4 - GROUND_Q4;
        let span = wanted.clamp(BASE_SPAN_Q4, MAX_TOP_Q4 - GROUND_Q4);
        self.vertical_span_q4 = span as u16;
        self.vertical_centre_q4 = (GROUND_Q4 + span / 2) as i16;
        self.fov_q4 = ((span * CHART_WIDTH + ROWS / 2) / ROWS) as u16;
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

/// The height the observer's eye stands on, before the 2 m of rider.
///
/// The lattice has two answers and both are wrong inside the cell of a crest: the bilinear surface
/// (`ground`) runs below two lifted nodes, and the cell's highest corner (`cell_top`) puts every
/// position in the cell on the summit. One cell can hold 57 m of relief, so a settled
/// map-referenced altimeter, which resolves single metres, decides whenever it lands inside the
/// band the lattice itself allows.
pub fn eye_ground(ground: f32, cell_top: f32, measured: Option<f32>) -> f32 {
    const SLACK_M: f32 = 30.0;
    match measured {
        Some(height) if (ground - SLACK_M..=cell_top + SLACK_M).contains(&height) => height,
        _ => cell_top,
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
        assert_eq!(bottom, GROUND_Q4);
        assert!(top >= 204, "nearby 45-degree terrain fits with label headroom");
        assert_eq!(profile.observer_elevation_m, 1002);
        let mut flat = PeakViewProfile::at(46_000_000, 8_000_000, 0);
        flat.set_ground(1000.0);
        assert_eq!(flat.horizontal_fov_q4(), 240, "the ordinary frame keeps its 60-degree window");
        assert!(!moved((46_000_000, 8_000_000), (46_000_050, 8_000_000)));
        assert!(moved((46_000_000, 8_000_000), (46_000_200, 8_000_000)));
    }

    /// A degree must be the same number of pixels on both axes, or every summit is drawn with the
    /// wrong shape. The chart is `CHART_WIDTH` by `ROWS` pixels wide and tall.
    fn square_pixels(profile: &PeakViewProfile) -> bool {
        let (bottom, top) = profile.vertical_bounds_q4();
        let horizontal = profile.horizontal_fov_q4() as f32 / CHART_WIDTH as f32;
        let vertical = (top - bottom) as f32 / ROWS as f32;
        (horizontal - vertical).abs() < 0.01
    }

    #[test]
    fn the_frame_sits_on_the_ground_angle_and_opens_upwards_for_a_high_summit() {
        let peaks = [PeakViewPeak { elevation_m: Some(1250), distance_m: 16_500, ..PeakViewPeak::EMPTY }];
        let mut profile = PeakViewProfile { peaks: &peaks, ..PeakViewProfile::at(0, 0, 0) };
        profile.set_ground(200.0);
        assert_eq!(
            profile.vertical_bounds_q4(),
            (GROUND_Q4, GROUND_Q4 + BASE_SPAN_Q4),
            "distant relief keeps the base window"
        );
        assert_eq!(profile.horizontal_fov_q4(), 240);
        assert!(square_pixels(&profile));
        let base = profile.vertical_bounds_q4();

        profile.default_heading_q4 = 720;
        profile.peaks = &[];
        assert_eq!(profile.detached().vertical_bounds_q4(), base, "heading and visibility do not rescale it");
        profile.set_ground(200.0);
        assert_eq!(profile.vertical_bounds_q4(), base, "an empty catalogue is not a reason to zoom");

        let below = [PeakViewPeak { elevation_m: Some(-500), distance_m: 1000, ..peaks[0] }];
        profile.peaks = &below;
        profile.set_ground(200.0);
        assert_eq!(profile.vertical_bounds_q4(), base, "the bottom of the frame never follows a summit");

        let overhead = [PeakViewPeak { elevation_m: Some(3000), distance_m: 1000, ..peaks[0] }];
        profile.peaks = &overhead;
        profile.set_ground(200.0);
        let (bottom, top) = profile.vertical_bounds_q4();
        assert_eq!(bottom, GROUND_Q4, "opening the frame keeps the same horizon position");
        assert!(top >= 282 + LABEL_HEADROOM_Q4, "a 70-degree summit fits with its label");
        assert!(square_pixels(&profile));
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

    /// Both positions share one Rigidalstock cell, 45 m apart: the cell top is 2592 m and the
    /// bilinear surface under the rider is 2588 m, while 2 m swissALTI3D measures 2564.4 m below
    /// the summit and 2591.4 m on it. Only a measurement can separate them.
    #[test]
    fn a_plausible_measurement_decides_the_eye_and_an_implausible_one_is_ignored() {
        assert_eq!(eye_ground(2588.0, 2592.0, Some(2564.4)), 2564.4);
        assert_eq!(eye_ground(2588.0, 2592.0, Some(2591.4)), 2591.4);
        assert_eq!(eye_ground(2588.0, 2592.0, Some(2400.0)), 2592.0, "outside the band the cell decides");
        assert_eq!(eye_ground(2588.0, 2592.0, None), 2592.0, "an unsettled altimeter offers nothing");
    }

    #[test]
    fn apparent_size_uses_the_observer_height() {
        let hump = PeakViewPeak { elevation_m: Some(3200), distance_m: 2000, ..PeakViewPeak::EMPTY };
        let landmark = PeakViewPeak { elevation_m: Some(4634), distance_m: 10000, ..hump };
        assert!(apparent_size(&landmark, 3100) > apparent_size(&hump, 3100));
        assert!(apparent_size(&hump, 3100) > apparent_size(&hump, 3300));
    }
}
