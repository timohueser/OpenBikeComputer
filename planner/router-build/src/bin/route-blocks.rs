use clap::Parser;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(about = "Publish a shared routing page pool for a grid of download cells")]
struct Args {
    source: PathBuf,
    output: PathBuf,
    /// JSON array of {id,bounds} cells in the source coverage.
    #[arg(long)]
    cells: PathBuf,
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.output.exists() {
        return Err("The routing publication already exists".into());
    }
    let definitions: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(&args.cells)?)?;
    let mut cells = Vec::new();
    for definition in definitions {
        let id = definition["id"].as_str().ok_or("Missing grid cell identity")?;
        cells.push((id.to_string(), serde_json::from_value(definition["bounds"].clone())?));
    }
    let parent = args.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".routing-publication-{}", std::process::id()));
    fs::create_dir(&temporary)?;
    let result = planner_router_build::blocks::publish(&args.source, &cells, &temporary)
        .and_then(|_| Ok(fs::rename(&temporary, &args.output)?));
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}
fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
