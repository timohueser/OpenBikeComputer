use obc_dem::geotiff::DemMosaic;

// demprobe <dem-dir> <lat_deg> <lon_deg>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mosaic = DemMosaic::open_dir(std::path::Path::new(&a[0])).unwrap();
    let (lat, lon): (f64, f64) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    match mosaic.height(lat, lon) {
        Some(h) => println!("{h:.2}"),
        None => panic!("{lat},{lon} is outside the mosaic"),
    }
}
