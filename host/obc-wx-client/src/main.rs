//! `obc-wx-client` — fetch one live OBCW bundle for a coordinate.
//!
//! The simulator calls the library directly; this binary exists so the same path is runnable and
//! inspectable by hand: it is how the checked-in fixtures were captured and how a live fetch is
//! reproduced in a PR body.

use std::path::PathBuf;
use std::time::Duration;

use obc_wx_client::http::{FailureControls, FaultyHttp, Http, UreqHttp};
use obc_wx_client::select::Corridor;
use obc_wx_client::{WeatherClient, DEFAULT_SERVICE_URL};

const USAGE: &str = "\
obc-wx-client — one live OBCW bundle for a coordinate (WX14)

usage:
  obc-wx-client fetch --lat DEG --lon DEG [options]

options:
  --lat DEG            rider latitude  (required)
  --lon DEG            rider longitude (required)
  --radius-km KM       corridor radius; default 15 (the phone's undirected disc: 10 km + 5 km margin)
  --service URL        weather service origin; default https://wx.openbikecomputer.com
  --out DIR            write the bundle to DIR/WEATHER.A (creating DIR); default: none, report only
  --now UNIX           evaluate freshness at this instant instead of the system clock
  --latency MS         add this delay to every request (failure control)
  --offline            fail every request (failure control)
  --fail-from N:CODE   answer request N and later with HTTP CODE (failure control)
  --corrupt-request N  flip a bit in request N's body (failure control)
  --truncate-request N halve request N's body (failure control)
  --json               print the diagnostics as JSON

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
    radius_km: f64,
    service: String,
    out: Option<PathBuf>,
    now: Option<i64>,
    json: bool,
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
    let mut args = Args { radius_km: 15.0, service: DEFAULT_SERVICE_URL.to_string(), ..Default::default() };
    let mut it = raw;
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--lat" => args.lat = Some(value()?.parse().map_err(|_| "--lat: not a number")?),
            "--lon" => args.lon = Some(value()?.parse().map_err(|_| "--lon: not a number")?),
            "--radius-km" => args.radius_km = value()?.parse().map_err(|_| "--radius-km: not a number")?,
            "--service" => args.service = value()?,
            "--out" => args.out = Some(PathBuf::from(value()?)),
            "--now" => args.now = Some(value()?.parse().map_err(|_| "--now: not unix seconds")?),
            "--json" => args.json = true,
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
    let corridor = Corridor::around((lat * 1e6).round() as i32, (lon * 1e6).round() as i32, args.radius_km * 1_000.0);

    let mut http = FaultyHttp::new(UreqHttp::new(), args.controls);
    let mut client = WeatherClient::new(&args.service);
    let bundle = client.fetch(&mut http, &corridor, now, 1).map_err(|error| error.to_string())?;
    let diagnostics = &bundle.diagnostics;

    if args.json {
        println!(
            "{{\"bytes\":{},\"requests\":{},\"service_bytes\":{},\"product\":{},\"tier\":{},\"expired\":{},\
             \"failed_frames\":{},\"dropped_incompatible\":{},\"no_rain_map\":{}}}",
            bundle.bytes.len(),
            http.requests(),
            diagnostics.service_bytes,
            diagnostics.product.as_ref().map_or("null".into(), |(id, _)| format!("{id:?}")),
            diagnostics.product.as_ref().map_or(0, |(_, tier)| *tier),
            serde_json::to_string(&diagnostics.expired_products).unwrap_or_default(),
            diagnostics.failed_frames,
            diagnostics.dropped_incompatible_frames,
            diagnostics.no_rain_map.as_ref().map_or("null".into(), |why| format!("{why:?}")),
        );
    } else {
        println!("bundle      {} bytes", bundle.bytes.len());
        match &diagnostics.product {
            Some((id, tier)) => println!("product     {id} (tier {tier})"),
            None => println!("product     none"),
        }
        if let Some(why) = &diagnostics.no_rain_map {
            println!("no rain map {why}");
        }
        if !diagnostics.expired_products.is_empty() {
            println!("expired     {}", diagnostics.expired_products.join(", "));
        }
        println!(
            "requests    {} ({} bytes from the service), {} to MET",
            http.requests(),
            diagnostics.service_bytes,
            diagnostics.met_requests
        );
        if diagnostics.failed_frames > 0 {
            println!("failed      {} frame(s)", diagnostics.failed_frames);
        }
        if diagnostics.dropped_incompatible_frames > 0 {
            println!(
                "dropped     {} frame(s) whose lattice could not tile the window",
                diagnostics.dropped_incompatible_frames
            );
        }
        for line in &diagnostics.attribution {
            println!("attribution {line}");
        }
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
