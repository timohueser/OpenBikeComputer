//! One map build, from a region list to a file on disk.
//!
//! The dev server does this by spawning `obc-pack`, reading its stdout a byte at a
//! time and matching stage prefixes. Here the packer is linked in, so the whole
//! thing is one worker thread calling [`obc_pack::pipeline::pack`] with a
//! [`Progress`] that turns each `(phase, line)` into an event on a Tauri channel.
//! The events keep the *shapes* the dev host's SSE stream uses, so the two hosts'
//! progress UIs are the same code reading the same vocabulary.
//!
//! **Cancellation is the reason this file is structured the way it is.** A flag
//! that is only read between stages would satisfy nobody: the two places a build
//! actually spends its minutes are the `.pbf` download and the per-feature GEOS
//! work, and both are inside a stage. So the token reaches both — the download
//! checks it every 64 KB chunk ([`crate::http::download`]) and the packer checks
//! it per blob, per land record and per feature ([`obc_pack::progress`]). When the
//! call unwinds, the thread drops the ingest buffers and exits: the memory goes
//! back to the allocator at that point, not when the user next starts something.
//!
//! **One build at a time**, for the same reason the dev server defaults to
//! `OBCM_MAX_CONCURRENT_JOBS=1`: a country-scale pack is measured in gigabytes,
//! and two of them on a laptop is not a feature.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use obc_pack::config::Config;
use obc_pack::ingest::Bbox;
use obc_pack::pipeline::{pack, PackOptions};
use obc_pack::progress::{CancelToken, PackError, Progress};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

/// The build request, in the frontend's own terms (`BuildRequest` in
/// `platform/types.ts`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildRequest {
    pub region_ids: Vec<String>,
    pub config: serde_json::Value,
    pub chunk_size: Option<usize>,
    pub output_name: String,
    /// [west, south, east, north] in degrees. The desktop tier has
    /// `caps.bboxCrop`, and `obc-pack` crops during ingest (D5 #910).
    pub bbox: Option<[f64; 4]>,
}

/// What the UI is told while a build runs. Field-for-field the dev server's SSE
/// events, so `BuildSession` implementations differ only in transport.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BuildEvent {
    /// `detail` is a [`obc_pack::progress::Phase`] name once the packer is
    /// running, and a free-text note before that.
    Status {
        status: &'static str,
        detail: String,
    },
    /// Byte progress on one region's download — the one phase with a real
    /// percentage rather than an index.
    Progress {
        phase: &'static str,
        region: String,
        pct: u8,
    },
    Log {
        line: String,
    },
    Done {
        path: String,
        filename: String,
        size: u64,
    },
    Error {
        message: String,
    },
    /// Distinct from `Error`: a cancelled build is what the user just asked for.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Done,
    Error,
    Cancelled,
}

/// The active (or most recent) build. Enough for a reloaded window to decide
/// whether to re-attach or start fresh.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub state: JobState,
}

struct Job {
    id: String,
    state: JobState,
    cancel: CancelToken,
    /// Append-only, replayed on re-attach — the same trick the SSE endpoint uses,
    /// and the reason a reload mid-build does not lose the log.
    log: Vec<BuildEvent>,
    channel: Option<Channel<BuildEvent>>,
}

/// The app's single build slot.
#[derive(Default)]
pub struct Jobs(Mutex<Option<Job>>);

/// Monotonic job ids. Only has to be unique within one run of the app — a job
/// does not outlive the process.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Jobs {
    pub fn snapshot(&self) -> Option<JobSnapshot> {
        let guard = self.0.lock().expect("jobs lock");
        guard.as_ref().map(|j| JobSnapshot { id: j.id.clone(), state: j.state })
    }

    /// Point a (new) channel at an existing job, replaying everything it has said
    /// so far. `false` if that job is gone.
    pub fn attach(&self, id: &str, channel: Channel<BuildEvent>) -> bool {
        let mut guard = self.0.lock().expect("jobs lock");
        let Some(job) = guard.as_mut().filter(|j| j.id == id) else {
            return false;
        };
        for event in &job.log {
            let _ = channel.send(event.clone());
        }
        // A finished job has nothing more to say; keeping the channel would leave
        // the window listening forever.
        job.channel = matches!(job.state, JobState::Running).then_some(channel);
        true
    }

    pub fn cancel(&self, id: &str) -> bool {
        let guard = self.0.lock().expect("jobs lock");
        match guard.as_ref().filter(|j| j.id == id && j.state == JobState::Running) {
            Some(job) => {
                job.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Record an event and deliver it.
    ///
    /// The send happens **after** the lock is released, and that is not an
    /// optimisation: a handler is free to call back into `Jobs` — cancelling on
    /// what it just read is the obvious thing to want — and `Mutex` is not
    /// reentrant, so sending under the lock is a deadlock waiting for the first
    /// handler that does. (It found one: `cancelling_actually_stops_the_work`
    /// trips the token from inside the channel.)
    fn emit(&self, id: &str, event: BuildEvent) {
        let channel = {
            let mut guard = self.0.lock().expect("jobs lock");
            let Some(job) = guard.as_mut().filter(|j| j.id == id) else {
                return;
            };
            job.log.push(event.clone());
            job.channel.clone()
        };
        if let Some(channel) = channel {
            // A send failure means the window went away; the log kept the event
            // for whoever attaches next.
            let _ = channel.send(event);
        }
    }

    fn finish(&self, id: &str, state: JobState) {
        let mut guard = self.0.lock().expect("jobs lock");
        if let Some(job) = guard.as_mut().filter(|j| j.id == id) {
            job.state = state;
            job.channel = None;
        }
    }
}

/// Everything the worker thread needs, resolved on the caller's thread so a bad
/// request fails *before* a job exists.
struct Plan {
    id: String,
    cancel: CancelToken,
    sources: Vec<(String, String)>,
    config: Config,
    opts: PackOptions,
    out_dir: std::path::PathBuf,
    filename: String,
}

/// Validate a request and open a job slot for it. Nothing has been downloaded and
/// nothing has been written when this returns.
fn plan(jobs: &Arc<Jobs>, req: BuildRequest, maps_dir: std::path::PathBuf) -> Result<Plan, String> {
    if req.region_ids.is_empty() {
        return Err("No regions selected".into());
    }
    let config = Config::parse(&serde_json::to_string(&req.config).map_err(|e| format!("config: {e}"))?)?;
    let bbox = req.bbox.map(|[w, s, e, n]| Bbox::parse(&format!("{w},{s},{e},{n}"))).transpose()?;
    let opts = PackOptions { bbox, chunk_size: req.chunk_size, ..PackOptions::default() };
    let sources = crate::regions::pbf_urls(&req.region_ids)?;
    let filename = crate::paths::sanitize_output_name(&req.output_name);

    let mut guard = jobs.0.lock().expect("jobs lock");
    if guard.as_ref().is_some_and(|j| j.state == JobState::Running) {
        return Err("A build is already running — cancel it first.".into());
    }
    let id = format!("build-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let cancel = CancelToken::new();
    *guard =
        Some(Job { id: id.clone(), state: JobState::Running, cancel: cancel.clone(), log: Vec::new(), channel: None });
    Ok(Plan { id, cancel, sources, config, opts, out_dir: maps_dir, filename })
}

/// Start a build. Returns its job id; everything after that arrives on `channel`.
pub fn start(
    jobs: Arc<Jobs>,
    req: BuildRequest,
    maps_dir: std::path::PathBuf,
    channel: Channel<BuildEvent>,
) -> Result<String, String> {
    let plan = plan(&jobs, req, maps_dir)?;
    let id = plan.id.clone();
    jobs.attach(&id, channel);
    let worker = Arc::clone(&jobs);
    std::thread::Builder::new()
        .name(format!("obc-build-{id}"))
        .spawn(move || {
            let id = plan.id.clone();
            let cancelled = plan.cancel.clone();
            let state = match run(&worker, plan) {
                Ok(done) => {
                    worker.emit(&id, done);
                    JobState::Done
                }
                Err(_) if cancelled.is_cancelled() => {
                    worker.emit(&id, BuildEvent::Cancelled);
                    JobState::Cancelled
                }
                Err(message) => {
                    worker.emit(&id, BuildEvent::Error { message });
                    JobState::Error
                }
            };
            worker.finish(&id, state);
        })
        .map_err(|e| format!("start the build thread: {e}"))?;
    Ok(id)
}

fn run(jobs: &Arc<Jobs>, plan: Plan) -> Result<BuildEvent, String> {
    let Plan { id, cancel, sources, config, opts, out_dir, filename } = plan;

    // --- The sources. Cached extracts are reused across the CLI, the dev server
    // and the app (they share `~/.cache/obcm/pbf`), so this is usually a no-op. ---
    let cache = crate::paths::pbf_cache();
    let mut pbfs: Vec<String> = Vec::with_capacity(sources.len());
    for (region_id, url) in &sources {
        // `<cache>/<region id>.osm.pbf`, the dev server's layout — 53 of the 555
        // Geofabrik ids contain a `/` (`us/california`), so those nest a
        // directory deep, and `http::download` creates it. Sharing the layout is
        // the point: an extract either host downloaded is one the other finds.
        // The id is not user input by the time it gets here — `pbf_urls` has
        // already matched it against the index, so it is one of those 555.
        let dest = cache.join(format!("{region_id}.osm.pbf"));
        if dest.metadata().is_ok_and(|m| m.len() > 0) {
            jobs.emit(&id, BuildEvent::Log { line: format!("Using cached PBF for {region_id}") });
        } else {
            jobs.emit(&id, BuildEvent::Status { status: "downloading", detail: region_id.clone() });
            let reporter = Progress::new(cancel.clone(), |_, _| {});
            let bytes = crate::http::download(url, &dest, &reporter, |pct| {
                jobs.emit(&id, BuildEvent::Progress { phase: "download", region: region_id.clone(), pct });
            })?;
            jobs.emit(&id, BuildEvent::Log { line: format!("Downloaded {region_id} ({bytes} bytes)") });
        }
        pbfs.push(dest.to_string_lossy().into_owned());
    }

    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let out_path = crate::paths::unique_in(&out_dir, &filename);
    jobs.emit(&id, BuildEvent::Status { status: "converting", detail: "starting".into() });

    // --- The pack. One call, the same one `obc-pack` makes. ---
    let sink_jobs = Arc::clone(jobs);
    let sink_id = id.clone();
    let progress = Progress::new(cancel, move |phase, line| {
        if let Some(phase) = phase {
            sink_jobs.emit(&sink_id, BuildEvent::Status { status: "converting", detail: phase.as_str().into() });
        }
        sink_jobs.emit(&sink_id, BuildEvent::Log { line: line.to_string() });
    });
    let summary = pack(&pbfs, &config, &out_path, &opts, &progress).map_err(|e| match e {
        PackError::Cancelled => "build cancelled".to_string(),
        PackError::Failed(e) => e,
    })?;

    Ok(BuildEvent::Done {
        path: out_path.to_string_lossy().into_owned(),
        filename: out_path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        size: summary.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(regions: &[&str]) -> BuildRequest {
        BuildRequest {
            region_ids: regions.iter().map(|s| (*s).to_string()).collect(),
            config: serde_json::json!({"lods": [{"max_mpp": null, "simplify": 0}], "features": {}}),
            chunk_size: None,
            output_name: "test.obcm".into(),
            bbox: None,
        }
    }

    #[test]
    fn an_empty_region_list_is_refused_before_a_job_exists() {
        let jobs = Arc::new(Jobs::default());
        assert!(plan(&jobs, request(&[]), std::env::temp_dir()).is_err());
        assert!(jobs.snapshot().is_none(), "a rejected request must not occupy the build slot");
    }

    #[test]
    fn a_config_the_packer_rejects_fails_the_request_not_the_build() {
        let jobs = Arc::new(Jobs::default());
        let mut req = request(&["monaco"]);
        req.config = serde_json::json!({"lods": "not a list"});
        assert!(plan(&jobs, req, std::env::temp_dir()).is_err());
        assert!(jobs.snapshot().is_none());
    }

    #[test]
    fn an_inside_out_bbox_is_refused_up_front() {
        let jobs = Arc::new(Jobs::default());
        let mut req = request(&["monaco"]);
        req.bbox = Some([10.0, 10.0, 9.0, 9.0]);
        let Err(err) = plan(&jobs, req, std::env::temp_dir()) else {
            panic!("an inside-out bbox must be refused");
        };
        assert!(err.to_lowercase().contains("bbox") || err.contains("W"), "unhelpful error: {err}");
    }

    /// The slot, without going near the network: a job is installed, it is the one
    /// `snapshot` reports, `cancel` reaches it by id and not by any other id, and
    /// the slot refuses a second build while it is running.
    #[test]
    fn the_single_build_slot_is_owned_by_one_job_at_a_time() {
        let jobs = Arc::new(Jobs::default());
        {
            let mut guard = jobs.0.lock().expect("lock");
            *guard = Some(Job {
                id: "build-1".into(),
                state: JobState::Running,
                cancel: CancelToken::new(),
                log: Vec::new(),
                channel: None,
            });
        }
        assert_eq!(jobs.snapshot().map(|s| s.id), Some("build-1".into()));
        assert!(!jobs.cancel("build-2"), "cancelling an unknown id must not touch the running one");
        assert!(jobs.cancel("build-1"));
        assert!(!jobs.attach("build-2", Channel::new(|_| Ok(()))));
        // A second request while one runs is refused rather than queued.
        assert!(plan(&jobs, request(&["monaco"]), std::env::temp_dir()).is_err());
    }

    /// A real build through the app's own backend, compared byte for byte with
    /// the CLI's — the acceptance criterion of #906, run rather than argued.
    ///
    /// Ignored by default because it is the one test here that needs the world:
    /// a cached `monaco.osm.pbf`, the Geofabrik index, and the land dataset. Run
    /// it on a machine that has them:
    ///
    /// ```text
    /// cargo build --release -p obc-pack --manifest-path ../Cargo.toml
    /// cargo test --manifest-path firmware/obc-desktop/Cargo.toml -- --ignored --nocapture
    /// ```
    ///
    /// What it proves that `obc-pack`'s own parity test does not: the path from
    /// a `BuildRequest` — region ids, a preset as the frontend posts it, the
    /// cache lookup — reaches the same bytes. The only thing between this and a
    /// click on "Build map" is the webview.
    #[test]
    #[ignore = "needs a cached monaco extract, the Geofabrik index and the land dataset"]
    fn the_app_and_the_cli_produce_the_same_bytes() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let preset = repo.join("packer/presets/default.json");
        let out_dir = std::env::temp_dir().join(format!("obc-desktop-parity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out_dir);

        let jobs = Arc::new(Jobs::default());
        let seen = Arc::new(Mutex::new(Vec::<BuildEvent>::new()));
        let sink = Arc::clone(&seen);
        let channel = Channel::new(move |body| {
            // The channel hands over the serialized IPC body; deserializing it
            // back is what a window would do, so a shape the frontend cannot
            // read fails here.
            if let tauri::ipc::InvokeResponseBody::Json(json) = &body {
                if let Ok(ev) = serde_json::from_str::<serde_json::Value>(json) {
                    sink.lock().expect("lock").push(BuildEvent::Log { line: ev.to_string() });
                }
            }
            Ok(())
        });

        let req = BuildRequest {
            region_ids: vec!["monaco".into()],
            // Exactly what the build card posts: the preset's config, `_meta`
            // and all (the parser ignores it — `config::tests::
            // unknown_tooling_metadata_remains_compatible`).
            config: serde_json::from_str(&std::fs::read_to_string(&preset).expect("preset")).expect("preset json"),
            chunk_size: Some(4096),
            output_name: "monaco-app.obcm".into(),
            bbox: None,
        };
        let id = start(Arc::clone(&jobs), req, out_dir.clone(), channel).expect("start");

        // Poll rather than block: the worker owns the job, and this is the same
        // "watch until it settles" the window does.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            match jobs.snapshot().expect("job").state {
                JobState::Running => {
                    assert!(std::time::Instant::now() < deadline, "the build did not finish in 10 minutes");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                JobState::Done => break,
                other => panic!("build ended {other:?}: {:?}", seen.lock().expect("lock").last()),
            }
        }
        assert_eq!(jobs.snapshot().expect("job").id, id);

        let app_out = out_dir.join("monaco-app.obcm");
        let cli_out = out_dir.join("monaco-cli.obcm");
        let cli = std::process::Command::new(repo.join("firmware/target/release/obc-pack"))
            .arg(crate::paths::pbf_cache().join("monaco.osm.pbf"))
            .arg(&preset)
            .arg(&cli_out)
            .arg("--chunk-size")
            .arg("4096")
            .output()
            .expect("run obc-pack (build it first: cargo build --release -p obc-pack)");
        assert!(cli.status.success(), "obc-pack failed: {}", String::from_utf8_lossy(&cli.stderr));

        let a = std::fs::read(&app_out).expect("the app's map");
        let b = std::fs::read(&cli_out).expect("the cli's map");
        let digest = |bytes: &[u8]| {
            use sha2::{Digest, Sha256};
            Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        println!("app {} bytes  sha256 {}", a.len(), digest(&a));
        println!("cli {} bytes  sha256 {}", b.len(), digest(&b));
        assert_eq!(a, b, "the app's map and the CLI's map must be the same bytes");
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    /// Cancellation, measured rather than asserted: the same build runs twice,
    /// once to completion and once cancelled **mid-pack** — the token is tripped
    /// the moment the packer reports it has started building a quadtree, which
    /// is the phase a real build spends its GEOS time in. The cancelled run has
    /// to be decisively shorter. An implementation that only checks its flag
    /// between stages would still finish the LOD it was in, and on a bigger
    /// region that is most of the wall clock.
    ///
    /// **Run it in `--release`.** In a debug build the tail is dominated by
    /// *teardown* rather than by work — dropping a country's worth of ingest
    /// geometry runs unoptimized drop glue and takes tens of seconds — so the
    /// ratio stops meaning what it says. The bound below is loosened for debug
    /// rather than skipped, because a completely broken cancel should still fail
    /// there.
    ///
    /// `OBC_TEST_TRACE=1` prints every event with its timestamp, which is how you
    /// find out *where* a cancel is stuck when this fails. (It is how the LOD-skip
    /// checkpoint in `pipeline.rs` got written: the first version of this test
    /// showed 28 s of the 53 s still running, all of it in the two LODs that were
    /// yet to start.)
    ///
    /// Ignored for the same reason as the parity test — it packs a real region.
    #[test]
    #[ignore = "needs a cached extract, the Geofabrik index and the land dataset; run with --release"]
    fn cancelling_actually_stops_the_work() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let preset = repo.join("packer/presets/default.json");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&preset).expect("preset")).expect("preset json");

        // The stage line that means the packer is inside its own GEOS work.
        const MID_PACK: &str = "Building Quadtree";
        // Monaco keeps the default run quick. A cancel matters most on a region
        // big enough for the quadtree phase to dominate, so the region is a knob:
        // `OBC_TEST_REGION=freiburg-regbez cargo test -- --ignored --nocapture`
        // (whatever you name must already be in ~/.cache/obcm/pbf).
        let region = std::env::var("OBC_TEST_REGION").unwrap_or_else(|_| "monaco".into());
        println!("region: {region}");

        /// One run, timed around the moment the packer entered the quadtree.
        struct Run {
            total: std::time::Duration,
            /// Elapsed when the first `MID_PACK` line arrived.
            mark: std::time::Duration,
            state: JobState,
            dir: std::path::PathBuf,
        }

        let run_once = |cancel_mid_pack: bool| -> Run {
            let out_dir = std::env::temp_dir().join(format!(
                "obc-desktop-cancel-{}-{}",
                std::process::id(),
                if cancel_mid_pack { "stop" } else { "full" }
            ));
            let _ = std::fs::remove_dir_all(&out_dir);
            let jobs = Arc::new(Jobs::default());
            let trip = Arc::clone(&jobs);
            let id_slot = Arc::new(Mutex::new(String::new()));
            let id_for_channel = Arc::clone(&id_slot);
            let started = std::time::Instant::now();
            let mark: Arc<Mutex<Option<std::time::Duration>>> = Arc::default();
            let mark_sink = Arc::clone(&mark);
            let channel = Channel::new(move |body| {
                let tauri::ipc::InvokeResponseBody::Json(json) = &body else { return Ok(()) };
                if std::env::var_os("OBC_TEST_TRACE").is_some() {
                    println!("[{:?}] {json}", started.elapsed());
                }
                if !json.contains(MID_PACK) {
                    return Ok(());
                }
                let mut mark = mark_sink.lock().expect("lock");
                if mark.is_none() {
                    *mark = Some(started.elapsed());
                    if cancel_mid_pack {
                        let id = id_for_channel.lock().expect("lock").clone();
                        assert!(!id.is_empty(), "the job id must be known before the packer reports a phase");
                        trip.cancel(&id);
                    }
                }
                Ok(())
            });
            let req = BuildRequest {
                region_ids: vec![region.clone()],
                config: config.clone(),
                chunk_size: Some(4096),
                output_name: "region.obcm".into(),
                bbox: None,
            };
            let id = start(Arc::clone(&jobs), req, out_dir.clone(), channel).expect("start");
            *id_slot.lock().expect("lock") = id;
            loop {
                let state = jobs.snapshot().expect("job").state;
                if state != JobState::Running {
                    let mark = mark.lock().expect("lock").expect("the packer never reached the quadtree");
                    return Run { total: started.elapsed(), mark, state, dir: out_dir };
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };

        let full = run_once(false);
        let stopped = run_once(true);
        // Comparing totals would be the wrong measurement: everything *before*
        // the trip point runs either way, and on a region as small as Monaco
        // that is most of the clock. What cancellation is answerable for is the
        // tail — how much of the remaining work still ran after the token flipped.
        let remaining = full.total - full.mark;
        let tail = stopped.total - stopped.mark;
        println!("full build   {:?} total, {remaining:?} of it after the first quadtree", full.total);
        println!("cancelled    {:?} total, {tail:?} of it after the cancel", stopped.total);
        println!(
            "             the run stopped in {:.1}% of the work it had left",
            tail.as_secs_f64() / remaining.as_secs_f64() * 100.0
        );

        assert_eq!(full.state, JobState::Done);
        assert_eq!(stopped.state, JobState::Cancelled);
        // What is left in the tail even when everything works is the teardown:
        // freeing the ingest buffers, which IS the "frees the memory" half of the
        // acceptance criterion rather than work that refused to stop.
        let budget = if cfg!(debug_assertions) { 1 } else { 4 };
        assert!(
            tail * budget < remaining,
            "after the cancel the build ran another {tail:?} of the {remaining:?} it had left — \
             the token is not reaching inside the per-feature work (OBC_TEST_TRACE=1 to see where)"
        );
        // …and left nothing that looks like a map behind.
        assert!(!stopped.dir.join("region.obcm").exists(), "a cancelled build left a partial .obcm");
        let _ = std::fs::remove_dir_all(&full.dir);
        let _ = std::fs::remove_dir_all(&stopped.dir);
    }

    #[test]
    fn the_log_is_replayed_to_a_channel_that_attaches_late() {
        let jobs = Arc::new(Jobs::default());
        {
            let mut guard = jobs.0.lock().expect("lock");
            *guard = Some(Job {
                id: "build-1".into(),
                state: JobState::Running,
                cancel: CancelToken::new(),
                log: Vec::new(),
                channel: None,
            });
        }
        jobs.emit("build-1", BuildEvent::Log { line: "first".into() });
        jobs.emit("build-1", BuildEvent::Log { line: "second".into() });

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let channel = Channel::new(move |body| {
            sink.lock().expect("lock").push(format!("{body:?}"));
            Ok(())
        });
        assert!(jobs.attach("build-1", channel));
        assert_eq!(seen.lock().expect("lock").len(), 2, "a reattached window must see what it missed");
    }
}
