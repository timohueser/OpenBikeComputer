//! The drawn Peak View skyline against seven photographs of the Engelberg mountains.
//!
//! This is the only ground truth Peak View has. The owner stood at four positions near Engelberg
//! and photographed the mountains; each photograph's skyline was registered on the 2 m swissALTI3D
//! skyline once, and the columns that survived — the bearings where the reference agrees with the
//! photograph within half a degree — are recorded in the fixture package as
//! `(bearing, elevation)` pairs on a quarter-degree grid. `fixtures/sources/peak-view/photos/`
//! holds the same files, and the package README records how they were made.
//!
//! The test draws the whole panorama with the production [`Builder`] over the package's terrain
//! shard, at the recorded position with the eye 1.6 m above the swissALTI3D ground — the height the
//! phone was held at, not the 2 m of rider the product assumes — and measures the root-mean-square
//! difference between the drawn skyline and the photograph's, bearing by bearing.
//!
//! Each limit is the measured value plus 0.05 degrees. A limit is therefore a **pin**, not an
//! accuracy target: it holds the surface, the crest lift rule and the observer's eye where the
//! photographs say they are now, and a change that moves any of them has to say why.

#![cfg(feature = "external-fixtures")]

use obc_app::peak_view::{
    panorama::{COLUMNS, ROWS},
    surface::Builder,
    terrain::Terrain,
    PeakViewProfile,
};
use obc_formats::io::{ByteSource, SliceSource};

/// The seven photographs, in the order the README lists them.
const VIEWS: [&str; 7] =
    ["hahnen", "rigidalstock-from-brunni", "urnerstaffel", "below-titlis", "below-west", "top-south", "top-north"];

/// The height above the ground the photographs were taken from.
const PHONE_M: f32 = 1.6;
/// One panorama column, in degrees of bearing.
const COLUMN_DEG: f32 = 360.0 / COLUMNS as f32;

/// One photograph's recorded skyline.
struct Photo {
    lat: i32,
    lon: i32,
    eye_m: i16,
    heading_q4: u16,
    columns: Vec<(f32, f32)>,
    limit_deg: f32,
}

fn photo(name: &str) -> Photo {
    let bytes = obc_fixtures::read("peak-view-photos", format!("photos/{name}.json"));
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("a recorded skyline is JSON");
    let number = |key: &str| value[key].as_f64().unwrap_or_else(|| panic!("{name}.json has no {key}"));
    let ground_m = number("ground_m") as f32;
    let columns: Vec<(f32, f32)> = value["columns"]
        .as_array()
        .expect("columns is an array")
        .iter()
        .map(|pair| (pair[0].as_f64().unwrap() as f32, pair[1].as_f64().unwrap() as f32))
        .collect();
    assert!(columns.len() >= 30, "{name} has only {} confirmed columns, which measures nothing", columns.len());
    Photo {
        lat: (number("lat") * 1e6).round() as i32,
        lon: (number("lon") * 1e6).round() as i32,
        // The device's eye is a whole number of metres, which is half a metre of slack at worst —
        // 0.03 degrees a kilometre out, and less than that everywhere these skylines are.
        eye_m: (ground_m + PHONE_M).round() as i16,
        heading_q4: (number("heading_deg") * 4.0).round() as u16 % 1440,
        columns,
        limit_deg: number("rms_limit_deg") as f32,
    }
}

/// The whole circle, drawn the way the device draws it.
fn draw(terrain: &mut Terrain<'_>, photo: &Photo) -> Builder {
    let mut profile = PeakViewProfile::at(photo.lat, photo.lon, photo.eye_m);
    profile.default_heading_q4 = photo.heading_q4;
    let mut builder = Builder::new(&profile);
    // The device spends a budget per frame; a test only has to reach the same end state.
    for _ in 0..10_000 {
        if builder.complete() {
            return builder;
        }
        builder.step(terrain, 4096);
    }
    panic!("the panorama did not finish");
}

/// The elevation of the drawn skyline in a panorama column, in degrees.
///
/// The row the terrain reached is the first row below the skyline, so the skyline itself lies
/// between that row's centre and the centre of the row above it — their shared edge.
fn column_elevation(builder: &Builder, profile: &PeakViewProfile, column: usize) -> f32 {
    let (bottom, top) = profile.vertical_bounds_q4();
    let per_row = (top - bottom) as f32 / ROWS as f32;
    (top as f32 - builder.skyline_row(column) as f32 * per_row) / 4.0
}

/// The drawn skyline at an arbitrary bearing: the two columns either side of it, interpolated.
fn elevation_at(builder: &Builder, profile: &PeakViewProfile, bearing_deg: f32) -> f32 {
    let position = bearing_deg.rem_euclid(360.0) / COLUMN_DEG;
    let low = position.floor();
    let fraction = position - low;
    let low = low as usize;
    let a = column_elevation(builder, profile, low);
    let b = column_elevation(builder, profile, low + 1);
    a + (b - a) * fraction
}

#[test]
fn the_drawn_skyline_matches_the_engelberg_photographs() {
    let bytes = obc_fixtures::read("peak-view-photos", "engelberg.obcd");
    let source = SliceSource(&bytes);
    let mut terrain = Terrain::parse(&source as &dyn ByteSource).expect("the packaged shard parses");

    let mut failures = Vec::new();
    for name in VIEWS {
        let photo = photo(name);
        let builder = draw(&mut terrain, &photo);
        let profile = builder.profile();
        let squares: f32 = photo
            .columns
            .iter()
            .map(|&(bearing, elevation)| (elevation_at(&builder, &profile, bearing) - elevation).powi(2))
            .sum();
        let rms = (squares / photo.columns.len() as f32).sqrt();
        println!("{name:26} rms {rms:.3} deg over {} columns, limit {:.2}", photo.columns.len(), photo.limit_deg);
        if rms > photo.limit_deg {
            failures.push(format!("{name}: rms {rms:.3} deg is past its {:.2} deg limit", photo.limit_deg));
        }
    }
    assert!(failures.is_empty(), "the drawn skyline moved away from the photographs:\n  {}", failures.join("\n  "));
    assert!(!terrain.failed(), "a storage read failed part way through");
}
