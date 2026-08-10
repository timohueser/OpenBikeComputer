//! `obc-wx-client` — fetch one live OBCW bundle for a coordinate.
//!
//! The simulator calls the library directly; this binary exists so the same path is runnable and
//! inspectable by hand: it is how the checked-in fixtures were captured and how a live fetch is
//! reproduced in a PR body.

use std::path::PathBuf;
use std::time::Duration;

use obc_wx_client::corridor::{Corridor, CORRIDOR_RADIUS_M};
use obc_wx_client::http::{FailureControls, FaultyHttp, Http, UreqHttp};
use obc_wx_client::{WeatherClient, DEFAULT_SERVICE_URL};

const USAGE: &str = "\
obc-wx-client — one live OBCW bundle for a coordinate (WX14)

usage:
  obc-wx-client fetch --lat DEG --lon DEG [options]

options:
  --lat DEG            rider latitude  (required)
  --lon DEG            rider longitude (required)
  --radius-km KM       corridor radius; default 90 km, the one corridor there is
  --service URL        weather service origin; default https://wx.openbikecomputer.com
  --out DIR            write the bundle to DIR/WEATHER.A (creating DIR); default: none, report only
  --now UNIX           evaluate freshness at this instant instead of the system clock
  --latency MS         add this delay to every request (failure control)
  --offline            fail every request (failure control)
  --fail-from N:CODE   answer request N and later with HTTP CODE (failure control)
  --corrupt-request N  flip a bit in request N's body (failure control)
  --truncate-request N halve request N's body (failure control)
  --json               print the diagnostics as JSON
  --dump               print the bundle's current rain frame as ASCII intensities (0 dry .. c max,
                       `.` no-data) — the cheapest proof that real cells reached the device format

The service never receives a coordinate: every OBC request is a Range read of a static, immutable
object. MET is the one third party that sees the position, rounded to four decimals.
";

fn main() {
    if let Err(error) = run() {
        eprintln!("obc-wx-client: {error}");
        std::process::exit(1);
    }
}

#[derive(Default)]
struct Args {
    lat: Option<f64>,
    lon: Option<f64>,
    radius_km: Option<f64>,
    service: String,
    out: Option<PathBuf>,
    now: Option<i64>,
    json: bool,
    dump: bool,
    controls: FailureControls,
}

fn run() -> Result<(), String> {
    let mut raw = std::env::args().skip(1);
    let command = raw.next().unwrap_or_default();
    if command != "fetch" {
        print!("{USAGE}");
        return if command.is_empty() || command == "--help" || command == "-h" {
            Ok(())
        } else {
            Err(format!("unknown command {command:?}"))
        };
    }
    let mut args = Args { service: DEFAULT_SERVICE_URL.to_string(), ..Default::default() };
    let mut it = raw;
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--lat" => args.lat = Some(value()?.parse().map_err(|_| "--lat: not a number")?),
            "--lon" => args.lon = Some(value()?.parse().map_err(|_| "--lon: not a number")?),
            "--radius-km" => args.radius_km = Some(value()?.parse().map_err(|_| "--radius-km: not a number")?),
            "--service" => args.service = value()?,
            "--out" => args.out = Some(PathBuf::from(value()?)),
            "--now" => args.now = Some(value()?.parse().map_err(|_| "--now: not unix seconds")?),
            "--json" => args.json = true,
            "--dump" => args.dump = true,
            "--offline" => args.controls.offline = true,
            "--latency" => {
                args.controls.latency =
                    Duration::from_millis(value()?.parse().map_err(|_| "--latency: not milliseconds")?)
            }
            "--fail-from" => {
                let spec = value()?;
                let (n, code) = spec.split_once(':').ok_or("--fail-from wants N:CODE")?;
                args.controls.fail_from = Some((
                    n.parse().map_err(|_| "--fail-from: bad request index")?,
                    code.parse().map_err(|_| "--fail-from: bad status code")?,
                ));
            }
            "--corrupt-request" => {
                args.controls.corrupt_request = Some(value()?.parse().map_err(|_| "--corrupt-request: bad index")?)
            }
            "--truncate-request" => {
                args.controls.truncate_request = Some(value()?.parse().map_err(|_| "--truncate-request: bad index")?)
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let (Some(lat), Some(lon)) = (args.lat, args.lon) else {
        return Err("fetch needs --lat and --lon".into());
    };
    let now = args.now.unwrap_or_else(|| chrono::Utc::now().timestamp());
    let (lat_udeg, lon_udeg) = ((lat * 1e6).round() as i32, (lon * 1e6).round() as i32);
    // One corridor shape: a disc around the rider. `--radius-km` narrows or widens it for a
    // capture; nothing about the answer changes with it except how many shards are read.
    let corridor = Corridor::around(lat_udeg, lon_udeg, args.radius_km.map_or(CORRIDOR_RADIUS_M, |km| km * 1_000.0));

    let mut http = FaultyHttp::new(UreqHttp::new(), args.controls);
    let mut client = WeatherClient::new(&args.service);
    let bundle = client.fetch(&mut http, &corridor, now, 1).map_err(|error| error.to_string())?;
    let diagnostics = &bundle.diagnostics;

    if args.json {
        println!(
            "{{\"bytes\":{},\"service_requests\":{},\"service_bytes\":{},\"met_requests\":{},\"met_bytes\":{},\
             \"cached_frames\":{},\"total_requests\":{},\"generation\":{},\"dry_shards\":{},\
             \"failed_frames\":{},\"skipped_manifest_frames\":{},\"no_rain_map\":{}}}",
            bundle.bytes.len(),
            diagnostics.service_requests,
            diagnostics.service_bytes,
            diagnostics.met_requests,
            diagnostics.met_bytes,
            diagnostics.cached_frames,
            http.requests(),
            diagnostics.generation.as_ref().map_or("null".into(), |id| format!("{id:?}")),
            diagnostics.dry_shards,
            diagnostics.failed_frames,
            diagnostics.skipped_manifest_frames,
            diagnostics.no_rain_map.as_ref().map_or("null".into(), |why| format!("{:?}", why.to_string())),
        );
    } else {
        println!("bundle      {} bytes", bundle.bytes.len());
        match &diagnostics.generation {
            Some(generation) => println!("generation  {generation}"),
            None => println!("generation  none"),
        }
        if let Some(why) = &diagnostics.no_rain_map {
            println!("no rain map {why}");
        }
        if diagnostics.dry_shards > 0 {
            println!("dry         {} shard(s) the baker measured as rain-free", diagnostics.dry_shards);
        }
        // Two lines, because they are two different disclosures: the service half is
        // coordinate-free and the MET half is the one request that carries the position.
        println!(
            "service     {} request(s), {} bytes (coordinate-free){}",
            diagnostics.service_requests,
            diagnostics.service_bytes,
            if diagnostics.cached_frames > 0 {
                format!(" — {} frame(s) served from cache", diagnostics.cached_frames)
            } else {
                String::new()
            }
        );
        println!("met         {} request(s), {} bytes", diagnostics.met_requests, diagnostics.met_bytes);
        if diagnostics.failed_frames > 0 {
            println!("failed      {} shard(s) — an object the manifest promised", diagnostics.failed_frames);
        }
        if diagnostics.dropped_oversize_frames > 0 {
            println!("dropped     {} frame(s) to fit the producer cap", diagnostics.dropped_oversize_frames);
        }
        for line in &diagnostics.attribution {
            println!("attribution {line}");
        }
    }

    if args.dump {
        dump(&bundle.bytes, now);
    }

    if let Some(out) = &args.out {
        std::fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
        let path = out.join("WEATHER.A");
        std::fs::write(&path, &bundle.bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        // The other slot must not hold a stale generation the boot selector would prefer.
        let _ = std::fs::remove_file(out.join("WEATHER.B"));
        if !args.json {
            println!("wrote       {}", path.display());
        }
    }
    Ok(())
}

/// The frame current at `now`, as one character per cell. Read through the **device's** reader,
/// so what this prints is what the firmware would sample — not what the builder thinks it wrote.
fn dump(bytes: &[u8], now: i64) {
    const GLYPHS: &[u8] = b"0123456789abc";
    let source = obc_formats::io::SliceSource(bytes);
    let Ok(reader) = obc_weather::WeatherReader::open(&source) else {
        println!("dump: the device reader refused this bundle");
        return;
    };
    let frame_count = reader.header().frame_count as usize;
    if frame_count == 0 {
        println!("dump: hourly only — no rain frames");
        return;
    }
    // The newest frame that is not in the future: what the rider is looking at right now.
    let mut chosen = 0usize;
    for index in 0..frame_count {
        if reader.frame(index).is_ok_and(|frame| frame.valid_at <= now) {
            chosen = index;
        }
    }
    let Ok(frame) = reader.frame(chosen) else { return };
    println!(
        "frame {}/{frame_count} valid_at {} — {}x{} cells at {} m",
        chosen + 1,
        frame.valid_at,
        frame.width,
        frame.height,
        frame.cell_size_m
    );
    let edge = obc_formats::precip4::TILE_EDGE as u32;
    let (width, height) = (u32::from(frame.width), u32::from(frame.height));
    let tile_cols = width.div_ceil(edge);
    let mut tile = [0u8; obc_formats::precip4::TILE_CELLS];
    let mut grid = vec![obc_formats::precip4::INTENSITY_NODATA; (width * height) as usize];
    for tile_index in 0..frame.tile_count {
        if reader.decode_tile(chosen, tile_index, &mut tile).is_err() {
            continue;
        }
        let (tile_col, tile_row) = (tile_index % tile_cols, tile_index / tile_cols);
        for local_row in 0..edge {
            for local_col in 0..edge {
                let (col, row) = (tile_col * edge + local_col, tile_row * edge + local_row);
                if col < width && row < height {
                    grid[(row * width + col) as usize] = tile[(local_row * edge + local_col) as usize];
                }
            }
        }
    }
    // Rows advance north in the format; print north-first so the picture reads like a map.
    for row in (0..height).rev() {
        let line: String = (0..width)
            .map(|col| {
                let value = grid[(row * width + col) as usize];
                GLYPHS.get(value as usize).map_or('.', |glyph| *glyph as char)
            })
            .collect();
        println!("  {line}");
    }
}
