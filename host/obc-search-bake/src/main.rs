use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Build planner addresses, POIs and localities from OSM")]
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
    /// Country policy exported by obc-search-bake/policy.py.
    #[arg(long)]
    policy: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    obc_search_bake::bake(&args.osm, &args.output, &args.default_country, args.country_grid.as_deref(), &args.policy)
}
