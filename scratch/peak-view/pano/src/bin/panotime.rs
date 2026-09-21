use std::time::Instant;

use obc_app::peak_view::{surface::Builder, terrain::Terrain, PeakViewProfile};
use obc_formats::io::{ByteSource, SliceSource};

// panotime <obcd> <lat_udeg> <lon_udeg> <n>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (lat, lon): (i32, i32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let n: u32 = a[3].parse().unwrap();
    let bytes = std::fs::read(&a[0]).unwrap();
    let source = SliceSource(&bytes);
    let mut terrain = Terrain::parse(&source as &dyn ByteSource).unwrap();
    let ground = terrain.ground_height(lat, lon).unwrap();
    let mut profile = PeakViewProfile::at(lat, lon, 0);
    profile.set_ground(ground);
    let mut worst = 0u128;
    let start = Instant::now();
    for i in 0..n {
        profile.default_heading_q4 = ((i * 360 / n) * 4) as u16;
        let t = Instant::now();
        let mut builder = Box::new(Builder::new(&profile));
        while !builder.complete() {
            builder.step(&mut terrain, 4096);
        }
        assert!(!terrain.failed());
        worst = worst.max(t.elapsed().as_micros());
    }
    println!("{}: mean {:.1} ms, worst {:.1} ms over {n} charts", a[0], start.elapsed().as_micros() as f64 / n as f64 / 1000.0, worst as f64 / 1000.0);
}
