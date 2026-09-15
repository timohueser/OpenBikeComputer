use obc_elevation::{ElevationSource, TerrainElevation};
use obc_formats::io::SliceSource;
use std::io::{self, BufRead};
fn main() {
    let bytes = std::fs::read(std::env::args().nth(1).expect("map path")).unwrap();
    assert_eq!(&bytes[..5], b"OBCM\x10");
    let unit = 1usize << bytes[40];
    let offset = u32::from_le_bytes(bytes[41..45].try_into().unwrap()) as usize * unit;
    let len = u32::from_le_bytes(bytes[45..49].try_into().unwrap()) as usize * unit;
    let source = SliceSource(&bytes[offset..offset + len]);
    let mut elevation = Box::new(TerrainElevation::<4>::parse(&source).unwrap());
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut fields = line.split_whitespace().map(|s| s.parse::<i32>().unwrap());
        let lat = fields.next().unwrap();
        let lon = fields.next().unwrap();
        println!("{}", elevation.sample(lat, lon).expect("source DEM has no sample"));
    }
}
