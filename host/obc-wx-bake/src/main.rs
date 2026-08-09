//! `obc-wx-bake` — the weather bakery CLI.
//!
//! ```text
//! obc-wx-bake cycle   [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   every adapter
//! obc-wx-bake dwd-rv  [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   Germany radar, tier 1
//! obc-wx-bake icon-eu [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   Europe model, tier 2
//! obc-wx-bake us      [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   CONUS MRMS+HRRR, tier 1
//! obc-wx-bake gfs     [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   worldwide floor, tier 3
//! obc-wx-bake schema                                                       print the manifest JSON Schema
//! ```
//!
//! One product's failure never blocks another's: run the per-product subcommands from separate
//! timers when that isolation matters more than a single-manifest cycle.
//!
//! Every invocation is idempotent and stateless: state lives only in the published manifest.
//! A single-adapter invocation is a first-class production mode — `ops/weather` runs one systemd
//! timer per adapter, so a broken upstream cannot cost the other products their freshness — and
//! it rewrites only its own product: every other still-unexpired product is carried forward from
//! the published manifest verbatim (see [`obc_wx_bake::cycle`]). Two invocations must never
//! overlap; the shipped units serialize every instance behind one `flock`.
//! `--now` exists for deterministic replays; production timers omit it. `--store <dir>`
//! publishes into a directory (any static host can serve it); `--r2` uses the `OBC_WX_R2_*`
//! environment (bucket `obc-wx` by default).

use obc_wx_bake::cycle::run_cycle;
use obc_wx_bake::fetch::HttpUpstream;
use obc_wx_bake::manifest;
use obc_wx_bake::publish::{DirStore, ObjectStore, RcloneStore};
use obc_wx_bake::source::{dwd_rv::DwdRv, gfs::GfsFloor, icon_eu::IconEu, us::UsComposite, Adapter};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("obc-wx-bake: {error}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).ok_or_else(usage)?;
    if command == "schema" {
        print!("{}", manifest::schema_json());
        return Ok(());
    }
    let dwd = DwdRv;
    let icon = IconEu;
    let us = UsComposite;
    let gfs = GfsFloor;
    let adapters: Vec<&dyn Adapter> = match command {
        "cycle" => vec![&dwd, &icon, &us, &gfs],
        "dwd-rv" => vec![&dwd],
        "icon-eu" => vec![&icon],
        "us" => vec![&us],
        "gfs" => vec![&gfs],
        _ => return Err(usage()),
    };

    let mut store_dir: Option<String> = None;
    let mut use_r2 = false;
    let mut now: Option<i64> = None;
    let mut dry_run = false;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--store" => {
                store_dir = Some(rest.next().ok_or("--store needs a directory")?.clone());
            }
            "--r2" => use_r2 = true,
            "--now" => {
                let text = rest.next().ok_or("--now needs an RFC 3339 timestamp")?;
                now = Some(manifest::parse_rfc3339(text).ok_or_else(|| format!("--now: {text} is not RFC 3339"))?);
            }
            "--dry-run" => dry_run = true,
            other => return Err(format!("unknown argument {other}\n{}", usage())),
        }
    }
    let mut store: Box<dyn ObjectStore> = match (store_dir, use_r2) {
        (Some(dir), false) => Box::new(DirStore::new(dir)),
        (None, true) => Box::new(RcloneStore::from_env()?),
        (None, false) => return Err(format!("pick a destination: --store <dir> or --r2\n{}", usage())),
        (Some(_), true) => return Err("--store and --r2 are mutually exclusive".into()),
    };
    let now = now.unwrap_or_else(|| chrono::Utc::now().timestamp());

    eprintln!("publishing to {}", store.describe());
    let mut upstream = HttpUpstream::new();
    let report = run_cycle(&adapters, &mut upstream, store.as_mut(), now, dry_run)?;
    eprintln!("{}", report.summary());
    Ok(())
}

fn usage() -> String {
    "usage: obc-wx-bake <cycle|dwd-rv|icon-eu|us|gfs> [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]\n       obc-wx-bake schema"
        .to_string()
}
