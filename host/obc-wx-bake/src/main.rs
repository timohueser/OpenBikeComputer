//! `obc-wx-bake` — the weather bakery CLI.
//!
//! ```text
//! obc-wx-bake cycle  [--store <dir>|--r2] [--now <rfc3339>] [--threads n] [--dry-run]
//! obc-wx-bake schema                                       print the manifest JSON Schema
//! ```
//!
//! One subcommand, because the bakery publishes **one dataset**. A cycle bakes every source in
//! [`obc_wx_bake::source::MOSAIC_PRIORITY`], mosaics them onto the global 0.01 degree lattice and
//! publishes 24 shards x 9 frames of provider-agnostic OBCG under `wx/v2/`, indexed by a manifest
//! with nothing selectable in it — one generation, one grid, a shard presence bitmap. Which source
//! painted which cell is baker configuration and reaches nobody.
//!
//! There used to be one subcommand per adapter, publishing four products at four resolutions into
//! a `wx/v1` tree that clients chose between by tier. #1246 deleted all of it, and the isolation
//! that arrangement bought — one broken upstream costing only its own product's freshness — goes
//! with it by construction: the mosaic needs every source's cells, so a cycle either publishes a
//! complete dataset or publishes nothing and leaves the previous generation standing.
//!
//! Every invocation is idempotent and stateless: state lives only in the published manifest. Two
//! invocations must never overlap; the shipped units serialize every instance behind one `flock`.
//! `--now` exists for deterministic replays; production timers omit it. `--store <dir>` publishes
//! into a directory (any static host can serve it); `--r2` uses the `OBC_WX_R2_*` environment
//! (bucket `obc-wx` by default).

use obc_wx_bake::canonical::{run_cycle, BAKE_THREADS, CANONICAL};
use obc_wx_bake::fetch::HttpUpstream;
use obc_wx_bake::manifest_v2;
use obc_wx_bake::publish::{DirStore, ObjectStore, RcloneStore};
use obc_wx_bake::source::{
    dwd_rv::DwdRv, gfs::GfsFloor, hrrr::Hrrr, icon_eu::IconEu, mrms::Mrms, opera_cirrus::OperaCirrus,
    opera_nimbus::OperaNimbus, Adapter,
};
use obc_wx_bake::timefmt;

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
        print!("{}", manifest_v2::schema_json());
        return Ok(());
    }
    if command != "cycle" {
        return Err(usage());
    }
    let dwd = DwdRv;
    let icon = IconEu;
    let mrms = Mrms;
    let hrrr = Hrrr;
    let gfs = GfsFloor;
    let cirrus = OperaCirrus;
    let nimbus = OperaNimbus;
    // Every source, every cycle. There is no subset to select: a shard's cells come from whichever
    // of these covers them best, so a missing source is a hole in the dataset rather than one
    // product fewer.
    let adapters: Vec<&dyn Adapter> = vec![&dwd, &mrms, &cirrus, &nimbus, &hrrr, &icon, &gfs];

    let mut store_dir: Option<String> = None;
    let mut use_r2 = false;
    let mut now: Option<i64> = None;
    let mut dry_run = false;
    let mut threads = BAKE_THREADS;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--store" => {
                store_dir = Some(rest.next().ok_or("--store needs a directory")?.clone());
            }
            "--r2" => use_r2 = true,
            "--now" => {
                let text = rest.next().ok_or("--now needs an RFC 3339 timestamp")?;
                now = Some(timefmt::parse_rfc3339(text).ok_or_else(|| format!("--now: {text} is not RFC 3339"))?);
            }
            "--threads" => {
                let text = rest.next().ok_or("--threads needs a count")?;
                threads = text.parse().map_err(|_| format!("--threads: {text} is not a count"))?;
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
    let report = run_cycle(&CANONICAL, &adapters, &mut upstream, store.as_mut(), now, threads, dry_run)?;
    eprintln!("{}", report.summary());
    Ok(())
}

fn usage() -> String {
    "usage: obc-wx-bake cycle [--store <dir>|--r2] [--now <rfc3339>] [--threads <n>] [--dry-run]\n       \
     --threads defaults to 4, the production VPS core count\n       \
     obc-wx-bake schema   print the manifest JSON Schema"
        .to_string()
}
