use clap::Parser;
use route_engine::{
    directory::{Directory, Writer},
    package::{Manifest, Package},
};
use std::{fs, path::PathBuf};

#[derive(Parser)]
#[command(about = "Copy selected profiles and their complete object closure into a smaller routing package")]
struct Args {
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, value_delimiter = ',')]
    profiles: Vec<String>,
    /// Omit build-only OSM objects after all runtime attributes and overlays are prepared.
    #[arg(long)]
    runtime: bool,
}

fn run(args: Args) -> Result<(), String> {
    if args.output.exists() {
        return Err("Output already exists; choose a fresh directory".into());
    }
    let input = Directory::open(&args.input).map_err(|e| e.to_string())?;
    let mut manifest: Manifest = input.manifest().clone();
    if args.profiles.iter().any(|id| !manifest.metrics.contains_key(id)) {
        return Err("Selected profile is absent from the input package".into());
    }
    if !args.profiles.is_empty() {
        manifest.metrics.retain(|id, _| args.profiles.contains(id));
        if let Some(index) = &mut manifest.landmarks {
            index.profiles.retain(|id, _| args.profiles.contains(id));
        }
    }
    if args.runtime {
        manifest.osm = Default::default();
    }
    let bytes = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    let package =
        Package::open(Directory::source(&args.input).map_err(|e| e.to_string())?, &bytes).map_err(|e| e.to_string())?;
    let keys = package.objects().map_err(|e| e.to_string())?;
    let temp = args.output.with_extension(format!("building-{}", std::process::id()));
    fs::create_dir(&temp).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut writer = Writer::create(&temp).map_err(|e| e.to_string())?;
        for key in &keys {
            writer.write(&package.bytes(key).map_err(|e| e.to_string())?)?;
        }
        writer.finish().map_err(|e| e.to_string())?;
        fs::write(temp.join("manifest.json"), bytes).map_err(|e| e.to_string())?;
        fs::rename(&temp, &args.output).map_err(|e| e.to_string())?;
        eprintln!("Ready: {} ({} profiles, {} objects)", args.output.display(), manifest.metrics.len(), keys.len());
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
