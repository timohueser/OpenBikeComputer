fn main() {
    let builder = core::mem::size_of::<obc_app::peak_view::surface::Builder>();
    let terrain = core::mem::size_of::<obc_app::peak_view::terrain::Terrain<'static>>();
    let cache = core::mem::size_of::<obc_elevation::surface::SurfaceCache>();
    println!("Builder          {builder:>7}");
    println!("Terrain          {terrain:>7}  (SurfaceCache {cache})");
    println!("PeakArm          {:>7}", builder + terrain);
    println!("arena            {:>7}", 128 * 1024);
    println!("headroom         {:>7}", 128 * 1024 - (builder + terrain));
}
