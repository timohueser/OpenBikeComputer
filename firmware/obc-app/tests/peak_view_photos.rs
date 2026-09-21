//! The drawn Peak View skyline against six photographs of the Engelberg mountains.
//!
//! This is the only ground truth Peak View has. Each photograph's skyline was registered on the 2 m
//! swissALTI3D skyline once, and the columns that survived — the bearings where the reference
//! agrees with the photograph within half a degree — are recorded in the fixture package as
//! `(bearing, elevation)` pairs on a quarter-degree grid. The package README records how they were
//! made. The test draws the whole panorama with the production [`Builder`] over the package's
//! terrain shard, at the recorded position, and measures the root-mean-square difference between
//! the drawn skyline and the photograph's, bearing by bearing.
//!
//! The eye comes from the photographs, not from the product: it stands 1.6 m above the swissALTI3D
//! ground, the height the phone was held at, which the device does not have. So this suite measures
//! the surface and the traversal over it, and deliberately does not exercise
//! [`obc_app::peak_view::eye_ground`], which has unit tests of its own.
//!
//! Each limit is the measured value plus 0.05 degrees, so a limit is a pin rather than an accuracy
//! target: it holds the surface and the crest lift rule where the photographs say they are, and a
//! change that moves either has to say why. The shard's digest is pinned with them, because a
//! repacked shard would move every number at once and silently.

#![cfg(feature = "external-fixtures")]

use obc_app::peak_view::{
    panorama::{COLUMNS, ROWS},
    surface::Builder,
    terrain::Terrain,
    PeakViewProfile,
};
use obc_formats::io::{ByteSource, SliceSource};

/// The six photographs, in the order the README lists them.
const VIEWS: [&str; 6] =
    ["hahnen", "rigidalstock-from-brunni", "urnerstaffel", "below-titlis", "below-west", "top-south"];

/// The shard every limit was measured on.
const SHARD_SHA256: &str = "f49c570cf0cfa4c37408f5e5183c07f25cd9f65fc878015622cc881c11ce233c";

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

/// The whole circle, drawn the way the device draws it, and the row terrain reached in each of its
/// columns.
///
/// `Builder` keeps only the last finished sector's skyline, because it sits in the device's
/// panorama arena and a full circle of rows would spend most of the space that arena has left, so
/// the circle is collected a sector at a time. One unit of work per `step` is what makes that
/// exact: a unit finishes at most one sector, and the same call begins the next one.
fn draw(terrain: &mut Terrain<'_>, photo: &Photo) -> (PeakViewProfile<'static>, [u8; COLUMNS]) {
    let mut profile = PeakViewProfile::at(photo.lat, photo.lon, photo.eye_m);
    profile.default_heading_q4 = photo.heading_q4;
    let mut builder = Builder::new(&profile);
    let mut rows = [ROWS as u8; COLUMNS];
    let mut collected = 0u64;
    while !builder.complete() {
        builder.step(terrain, 1);
        if builder.finished_sectors() == collected {
            continue;
        }
        collected = builder.finished_sectors();
        let (column, sector) = builder.last_sector_skyline();
        for (offset, &row) in sector.iter().enumerate() {
            rows[(column + offset) % COLUMNS] = row;
        }
    }
    (builder.profile(), rows)
}

/// The elevation of the drawn skyline in a panorama column, in degrees.
///
/// The row terrain reached is the first row below the skyline, so the skyline itself lies between
/// that row's centre and the centre of the row above it — their shared edge.
fn column_elevation(rows: &[u8; COLUMNS], profile: &PeakViewProfile, column: usize) -> f32 {
    let (bottom, top) = profile.vertical_bounds_q4();
    let per_row = (top - bottom) as f32 / ROWS as f32;
    (top as f32 - f32::from(rows[column % COLUMNS]) * per_row) / 4.0
}

/// The drawn skyline at an arbitrary bearing: the two columns either side of it, interpolated.
fn elevation_at(rows: &[u8; COLUMNS], profile: &PeakViewProfile, bearing_deg: f32) -> f32 {
    let position = bearing_deg.rem_euclid(360.0) / COLUMN_DEG;
    let low = position.floor();
    let fraction = position - low;
    let low = low as usize;
    let a = column_elevation(rows, profile, low);
    let b = column_elevation(rows, profile, low + 1);
    a + (b - a) * fraction
}

#[test]
fn the_drawn_skyline_matches_the_engelberg_photographs() {
    let bytes = obc_fixtures::read("peak-view-photos", "engelberg.obcd");
    use sha2::Digest;
    let digest: String = sha2::Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(digest, SHARD_SHA256, "the packaged shard is not the one the limits were measured on");
    let source = SliceSource(&bytes);
    let mut terrain = Terrain::parse(&source as &dyn ByteSource).expect("the packaged shard parses");

    let mut failures = Vec::new();
    for name in VIEWS {
        let photo = photo(name);
        let (profile, rows) = draw(&mut terrain, &photo);
        let squares: f32 = photo
            .columns
            .iter()
            .map(|&(bearing, elevation)| (elevation_at(&rows, &profile, bearing) - elevation).powi(2))
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
