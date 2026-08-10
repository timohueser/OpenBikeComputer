//! `obc-wx-pack` — capture a real past weather event into a reusable event pack.
//!
//! ```text
//! obc-wx-pack capture <event-id> --at <rfc3339> [--out <dir>] [--title <t>] [--region <r>]
//!                                [--bbox <s,w,n,e>] [--truth-offsets 15,30,...|none]
//!                                [--store-truth-upstream]
//! obc-wx-pack rebake  <pack-dir> [--out <dir>]   re-bake upstream/ and byte-compare service/
//! obc-wx-pack verify  <pack-dir>                 sha256 every stored member, then rebake
//! obc-wx-pack fetch   <pack-dir>                 materialize the members that are not checked in
//! obc-wx-pack show    <pack-dir>                 what the pack contains, in bytes
//! ```
//!
//! The point of the tool is the simulator and the test suite: `service/` is a frozen slice of the
//! real published weather service for a real storm, and `truth/` is what actually happened next.
//! Everything upstream of that — the nowcaster, the simulator's replay mode, the scorer — is a
//! later step and lives nowhere in this binary.
//!
//! Capture talks to the historical archives named in [`obc_wx_bake::pack::archive`]; be a polite
//! client, they are free public mirrors.

use std::path::{Path, PathBuf};

use obc_wx_bake::fetch::HttpUpstream;
use obc_wx_bake::manifest;
use obc_wx_bake::pack::capture::{capture, materialize, CaptureRequest, DEFAULT_TRUTH_OFFSETS_MIN};
use obc_wx_bake::pack::{archive, rebake, verify_digests, BboxUdeg, Event, Role};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("obc-wx-pack: {error}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str).ok_or_else(usage)? {
        "capture" => run_capture(&args[1..]),
        "rebake" => run_rebake(&args[1..]),
        "verify" => run_verify(&args[1..]),
        "fetch" => run_fetch(&args[1..]),
        "show" => run_show(&args[1..]),
        other => Err(format!("unknown command {other}\n{}", usage())),
    }
}

fn usage() -> String {
    concat!(
        "usage: obc-wx-pack capture <event-id> --at <rfc3339> [--out <dir>] [--title <t>] [--region <r>]\n",
        "                           [--bbox <south,west,north,east>] [--truth-offsets <m,m,...>|none]\n",
        "                           [--store-truth-upstream]\n",
        "       obc-wx-pack rebake <pack-dir> [--out <dir>]\n",
        "       obc-wx-pack verify <pack-dir>\n",
        "       obc-wx-pack fetch  <pack-dir>\n",
        "       obc-wx-pack show   <pack-dir>"
    )
    .to_string()
}

/// A tiny positional-then-flags parser, the same shape `obc-wx-bake` uses.
struct Flags {
    positional: Vec<String>,
    named: Vec<(String, Option<String>)>,
}

fn parse(args: &[String], valueless: &[&str]) -> Result<Flags, String> {
    let mut flags = Flags { positional: Vec::new(), named: Vec::new() };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if let Some(name) = arg.strip_prefix("--") {
            if valueless.contains(&name) {
                flags.named.push((name.to_string(), None));
            } else {
                let value = rest.next().ok_or_else(|| format!("--{name} needs a value"))?;
                flags.named.push((name.to_string(), Some(value.clone())));
            }
        } else {
            flags.positional.push(arg.clone());
        }
    }
    Ok(flags)
}

impl Flags {
    fn value(&self, name: &str) -> Option<&str> {
        self.named.iter().find(|(key, _)| key == name).and_then(|(_, value)| value.as_deref())
    }
    fn present(&self, name: &str) -> bool {
        self.named.iter().any(|(key, _)| key == name)
    }
    fn one_positional(&self, what: &str) -> Result<&str, String> {
        match self.positional.as_slice() {
            [only] => Ok(only),
            _ => Err(format!("expected exactly one {what}\n{}", usage())),
        }
    }
    fn reject_unknown(&self, known: &[&str]) -> Result<(), String> {
        match self.named.iter().find(|(key, _)| !known.contains(&key.as_str())) {
            Some((key, _)) => Err(format!("unknown argument --{key}\n{}", usage())),
            None => Ok(()),
        }
    }
}

const CAPTURE_FLAGS: [&str; 7] = ["at", "out", "title", "region", "bbox", "truth-offsets", "store-truth-upstream"];

fn run_capture(args: &[String]) -> Result<(), String> {
    let flags = parse(args, &["store-truth-upstream"])?;
    flags.reject_unknown(&CAPTURE_FLAGS)?;
    let id = flags.one_positional("event id")?.to_string();
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return Err(format!("event id {id:?} must be ASCII alphanumerics and dashes"));
    }
    let at = flags.value("at").ok_or("capture needs --at <rfc3339>")?;
    let now = manifest::parse_rfc3339(at).ok_or_else(|| format!("--at: {at} is not RFC 3339"))?;
    let bbox = match flags.value("bbox") {
        Some(text) => Some(BboxUdeg::parse(text)?),
        None => None,
    };
    let truth_offsets_min = match flags.value("truth-offsets") {
        None => DEFAULT_TRUTH_OFFSETS_MIN.to_vec(),
        Some("none") => Vec::new(),
        Some(text) => text
            .split(',')
            .map(|part| {
                part.trim().parse::<u32>().map_err(|_| format!("--truth-offsets: {part:?} is not a minute count"))
            })
            .collect::<Result<Vec<u32>, String>>()?,
    };
    let out = PathBuf::from(flags.value("out").unwrap_or("wx-events"));
    let root = out.join(&id);
    let request = CaptureRequest {
        title: flags.value("title").unwrap_or(&id).to_string(),
        region: flags.value("region").unwrap_or("conus").to_string(),
        id,
        now,
        bbox,
        truth_offsets_min,
        store_truth_upstream: flags.present("store-truth-upstream"),
    };

    eprintln!(
        "capturing {} at {} into {} (adapter {}, archives: {})",
        request.id,
        manifest::rfc3339(now),
        root.display(),
        archive::SUPPORTED_ADAPTERS.join(", "),
        archive::MTARCHIVE
    );
    let mut network = HttpUpstream::new();
    let report = capture(&root, &request, &mut network)?;
    eprintln!("{}", report.cycle.summary());
    eprintln!(
        "pack: {} members ({} upstream bytes checked in), {} service bytes, {} truth bytes",
        report.event.members.len(),
        report.upstream_bytes,
        report.service_bytes,
        report.truth_bytes
    );
    let deferred = rebake::unmaterialized(&report.event);
    if !deferred.is_empty() {
        eprintln!("{} recorded members are not checked in; `obc-wx-pack fetch` restores them", deferred.len());
    }

    // A pack that cannot re-bake itself is not a pack. Prove it before the tool exits.
    let scratch = std::env::temp_dir().join(format!("obc-wx-pack-selfcheck-{}", std::process::id()));
    rebake::verify_rebake(&root, &report.event, &scratch)?;
    let _ = std::fs::remove_dir_all(&scratch);
    eprintln!("self-check: the pack re-bakes byte-identically");
    Ok(())
}

fn pack_root(flags: &Flags) -> Result<(PathBuf, Event), String> {
    let root = PathBuf::from(flags.one_positional("pack directory")?);
    let event = Event::read(&root)?;
    Ok((root, event))
}

fn run_rebake(args: &[String]) -> Result<(), String> {
    let flags = parse(args, &[])?;
    flags.reject_unknown(&["out"])?;
    let (root, event) = pack_root(&flags)?;
    let scratch = match flags.value("out") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join(format!("obc-wx-pack-rebake-{}", std::process::id())),
    };
    let report = rebake::verify_rebake(&root, &event, &scratch)?;
    eprintln!("{}", report.summary());
    println!("{}: re-bakes byte-identically into {}", event.id, scratch.display());
    Ok(())
}

fn run_verify(args: &[String]) -> Result<(), String> {
    let flags = parse(args, &[])?;
    flags.reject_unknown(&[])?;
    let (root, event) = pack_root(&flags)?;
    let digests = verify_digests(&root, &event)?;
    let scratch = std::env::temp_dir().join(format!("obc-wx-pack-verify-{}", std::process::id()));
    rebake::verify_rebake(&root, &event, &scratch)?;
    let _ = std::fs::remove_dir_all(&scratch);
    println!(
        "{}: {} digests verified, {} recorded but not checked in, re-bakes byte-identically",
        event.id,
        digests.verified,
        digests.unmaterialized.len()
    );
    Ok(())
}

fn run_fetch(args: &[String]) -> Result<(), String> {
    let flags = parse(args, &[])?;
    flags.reject_unknown(&[])?;
    let (root, mut event) = pack_root(&flags)?;
    let mut network = HttpUpstream::new();
    let restored = materialize(&root, &mut event, &mut network)?;
    println!("{}: materialized {restored} members", event.id);
    Ok(())
}

fn run_show(args: &[String]) -> Result<(), String> {
    let flags = parse(args, &[])?;
    flags.reject_unknown(&[])?;
    let (root, event) = pack_root(&flags)?;
    println!("{} — {}", event.id, event.title);
    println!("  region     {}", event.region);
    println!("  window     {} .. {}", event.window_start, event.window_end);
    println!("  bake       adapter {} at {}", event.bake.adapter, event.bake.now);
    match event.bake.bbox_udeg {
        Some(bbox) => println!(
            "  crop       {:.3},{:.3} .. {:.3},{:.3}",
            bbox.south_udeg as f64 / 1e6,
            bbox.west_udeg as f64 / 1e6,
            bbox.north_udeg as f64 / 1e6,
            bbox.east_udeg as f64 / 1e6
        ),
        None => println!("  crop       none (full domain)"),
    }
    let mut stored = 0u64;
    let mut recorded = 0u64;
    for member in &event.members {
        match (member.length, member.stored) {
            (Some(length), true) => stored += length,
            (Some(length), false) => recorded += length,
            _ => {}
        }
    }
    let service: u64 = event.service.iter().map(|object| object.bytes).sum();
    let truth: u64 = event.truth_frames.iter().map(|frame| frame.bytes).sum();
    println!(
        "  upstream   {} members, {stored} bytes checked in, {recorded} bytes recorded only",
        event.members.iter().filter(|member| member.is_body_like()).count()
    );
    println!("  service    {} objects, {service} bytes", event.service.len());
    println!("  truth      {} frames, {truth} bytes", event.truth_frames.len());
    println!("  on disk    {} bytes", on_disk(&root)?);
    for frame in &event.truth_frames {
        if frame.offset_min != frame.requested_offset_min {
            println!(
                "  note       truth +{} min floored to +{} min ({}) — the observation cadence is {} s",
                frame.requested_offset_min, frame.offset_min, frame.valid_at, event.truth.cadence_seconds
            );
        }
    }
    for member in event.members.iter().filter(|member| member.role == Role::Truth && !member.stored) {
        println!("  fetch      {} <- {}", member.path.as_deref().unwrap_or("?"), member.archive_url);
    }
    Ok(())
}

fn on_disk(root: &Path) -> Result<u64, String> {
    Ok(obc_wx_bake::pack::read_tree(root)?.values().map(|bytes| bytes.len() as u64).sum())
}
