use obc_dem::geotiff::DemMosaic;

// hybridsky <fine-dir> <coarse-dir> <lat> <lon> <eye_m> <b0> <b1> <bstep>
// The truth skyline: 2 m LiDAR wherever it reaches, GLO-30 beyond it. A 30 m error at 40 km is
// 0.04 degrees, so the far ranges cost nothing; the near ridges are where the fine data matters.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let fine = DemMosaic::open_dir(std::path::Path::new(&a[0])).unwrap();
    let coarse = DemMosaic::open_dir(std::path::Path::new(&a[1])).unwrap();
    let (lat, lon, eye): (f64, f64, f64) = (a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let (b0, b1, bs): (f64, f64, f64) = (a[5].parse().unwrap(), a[6].parse().unwrap(), a[7].parse().unwrap());
    let k = 0.87 / (2.0 * 6_371_000.0);
    let mut b = b0;
    while b <= b1 + 1e-9 {
        let (s, c) = b.to_radians().sin_cos();
        let (mut best, mut at, mut near, mut d) = (f64::MIN, 0.0, false, 15.0);
        while d < 60_000.0 {
            let y = lat + c * d / 111_320.0;
            let x = lon + s * d / (111_320.0 * lat.to_radians().cos());
            let hit = fine.height(y, x).map(|h| (h, true)).or_else(|| coarse.height(y, x).map(|h| (h, false)));
            if let Some((h, is_fine)) = hit {
                let slope = (h - eye) / d - k * d;
                if slope > best {
                    (best, at, near) = (slope, d, is_fine);
                }
            }
            d += if d < 3_000.0 { 4.0 } else if d < 12_000.0 { 12.0 } else { 30.0 };
        }
        println!("{b:.4} {:.4} {at:.0} {}", best.atan().to_degrees(), u8::from(near));
        b += bs;
    }
}
