use obc_app::peak_view::terrain::Terrain;
use obc_dem::geotiff::DemMosaic;
use obc_formats::io::{ByteSource, SliceSource};

// diffstat <dem-dir> <obcd> <lat0> <lon0> <lat1> <lon1> <step_m>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mosaic = DemMosaic::open_dir(std::path::Path::new(&a[0])).unwrap();
    let bytes = std::fs::read(&a[1]).unwrap();
    let source = SliceSource(&bytes);
    let mut terrain = Terrain::parse(&source as &dyn ByteSource).unwrap();
    let (lat0, lon0, lat1, lon1): (f64, f64, f64, f64) =
        (a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap(), a[5].parse().unwrap());
    let step: f64 = a[6].parse().unwrap();
    let dlat = step / 111_320.0;
    let dlon = step / (111_320.0 * ((lat0 + lat1) / 2.0).to_radians().cos());
    let mut diffs: Vec<f64> = Vec::new();
    let mut worst = (0.0f64, 0.0, 0.0, 0.0);
    // local maxima of the source within a 3x3 fine window, above 2000 m
    let mut peak_diffs: Vec<(f64, f64, f64, f64)> = Vec::new();
    let rows = ((lat1 - lat0) / dlat) as usize;
    let cols = ((lon1 - lon0) / dlon) as usize;
    let mut src = vec![f64::NAN; (rows + 1) * (cols + 1)];
    let mut bak = vec![f64::NAN; (rows + 1) * (cols + 1)];
    for i in 0..=rows {
        let y = lat0 + i as f64 * dlat;
        for j in 0..=cols {
            let x = lon0 + j as f64 * dlon;
            let s = mosaic.height(y, x);
            let b = terrain.ground_height((y * 1e6) as i32, (x * 1e6) as i32);
            if let (Some(s), Some(b)) = (s, b) {
                src[i * (cols + 1) + j] = s;
                bak[i * (cols + 1) + j] = f64::from(b);
                diffs.push(f64::from(b) - s);
                if (f64::from(b) - s) < worst.0 {
                    worst = (f64::from(b) - s, y, x, s);
                }
            }
        }
    }
    let radius = (60.0 / step).round() as usize;
    for i in radius..rows - radius {
        for j in radius..cols - radius {
            let v = src[i * (cols + 1) + j];
            if v.is_nan() || v < 1800.0 {
                continue;
            }
            let mut top = true;
            for di in 0..=2 * radius {
                for dj in 0..=2 * radius {
                    let o = src[(i + di - radius) * (cols + 1) + j + dj - radius];
                    if o > v {
                        top = false;
                    }
                }
            }
            if top {
                peak_diffs.push((bak[i * (cols + 1) + j] - v, v, lat0 + i as f64 * dlat, lon0 + j as f64 * dlon));
            }
        }
    }
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = diffs.len();
    println!("samples {n}");
    println!("baked - GLO30:  p1 {:.1}  p50 {:.1}  p99 {:.1}  min {:.1}  max {:.1}",
        diffs[n / 100], diffs[n / 2], diffs[n * 99 / 100], diffs[0], diffs[n - 1]);
    println!("worst undershoot {:.1} m at {:.5},{:.5} (source {:.0} m)", worst.0, worst.1, worst.2, worst.3);
    peak_diffs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("\nlocal summits (>1800 m, 60 m radius): {}", peak_diffs.len());
    for (d, h, y, x) in peak_diffs.iter().take(15) {
        println!("  {:.5},{:.5}  source {:.0} m  baked {:+.1} m", y, x, h, d);
    }
    let s: f64 = peak_diffs.iter().map(|p| p.0).sum();
    println!("  mean summit error {:.1} m", s / peak_diffs.len() as f64);
}
