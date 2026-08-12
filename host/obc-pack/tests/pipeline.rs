//! The pipeline as a library: that the CLI and an in-process caller are the same
//! code path, that a cancelled run stops and cleans up, and that the phases a host
//! shows still line up with the ones the web builder scrapes.
//!
//! The byte-identity test is the one that matters for #906: the desktop app links
//! `obc-pack` instead of spawning it, and "the app produces a byte-identical
//! `.obcm` to the CLI" is only worth asserting if the two can drift. Here they run
//! against the same fixture and the same preset and their outputs are compared
//! whole — the binary through `CARGO_BIN_EXE_obc-pack`, so it is the real
//! executable and the real argument parsing, not a re-implementation of them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use obc_pack::config::Config;
use obc_pack::pipeline::{pack, PackOptions};
use obc_pack::progress::{CancelToken, PackError, Phase, Progress};

/// Everything a run said, in order, as a test can inspect it.
type Reported<T> = Arc<Mutex<Vec<(T, String)>>>;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn fixture_pbf() -> String {
    repo("builder/tests/corpus/data/tiny.osm.pbf").to_string_lossy().into_owned()
}

/// A shipped preset, used exactly as the CLI and the app would use it — the
/// `_meta` block rides along and is ignored (`config::tests::
/// unknown_tooling_metadata_remains_compatible`).
fn preset_path() -> PathBuf {
    repo("builder/presets/schema.json")
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-pack-pipeline-{}-{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Land generation is skipped everywhere here: it needs the ~950 MB global
/// land-polygon dataset, which is a network download, not a fixture. Nothing in
/// these tests is about land.
fn opts() -> PackOptions {
    PackOptions { no_land: true, ..PackOptions::default() }
}

#[test]
fn the_cli_and_the_library_produce_the_same_bytes() {
    let dir = out_dir("parity");
    let via_lib = dir.join("lib.obcm");
    let via_cli = dir.join("cli.obcm");

    let config = Config::load(&preset_path().to_string_lossy()).expect("preset parses");
    let summary = pack(&[fixture_pbf()], &config, &via_lib, &opts(), &Progress::silent()).expect("library pack");

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_obc-pack"))
        .arg(fixture_pbf())
        .arg(preset_path())
        .arg(&via_cli)
        .arg("--no-land")
        .output()
        .expect("run obc-pack");
    assert!(status.status.success(), "obc-pack failed: {}", String::from_utf8_lossy(&status.stderr));

    let a = std::fs::read(&via_lib).expect("library output");
    let b = std::fs::read(&via_cli).expect("cli output");
    assert_eq!(a.len() as u64, summary.bytes, "the summary must report what was written");
    assert_eq!(a, b, "the app's map and the CLI's map must be the same bytes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_reports_its_phases_in_order() {
    let dir = out_dir("phases");
    let seen: Reported<Option<Phase>> = Arc::default();
    let sink = Arc::clone(&seen);
    let progress = Progress::new(CancelToken::new(), move |phase, line| {
        sink.lock().unwrap().push((phase, line.to_string()));
    });

    let config = Config::load(&preset_path().to_string_lossy()).expect("preset parses");
    pack(&[fixture_pbf()], &config, &dir.join("out.obcm"), &opts(), &progress).expect("pack");

    let seen = seen.lock().unwrap();
    let phases: Vec<Phase> = seen.iter().filter_map(|(p, _)| *p).collect();
    assert!(!phases.is_empty(), "a run must say where it is");
    // The UI turns a phase's index into a percentage, so the bar can only ever
    // move forwards. One source and no land, hence no Merging and no Land.
    assert!(phases.windows(2).all(|w| w[0] <= w[1]), "phases went backwards: {phases:?}");
    for want in [Phase::Ingest, Phase::Bbox, Phase::Quadtree, Phase::Serialize] {
        assert!(phases.contains(&want), "{want:?} never reported");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The dev server still scrapes these prefixes off the CLI's stdout
/// (`builder/server/jobs.py`'s `_STAGE_MARKERS`) while the desktop app reads
/// the [`Phase`] beside them. They are the same events, so a renamed stage line
/// must break here rather than silently freeze one host's progress bar.
#[test]
fn stage_lines_still_match_the_web_builders_markers() {
    let dir = out_dir("markers");
    let seen: Reported<Phase> = Arc::default();
    let sink = Arc::clone(&seen);
    let progress = Progress::new(CancelToken::new(), move |phase, line| {
        if let Some(p) = phase {
            sink.lock().unwrap().push((p, line.to_string()));
        }
    });

    let config = Config::load(&preset_path().to_string_lossy()).expect("preset parses");
    pack(&[fixture_pbf()], &config, &dir.join("out.obcm"), &opts(), &progress).expect("pack");

    // marker prefix → the phase jobs.py maps it to.
    let markers: &[(&str, Phase)] = &[
        ("Merging", Phase::Merging),
        ("Pass 0", Phase::Ingest),
        ("Pass 1", Phase::Ingest),
        ("Pass 2", Phase::Ingest),
        ("Calculating BBox", Phase::Bbox),
        ("Generating land", Phase::Land),
        ("Building Quadtree", Phase::Quadtree),
        ("Serializing", Phase::Serialize),
        ("Writing", Phase::Serialize),
    ];
    for (phase, line) in seen.lock().unwrap().iter() {
        let hit = markers.iter().find(|(m, _)| line.starts_with(m));
        let (marker, scraped) = hit.unwrap_or_else(|| {
            panic!("stage line {line:?} matches no marker in jobs.py's _STAGE_MARKERS — add it there too")
        });
        assert_eq!(scraped, phase, "marker {marker:?} maps to a different phase in jobs.py than in Rust");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancellation, end to end: the token is tripped from inside the run itself (on
/// the *first* thing it says), so this pins the checkpoint being reached rather
/// than a race between two threads. What it asserts is the contract the UI
/// depends on — the call returns `Cancelled`, and no half-written map is left
/// where a finished one would be.
#[test]
fn cancelling_stops_the_run_and_removes_the_partial_output() {
    let dir = out_dir("cancel");
    let out = dir.join("cancelled.obcm");
    let cancel = CancelToken::new();
    let trip = cancel.clone();
    let after = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&after);
    let progress = Progress::new(cancel, move |_, _| {
        // Everything the run says *after* the token is set is work that started
        // before the next checkpoint; the count is the cancellation's granularity,
        // in lines.
        if trip.is_cancelled() {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        trip.cancel();
    });

    let config = Config::load(&preset_path().to_string_lossy()).expect("preset parses");
    match pack(&[fixture_pbf()], &config, &out, &opts(), &progress) {
        Err(PackError::Cancelled) => {}
        Err(PackError::Failed(e)) => panic!("cancelled run reported a failure: {e}"),
        Ok(_) => panic!("a cancelled run produced a map"),
    }
    assert!(!out.exists(), "a cancelled build must not leave a partial .obcm behind");
    // The fixture is tiny, so the run stops within a couple of checkpoints. The
    // bound is loose on purpose — this is a smell test for "the flag is only read
    // once, at the end", not a timing assertion.
    assert!(after.load(Ordering::Relaxed) < 8, "the run kept going for {} more stages", after.load(Ordering::Relaxed));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The shipped preset with every tier's `min_line_km` forced to `value`, so a test can move one
/// knob and change nothing else about a real 9-tier ladder.
fn preset_with_min_line_km(value: f64) -> Config {
    let text = std::fs::read_to_string(preset_path()).expect("preset readable");
    let mut doc: serde_json::Value = serde_json::from_str(&text).expect("preset is JSON");
    for lod in doc["lods"].as_array_mut().expect("lods array") {
        lod["min_line_km"] = serde_json::json!(value);
    }
    Config::parse(&doc.to_string()).expect("edited preset parses")
}

/// `min_line_km: 0` is the off value, and off must mean *untouched*: a ladder that sets it to zero
/// everywhere packs to the same bytes as one that never mentions the knob. This is what lets the
/// cull ship on two far-zoom tiers without disturbing the seven the preset already had.
#[test]
fn the_line_cull_switched_off_is_byte_identical() {
    let dir = out_dir("cull-off");
    let shipped = Config::load(&preset_path().to_string_lossy()).expect("preset parses");
    let zeroed = preset_with_min_line_km(0.0);

    let a = dir.join("shipped.obcm");
    let b = dir.join("zeroed.obcm");
    pack(&[fixture_pbf()], &shipped, &a, &opts(), &Progress::silent()).expect("pack");
    pack(&[fixture_pbf()], &zeroed, &b, &opts(), &Progress::silent()).expect("pack");
    // The shipped preset does cull on tiers 0-1, so zeroing it can only *add* lines back; the
    // point is that the zeroed run is exactly a run of the pre-knob packer.
    assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "the shipped ladder does cull something");

    let mut no_knob: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(preset_path()).unwrap()).expect("preset is JSON");
    for lod in no_knob["lods"].as_array_mut().unwrap() {
        lod.as_object_mut().unwrap().remove("min_line_km");
    }
    let c = dir.join("absent.obcm");
    let absent = Config::parse(&no_knob.to_string()).expect("knob-free preset parses");
    pack(&[fixture_pbf()], &absent, &c, &opts(), &Progress::silent()).expect("pack");
    assert_eq!(
        std::fs::read(&b).unwrap(),
        std::fs::read(&c).unwrap(),
        "an explicit 0 and an absent key must pack identically"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The cull is wired in after `merge_lines` and says so. A threshold larger than the whole fixture
/// takes every line on the tiers that run it, which is the loudest possible proof that the knob
/// reaches real geometry rather than sitting unread in the config.
#[test]
fn the_line_cull_runs_after_stitching_and_reports() {
    let dir = out_dir("cull-on");
    let seen: Reported<Option<Phase>> = Arc::default();
    let sink = Arc::clone(&seen);
    let progress = Progress::new(CancelToken::new(), move |phase, line| {
        sink.lock().unwrap().push((phase, line.to_string()));
    });

    pack(&[fixture_pbf()], &preset_with_min_line_km(10_000.0), &dir.join("out.obcm"), &opts(), &progress)
        .expect("pack");

    let seen = seen.lock().unwrap();
    let lines: Vec<&str> = seen.iter().map(|(_, l)| l.as_str()).collect();
    let culls: Vec<&&str> = lines.iter().filter(|l| l.contains("stitched line(s) shorter than")).collect();
    assert!(!culls.is_empty(), "a 10 000 km threshold culled nothing: {lines:?}");
    // Ordering is the safety property: measuring before the stitch would measure raw OSM ways.
    // `report_merge` stays quiet when a tier had nothing to stitch, which is the usual case on a
    // fixture this small — so this pins the order only on the tiers that did say something.
    if let Some(stitched) = lines.iter().position(|l| l.contains("line fragment(s) into")) {
        let culled = lines.iter().position(|l| l.contains("stitched line(s) shorter than")).expect("cull reported");
        assert!(stitched < culled, "the cull must run after the stitch, not before it");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_that_was_never_cancelled_reports_a_real_failure() {
    let dir = out_dir("failure");
    let config = Config::load(&preset_path().to_string_lossy()).expect("preset parses");
    let err = pack(&["/nonexistent.osm.pbf".to_string()], &config, &dir.join("x.obcm"), &opts(), &Progress::silent())
        .expect_err("a missing source must fail");
    match err {
        PackError::Failed(e) => assert!(e.contains("nonexistent"), "unhelpful error: {e}"),
        PackError::Cancelled => panic!("a failure was reported as a cancellation"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
