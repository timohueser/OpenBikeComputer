//! Real-location Peak View fixtures for fast simulator UI iteration.
//!
//! Generated 2026-08-30 from AWS Terrarium elevation tiles (zoom 13 within 5 km, 12 within 40 km,
//! 11 to the 100 km ray limit) and OSM Overpass peak nodes. Each distance band casts 1440 rays
//! (0.25-degree spacing, starting 200 m out so the observer's own DEM cell cannot form a phantom
//! foreground) with earth curvature at refraction k = 0.13, then max-pools into the stored
//! 2-degree samples so narrow summits such as the Matterhorn keep their full apparent height
//! and silhouettes keep their shoulders instead of turning into linear-interpolation facets.
//! Each named summit's catalog angle is spliced into its band so a catalog peak is never below
//! its own rendered ridge. The screen interpolates the samples linearly. These fixtures are
//! host-only and intentionally define no device storage or OBCM contract.

use obc_app::{PeakViewPeak, PeakViewProfile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Preset {
    Gornergrat,
    KleineScheidegg,
    Grossglockner,
}

impl Preset {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "gornergrat" => Ok(Self::Gornergrat),
            "scheidegg" | "kleine-scheidegg" => Ok(Self::KleineScheidegg),
            "glockner" | "grossglockner" => Ok(Self::Grossglockner),
            _ => Err("--peak-view needs gornergrat|scheidegg|glockner".into()),
        }
    }

    pub(crate) fn profile(self) -> &'static PeakViewProfile {
        match self {
            Self::Gornergrat => &GORNERGRAT,
            Self::KleineScheidegg => &KLEINE_SCHEIDEGG,
            Self::Grossglockner => &GROSSGLOCKNER,
        }
    }
}

static GORNERGRAT_NEAR: [i16; 180] = [
    -57, -54, -51, -49, -45, -43, -40, -37, -35, -32, -30, -28, -26, -24, -22, -20, -18, -16, -14, -12, -10, -8, -6,
    -4, -2, 0, 2, 5, 9, 14, 19, 23, 30, 25, 23, 19, 18, 15, 14, 16, 18, 21, 21, 18, 13, 7, 1, -4, -9, -16, -17, -16,
    -16, -16, -16, -16, -16, -16, -17, -18, -18, -17, -18, -20, -23, -30, -35, -38, -39, -39, -41, -41, -40, -39, -38,
    -37, -36, -35, -35, -35, -35, -35, -35, -29, -23, -22, -22, -23, -23, -21, -17, -17, -18, -22, -27, -29, -29, -30,
    -32, -33, -32, -33, -34, -35, -35, -37, -38, -37, -37, -38, -37, -35, -35, -35, -36, -37, -39, -41, -43, -44, -49,
    -52, -52, -49, -46, -42, -39, -36, -34, -32, -30, -29, -27, -27, -26, -26, -26, -26, -27, -28, -29, -30, -31, -33,
    -36, -38, -40, -41, -42, -42, -42, -42, -42, -42, -42, -43, -43, -43, -43, -44, -44, -44, -47, -50, -51, -55, -56,
    -56, -56, -56, -57, -58, -58, -58, -58, -59, -59, -60, -60, -59,
];
static GORNERGRAT_MIDDLE: [i16; 180] = [
    -17, -14, -11, -7, -3, 1, 4, 3, 3, 5, 7, 11, 11, 9, 6, 5, 4, 5, 7, 7, 5, 2, 1, 3, 4, 4, 6, 8, 8, 7, 3, 0, 3, 3, -1,
    0, 0, 5, 9, 13, 17, 21, 22, 25, 20, 14, 13, 14, 14, 14, 14, 13, 12, 12, 13, 13, 12, 16, 29, 33, 34, 34, 26, 27, 28,
    28, 27, 25, 20, 21, 16, 14, 14, 14, 12, 14, 23, 31, 37, 38, 37, 31, 25, 21, 23, 24, 25, 27, 27, 31, 37, 34, 28, 25,
    32, 37, 37, 39, 45, 38, 37, 39, 44, 42, 40, 41, 44, 40, 32, 24, 21, 20, 20, 18, 12, 5, 3, 3, 3, 3, 2, 2, 2, 1, 0,
    -1, -3, -4, -5, -6, -7, -8, -8, -10, -13, -16, -16, -15, -15, -16, -19, -21, -22, -24, -26, -29, -33, -37, -37,
    -29, -21, -17, -16, -16, -16, -16, -17, -19, -25, -31, -29, -23, -21, -22, -23, -25, -31, -37, -39, -41, -41, -46,
    -45, -44, -41, -37, -32, -29, -25, -21,
];
static GORNERGRAT_FAR: [i16; 180] = [
    1, 0, 0, 3, 1, 3, 4, 6, 9, 12, 16, 18, 23, 24, 22, 18, 21, 23, 23, 19, 17, 16, 16, 16, 15, 18, 17, 16, 22, 25, 28,
    21, 18, 22, 24, 26, 19, 18, 15, 10, 10, 12, 12, 12, 15, 18, 19, 15, 15, 16, 16, 15, 14, 16, 17, 15, 16, 18, 26, 33,
    37, 38, 40, 41, 39, 43, 38, 36, 30, 29, 28, 28, 30, 33, 35, 41, 40, 42, 43, 43, 39, 36, 34, 32, 31, 30, 33, 35, 37,
    31, 26, 24, 23, 22, 20, 18, 17, 15, 15, 17, 17, 16, 19, 22, 25, 25, 25, 24, 22, 23, 24, 23, 21, 19, 12, 9, 8, 7, 8,
    10, 9, 9, 10, 9, 8, 10, 9, 12, 11, 11, 11, 14, 25, 32, 24, 14, 8, 7, 6, 7, 6, 6, 8, 9, 12, 14, 18, 18, 11, 11, 11,
    15, 18, 15, 16, 13, 11, 10, 14, 15, 19, 18, 14, 12, 12, 13, 14, 13, 18, 21, 17, 13, 10, 8, 9, 9, 4, 2, 1, 1,
];
static GORNERGRAT_PEAKS: [PeakViewPeak; 18] = [
    peak("Rimpfischhorn", 4198, 8804, 240, 28, 64100, 2),
    peak("Hohtälli", 3286, 1442, 257, 30, 5445, 0),
    peak("Strahlhorn", 4190, 9610, 279, 26, 7507, 2),
    peak("Rote Nase", 3247, 2131, 315, 16, 2653, 0),
    peak("Stockhorn", 3532, 4058, 346, 25, 12422, 1),
    peak("Cima di Jazzi", 3803, 8435, 368, 19, 12068, 2),
    peak("Torre Castelfranco", 3627, 7361, 397, 16, 3934, 2),
    peak("Grosses Fillarh.", 3676, 7724, 428, 17, 4486, 2),
    peak("Dufourspitze", 4634, 8138, 518, 43, 216500, 2),
    peak("Liskamm East", 4527, 7806, 601, 41, 37600, 2),
    peak("Castor", 4228, 6982, 700, 37, 16500, 2),
    peak("Pollux", 4092, 6182, 720, 37, 3653, 1),
    peak("Breithorn East", 4139, 5296, 782, 45, 3360, 1),
    peak("Breithorn Central", 4159, 5433, 816, 44, 3086, 1),
    peak("Breithorn West", 4164, 5539, 847, 44, 17105, 1),
    peak("Furggen", 3492, 8572, 999, 10, 9052, 2),
    peak("Punta Giordano", 3878, 15136, 1016, 12, 10755, 2),
    peak("Matterhorn", 4478, 9828, 1062, 32, 103800, 2),
];
static GORNERGRAT: PeakViewProfile = PeakViewProfile {
    id: 1,
    name: "Gornergrat",
    observer_lat: 45_983_400,
    observer_lon: 7_785_400,
    observer_elevation_m: 3095,
    default_heading_q4: 940,
    sample_step_q4: 8,
    angle_bottom_q4: -44,
    angle_top_q4: 113,
    layers_q4: [&GORNERGRAT_NEAR, &GORNERGRAT_MIDDLE, &GORNERGRAT_FAR],
    peaks: &GORNERGRAT_PEAKS,
};

static SCHEIDEGG_NEAR: [i16; 180] = [
    17, 15, 13, 10, 8, 6, 4, 3, 1, 0, -2, -5, -7, -9, -11, -14, -16, -19, -21, -23, -24, -25, -26, -26, -29, -29, -30,
    -29, -29, -29, -28, -27, -24, -18, -14, -9, -1, 9, 17, 25, 32, 39, 40, 44, 51, 56, 66, 77, 87, 97, 108, 112, 115,
    112, 109, 106, 102, 98, 95, 93, 99, 94, 92, 92, 92, 94, 97, 102, 107, 105, 104, 101, 96, 90, 86, 78, 74, 71, 70,
    71, 71, 72, 74, 74, 74, 76, 79, 82, 82, 86, 83, 81, 76, 72, 69, 74, 70, 68, 64, 60, 56, 51, 42, 33, 29, 29, 28, 28,
    27, 25, 23, 19, 16, 13, 16, 19, 22, 24, 27, 29, 32, 34, 35, 37, 38, 40, 42, 43, 45, 47, 48, 49, 50, 51, 52, 54, 55,
    55, 56, 57, 58, 59, 61, 62, 62, 63, 64, 64, 64, 65, 65, 66, 66, 66, 66, 71, 61, 58, 53, 52, 51, 50, 49, 47, 46, 48,
    55, 49, 46, 41, 38, 36, 35, 33, 31, 29, 27, 25, 22, 20,
];
static SCHEIDEGG_MIDDLE: [i16; 180] = [
    7, 11, 12, 12, 12, 10, 11, 11, 13, 12, 13, 15, 16, 13, 10, 10, 11, 11, 12, 14, 11, 9, 6, 3, 2, -1, -1, -1, 1, 11,
    22, 25, 25, 27, 25, 24, 22, 23, 24, 24, 29, 28, 32, 34, 36, 33, 32, 26, 25, 23, 23, 26, 29, 31, 37, 41, 44, 50, 50,
    49, 46, 43, 42, 42, 41, 43, 45, 47, 50, 50, 49, 47, 45, 43, 40, 34, 31, 30, 31, 35, 39, 43, 49, 51, 51, 51, 51, 51,
    50, 50, 49, 48, 49, 48, 46, 44, 41, 40, 38, 32, 31, 29, 24, 24, 27, 27, 21, 19, 23, 21, 16, 14, 19, 20, 20, 20, 22,
    21, 18, 16, 16, 12, 11, 14, 15, 17, 20, 19, 17, 17, 17, 20, 20, 17, 16, 16, 14, 14, 14, 13, 8, 9, 9, 7, 6, 6, 7, 8,
    8, 5, 0, 0, 0, -3, -6, -6, -4, -2, -2, -1, -1, -1, -1, -2, -2, -2, -1, -1, -2, -3, -2, -1, -1, 1, 2, 2, 2, 4, 5, 7,
];
static SCHEIDEGG_FAR: [i16; 180] = [
    -1, -1, -2, -3, -4, -2, -1, -1, -1, 0, 1, 0, 1, -1, -1, -3, -4, -4, -2, -1, -1, 0, 1, 2, 3, 3, 3, 2, 3, 4, 5, 4, 4,
    6, 5, 7, 8, 8, 9, 9, 9, 9, 8, 8, 7, 6, 7, 6, 6, 5, 6, 7, 7, 6, 5, 4, 4, 5, 5, 7, 7, 8, 9, 7, 7, 7, 7, 5, 5, 5, 4,
    4, 6, 6, 5, 7, 5, 5, 7, 6, 6, 7, 7, 7, 5, 5, 4, 6, 8, 7, 5, 5, 6, 6, 9, 9, 7, 5, 6, 8, 8, 8, 10, 10, 10, 8, 4, 4,
    4, 4, 5, 9, 10, 8, 13, 13, 11, 8, 6, 6, 6, 4, 6, 6, 3, 4, 3, 3, 3, 2, 1, 3, 3, 2, 2, 2, 1, 2, 1, 2, 2, 2, 0, 2, 0,
    0, -1, -3, -4, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -3, -4, -4, -4, -4, -4,
    -4, -4, -4, -4, -4, -1,
];
static SCHEIDEGG_PEAKS: [PeakViewPeak; 13] = [
    peak("Indri Sägissa", 2462, 9333, 5, 10, 4990, 1),
    peak("Reeti", 2757, 9692, 94, 16, 13106, 1),
    peak("Schwarzhoren", 2927, 14249, 152, 14, 15483, 1),
    peak("Läuber", 2491, 29992, 208, 3, 16210, 2),
    peak("Mittelhorn", 3704, 13694, 264, 27, 17091, 1),
    peak("Eiger", 3970, 3496, 414, 115, 36400, 0),
    peak("Chlyne Eiger", 3467, 3064, 477, 99, 3458, 0),
    peak("Mönch", 4107, 4045, 547, 107, 58900, 0),
    peak("Jungfrau", 4158, 5364, 715, 86, 68700, 0),
    peak("Silberhorn", 3690, 4873, 763, 74, 4161, 0),
    peak("Blüemlisalphorn", 3663, 17936, 934, 20, 29675, 1),
    peak("Lauberhorn", 2472, 1297, 1241, 71, 2135, 0),
    peak("Tschuggen", 2520, 1905, 1331, 55, 9239, 0),
];
static KLEINE_SCHEIDEGG: PeakViewProfile = PeakViewProfile {
    id: 2,
    name: "Kleine Scheidegg",
    observer_lat: 46_585_000,
    observer_lon: 7_961_000,
    observer_elevation_m: 2056,
    default_heading_q4: 565,
    sample_step_q4: 8,
    angle_bottom_q4: -20,
    angle_top_q4: 172,
    layers_q4: [&SCHEIDEGG_NEAR, &SCHEIDEGG_MIDDLE, &SCHEIDEGG_FAR],
    peaks: &SCHEIDEGG_PEAKS,
};

static GLOCKNER_NEAR: [i16; 180] = [
    110, 114, 118, 119, 120, 124, 127, 130, 132, 134, 136, 138, 141, 142, 143, 144, 142, 139, 137, 137, 140, 143, 143,
    142, 139, 136, 136, 136, 136, 136, 135, 134, 133, 130, 127, 123, 117, 112, 107, 103, 99, 95, 91, 87, 81, 76, 72,
    68, 65, 62, 56, 50, 44, 39, 34, 28, 22, 20, 18, 16, 12, 7, 4, 0, -4, -8, -13, -20, -26, -26, -23, -16, -9, -1, 4,
    6, 7, 5, 6, 9, 12, 16, 18, 20, 23, 26, 32, 36, 39, 42, 44, 47, 50, 50, 48, 48, 48, 50, 52, 53, 55, 57, 60, 63, 63,
    63, 62, 60, 59, 58, 57, 58, 62, 63, 63, 63, 64, 63, 63, 66, 68, 69, 69, 68, 63, 65, 66, 62, 58, 57, 57, 56, 57, 58,
    57, 56, 53, 50, 38, 29, 23, 21, 17, 12, 11, 11, 10, 3, -4, -4, -4, -4, -4, -3, 6, 13, 16, 17, 18, 19, 25, 29, 33,
    36, 39, 44, 48, 52, 56, 62, 68, 72, 77, 83, 88, 92, 98, 101, 100, 104,
];
static GLOCKNER_MIDDLE: [i16; 180] = [
    28, 28, 22, 15, 11, 9, 4, -1, -6, -3, 0, 3, 10, 14, 14, 18, 17, 14, 10, 6, 4, 9, 14, 16, 16, 20, 25, 29, 36, 37,
    35, 33, 30, 24, 20, 17, 16, 17, 17, 14, 15, 15, 14, 21, 22, 20, 22, 22, 22, 20, 13, 8, 3, -2, -5, 1, 1, -2, -5, -9,
    -12, -14, -15, -18, -11, -5, -2, 1, 2, 3, 5, 5, 8, 9, 10, 12, 14, 16, 17, 17, 16, 15, 13, 12, 13, 15, 15, 15, 17,
    19, 20, 17, 18, 19, 19, 19, 18, 18, 18, 15, 14, 14, 15, 15, 16, 14, 13, 16, 18, 20, 21, 24, 25, 24, 24, 29, 31, 32,
    33, 31, 31, 30, 31, 32, 34, 36, 41, 42, 43, 49, 57, 59, 60, 61, 66, 69, 65, 60, 58, 56, 53, 47, 43, 41, 37, 33, 29,
    24, 25, 28, 30, 33, 33, 29, 25, 25, 27, 27, 22, 27, 29, 30, 30, 32, 32, 34, 33, 32, 34, 34, 36, 38, 36, 34, 32, 32,
    33, 32, 27, 27,
];
static GLOCKNER_FAR: [i16; 180] = [
    12, 17, 16, 13, 12, 9, 7, 5, 1, 0, 0, 0, 0, 0, 1, 2, 2, 2, 3, 6, 5, 2, -1, -3, -2, -1, -2, 0, 4, 5, 5, 7, 6, 6, 4,
    2, 3, 2, 5, 8, 7, 7, 9, 13, 13, 14, 12, 10, 11, 9, 9, 9, 11, 11, 10, 9, 6, 5, 4, 5, 5, 4, 3, 2, 1, 3, 2, 1, 1, 0,
    1, 1, 1, 0, 1, 3, 4, 7, 11, 9, 10, 10, 13, 13, 11, 15, 15, 11, 12, 13, 9, 11, 12, 10, 12, 14, 10, 12, 12, 11, 10,
    7, 7, 6, 6, 4, 3, 3, 2, 3, 3, 2, 2, 3, 5, 4, 2, 3, 2, 1, 2, 3, 2, 3, 5, 9, 12, 11, 12, 11, 8, 11, 12, 13, 14, 15,
    12, 10, 12, 12, 10, 9, 6, 7, 9, 12, 11, 11, 10, 11, 8, 8, 8, 8, 6, 4, 3, 2, 0, -2, -2, 1, 2, 3, 5, 7, 8, 9, 9, 13,
    12, 7, 4, 2, -1, -1, -1, 3, 8, 12,
];
static GLOCKNER_PEAKS: [PeakViewPeak; 12] = [
    peak("Freiwandkopf", 2854, 685, 74, 134, 2155, 0),
    peak("Magernigspitz", 2640, 22289, 498, 2, 5340, 2),
    peak("Erster Leiterkopf", 2483, 2701, 606, 7, 3733, 0),
    peak("Karlkamp", 3114, 9355, 630, 17, 11406, 1),
    peak("Leiterkopf", 2891, 2190, 741, 50, 2735, 0),
    peak("Schwertkopf", 3099, 2452, 831, 63, 3622, 0),
    peak("Schwerteck", 3247, 2972, 920, 63, 3544, 0),
    peak("Kellerskopf", 3239, 2684, 972, 69, 1556, 0),
    peak("Kellersberg", 3265, 2924, 1006, 66, 1842, 0),
    peak("Großglockner", 3798, 4477, 1080, 69, 242800, 1),
    peak("Johannisberg", 3453, 7173, 1211, 33, 5748, 1),
    peak("Mittlerer Burgstall", 2933, 4466, 1250, 27, 6735, 1),
];
static GROSSGLOCKNER: PeakViewProfile = PeakViewProfile {
    id: 3,
    name: "Kaiser-Franz-Josefs-Höhe",
    observer_lat: 47_074_500,
    observer_lon: 12_753_000,
    observer_elevation_m: 2402,
    default_heading_q4: 1000,
    sample_step_q4: 8,
    angle_bottom_q4: -24,
    angle_top_q4: 152,
    layers_q4: [&GLOCKNER_NEAR, &GLOCKNER_MIDDLE, &GLOCKNER_FAR],
    peaks: &GLOCKNER_PEAKS,
};

const fn peak(
    name: &'static str,
    elevation_m: u16,
    distance_m: u32,
    azimuth_q4: u16,
    angle_q4: i16,
    score: u32,
    layer: u8,
) -> PeakViewPeak {
    PeakViewPeak { name, elevation_m, distance_m, azimuth_q4, angle_q4, layer, score }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_three_complete_horizons_and_sorted_peaks() {
        for preset in [Preset::Gornergrat, Preset::KleineScheidegg, Preset::Grossglockner] {
            let profile = preset.profile();
            assert_eq!(profile.sample_step_q4 as usize * profile.layers_q4[0].len(), 360 * 4);
            assert!(profile.layers_q4.iter().all(|layer| layer.len() == profile.layers_q4[0].len()));
            assert!(profile.peaks.windows(2).all(|pair| pair[0].azimuth_q4 <= pair[1].azimuth_q4));
            assert!((0..3).all(|layer| profile.peaks.iter().any(|peak| peak.layer == layer)));
            assert!(profile.peaks.iter().any(|peak| {
                let delta =
                    (i32::from(peak.azimuth_q4) - i32::from(profile.default_heading_q4) + 720).rem_euclid(1440) - 720;
                delta.abs() > 60 * 4
            }));
        }
    }
}
