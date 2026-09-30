use clap::Parser;
use route_engine::{
    directory::{Directory, Writer},
    package::Package,
};
use std::{collections::BTreeSet, fs, path::PathBuf, time::Instant};

#[derive(Parser)]
#[command(about = "Prepare all installed profiles for a bounding box from an existing routing package")]
struct Args {
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value = "extract")]
    region: String,
    /// West,south,east,north. Intersecting roads and source OSM records stay complete.
    #[arg(long, allow_hyphen_values = true)]
    bounds: String,
    /// Omit source OSM pages; use the source package's compiled overlays.
    #[arg(long)]
    runtime: bool,
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let bounds: [f64; 4] = args
        .bounds
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<f64>, _>>()?
        .try_into()
        .map_err(|_| "Provide four bounds")?;
    if args.output.exists() {
        return Err("Output already exists; choose a fresh directory".into());
    }
    let started = Instant::now();
    let mut input = Directory::open(&args.input)?;
    let temp = args.output.with_extension(format!("building-{}", std::process::id()));
    fs::create_dir(&temp)?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut writer = Writer::create(&temp)?;
        let manifest =
            route_build::extract::prepare(&mut input, args.region, bounds, !args.runtime, |bytes| writer.write(bytes))?;
        writer.finish()?;
        fs::write(temp.join("manifest.json"), serde_json::to_vec(&manifest)?)?;
        let prepare_seconds = started.elapsed().as_secs_f64();
        let mut package = Directory::open(&temp)?;
        package.verify()?;
        let mut geometry_bounds = [180.0f64, 90.0f64, -180.0f64, -90.0f64];
        for id in 0..manifest.roads {
            for point in package.road(id)?.shape {
                let (lon, lat) = (point.lon as f64 * 1e-6, point.lat as f64 * 1e-6);
                geometry_bounds[0] = geometry_bounds[0].min(lon);
                geometry_bounds[1] = geometry_bounds[1].min(lat);
                geometry_bounds[2] = geometry_bounds[2].max(lon);
                geometry_bounds[3] = geometry_bounds[3].max(lat);
            }
        }
        let source_bytes = source_bytes(&package)?;
        let routing_bytes = ["manifest.json", "pages.idx", "pages.bin"]
            .iter()
            .map(|name| fs::metadata(temp.join(name)).map(|m| m.len()))
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .sum::<u64>();
        drop(package);
        fs::rename(&temp, &args.output)?;
        println!(
            "{}",
            serde_json::json!({"bounds": bounds, "geometry_bounds": geometry_bounds,
            "roads": manifest.roads, "profiles": manifest.metrics.len(), "routing_bytes": routing_bytes,
            "source_osm_bytes": source_bytes, "prepare_seconds": prepare_seconds,
            "total_seconds": started.elapsed().as_secs_f64()})
        );
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
    }
    result
}

fn source_bytes<S: route_engine::package::Source>(package: &Package<S>) -> Result<u64, route_engine::Error> {
    let mut keys = BTreeSet::new();
    for table in package.manifest().osm.tables() {
        keys.extend(table.blocks.iter().cloned());
        keys.extend(package.keys(table)?);
    }
    keys.into_iter().map(|key| package.bytes(&key).map(|bytes| bytes.len() as u64)).sum()
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
