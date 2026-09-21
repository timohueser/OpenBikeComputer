use obc_app::peak_view::terrain::Terrain;
use obc_dem::geotiff::DemMosaic;
use obc_formats::io::{ByteSource, SliceSource};

// probe <dem-dir> <obcd> <lat_deg> <lon_deg> <radius_m>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mosaic = DemMosaic::open_dir(std::path::Path::new(&a[0])).unwrap();
    let bytes = std::fs::read(&a[1]).unwrap();
    let source = SliceSource(&bytes);
    let mut terrain = Terrain::parse(&source as &dyn ByteSource).unwrap();
    let lat: f64 = a[2].parse().unwrap();
    let lon: f64 = a[3].parse().unwrap();
    let radius: f64 = a[4].parse().unwrap();

    let dlat = radius / 111_320.0;
    let dlon = radius / (111_320.0 * lat.to_radians().cos());
    let steps = 240;
    let (mut src_max, mut src_at) = (f64::MIN, (0.0, 0.0));
    let (mut bake_max, mut bake_at) = (f32::MIN, (0.0, 0.0));
    for i in 0..=steps {
        let y = lat - dlat + 2.0 * dlat * i as f64 / steps as f64;
        for j in 0..=steps {
            let x = lon - dlon + 2.0 * dlon * j as f64 / steps as f64;
            if let Some(h) = mosaic.height(y, x) {
                if h > src_max {
                    src_max = h;
                    src_at = (y, x);
                }
            }
            if let Some(h) = terrain.ground_height((y * 1e6) as i32, (x * 1e6) as i32) {
                if h > bake_max {
                    bake_max = h;
                    bake_at = (y, x);
                }
            }
        }
    }
    println!("GLO-30 max in +-{radius} m : {src_max:.1} m at {:.5},{:.5}", src_at.0, src_at.1);
    println!("baked  max in +-{radius} m : {bake_max:.1} m at {:.5},{:.5}", bake_at.0, bake_at.1);
    println!("point  GLO-30 {:.1} m  baked {:.1} m",
        mosaic.height(lat, lon).unwrap_or(f64::NAN),
        terrain.ground_height((lat * 1e6) as i32, (lon * 1e6) as i32).unwrap_or(f32::NAN));
}
