use clap::Parser;
use obc_dem::planner::{tiles_in, Terrain};
use planner_router_build::step::{build, Build};
use std::{fs, path::PathBuf};

#[derive(Parser)]
#[command(about = "Build a regional routing package and its overlay index from OSM PBF files")]
struct Args {
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    region: String,
    #[arg(long)]
    country: String,
    /// West,south,east,north in degrees. Routes are exact within this clipped graph.
    #[arg(long, allow_hyphen_values = true)]
    bounds: String,
    /// Comma-separated catalogue IDs, or all.
    #[arg(long, default_value = "touring,road,gravel,mtb,hiking", value_delimiter = ',')]
    profiles: Vec<String>,
    /// The ISO codes of the region's countries. With them, the build also writes the route catalog.
    #[arg(long, value_delimiter = ',')]
    countries: Vec<String>,
    /// Local Copernicus GeoTIFF directory, used where the ground archive has no sample.
    #[arg(long)]
    dem: Option<PathBuf>,
    /// Local bare-earth terrain archive.
    #[arg(long)]
    reference: Option<PathBuf>,
}

fn run(args: Args) -> Result<(), String> {
    let bounds: [f64; 4] = args
        .bounds
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<f64>, _>>()
        .map_err(|_| "Bounds must be numbers")?
        .try_into()
        .map_err(|_| "Provide four bounds")?;
    if args.output.exists() {
        return Err("Output already exists; build a new package and switch the consumer atomically".into());
    }
    let terrain = match (&args.dem, &args.reference) {
        (None, None) => None,
        (dem, reference) => {
            let glo30 = dem.as_deref().map(|dem| tiles_in(dem, bounds)).unwrap_or_default();
            Some(Terrain::open(&glo30, reference.as_deref(), bounds)?)
        }
    };
    let build_args = Build {
        inputs: args.inputs,
        region: args.region,
        country: args.country,
        bounds,
        profiles: args.profiles,
        countries: args.countries,
    };
    let temp = args.output.with_extension(format!("building-{}", std::process::id()));
    fs::create_dir(&temp).map_err(|e| e.to_string())?;
    let result = (|| {
        if let Some(report) = build(build_args, terrain, &temp)? {
            println!("{}", report.to_json());
        }
        fs::rename(&temp, &args.output).map_err(|e| e.to_string())?;
        eprintln!("Ready: {}", args.output.display());
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
    }
    result
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
