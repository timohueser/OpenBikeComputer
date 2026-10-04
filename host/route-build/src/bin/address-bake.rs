#[path = "../address_bake/mod.rs"]
mod address_bake;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Experimental OSM address enrichment; emits the planner's Photon record format")]
struct Args {
    osm: PathBuf,
    #[arg(long)]
    output: PathBuf,
    /// Country for clipped extracts without complete national boundaries.
    #[arg(long)]
    default_country: String,
    /// Nominatim's static OSM country grid (country_osm_grid.sql.gz). No database is needed.
    #[arg(long)]
    country_grid: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if !matches!(args.default_country.as_str(), "de" | "ch" | "us") {
        return Err("The prototype supports --default-country de, ch or us".into());
    }
    address_bake::bake(&args.osm, &args.output, &args.default_country, args.country_grid.as_deref())
}
