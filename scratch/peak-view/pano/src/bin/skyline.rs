use obc_dem::geotiff::DemMosaic;

// skyline <dem-dir> <lat_deg> <lon_deg> <eye_m> <b0> <b1> <bstep>
// Brute-force horizon elevation angle over the raw GLO-30 mosaic, same refraction model as the app.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mosaic = DemMosaic::open_dir(std::path::Path::new(&a[0])).unwrap();
    let lat: f64 = a[1].parse().unwrap();
    let lon: f64 = a[2].parse().unwrap();
    let eye: f64 = a[3].parse().unwrap();
    let (b0, b1, bs): (f64, f64, f64) = (a[4].parse().unwrap(), a[5].parse().unwrap(), a[6].parse().unwrap());
    let curvature = 0.87 / (2.0 * 6_371_000.0);
    let mut b = b0;
    while b <= b1 + 1e-9 {
        let (s, c) = b.to_radians().sin_cos();
        let mut best = f64::MIN;
        let mut at = 0.0;
        let mut d = 20.0;
        while d < 40_000.0 {
            let y = lat + c * d / 111_320.0;
            let x = lon + s * d / (111_320.0 * lat.to_radians().cos());
            if let Some(h) = mosaic.height(y, x) {
                let slope = (h - eye) / d - curvature * d;
                if slope > best {
                    best = slope;
                    at = d;
                }
            }
            d += if d < 5_000.0 { 10.0 } else { 25.0 };
        }
        println!("{b:.3} {:.3} {at:.0}", best.atan().to_degrees());
        b += bs;
    }
}
