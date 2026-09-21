use obc_app::peak_view::{panorama, surface::Builder, terrain::Terrain, PeakViewProfile};
use obc_formats::io::{ByteSource, SliceSource};

// pano <obcd> <lat_udeg> <lon_udeg> <out.pgm> <heading_deg> [fov_q4 centre_q4 span_q4]
// Writes the 240x222 chart the device screen draws, sampled exactly as `screen::peak_view::terrain`.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let lat: i32 = a[1].parse().unwrap();
    let lon: i32 = a[2].parse().unwrap();
    let out = &a[3];
    let heading: f32 = a[4].parse().unwrap();
    let bytes = std::fs::read(&a[0]).unwrap();
    let source = SliceSource(&bytes);
    let mut terrain = Terrain::parse(&source as &dyn ByteSource).unwrap();
    let ground = terrain.observer_ground(lat, lon).expect("no terrain at observer");
    let mut profile = PeakViewProfile::at(lat, lon, 0);
    profile.set_ground(ground);
    if a.len() > 7 {
        profile.fov_q4 = a[5].parse().unwrap();
        profile.vertical_centre_q4 = a[6].parse().unwrap();
        profile.vertical_span_q4 = a[7].parse().unwrap();
    }
    profile.default_heading_q4 = (heading * 4.0) as u16;
    let (bottom, top) = profile.vertical_bounds_q4();
    eprintln!(
        "ground={ground:.1} eye={} fov={:.1} deg  vertical {:.1}..{:.1} deg  deg/col {:.4} deg/row {:.4}",
        profile.observer_elevation_m,
        f32::from(profile.fov_q4) / 4.0,
        bottom as f32 / 4.0,
        top as f32 / 4.0,
        f32::from(profile.fov_q4) / 4.0 / 240.0,
        (top - bottom) as f32 / 4.0 / panorama::ROWS as f32
    );
    let mut builder = Box::new(Builder::new(&profile));
    while !builder.complete() {
        builder.step(&mut terrain, 4096);
        assert!(!terrain.failed(), "terrain read failed");
    }
    let p = &builder.panorama;
    let (w, h) = (240usize, panorama::ROWS);
    let fov = i32::from(profile.fov_q4);
    let heading_q4 = i32::from(profile.default_heading_q4);
    let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
    for row in 0..h {
        for x in 0..w {
            let bearing = (heading_q4 - fov / 2 + x as i32 * fov / (w as i32 - 1)).rem_euclid(1440) as u16;
            pgm.push(match p.tone_at_bearing_q4(bearing, row) {
                0 => 255,
                1 => 170,
                2 => 85,
                _ => 0,
            });
        }
    }
    std::fs::write(out, pgm).unwrap();
}
