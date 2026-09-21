//! What a packing run says about itself while it runs, and how it is stopped.
//!
//! The narration is a [`Progress`] sink the caller supplies: the CLI's prints the line, and the
//! desktop app's forwards `(phase, line)` to the webview.
//!
//! Two rules keep the two hosts from drifting apart. The phase vocabulary is [`Phase`] and it is
//! closed, because the build UI derives a percentage from a phase's index, so a phase is a value
//! with an order and not a string someone typed; the web builder holds the same list, pinned by a
//! test. And the line is still the CLI's line: every `progress.stage()` call passes the sentence the
//! packer would print, so a terminal and a log pane show the same text.
//!
//! Cancellation shares the struct because it shares the call sites: everywhere the pipeline is far
//! enough along to say where it is, it is also far enough along to notice it should stop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The coarse phases of a build, in the order they happen.
///
/// Order is load-bearing: the UI turns a phase's index into a percentage, so a phase must never be
/// reported after a later-indexed one. Sub-steps live in the line, not in a new variant, so a
/// pipeline change cannot move the progress bar's meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Several `.pbf` sources are being folded together (multi-region builds only).
    Merging,
    /// The `.pbf` reading passes, including the `--bbox` crop's pass 0.
    Ingest,
    /// The global bounding box over everything ingested.
    Bbox,
    /// Clipping the land-polygon dataset to that box.
    Land,
    /// Tracing contours out of the run's OBCT terrain (only when the config asks for them).
    Contours,
    /// Per-LOD simplify + quadtree build.
    Quadtree,
    /// Packing a tree into bytes and streaming it to the output.
    Serialize,
}

impl Phase {
    /// Every phase, in order. The UI's percentage scale is derived from this.
    pub const ALL: [Phase; 7] =
        [Phase::Merging, Phase::Ingest, Phase::Bbox, Phase::Land, Phase::Contours, Phase::Quadtree, Phase::Serialize];

    /// The wire name. Matches the strings in `jobs.svelte.ts`'s `PHASES`.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Merging => "merging",
            Phase::Ingest => "ingest",
            Phase::Bbox => "bbox",
            Phase::Land => "land",
            Phase::Contours => "contours",
            Phase::Quadtree => "quadtree",
            Phase::Serialize => "serialize",
        }
    }
}

/// A flag one thread sets and the pipeline reads. Cloneable so the caller keeps a
/// handle after handing one to [`Progress`] — that handle is the only way to stop
/// a build that is already running.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the run to stop. Idempotent, and safe from any thread.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

type Sink = dyn Fn(Option<Phase>, &str) + Send + Sync;

/// Where a run's narration goes, and whether it should still be running.
///
/// `Send + Sync` because the ingest passes and the per-feature simplify are
/// rayon-parallel: every worker reports and every worker checks.
pub struct Progress {
    sink: Box<Sink>,
    warn: Box<Sink>,
    cancel: CancelToken,
}

impl Progress {
    /// The CLI's reporter: the line, on stdout. Warnings keep their stderr stream, so redirecting
    /// stdout to a log still separates them.
    pub fn stdout() -> Self {
        Progress {
            sink: Box::new(|_, line| println!("{line}")),
            warn: Box::new(|_, line| eprintln!("{line}")),
            cancel: CancelToken::new(),
        }
    }

    /// Says nothing and never cancels — for tests and for callers that only want the artifact.
    pub fn silent() -> Self {
        Progress { sink: Box::new(|_, _| {}), warn: Box::new(|_, _| {}), cancel: CancelToken::new() }
    }

    /// A reporter that forwards every line to `sink`, cancellable through `cancel`. Warnings arrive
    /// at the same sink with no phase, because a host with one log pane has one place to put them.
    pub fn new(cancel: CancelToken, sink: impl Fn(Option<Phase>, &str) + Send + Sync + 'static) -> Self {
        let sink = Arc::new(sink);
        let warn = Arc::clone(&sink);
        Progress {
            sink: Box::new(move |phase, line| sink(phase, line)),
            warn: Box::new(move |_, line| warn(None, line)),
            cancel,
        }
    }

    /// Enter `phase`, announcing it with `line`.
    pub fn stage(&self, phase: Phase, line: impl AsRef<str>) {
        (self.sink)(Some(phase), line.as_ref());
    }

    /// A line that belongs to whatever phase is current — a count, a summary.
    pub fn log(&self, line: impl AsRef<str>) {
        (self.sink)(None, line.as_ref());
    }

    /// Something the operator must see but that does not stop the build.
    pub fn warn(&self, line: impl AsRef<str>) {
        (self.warn)(None, line.as_ref());
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// `Err` once the run has been cancelled, so a `?` at a checkpoint unwinds the pipeline.
    ///
    /// This is a relaxed atomic load, cheap enough to sit inside the per-blob and per-feature loops,
    /// and that is where it sits: per `.pbf` blob in the ingest passes, per shapefile record in the
    /// land clip, per style class in the merge passes, per feature in the simplify, cull and
    /// quadtree build, and per LOD in the pipeline's closure, which skips the level whole.
    ///
    /// What it cannot interrupt is a single call below those: one GEOS operation on one very large
    /// geometry runs to completion, and so does the teardown, since freeing a country's worth of
    /// ingested geometry is measured in seconds.
    ///
    /// The message is filler: [`crate::pipeline::pack`] consults the token, not the string.
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            return Err("build cancelled".into());
        }
        Ok(())
    }
}

/// How a pack run ended when it didn't produce a map.
///
/// Cancellation is its own variant rather than an error string because the callers
/// treat it differently: a UI reports a failure and keeps the log open, but a
/// cancelled build is what the user just asked for and says so quietly.
#[derive(Debug)]
pub enum PackError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::Cancelled => f.write_str("build cancelled"),
            PackError::Failed(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for PackError {}

impl From<String> for PackError {
    fn from(e: String) -> Self {
        PackError::Failed(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn phase_order_is_the_uis_percentage_scale() {
        let names: Vec<&str> = Phase::ALL.iter().map(|p| p.as_str()).collect();
        // Mirrors the web builder's PHASES, minus "downloading", which is the host's own phase: the
        // packer is handed local files and never downloads a source.
        assert_eq!(names, ["merging", "ingest", "bbox", "land", "contours", "quadtree", "serialize"]);
        assert!(Phase::ALL.windows(2).all(|w| w[0] < w[1]), "ALL must be in reported order");
    }

    #[test]
    fn a_cancelled_token_makes_every_checkpoint_fail() {
        let cancel = CancelToken::new();
        let p = Progress::new(cancel.clone(), |_, _| {});
        assert!(p.check().is_ok());
        cancel.cancel();
        assert!(p.check().is_err());
        assert!(p.is_cancelled());
    }

    #[test]
    fn a_sink_sees_the_phase_and_the_line() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let p = Progress::new(CancelToken::new(), move |phase, line| {
            sink.lock().unwrap().push((phase, line.to_string()));
        });
        p.stage(Phase::Ingest, "Pass 1: reading nodes...");
        p.log("  12 features");
        p.warn("warning: something");
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0], (Some(Phase::Ingest), "Pass 1: reading nodes...".to_string()));
        assert_eq!(seen[1], (None, "  12 features".to_string()));
        assert_eq!(seen[2], (None, "warning: something".to_string()));
    }
}
