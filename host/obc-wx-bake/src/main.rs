//! `obc-wx-bake` — the weather bakery CLI.
//!
//! ```text
//! obc-wx-bake canonical [--store <dir>|--r2] [--now <rfc3339>] [--threads n] [--dry-run]
//!                                                                          the one mosaic dataset
//! obc-wx-bake cycle   [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   every adapter
//! obc-wx-bake dwd-rv  [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   Germany radar, tier 1
//! obc-wx-bake icon-eu [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   Europe model, tier 2
//! obc-wx-bake us      [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   CONUS MRMS+HRRR, tier 1
//! obc-wx-bake gfs     [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]   worldwide floor, tier 3
//! obc-wx-bake opera-cirrus  [...]                                          Europe 1 km radar, tier 1
//! obc-wx-bake opera-nimbus  [...]                                          Europe 2 km radar, tier 1
//! obc-wx-bake schema                                                       print the manifest JSON Schema
//! obc-wx-bake spike   [--threads 4] [...]                                  WXR1 #1240 measurement harness
//! ```
//!
//! `canonical` is the WXR3 (#1242) path and the one the service is moving to: it bakes **every**
//! adapter, mosaics them onto the canonical global 0.01 degree lattice by the ordered
//! `source::MOSAIC_PRIORITY` table, and publishes one provider-agnostic dataset of 24 shards x 9
//! frames under `wx/v2/` — beside the live `wx/v1` tree, never over it. Its manifest is a
//! deliberate placeholder until WXR4 #1243 designs the real one. Everything else below is the
//! multi-product path WXR7 #1246 deletes.
//!
//! The two OPERA adapters (WXR6, #1245) sit **only** on that side of the line: they are in
//! `canonical`, which writes `wx/v2` and which nothing reads yet, and deliberately **not** in
//! `cycle`, whose `wx/v1` manifest the shipped clients do read. Publishing them there today
//! would add two tier-1 products over Europe for clients whose selection policy WXR3/WXR5/WXR7
//! are in the middle of deleting. Their `ops/weather/adapters.conf` rows are commented out and
//! `--r2` is refused for the two per-source subcommands for the same reason.
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

use obc_wx_bake::canonical::{run_canonical_cycle, BAKE_THREADS, CANONICAL};
use obc_wx_bake::cycle::run_cycle;
use obc_wx_bake::fetch::HttpUpstream;
use obc_wx_bake::manifest;
use obc_wx_bake::publish::{DirStore, ObjectStore, RcloneStore};
use obc_wx_bake::source::{
    dwd_rv::DwdRv, gfs::GfsFloor, icon_eu::IconEu, opera_cirrus::OperaCirrus, opera_nimbus::OperaNimbus,
    us::UsComposite, Adapter,
};

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

/// The adapters that exist but must not reach the live service yet (WXR6, #1245).
const OPERA_ADAPTERS: [&str; 2] = [obc_wx_bake::source::opera_cirrus::ID, obc_wx_bake::source::opera_nimbus::ID];

fn run(args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).ok_or_else(usage)?;
    if command == "schema" {
        print!("{}", manifest::schema_json());
        return Ok(());
    }
    // The WXR1 (#1240) measurement spike: fixtures in, numbers out, nothing published.
    if command == "spike" {
        return obc_wx_bake::spike::run(&args[1..]);
    }
    let dwd = DwdRv;
    let icon = IconEu;
    let us = UsComposite;
    let gfs = GfsFloor;
    let cirrus = OperaCirrus;
    let nimbus = OperaNimbus;
    let adapters: Vec<&dyn Adapter> = match command {
        // The mosaic takes every source, OPERA included: `canonical` writes `wx/v2`, which is
        // beside the live tree and which nothing reads yet.
        "canonical" => vec![&dwd, &icon, &us, &gfs, &cirrus, &nimbus],
        // `cycle` is the live `wx/v1` multi-product set, and OPERA stays out of it until the
        // client-side tier policy is gone — see the module comment.
        "cycle" => vec![&dwd, &icon, &us, &gfs],
        "dwd-rv" => vec![&dwd],
        "icon-eu" => vec![&icon],
        "us" => vec![&us],
        "gfs" => vec![&gfs],
        "opera-cirrus" => vec![&cirrus],
        "opera-nimbus" => vec![&nimbus],
        _ => return Err(usage()),
    };

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
                now = Some(manifest::parse_rfc3339(text).ok_or_else(|| format!("--now: {text} is not RFC 3339"))?);
            }
            "--threads" => {
                let text = rest.next().ok_or("--threads needs a count")?;
                threads = text.parse().map_err(|_| format!("--threads: {text} is not a count"))?;
            }
            "--dry-run" => dry_run = true,
            other => return Err(format!("unknown argument {other}\n{}", usage())),
        }
    }
    // The one remaining path to the live service, closed by hand until WXR3 flips these on. The
    // `adapters.conf` rows are comments and `cycle` excludes both, so nothing automatic can
    // publish OPERA — but `--r2` is one mistyped word away, and `run_cycle` carries every other
    // product forward, so that one word would republish the live v1 manifest with two new tier-1
    // European products for clients whose selection policy is mid-deletion. Deleting this guard
    // is part of uncommenting the rows, not a separate decision.
    if use_r2 && OPERA_ADAPTERS.contains(&command) {
        return Err(format!(
            "{command} must not publish to the live service yet (WXR6/#1245): it would add a tier-1 product \
             the shipped clients would immediately select. Bake it with --store <dir>; WXR3 removes this guard \
             together with the commented ops/weather/adapters.conf rows."
        ));
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
    let summary = if command == "canonical" {
        run_canonical_cycle(&CANONICAL, &adapters, &mut upstream, store.as_mut(), now, threads, dry_run)?.summary()
    } else {
        run_cycle(&adapters, &mut upstream, store.as_mut(), now, dry_run)?.summary()
    };
    eprintln!("{summary}");
    Ok(())
}

fn usage() -> String {
    "usage: obc-wx-bake <canonical|cycle|dwd-rv|icon-eu|us|gfs|opera-cirrus|opera-nimbus> [--store <dir>|--r2] [--now <rfc3339>] [--dry-run]\n       canonical also takes [--threads <n>] (default 4, the production VPS core count)\n       obc-wx-bake schema"
        .to_string()
}
