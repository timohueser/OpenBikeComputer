use obc_app::peak_view::terrain::Terrain;
use obc_formats::io::{ByteSource, SliceSource};

// summits <obcd> <lat> <lon> <radius_m> <min_h>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let source = SliceSource(&bytes);
    let mut t = Terrain::parse(&source as &dyn ByteSource).unwrap();
    let (lat, lon): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let radius: f64 = a[3].parse().unwrap();
    let min_h: f32 = a[4].parse().unwrap();
    let step = 40.0;
    let n = (2.0 * radius / step) as i32;
    let mut grid = vec![f32::NAN; ((n + 1) * (n + 1)) as usize];
    for i in 0..=n {
        let y = lat + (i as f64 * step - radius) / 111_320.0;
        for j in 0..=n {
            let x = lon + (j as f64 * step - radius) / (111_320.0 * lat.to_radians().cos());
            if let Some(h) = t.ground_height((y * 1e6) as i32, (x * 1e6) as i32) {
                grid[(i * (n + 1) + j) as usize] = h;
            }
        }
    }
    let r = 6; // 240 m neighbourhood
    let mut found: Vec<(f32, f64, f64, f64, f64)> = Vec::new();
    for i in r..n - r {
        for j in r..n - r {
            let h = grid[(i * (n + 1) + j) as usize];
            if !(h >= min_h) {
                continue;
            }
            let mut top = true;
            for di in -r..=r {
                for dj in -r..=r {
                    if grid[((i + di) * (n + 1) + j + dj) as usize] > h {
                        top = false;
                    }
                }
            }
            if !top {
                continue;
            }
            let (dy, dx) = (i as f64 * step - radius, j as f64 * step - radius);
            let d = (dy * dy + dx * dx).sqrt();
            let b = dx.atan2(dy).to_degrees().rem_euclid(360.0);
            found.push((h, lat + dy / 111_320.0, lon + dx / (111_320.0 * lat.to_radians().cos()), b, d));
        }
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (h, y, x, b, d) in found.iter().take(25) {
        println!("{:7.0} m  {:.5},{:.5}  bearing {:6.1}  distance {:6.0} m", h, y, x, b, d);
    }
}
