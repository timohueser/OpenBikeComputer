use clap::Parser;
use image::{codecs::webp::WebPEncoder, ExtendedColorType};
use route_build::obc_terrain::Terrain;
use route_engine::model::Point;
use rusqlite::{params, Connection};
use std::{f64::consts::PI, path::PathBuf};

const SIZE: u32 = 512;

#[derive(Parser)]
#[command(about = "Bake lossless Terrarium MBTiles from the routing terrain sources")]
struct Args {
    #[arg(long)]
    dem: PathBuf,
    #[arg(long)]
    reference: Option<PathBuf>,
    #[arg(long, allow_hyphen_values = true)]
    bounds: String,
    #[arg(long)]
    output: PathBuf,
}

fn latitude(y: f64, n: f64) -> f64 {
    ((PI * (1.0 - 2.0 * y / n)).sinh().atan()).to_degrees()
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let bounds: [f64; 4] = args
        .bounds
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<f64>, _>>()?
        .try_into()
        .map_err(|_| "Provide west,south,east,north")?;
    let [w, s, e, n] = bounds;
    if !bounds.iter().all(|v| v.is_finite()) || w < -180.0 || e > 180.0 || s < -85.0 || n > 85.0 || w >= e || s >= n {
        return Err("Invalid terrain bounds".into());
    }
    if args.output.exists() {
        return Err("Output exists; choose a fresh path".into());
    }
    let mut terrain = Terrain::open(Some(&args.dem), args.reference.as_deref(), bounds)?;
    let db = Connection::open(&args.output)?;
    db.execute_batch(
        "PRAGMA journal_mode=OFF; CREATE TABLE metadata(name TEXT, value TEXT);
        CREATE TABLE tiles(zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB,
        PRIMARY KEY(zoom_level,tile_column,tile_row)); BEGIN;",
    )?;
    let row =
        |lat: f64, count: f64| (1.0 - (lat.to_radians().tan() + 1.0 / lat.to_radians().cos()).ln() / PI) / 2.0 * count;
    for z in 0..=12 {
        let count = f64::from(1u32 << z);
        let west = ((w + 180.0) / 360.0 * count).floor() as u32;
        let east = (((e + 180.0) / 360.0 * count).ceil() as u32).min(count as u32);
        let north = row(n, count).floor() as u32;
        let south = (row(s, count).ceil() as u32).min(count as u32);
        for y in north..south {
            let latitudes: Vec<_> = (0..SIZE)
                .map(|py| {
                    (latitude(f64::from(y) + (f64::from(py) + 0.5) / f64::from(SIZE), count) * 1e6).round() as i32
                })
                .collect();
            for x in west..east {
                let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];
                let mut known = false;
                for (py, &lat) in latitudes.iter().enumerate() {
                    for px in 0..SIZE {
                        let lon =
                            ((f64::from(x) + (f64::from(px) + 0.5) / f64::from(SIZE)) / count * 360.0 - 180.0) * 1e6;
                        let height = terrain.height(Point {
                            lat,
                            lon: lon.round() as i32,
                            elevation: route_engine::model::NO_ELEVATION,
                        })?;
                        let value = height
                            .map(|h| ((h + 32768.0) * 256.0).round().clamp(0.0, 16777215.0) as u32)
                            .unwrap_or(8388608);
                        let i = (py * SIZE as usize + px as usize) * 4;
                        rgba[i..i + 4].copy_from_slice(&[
                            (value >> 16) as u8,
                            (value >> 8) as u8,
                            value as u8,
                            if height.is_some() { 255 } else { 0 },
                        ]);
                        known |= height.is_some();
                    }
                }
                if known {
                    let mut bytes = Vec::new();
                    WebPEncoder::new_lossless(&mut bytes).encode(&rgba, SIZE, SIZE, ExtendedColorType::Rgba8)?;
                    db.execute("INSERT INTO tiles VALUES (?1,?2,?3,?4)", params![z, x, (1u32 << z) - y - 1, bytes])?;
                }
            }
        }
        db.execute_batch("COMMIT; BEGIN;")?;
        eprintln!("Terrain zoom {z} complete");
    }
    for (key, value) in [
        ("name", "OpenBikeComputer terrain".into()),
        ("format", "webp".into()),
        ("type", "baselayer".into()),
        ("minzoom", "0".into()),
        ("maxzoom", "12".into()),
        ("bounds", args.bounds),
        ("attribution", terrain.attribution().join("; ")),
        ("source_sha256", serde_json::to_string(&terrain.identities)?),
    ] {
        db.execute("INSERT INTO metadata VALUES (?1,?2)", params![key, value])?;
    }
    db.execute_batch("COMMIT;")?;
    Ok(())
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
