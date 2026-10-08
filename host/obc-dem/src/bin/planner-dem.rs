//! `planner-dem --dem DIR [--reference DIR] --bounds W,S,E,N --output FILE`: the terrain of the
//! planner maps as lossless Terrarium MBTiles, from the GLO-30 files in `--dem` and a reference
//! archive.

use std::path::PathBuf;
use std::process::ExitCode;

use obc_dem::planner::{tiles_in, Terrain};

fn run(args: &[String]) -> Result<(), String> {
    let (mut dem, mut reference, mut bounds, mut output) = (None, None, None, None);
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--dem" => dem = Some(PathBuf::from(value)),
            "--reference" => reference = Some(PathBuf::from(value)),
            "--bounds" => bounds = Some(value.clone()),
            "--output" => output = Some(PathBuf::from(value)),
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    let (Some(dem), Some(text), Some(output)) = (dem, bounds, output) else {
        return Err("usage: planner-dem --dem DIR [--reference DIR] --bounds W,S,E,N --output FILE".into());
    };
    let values = text.split(',').map(str::parse).collect::<Result<Vec<f64>, _>>().map_err(|e| e.to_string())?;
    let bounds: [f64; 4] = values.try_into().map_err(|_| "Provide west,south,east,north")?;
    let mut terrain = Terrain::open(&tiles_in(&dem, bounds), reference.as_deref(), bounds)?;
    obc_dem::terrarium::write(&mut terrain, bounds, &text, &output)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
