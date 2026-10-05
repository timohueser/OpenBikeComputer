use clap::Parser;
use route_build::{catalog, overlays};
use route_engine::{directory::Writer, model::Profile, package};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf};

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
    #[cfg(feature = "obc-terrain")]
    #[arg(long)]
    dem: Option<PathBuf>,
    /// Local bare-earth terrain archive.
    #[cfg(feature = "obc-terrain")]
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
    let profiles: Vec<_> = Profile::presets()
        .into_iter()
        .filter(|p| args.profiles.iter().any(|name| name == "all" || name == &p.name))
        .collect();
    if profiles.is_empty()
        || args.profiles.iter().any(|name| name != "all" && !profiles.iter().any(|p| &p.name == name))
    {
        return Err("Unknown profile ID".into());
    }
    let france = (!args.countries.is_empty()).then(|| catalog::check(&args.countries, &profiles)).transpose()?;
    let mut identities = Vec::new();
    for path in &args.inputs {
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        identities.push(format!("{:x}", hash.finalize()));
    }
    eprintln!("Importing {}", args.region);
    let graph = route_build::osm::import(&args.inputs, bounds, &args.country)?;
    #[cfg(feature = "obc-terrain")]
    let graph = if args.dem.is_some() || args.reference.is_some() {
        let mut graph = graph;
        eprintln!("Sampling terrain");
        let mut terrain =
            route_build::obc_terrain::Terrain::open(args.dem.as_deref(), args.reference.as_deref(), bounds)?;
        route_build::terrain::apply(&mut graph, |point| terrain.height(point))?;
        identities.extend(terrain.identities.iter().cloned());
        graph.warnings.extend(terrain.attribution());
        graph
    } else {
        graph
    };
    eprintln!("Preparing {} profiles for {} directed roads", profiles.len(), graph.roads.len());
    let mut graph = graph;
    route_build::layout::spatial_order(&mut graph)?;
    let temp = args.output.with_extension(format!("building-{}", std::process::id()));
    fs::create_dir(&temp).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut writer = Writer::create(&temp).map_err(|e| e.to_string())?;
        // Only the bytes outlive this statement, not the decoded manifest.
        let manifest =
            serde_json::to_vec(&route_build::prepare(&graph, args.region, bounds, &profiles, identities, |bytes| {
                writer.write(bytes)
            })?)
            .map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
        fs::write(temp.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
        // The overlay index and the route catalog read only the source objects. The road graph is
        // freed here, so the catalog routers get its memory.
        let osm = std::mem::take(&mut graph.osm);
        drop(graph);
        eprintln!("Writing the overlay index");
        overlays::write(&temp.join(overlays::FILE), &package::digest(&manifest), bounds, &osm)?;
        if let Some(france) = france {
            eprintln!("Writing the route catalog");
            println!("{}", catalog::write(&temp, osm, france)?.to_json());
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
