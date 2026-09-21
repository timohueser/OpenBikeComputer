use obc_app::peak_view::terrain::Terrain;
use obc_formats::io::{ByteSource, SliceSource};

// obctsky <obcd> <lat> <lon> <eye_offset_m> <b0> <b1> <step> <max_m>
// The skyline of OUR baked OBCT surface, marched densely over the same bilinear patches the
// panorama renderer integrates, with the same refraction model.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let source = SliceSource(&bytes);
    let mut t = Terrain::parse(&source as &dyn ByteSource).unwrap();
    let (lat, lon): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let eye_off: f64 = a[3].parse().unwrap();
    let (b0, b1, step, far): (f64, f64, f64, f64) =
        (a[4].parse().unwrap(), a[5].parse().unwrap(), a[6].parse().unwrap(), a[7].parse().unwrap());
    // Optional catalogue summits: lat,lon,height,slope — the surface may rise to a recorded top.
    let summits: Vec<(f64, f64, f64, f64)> = a[8..]
        .iter()
        .map(|t| {
            let v: Vec<f64> = t.split(',').map(|x| x.parse().unwrap()).collect();
            (v[0], v[1], v[2], v[3])
        })
        .collect();
    let ground = t.eye_ground((lat * 1e6) as i32, (lon * 1e6) as i32, None).expect("observer outside coverage");
    let eye = f64::from(ground) + eye_off;
    eprintln!("ground {ground:.1} m, eye {eye:.1} m");
    let k = 0.87 / (2.0 * 6_371_000.0);
    let mut b = b0;
    while b <= b1 + 1e-9 {
        let (s, c) = b.to_radians().sin_cos();
        let (mut best, mut at, mut d) = (f64::MIN, 0.0, 15.0);
        while d < far {
            let y = lat + c * d / 111_320.0;
            let x = lon + s * d / (111_320.0 * lat.to_radians().cos());
            if let Some(h) = t.ground_height((y * 1e6) as i32, (x * 1e6) as i32) {
                let mut h = f64::from(h);
                for &(sy, sx, sh, ss) in &summits {
                    let dn = (y - sy) * 111_320.0;
                    let de = (x - sx) * 111_320.0 * sy.to_radians().cos();
                    h = h.max(sh - ss * (dn * dn + de * de).sqrt());
                }
                let slope = (h - eye) / d - k * d;
                if slope > best {
                    best = slope;
                    at = d;
                }
            }
            d += if d < 2_000.0 { 4.0 } else { 12.0 };
        }
        println!("{b:.4} {:.4} {at:.0}", best.atan().to_degrees());
        b += step;
    }
}
