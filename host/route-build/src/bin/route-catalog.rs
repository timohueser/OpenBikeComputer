use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Write the signed-route catalog into a routing package with source OSM tables")]
struct Args {
    package: PathBuf,
    /// The ISO codes of the region's countries.
    #[arg(long, required = true, value_delimiter = ',')]
    countries: Vec<String>,
    #[arg(long, default_value_t = std::thread::available_parallelism().map_or(1, |n| n.get()))]
    workers: usize,
}

fn main() {
    let args = Args::parse();
    match route_build::catalog::build(&args.package, &args.countries, args.workers) {
        Ok(Some(report)) => println!("{}", report.to_json()),
        Ok(None) => eprintln!("The route catalog is current"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
