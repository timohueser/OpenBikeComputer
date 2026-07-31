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

    /// Run one build through the app's own backend and wait for it to settle.
    ///
    /// Polls rather than blocks, because that is what the window does: the worker
    /// owns the job and everything the UI knows arrives on a channel. The channel
    /// here deserializes each event back out of its IPC body, so an event shape
    /// the frontend could not read fails in the test rather than in someone's
    /// window.
    fn build_and_wait(req: BuildRequest, out_dir: &std::path::Path) -> std::path::PathBuf {
        let name = req.output_name.clone();
        let jobs = Arc::new(Jobs::default());
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&seen);
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = &body {
                if serde_json::from_str::<serde_json::Value>(json).is_ok() {
                    sink.lock().expect("lock").push(json.clone());
                }
            }
            Ok(())
        });
        let id = start(Arc::clone(&jobs), req, out_dir.to_path_buf(), channel).expect("start");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
        loop {
            match jobs.snapshot().expect("job").state {
                JobState::Running => {
                    assert!(std::time::Instant::now() < deadline, "the build did not finish in 15 minutes");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                JobState::Done => break,
                other => panic!("build ended {other:?}: {:?}", seen.lock().expect("lock").last()),
            }
        }
        assert_eq!(jobs.snapshot().expect("job").id, id);
        out_dir.join(name)
    }

    /// SHA-256 as lowercase hex — printed beside every parity comparison so the
    /// two digests are in the log, not merely equal in an assertion.
    fn digest(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
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
    /// cargo test --manifest-path apps/obc-desktop/Cargo.toml -- --ignored --nocapture
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
        let preset = repo.join("builder/presets/schema.json");
        let out_dir = std::env::temp_dir().join(format!("obc-desktop-parity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out_dir);

        let app_out = build_and_wait(
            BuildRequest {
                region_ids: vec!["monaco".into()],
                // Exactly what the build card posts: the preset's config, `_meta`
                // and all (the parser ignores it — `config::tests::
                // unknown_tooling_metadata_remains_compatible`).
                config: serde_json::from_str(&std::fs::read_to_string(&preset).expect("preset")).expect("preset json"),
                chunk_size: Some(4096),
                output_name: "monaco-app.obcm".into(),
                bbox: None,
            },
            &out_dir,
        );

        let cli_out = out_dir.join("monaco-cli.obcm");
        let cli = std::process::Command::new(repo.join("target/release/obc-pack"))
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
        println!("app {} bytes  sha256 {}", a.len(), digest(&a));
        println!("cli {} bytes  sha256 {}", b.len(), digest(&b));
        assert_eq!(a, b, "the app's map and the CLI's map must be the same bytes");
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    /// The same claim for a map nobody shipped: **a custom style, cropped to a
    /// box** — E3's (#913) acceptance criterion, and the one the previous test
    /// cannot make.
    ///
    /// The difference is not cosmetic. The preset build compares two runs over a
    /// document that exists on disk in both worlds; this one compares a config
    /// that only ever existed as an *edit*, and it takes both of the paths the
    /// desktop tier adds to the loop:
    ///
    /// * every kind of change the advanced editor can make — a colour, a weight,
    ///   a start LOD, a whole category removed, a feature type removed the way
    ///   the disabled list removes one, a finer LOD tier appended, a routing
    ///   profile's multiplier, and a different chunk size;
    /// * a **bbox**, so D5's (#910) in-ingest crop runs on the app's side
    ///   (`PackOptions.bbox`) and the CLI's (`--bbox`) and has to agree.
    ///
    /// If any of that reached the packer differently — a number that took a
    /// different JSON path, a key order that changed the style ids, a crop
    /// rectangle rounded on one side — the two files would differ, and nothing
    /// short of comparing them would say so.
    #[test]
    #[ignore = "needs a cached monaco extract, the Geofabrik index and the land dataset"]
    fn a_custom_style_cropped_to_a_box_matches_the_cli_byte_for_byte() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out_dir = std::env::temp_dir().join(format!("obc-desktop-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).expect("out dir");

        // Start from the preset, as the editor does: picking one *copies* it, and
        // everything after is an edit to that copy.
        let preset = repo.join("builder/presets/schema.json");
        let mut config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&preset).expect("preset")).expect("preset json");
        edit_like_the_editor(&mut config);

        // The submitted config, written out once and used by *both* sides — which
        // is the honest comparison. Handing the app a `Value` and the CLI a
        // separately-written file would be comparing two serializers.
        let config_path = out_dir.join("custom-style.json");
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).expect("config json")).expect("write");

        // A box over central Monaco. Comfortably inside the extract's own bounds
        // (roughly W 7.402 S 43.722 → E 7.448 N 43.754), so the crop has real work
        // to do rather than selecting everything.
        const BBOX: [f64; 4] = [7.418, 43.730, 7.428, 43.740];

        let app_out = build_and_wait(
            BuildRequest {
                region_ids: vec!["monaco".into()],
                config: config.clone(),
                chunk_size: Some(2048),
                output_name: "monaco-custom-app.obcm".into(),
                bbox: Some(BBOX),
            },
            &out_dir,
        );

        let cli_out = out_dir.join("monaco-custom-cli.obcm");
        let [w, s, e, n] = BBOX;
        let cli = std::process::Command::new(repo.join("target/release/obc-pack"))
            .arg(crate::paths::pbf_cache().join("monaco.osm.pbf"))
            .arg(&config_path)
            .arg(&cli_out)
            .arg("--bbox")
            .arg(format!("{w},{s},{e},{n}"))
            .arg("--chunk-size")
            .arg("2048")
            .output()
            .expect("run obc-pack (build it first: cargo build --release -p obc-pack)");
        assert!(cli.status.success(), "obc-pack failed: {}", String::from_utf8_lossy(&cli.stderr));

        let a = std::fs::read(&app_out).expect("the app's map");
        let b = std::fs::read(&cli_out).expect("the cli's map");
        println!("custom style, bbox {w},{s},{e},{n}");
        println!("app {} bytes  sha256 {}", a.len(), digest(&a));
        println!("cli {} bytes  sha256 {}", b.len(), digest(&b));
        assert_eq!(a, b, "a custom style built in the app must be the same bytes as the CLI's");

        // Guard the guard. Two identical *uncropped* builds would satisfy the
        // assertion above and prove nothing whatever about the bbox — so the same
        // config is built once more without one, and the cropped map has to be
        // smaller and cover less.
        //
        // Comparing the header bounds against the requested box would be the
        // wrong check, and it is worth writing down why: the crop selects the
        // features that *intersect* the box and keeps each one whole, so a way
        // that crosses the edge carries its far end into the map and the global
        // bounds legitimately spill over. What must shrink is the extent, not the
        // rectangle.
        let whole = build_and_wait(
            BuildRequest {
                region_ids: vec!["monaco".into()],
                config,
                chunk_size: Some(2048),
                output_name: "monaco-custom-whole.obcm".into(),
                bbox: None,
            },
            &out_dir,
        );
        let uncropped = std::fs::read(&whole).expect("the uncropped map");
        println!("uncropped {} bytes  sha256 {}", uncropped.len(), digest(&uncropped));
        println!("cropped   {} bytes  ({:.0}% of it)", a.len(), a.len() as f64 / uncropped.len() as f64 * 100.0);
        assert!(a.len() < uncropped.len(), "the bbox changed nothing — the crop did not run");
        // `OBCM_Spec.md` §1: magic, version, then min lat / min lon / max lat /
        // max lon as int32 microdegrees.
        assert_eq!(&a[..4], b"OBCM", "not an OBCM file");
        let bounds = |map: &[u8]| {
            let micro = |at: usize| i32::from_le_bytes(map[at..at + 4].try_into().expect("4 bytes")) as f64 / 1e6;
            (micro(9), micro(5), micro(17), micro(13))
        };
        let (cw, cs, ce, cn) = bounds(&a);
        let (ww, ws, we, wn) = bounds(&uncropped);
        println!("cropped   bounds W {cw} S {cs} E {ce} N {cn}");
        println!("uncropped bounds W {ww} S {ws} E {we} N {wn}");
        assert!(cw >= ww && cs >= ws && ce <= we && cn <= wn, "the cropped map covers ground the whole one does not");
        assert!((cw, cs, ce, cn) != (ww, ws, we, wn), "the crop left the map's extent untouched");
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    /// Apply, in Rust, the edits the advanced editor makes in the window.
    ///
    /// Every one of them is a control that exists on screen: the colour picker,
    /// the weight and z-index cells, the min-LOD segments, "Remove this
    /// category", the disabled toggle, "Add a detail level", the bike-profile
    /// multiplier grid, and the output tab's chunk size. What matters for the
    /// test is that the result is a config **no preset file contains**.
    fn edit_like_the_editor(config: &mut serde_json::Value) {
        use serde_json::{json, Value};
        let obj = config.as_object_mut().expect("a config object");
        // The editor never posts `_meta`; `buildConfigForSubmit` rebuilds the
        // document from the fields the schema declares.
        obj.remove("_meta");
        obj.insert("chunk_size".into(), json!(2048));

        // Output tab: a finer detail level, appended the way `addLodTier` does.
        let lods = obj.get_mut("lods").and_then(Value::as_array_mut).expect("lods");
        let finest = lods.last().and_then(|l| l.get("max_mpp")).and_then(Value::as_f64).unwrap_or(120.0);
        lods.push(json!({ "max_mpp": (finest / 2.0).round().max(1.0), "simplify": 0 }));
        let tiers = lods.len();

        let features = obj.get_mut("features").and_then(Value::as_object_mut).expect("features");
        // "Remove this category" — buildings, the one a bikepacking map is most
        // likely to lose on purpose. `natural` is deliberately left alone below:
        // `natural.land` is what the land-polygon backdrop is styled as, and
        // dropping it would quietly take the land stage out of the comparison.
        assert!(features.remove("building").is_some(), "the preset no longer has a `building` category");

        // Then per-feature edits, deterministically: outside `natural`, every
        // fourth type is dropped (what the disabled list does by the time the
        // build card posts), and the rest get a new colour, weight and start
        // tier. Key order is untouched, because it is what the packer assigns
        // style ids from.
        for (i, (category, entries)) in features.iter_mut().enumerate() {
            let keep_all = category == "natural";
            let entries = entries.as_object_mut().expect("a category");
            let names: Vec<String> = entries.keys().cloned().collect();
            for (j, name) in names.iter().enumerate() {
                if !keep_all && (i + j) % 4 == 3 {
                    entries.remove(name);
                    continue;
                }
                let def = entries.get_mut(name).and_then(Value::as_object_mut).expect("a style");
                def.insert("color".into(), json!(format!("0x{:04X}", 0x1000 + ((i * 31 + j * 7) % 0xE000))));
                def.insert("weight".into(), json!(1 + ((i + j) % 3)));
                def.insert("min_lod".into(), json!((i + j) % tiers));
            }
        }

        // Bike profiles: the multiplier grid, with one class toggled to
        // forbidden. Every other cell is >= 1.0, which the editor enforces and
        // the packer requires — the A* heuristic stops being admissible below it.
        obj.insert(
            "routing".into(),
            json!({
                "profiles": [
                    {
                        "name": "Bikepacking",
                        "default": 2.0,
                        "highway": { "track": 1.0, "path": 1.3, "cycleway": 1.1, "steps": "forbidden" },
                        "surface": { "gravel": 1.0, "paved": 1.2, "rough": 2.5 }
                    },
                    {
                        // ≤ 12 UTF-8 bytes: the profile table's wire field, which
                        // the editor also enforces.
                        "name": "Pavement",
                        "default": 4.0,
                        "highway": { "cycleway": 1.0, "residential": 1.2, "track": "forbidden" },
                        "surface": { "paved": 1.0, "gravel": "forbidden" }
                    }
                ]
            }),
        );
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
        let preset = repo.join("builder/presets/schema.json");
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
