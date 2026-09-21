use obc_app::peak_view::terrain::Terrain;
use obc_formats::io::{ByteSource, SliceSource};
// profile <obcd> <lat> <lon> <bearing> <max_m> <step_m>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let source = SliceSource(&bytes);
    let mut t = Terrain::parse(&source as &dyn ByteSource).unwrap();
    let (lat, lon, b, far, step): (f64, f64, f64, f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap(),
        a[3].parse().unwrap(), a[4].parse().unwrap(), a[5].parse().unwrap());
    let (s, c) = b.to_radians().sin_cos();
    let mut d = 0.0;
    while d <= far {
        let y = lat + c * d / 111_320.0;
        let x = lon + s * d / (111_320.0 * lat.to_radians().cos());
        let h = t.ground_height((y * 1e6) as i32, (x * 1e6) as i32).unwrap_or(f32::NAN);
        println!("{:6.0} m  {:.5},{:.5}  {:7.1} m", d, y, x, h);
        d += step;
    }
}
