//! The target-independent assembly driver: cell byte buffers in, output file byte buffers out, a
//! progress/abort seam, and a typed failure carrying the engine's own message.
//!
//! Nothing here knows about the browser — that is the point. `cargo test` runs this natively, so
//! the phase mapping, the error vocabulary and the byte-for-byte determinism pin are covered by the
//! workspace suite rather than only by whatever a browser happens to exercise.
//!
//! # How progress is observed without touching the engine
//!
//! [`obcm_assemble::assemble`] takes two host-supplied seams — a [`Clock`] and a [`ShardStore`] —
//! and calls them at exactly the points a progress bar cares about. That is the whole hook:
//!
//! * The engine calls the clock **once per phase boundary**, in a fixed order (`t_start`, `t_open`,
//!   `t_poi`, `t_nav`, `t_plan`, then per-shard timings). Ticks 1–5 therefore name the phases
//!   `open → poi → nav → plan → write`, and `tests/determinism.rs`'s
//!   `the_phase_sequence_is_the_one_the_engine_calls` pins that mapping so an engine refactor breaks
//!   a test instead of silently mislabelling a bar.
//! * Once the write loop starts, the store's own calls are unambiguous and take over:
//!   [`ShardStore::write`] means bytes of geometry are moving, [`ShardStore::source`] means the §4.8
//!   verify pass is about to read a sealed shard back, and [`ShardStore::manifest`] means the set is
//!   done.
//! * The verify pass itself is observed one level deeper. [`ShardStore::source`] is called **once
//!   per shard** and everything after it happens inside the engine, so a bar driven by store calls
//!   alone would sit still through §4.8 — which is **60 % of a measured region-scale run**, 9.6 s of
//!   baden-württemberg's 16.1 s (#1116's harness). [`VerifySource`] therefore wraps the sealed shard
//!   the engine reads back and reports from [`ByteSource::read_at`], which is the read-back's own
//!   inner loop.
//!
//! # Abort granularity
//!
//! [`Hooks::progress`] returns `true` to abort. It is called at every phase boundary, every time the
//! accumulated write crosses [`PROGRESS_STEP`] of the projected output, and every time the verify
//! read-back crosses the same step — i.e. roughly a hundred times over write+verify, which together
//! are ~83 % of a run.
//!
//! The request takes effect at the **next store call or verify read**, whichever comes first. Two
//! consequences worth stating plainly, because a UI's cancel button depends on them:
//!
//! * **Inside write or verify, cancellation is prompt** — the very next `write`/`read_at` refuses,
//!   and the refusal is reported as [`ErrorCode::Aborted`], never as a §4.8 [`ErrorCode::Verify`]
//!   defect ([`map_error`] reads the abort flag before it reads the engine's error class, because
//!   `verify_shard` turns any read failure into `Error::Verify` and a cancelled run must never look
//!   like a broken assembler).
//! * **Inside the nav rewrite it is not.** §4.6 makes no store calls at all, so an abort requested
//!   during it is honoured when the phase ends. That is now the *only* uninterruptible stretch, and
//!   at a measured 16 % of the run it is the shorter of the two the bridge used to be blind to.
//!   Making it finer needs a seam inside the engine (see the PR's engine-API follow-ups).
//!
//! And the constraint that outranks all of this: [`assemble_cells`] **blocks**. A Bundesland-scale
//! assembly is ~16 s of straight-line compute, so it must run in a **Web Worker** — see the contract
//! paragraph in `builder/app/src/lib/assemble/bridge.ts`. A cooperative abort cannot be observed
//! from a main thread that is itself blocked; the UI's cancel is `worker.terminate()`, and this seam
//! is for the policies the worker runs *itself* (a memory watchdog, a deadline).
//!
//! # Handing shards out as they are verified (#1116 B1)
//!
//! The store above keeps every sealed shard until the run ends, so a browser's peak carries the
//! **whole output** on top of the whole input. It does not have to: in the engine's write/verify
//! loop, [`ShardStore::source`] for shard *i* is only ever called inside the iteration that wrote
//! shard *i*, and once that §4.8 pass returns nothing reads those bytes again — `check_set_invariants`
//! and the manifest are built from the plans. So the moment the engine asks for the **next**
//! [`ShardStore::begin`], or for the [`ShardStore::manifest`], shard *i*'s buffer is dead weight.
//!
//! [`Hooks::take_shard`] is that seam, and it is **opt-in**: a caller turns it on by returning `true`
//! from [`Hooks::wants_shards`], and then receives each shard — name, role, digest, bytes — at the
//! first store call after its verify. What it takes does **not** appear in [`Outcome::files`]. The
//! default is the old behaviour to the byte: no hand-off, the whole set in `files`, which is what
//! [`NoHooks`] and every native caller still get.
//!
//! Two consequences a caller must plan for:
//!
//! * **A failed or cancelled run may already have handed shards out.** Cleaning those up is the
//!   caller's job. It is safe by construction rather than by cleanup, though: the OBCS manifest is
//!   written **last** (OBCA §5.4), so a partial set is not a map to a device however many `.OBM`
//!   files it has.
//! * **The terrain shard and the manifest are not evicted.** Terrain is written through the engine's
//!   own sink, not the store, and the manifest is the last thing that exists; both stay in
//!   [`Outcome::files`].

use std::cell::RefCell;

use obc_formats::io::{ByteSource, SliceSource};
use obcm_assemble::grid::CellId;
use obcm_assemble::schema::{Schema, Skin};
use obcm_assemble::{
    assemble_full, CellInput, Clock, Error, KnownEmptyInput, MemorySource, Options, ShardPlan, ShardStore,
    TerrainCellInput, TerrainJob, TerrainParams,
};
use sha2::{Digest, Sha256};

/// One downloaded cell, as the caller hands it over: the catalog's identity plus the verified bytes.
///
/// `band` is deliberately not inferable from the bytes (OBCA §3.1 — a legitimately empty cell is
/// indistinguishable from an out-of-band one), so the catalog states it.
pub struct CellBytes {
    /// The canonical cell id, `<log2>/<i>/<j>`.
    pub id: String,
    pub band: String,
    /// The catalog's `partial` flag (OBCA §3.7).
    pub partial: bool,
    pub bytes: Vec<u8>,
}

/// One selected cell which the pinned catalog asserts has canonical empty
/// content. It has an identity but deliberately no payload buffer.
pub struct KnownEmptyCell {
    pub id: String,
    pub band: String,
}

/// One downloaded terrain cell (EL4): its id on the terrain grid, the whole `.obcd` object, and the
/// `sha256` the pinned terrain index published for it.
///
/// A known-empty terrain square is simply **not handed over** — it has no object to fetch
/// (`OBCC_Spec.md` §13.6) and an absent cell reads identically to an all-`NODATA` one, so it needs
/// no identity here the way an empty *band* cell does.
pub struct TerrainCellBytes {
    /// The canonical cell id, `<cell_log2>/<i>/<j>`, on the terrain grid.
    pub id: String,
    /// Lowercase-hex SHA-256 from the terrain index. Empty means "no catalog", which is not a
    /// case the browser has — it is here so the type can be built in a test.
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// The terrain store's lattice, verbatim from the catalog's `terrain` block (`OBCC_Spec.md` §13.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainLattice {
    pub posting_log2: u8,
    pub cell_log2: u8,
}

/// What an assembly can be told to do differently — [`obcm_assemble::Options`] minus `skip_verify`.
///
/// The omission is deliberate and is the issue's own requirement: OBCA §4.8 makes verification a
/// **precondition of writing a set**, and this bridge exists to hand bytes to a device. There is no
/// browser caller for whom skipping the read-back is the right answer, so the knob is not offered.
#[derive(Clone, Debug)]
pub struct BridgeOptions {
    /// The set's display name (24 bytes on the wire, OBCA §5.2).
    pub name: String,
    /// The card id the derived filenames use (`MS<id>S<kk>.OBM`), 0..=999.
    pub card_id: u16,
    /// Split a geometry shard wherever it exceeds this. Only bites once the map needs a set at all,
    /// or with `force_split`.
    pub target_shard_bytes: u64,
    /// Proceed although a selected cell is missing (OBCA §4.1).
    pub accept_holes: bool,
    /// Proceed although a cell is `partial` (OBCA §3.7).
    pub accept_partial: bool,
    /// Write a role-partitioned set even when the whole map would fit one file — smaller files are
    /// better resumable upload units.
    pub force_split: bool,
}

impl Default for BridgeOptions {
    fn default() -> Self {
        let d = Options::default();
        BridgeOptions {
            name: d.name,
            card_id: d.card_id,
            target_shard_bytes: d.target_shard_bytes,
            accept_holes: d.accept_holes,
            accept_partial: d.accept_partial,
            force_split: d.force_split,
        }
    }
}

impl BridgeOptions {
    /// Parse the options object the browser hands in — every field optional, defaults from
    /// [`Options::default`]. Read by hand rather than with `serde`'s derive because six fields are
    /// not worth a proc-macro dependency in a bundle that ships to every visitor, and because an
    /// unknown key here should be ignored (a newer builder talking to an older module) rather than
    /// fatal.
    pub fn parse(json: &str) -> Result<BridgeOptions, String> {
        let mut o = BridgeOptions::default();
        let trimmed = json.trim();
        if trimmed.is_empty() {
            return Ok(o);
        }
        let v: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| format!("options: {e}"))?;
        let Some(map) = v.as_object() else {
            return Err(format!("options: expected a JSON object, got {v}"));
        };
        for (key, value) in map {
            match key.as_str() {
                "name" => o.name = value.as_str().ok_or("options.name must be a string")?.to_string(),
                "cardId" => {
                    o.card_id = u16::try_from(value.as_u64().ok_or("options.cardId must be a number")?)
                        .map_err(|_| "options.cardId is out of range (0..=999)")?
                }
                "targetShardBytes" => {
                    o.target_shard_bytes = value.as_u64().ok_or("options.targetShardBytes must be a number")?
                }
                "acceptHoles" => o.accept_holes = value.as_bool().ok_or("options.acceptHoles must be a boolean")?,
                "acceptPartial" => {
                    o.accept_partial = value.as_bool().ok_or("options.acceptPartial must be a boolean")?
                }
                "forceSplit" => o.force_split = value.as_bool().ok_or("options.forceSplit must be a boolean")?,
                _ => {}
            }
        }
        Ok(o)
    }
}

/// Which stage of the assembly is running. The string form ([`Phase::as_str`]) is the **wire
/// contract** with the browser wrapper — renaming one is a breaking change for the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Opening every cell through the real reader and checking the §4.1 preconditions.
    Open,
    /// Merging, deduplicating and re-binning the POI section (§4.5).
    Poi,
    /// The nav rewrite (§4.6) — the engine's one O(rewrite) component, and the memory peak.
    Nav,
    /// Planning the volume set (§5).
    Plan,
    /// Writing the shards, which is where the verbatim geometry graft happens (§4.4).
    Write,
    /// Reading a sealed shard back through the real reader (§4.8).
    Verify,
    /// Writing the OBCS manifest, always last (§5.4).
    Manifest,
    /// Everything is written and verified.
    Done,
}

impl Phase {
    /// The stable identifier the browser wrapper re-exports as its `AssemblePhase` union. Keep
    /// these in sync with `builder/app/src/lib/assemble/bridge.ts`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Phase::Open => "open",
            Phase::Poi => "poi",
            Phase::Nav => "nav",
            Phase::Plan => "plan",
            Phase::Write => "write",
            Phase::Verify => "verify",
            Phase::Manifest => "manifest",
            Phase::Done => "done",
        }
    }

    /// This phase's share of the wall clock, and everything before it.
    ///
    /// Calibrated on the **measured** runs of #1116's `mem-profile` harness (macOS arm64, release,
    /// published v12 catalog, single-file fast path), which is where these have to come from now:
    /// the C-series (#1118, #1119, #1120) made the §4.8 read-back about a third faster and left
    /// write a far larger share of the run than PR #1027's switzerland measurement had it.
    ///
    /// * baden-württemberg, 215 cells / 795 MB: open 0.016 s · poi 0.013 s · nav 2.58 s · plan
    ///   0.004 s · write 3.82 s · verify 9.63 s of 16.05 s total — nav 0.160, write 0.238,
    ///   verify 0.600.
    /// * freiburg-regbez, 77 cells / 264 MB: open 0.029 s · poi 0.006 s · nav 0.82 s · plan 0.006 s
    ///   · write 1.26 s · verify 2.80 s of 4.92 s total — nav 0.167, write 0.256, verify 0.569.
    ///
    /// The weights follow **BW**, which is the shape a long run has; freiburg spends a little more
    /// of a shorter run in write and correspondingly less in verify, so the two bracket these
    /// numbers rather than disagreeing about them. The superseded weights had the bar running some
    /// 14 points behind reality all the way through write and then racing through verify.
    const fn weight(self) -> f64 {
        match self {
            Phase::Open => 0.002,
            Phase::Poi => 0.001,
            Phase::Nav => 0.163,
            Phase::Plan => 0.001,
            Phase::Write => 0.240,
            Phase::Verify => 0.593,
            Phase::Manifest | Phase::Done => 0.0,
        }
    }

    /// Fraction of the run complete when this phase starts.
    fn prefix(self) -> f64 {
        const ORDER: [Phase; 6] = [Phase::Open, Phase::Poi, Phase::Nav, Phase::Plan, Phase::Write, Phase::Verify];
        let mut sum = 0.0;
        for p in ORDER {
            if p == self {
                return sum;
            }
            sum += p.weight();
        }
        1.0
    }
}

/// Emit progress when the overall fraction has advanced by at least this much — about a hundred
/// callbacks over a whole assembly, plus one per phase boundary. Also the abort poll interval: the
/// flag can only be *set* by a callback, so polling it wherever one might have fired is exhaustive.
const PROGRESS_STEP: f64 = 0.01;

/// The host's side of the two seams: a clock the engine can read, and a progress sink that doubles
/// as the abort signal.
pub trait Hooks {
    /// Monotonic microseconds. `0` is a legal implementation (see [`obcm_assemble::NoClock`]) and
    /// only costs the reported phase split.
    fn now_us(&mut self) -> u64;
    /// Report `phase` with the overall completed `fraction` (0.0..=1.0). Return `true` to abort;
    /// see the module header for the granularity that request is honoured at.
    fn progress(&mut self, phase: Phase, fraction: f64) -> bool;

    /// Whether this caller wants each shard handed to it as soon as its §4.8 verify has passed,
    /// instead of holding the whole set until the run ends (#1116 B1).
    ///
    /// **This is the switch**, and [`Hooks::take_shard`] is never called without it — overriding
    /// that alone does nothing (`shards_are_only_handed_out_when_the_caller_asks` pins it). It is
    /// read once, before the assembly starts, and it also decides whether the store pays for the
    /// digest it hands over: a caller that keeps everything gets exactly the work it got before.
    fn wants_shards(&self) -> bool {
        false
    }

    /// Take ownership of one sealed, verified shard. Called at the first store call **after** the
    /// shard's §4.8 read-back — the next [`ShardStore::begin`], or the manifest — and only when
    /// [`Hooks::wants_shards`] said so.
    ///
    /// * `Ok(None)` — taken. The store frees its slot, and the file is **not** in [`Outcome::files`].
    /// * `Ok(Some(shard))` — handed back, kept as before. The default, so a hook that ignores this
    ///   method cannot silently lose a shard.
    /// * `Err(message)` — the sink failed and the bytes are gone. The run stops with
    ///   [`ErrorCode::Io`] carrying `message`, because a set missing a shard must never be reported
    ///   as finished.
    ///
    /// The `sha256` handed over is computed by this crate's own store as the bytes were written, and
    /// is cross-checked against the engine's digest when the run ends (a disagreement is
    /// [`ErrorCode::Internal`] — see [`assemble_everything`]).
    fn take_shard(&mut self, shard: OutputFile) -> Result<Option<OutputFile>, String> {
        Ok(Some(shard))
    }
}

/// Hooks that do nothing — the default for a caller that only wants the bytes.
pub struct NoHooks;

impl Hooks for NoHooks {
    fn now_us(&mut self) -> u64 {
        0
    }
    fn progress(&mut self, _phase: Phase, _fraction: f64) -> bool {
        false
    }
}

/// Why an assembly failed: a stable machine-readable [`ErrorCode`] plus the engine's own prose,
/// unchanged. The two travel together deliberately — a caller that wants to special-case one cause
/// branches on `code`, and everyone else displays `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembleFailure {
    pub code: ErrorCode,
    pub message: String,
}

impl AssembleFailure {
    fn new(code: ErrorCode, message: impl Into<String>) -> AssembleFailure {
        AssembleFailure { code, message: message.into() }
    }
}

impl core::fmt::Display for AssembleFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for AssembleFailure {}

/// Why an assembly failed. These are the engine's own refusal classes, kept apart because they mean
/// different things to a caller — and in particular because the issue requires that a §4.8 **verify**
/// failure (a defect in the engine, and the one class that must never be shipped past) is
/// distinguishable from a §4.1 **input** refusal (a selection to fix) and a §5.7 **capacity** refusal
/// (coverage to reduce).
///
/// The string form ([`ErrorCode::as_str`]) is the **wire contract** with the browser wrapper — it
/// lands on the thrown JS `Error` as `.code`, so renaming one is a breaking change for the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// OBCA §4.1: mixed schemas, an unaccepted hole, an unaccepted partial cell, a skin from another
    /// schema revision, a band the schema does not have. **The selection is wrong** — fix it and
    /// re-run; the cells are fine.
    Input,
    /// A cell does not honour the format or the cell contract. The download is corrupt or the
    /// catalog is serving something that is not a cell.
    Format,
    /// OBCA §5.7: the 4 GiB per-file ceiling, the `HoursRef` pool, the `uint32` index space.
    /// **Coverage must be reduced** — and per §5.7 this bridge never "solves" it by dropping any.
    Capacity,
    /// The §4.8 verify pass rejected the output: the engine wrote a set the real reader cannot read.
    /// A **defect in the assembler**, and the one code a caller must never retry past — nothing was
    /// handed on, and nothing should be.
    Verify,
    /// The byte source or sink failed.
    Io,
    /// The caller's own [`Hooks::progress`] asked to stop.
    Aborted,
    /// A defect in this bridge: an unparseable cell id, a schema or skin document that is not JSON,
    /// or a module that failed to load. The message says so.
    Internal,
}

impl ErrorCode {
    /// The stable kebab-case identifier the browser wrapper re-exports as its `AssembleErrorCode`
    /// union. Keep these in sync with `builder/app/src/lib/assemble/bridge.ts`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Input => "input",
            ErrorCode::Format => "format",
            ErrorCode::Capacity => "capacity",
            ErrorCode::Verify => "verify",
            ErrorCode::Io => "io",
            ErrorCode::Aborted => "aborted",
            ErrorCode::Internal => "internal",
        }
    }
}

/// One file of the finished set, in the order it must be handed on: every shard, then the OBCS
/// manifest **last** (OBCA §5.4 — a half-written set with no manifest is invisible as a map, which
/// is exactly the property an interrupted upload wants).
pub struct OutputFile {
    /// The derived 8.3 filename (`MS<id>S<kk>.OBM`, `MS<id>.OBS`).
    pub name: String,
    /// `"core"`, `"coarse"`, `"geometry"` — or `"manifest"` for the OBCS document.
    pub role: &'static str,
    /// Lowercase hex SHA-256 of `bytes`, as the manifest records it (empty for the manifest itself,
    /// which nothing digests).
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// Hand-written so a failing assertion prints the set's *shape* rather than a megabyte of hex.
impl core::fmt::Debug for OutputFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} ({}, {} B)", self.name, self.role, self.bytes.len())
    }
}

/// What an assembly produced: the files, the engine's `SHOULD`-report warnings, and the same
/// summary document `obcm-assemble --json` prints.
pub struct Outcome {
    pub files: Vec<OutputFile>,
    /// Everything OBCA says a producer SHOULD *report* rather than refuse (§5.7's core-headroom
    /// warning, §4.5.2's dropped duplicate POIs, `OBCM_Spec.md` §8.3's degree-cap truncations).
    /// A caller that ignores this ships the same bytes; a caller that shows it tells the rider what
    /// the spec wanted them told.
    pub warnings: Vec<String>,
    /// The summary as JSON, in the shape `obcm-assemble --json` prints (minus the CLI's output
    /// paths, which do not exist here).
    pub summary_json: String,
}

/// …and the same for the outcome: the files by shape, the warnings verbatim, and the summary only
/// by length (it is JSON, and a test that wants it reads it).
impl core::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Outcome")
            .field("files", &self.files)
            .field("warnings", &self.warnings)
            .field("summary_json_len", &self.summary_json.len())
            .finish()
    }
}

/// Shared state behind the two seams. Both the clock and the store hold `&RefCell<Progress>` — the
/// engine takes the store by `&mut` and the clock by `&`, so a single owner is not possible.
struct Progress<'h> {
    hooks: &'h mut dyn Hooks,
    phase: Phase,
    /// How many times the engine has read the clock. See the module header.
    ticks: u32,
    /// Bytes handed to [`ShardStore::write`] so far.
    written: u64,
    /// Bytes the §4.8 read-back has pulled through [`VerifySource::read_at`] so far.
    verified: u64,
    /// Projected output size — the sum of the input cells' bytes. Re-measured on #1116's harness:
    /// 794 735 626 B in → 793 891 927 B out on baden-württemberg and 263 616 395 → 263 309 260 on
    /// freiburg-regbez, i.e. 1.00 both times (0.9989 and 0.9988): geometry is copied verbatim and
    /// the nav section is rewritten to about the size the cells' own nav sections had.
    projected: u64,
    /// Overall fraction at the last emitted callback, so [`PROGRESS_STEP`] can throttle.
    last: f64,
    /// A [`Hooks::progress`] call asked to stop. Surfaced at the next store call.
    aborted: bool,
    /// The hand-off path's own failure, if it raised one. The engine's sink contract has a single
    /// refusal (`Error::Io`) which says nothing about *why*, so the driver keeps the real one here
    /// and [`map_error`] prefers it — a sink that could not take a shard is [`ErrorCode::Io`] with
    /// the caller's message, and a shard evicted too early is [`ErrorCode::Internal`].
    failure: Option<AssembleFailure>,
}

impl Progress<'_> {
    /// Emit `(phase, fraction)` unless it is within [`PROGRESS_STEP`] of the last one, and record an
    /// abort request. A phase change always emits.
    ///
    /// The reported fraction never goes below the last one. Both terms of
    /// [`Progress::write_verify_fraction`] are individually monotone, so this only bites if a future
    /// phase weight is re-tuned into an inconsistency — but a bar that goes backwards is the one
    /// thing a caller may never be shown, so it is enforced here rather than reasoned about.
    fn emit(&mut self, phase: Phase, fraction: f64) {
        let fraction = fraction.clamp(0.0, 1.0).max(self.last);
        if phase == self.phase && (fraction - self.last).abs() < PROGRESS_STEP {
            return;
        }
        self.phase = phase;
        self.last = fraction;
        if self.hooks.progress(phase, fraction) {
            self.aborted = true;
        }
    }

    /// The write/verify loop's bar. Two independent terms over one shared span, because the engine
    /// **interleaves** the two — it writes shard *i*, verifies shard *i*, then writes shard *i+1* —
    /// so a single counter would have to run backwards at every shard boundary.
    ///
    /// Both are measured against the same denominator, the input cells' byte count: geometry is
    /// copied verbatim and the nav section is rewritten to about the size the cells' own had, so
    /// output ≈ input (measured 1.00 on both of #1116's harness regions), and §4.8 reads that output
    /// back in full through the real reader. Each term is a ratio of a counter that only grows to a
    /// constant, hence monotone; both are clamped, so a projection that is off by a little costs bar
    /// accuracy near the end of a phase and nothing else — the boundaries themselves are pinned by
    /// the store's own calls, and `manifest` lands on exactly 1.0.
    fn write_verify_fraction(&self) -> f64 {
        Phase::Write.prefix()
            + Phase::Write.weight() * self.ratio(self.written)
            + Phase::Verify.weight() * self.ratio(self.verified)
    }

    /// `n` against the projected output, clamped into 0..=1. An assembly of nothing is complete.
    fn ratio(&self, n: u64) -> f64 {
        if self.projected == 0 {
            return 1.0;
        }
        (n as f64 / self.projected as f64).clamp(0.0, 1.0)
    }
}

/// The clock the engine reads. Its **call sequence** is the phase seam (see the module header); the
/// value it returns is what the summary's phase split is computed from.
struct HookedClock<'a, 'h> {
    p: &'a RefCell<Progress<'h>>,
}

impl Clock for HookedClock<'_, '_> {
    fn now_us(&self) -> u64 {
        let mut p = self.p.borrow_mut();
        p.ticks += 1;
        // Ticks 1..=5 are `t_start`, `t_open`, `t_poi`, `t_nav`, `t_plan` — each one *starts* the
        // next phase. From tick 6 the engine is inside the write/verify loop, where the store's own
        // calls say which of the two is running, so the clock stops naming phases.
        let phase = match p.ticks {
            1 => Some(Phase::Open),
            2 => Some(Phase::Poi),
            3 => Some(Phase::Nav),
            4 => Some(Phase::Plan),
            5 => Some(Phase::Write),
            _ => None,
        };
        if let Some(phase) = phase {
            let at = phase.prefix();
            p.emit(phase, at);
        }
        p.hooks.now_us()
    }
}

/// A sealed shard as the §4.8 verify pass reads it back — an owned buffer, plus the progress and
/// abort seam **inside the read-back's own loop**.
///
/// This is [`obcm_assemble::MemorySource`] with two lines added, and those two lines are the reason
/// this crate keeps its own store instead of delegating to [`obcm_assemble::MemoryStore`]:
/// [`ShardStore::source`] hands the engine a `&dyn ByteSource` and everything §4.8 does happens
/// behind it, so `read_at` is the only place a browser can learn that verify is moving — or tell it
/// to stop.
struct VerifySource<'a, 'h> {
    bytes: Vec<u8>,
    p: &'a RefCell<Progress<'h>>,
}

impl ByteSource for VerifySource<'_, '_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        {
            let mut p = self.p.borrow_mut();
            p.verified += buf.len() as u64;
            let at = p.write_verify_fraction();
            // Throttled by `emit` to [`PROGRESS_STEP`], so the callback fires on the order of a
            // hundred times over the pass however many reads the reader makes.
            p.emit(Phase::Verify, at);
            if p.aborted {
                // The read-back's own error channel is all there is down here. `verify_shard` turns
                // it into `Error::Verify`, which is why `map_error` reads the abort flag first.
                return Err(obc_formats::io::Error::Io);
            }
        }
        SliceSource(&self.bytes).read_at(offset, buf)
    }

    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

/// One shard's slot in the store: the buffer §4.8 reads back, the identity the hand-off needs before
/// the run's `Summary` exists, and the digest of what was actually written.
struct StoredShard<'a, 'h> {
    src: VerifySource<'a, 'h>,
    /// The §5.2 derived filename, computed here rather than waited for: the hand-off happens
    /// mid-run, and `Summary::shards` does not exist until the run ends. Cross-checked against it.
    name: String,
    role: &'static str,
    /// Fed by every [`ShardStore::write`] and finalized at [`ShardStore::seal`] — but only when the
    /// caller [`Hooks::wants_shards`], because a second SHA-256 pass over a gigabyte-scale set is
    /// real time to spend on a digest nobody asked for.
    hasher: Sha256,
    /// Lowercase hex, once sealed. Empty while the shard is open, and for a run with no hand-off.
    sha256: String,
    /// [`Hooks::take_shard`] has already been offered this shard. Every slot is offered **once**:
    /// the hand-off loop runs over all sealed shards at each of its two moments, and a caller that
    /// handed one back (`Ok(Some(_))`) meant "keep it", not "ask me again next time".
    offered: bool,
    /// …and it took the bytes, so the slot is empty and the file is not this crate's to hand on
    /// again.
    handed_out: bool,
}

/// The in-memory [`ShardStore`], plus the progress and abort seam.
///
/// In memory is not a shortcut: OBCA §4.8 requires every shard to be **read back through the real
/// reader before the manifest is written**, so a sealed shard has to be randomly addressable, and
/// the browser has nowhere else to put it. What it does *not* require is that it stay there
/// afterwards — see the module header's hand-off section, which is what keeps this to at most one
/// shard when the caller asks for it.
struct HookedStore<'a, 'h> {
    shards: Vec<StoredShard<'a, 'h>>,
    manifest: Vec<u8>,
    /// Needed to derive a shard's filename at hand-off time, mid-run.
    card_id: u16,
    /// [`Hooks::wants_shards`], read once before the assembly starts.
    hand_off: bool,
    p: &'a RefCell<Progress<'h>>,
}

impl HookedStore<'_, '_> {
    /// The abort check every store entry point runs first. `Error::Io` is the only refusal the
    /// engine's sink contract has; [`assemble_cells`] turns it back into [`ErrorCode::Aborted`]
    /// because it knows why it was raised.
    fn check_abort(&self) -> obcm_assemble::Result<()> {
        if self.p.borrow().aborted {
            return Err(Error::Io(obc_formats::io::Error::Io));
        }
        Ok(())
    }

    /// Offer every sealed-and-verified shard that is still resident to [`Hooks::take_shard`].
    ///
    /// Called from [`ShardStore::begin`] and [`ShardStore::manifest`] — the two observable moments
    /// at which the engine has finished with the previous shard (module header). Every shard in
    /// `self.shards` at those points has been through §4.8: `begin` is called before the new slot is
    /// pushed, and the manifest is written after the whole loop.
    ///
    /// Runs **after** each caller's `check_abort`, so a cancelled run hands nothing further out and
    /// a sink failure cannot be confused with a cancellation.
    fn hand_out_verified(&mut self) -> obcm_assemble::Result<()> {
        if !self.hand_off {
            return Ok(());
        }
        // A copy of the shared handle, so the loop below can borrow `self.shards` mutably.
        let p = self.p;
        for shard in self.shards.iter_mut().filter(|s| !s.offered) {
            shard.offered = true;
            let file = OutputFile {
                name: shard.name.clone(),
                role: shard.role,
                sha256: shard.sha256.clone(),
                bytes: core::mem::take(&mut shard.src.bytes),
            };
            let taken = p.borrow_mut().hooks.take_shard(file);
            match taken {
                Ok(None) => shard.handed_out = true,
                Ok(Some(back)) => shard.src.bytes = back.bytes,
                Err(message) => {
                    // The bytes are already gone; the run must not go on to report a set.
                    p.borrow_mut().failure = Some(AssembleFailure::new(ErrorCode::Io, message));
                    return Err(Error::Io(obc_formats::io::Error::Io));
                }
            }
        }
        Ok(())
    }
}

impl<'a, 'h> ShardStore for HookedStore<'a, 'h> {
    fn begin(&mut self, plan: &ShardPlan) -> obcm_assemble::Result<()> {
        self.check_abort()?;
        // Before the next buffer is reserved, not after: the whole point is that the peak carries
        // one shard, and reserving first would make it two.
        self.hand_out_verified()?;
        {
            let mut p = self.p.borrow_mut();
            let at = p.write_verify_fraction();
            p.emit(Phase::Write, at);
        }
        debug_assert_eq!(plan.index, self.shards.len());
        let mut bytes = Vec::new();
        // §5 computes a shard's exact size before a byte of it is written, so the browser can have
        // the buffer it needs in one allocation instead of a doubling ladder whose last step
        // transiently holds 1.5× a gigabyte-scale shard — memory the estimate does not model and a
        // tab may not have contiguously. `try_` because a refusal here is recoverable: the write
        // path would then grow the vector itself and fail (or not) exactly as it used to, whereas a
        // plain `reserve_exact` would abort the whole module on a capacity a shard might never
        // actually reach.
        let _ = bytes.try_reserve_exact(usize::try_from(plan.bytes).unwrap_or(0));
        self.shards.push(StoredShard {
            src: VerifySource { bytes, p: self.p },
            name: obcm_assemble::shard::shard_filename(self.card_id, plan.index),
            role: plan.role.as_str(),
            hasher: Sha256::new(),
            sha256: String::new(),
            offered: false,
            handed_out: false,
        });
        Ok(())
    }

    fn write(&mut self, buf: &[u8]) -> obcm_assemble::Result<()> {
        {
            let mut p = self.p.borrow_mut();
            p.written += buf.len() as u64;
            let at = p.write_verify_fraction();
            p.emit(Phase::Write, at);
        }
        // Checked *after* accounting so the callback that observes the abort request is the one
        // that also reports where it got to.
        self.check_abort()?;
        let hand_off = self.hand_off;
        let shard = self.shards.last_mut().expect("a shard is open");
        // The engine hashes the same bytes on its way through `shard::write`, but it keeps that
        // digest to itself (it lands in the plan *after* `seal`). Hashing here is therefore what
        // makes a mid-run hand-off able to name its own file — and the end-of-run comparison against
        // the engine's figure is then a real check that the buffer handed out is the buffer the
        // engine wrote, not a restatement.
        if hand_off {
            shard.hasher.update(buf);
        }
        shard.src.bytes.extend_from_slice(buf);
        Ok(())
    }

    fn seal(&mut self) -> obcm_assemble::Result<()> {
        if self.hand_off {
            let shard = self.shards.last_mut().expect("a shard is open");
            let digest = core::mem::take(&mut shard.hasher).finalize();
            shard.sha256 = digest.iter().map(|b| format!("{b:02x}")).collect();
        }
        self.check_abort()
    }

    fn source(&self, index: usize) -> obcm_assemble::Result<&dyn ByteSource> {
        // The engine asks for a sealed shard for exactly one reason: the §4.8 verify pass. This is
        // the phase boundary; the reads that follow are what report the pass's own progress.
        {
            let mut p = self.p.borrow_mut();
            let at = p.write_verify_fraction();
            p.emit(Phase::Verify, at);
        }
        self.check_abort()?;
        // A handed-out shard reads as an empty file rather than as a missing one, which §4.8 would
        // report as a defect in the *engine*. It cannot happen — `source(i)` is only called inside
        // the iteration that wrote shard `i`, which is the whole premise of the hand-off — so if it
        // ever does, it is this bridge that broke the premise, and it says so.
        match self.shards.get(index) {
            Some(s) if s.handed_out => {
                self.p.borrow_mut().failure = Some(AssembleFailure::new(
                    ErrorCode::Internal,
                    format!(
                        "shard {index} ({}) was handed to the caller and then read back — the driver evicted a shard \
                         the §4.8 pass still needed. This is a bug in obc-web-assemble, not in the assembler.",
                        s.name
                    ),
                ));
                Err(Error::Io(obc_formats::io::Error::BadOffset))
            }
            Some(s) => Ok(&s.src as &dyn ByteSource),
            None => Err(Error::Io(obc_formats::io::Error::BadOffset)),
        }
    }

    fn manifest(&mut self, bytes: &[u8]) -> obcm_assemble::Result<()> {
        self.check_abort()?;
        // The last shard's own hand-off moment: §5.4's manifest is the first thing that happens
        // after the final §4.8 pass.
        self.hand_out_verified()?;
        {
            let mut p = self.p.borrow_mut();
            p.emit(Phase::Manifest, 1.0);
        }
        self.manifest = bytes.to_vec();
        Ok(())
    }
}

/// Assemble `cells` into one `.obcm` or an OBCA volume set, reporting through `hooks`.
///
/// Byte-for-byte the same output as the native CLI on the same inputs — this function contributes no
/// format knowledge, only the buffer adapter and the seams above. `tests/determinism.rs` pins that
/// equality against a checked-in set the CLI produced, and `builder/app/src/lib/assemble/
/// bridge.test.ts` pins the *wasm* build to the same bytes.
pub fn assemble_cells(
    cells: Vec<CellBytes>,
    schema_json: &str,
    skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    assemble_cells_with_known_empty(cells, Vec::new(), schema_json, skin_json, opts, hooks)
}

/// Assemble downloaded artifacts while retaining selected canonical-empty
/// cells in coverage and bbox arithmetic.
pub fn assemble_cells_with_known_empty(
    cells: Vec<CellBytes>,
    known_empty: Vec<KnownEmptyCell>,
    schema_json: &str,
    skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    assemble_everything(cells, known_empty, None, Vec::new(), schema_json, skin_json, opts, hooks)
}

/// The full assembly, raster included (EL4, #1072).
///
/// `terrain` is the catalog's lattice; `None` means the catalog publishes no terrain block, or the
/// rider's selection covers no terrain object, and the set is written without a `terrain` role — a
/// complete, ordinary map whose profiles are flat.
// One assembly is exactly these eight things; a struct would restate the signature.
#[allow(clippy::too_many_arguments)]
pub fn assemble_everything(
    cells: Vec<CellBytes>,
    known_empty: Vec<KnownEmptyCell>,
    terrain: Option<TerrainLattice>,
    terrain_cells: Vec<TerrainCellBytes>,
    schema_json: &str,
    skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    if cells.is_empty() {
        return Err(AssembleFailure::new(
            ErrorCode::Input,
            "No OBCM cell artifact was handed to the assembler. An assembly needs at least one artifact to verify \
             the schema revision's binary style and routing-profile tables (OBCA §3.8).",
        ));
    }
    let schema = Schema::parse(schema_json).map_err(|e| AssembleFailure::new(ErrorCode::Internal, e))?;
    let skin = Skin::parse(skin_json).map_err(|e| AssembleFailure::new(ErrorCode::Internal, e))?;

    let projected: u64 = cells.iter().map(|c| c.bytes.len() as u64).sum();
    // Split identity from payload so the payload can be moved into the byte sources without a copy.
    let mut ids = Vec::with_capacity(cells.len());
    let mut sources = Vec::with_capacity(cells.len());
    for c in cells {
        let id = CellId::parse(&c.id).map_err(|e| {
            AssembleFailure::new(ErrorCode::Internal, format!("cell id {:?} is not a `<log2>/<i>/<j>` id: {e}", c.id))
        })?;
        ids.push((id, c.band, c.partial));
        sources.push(MemorySource(c.bytes));
    }
    let inputs: Vec<CellInput<'_>> = ids
        .iter()
        .zip(&sources)
        .map(|((id, band, partial), src)| CellInput { id: *id, band: band.clone(), src, partial: *partial })
        .collect();
    let known_empty: Vec<KnownEmptyInput> = known_empty
        .into_iter()
        .map(|cell| {
            CellId::parse(&cell.id).map(|id| KnownEmptyInput { id, band: cell.band }).map_err(|e| {
                AssembleFailure::new(
                    ErrorCode::Internal,
                    format!("known-empty cell id {:?} is not a `<log2>/<i>/<j>` id: {e}", cell.id),
                )
            })
        })
        .collect::<Result<_, _>>()?;

    // The raster's inputs, split identity from payload the same way.
    let mut terrain_ids = Vec::with_capacity(terrain_cells.len());
    let mut terrain_sources = Vec::with_capacity(terrain_cells.len());
    for c in terrain_cells {
        let id = CellId::parse(&c.id).map_err(|e| {
            AssembleFailure::new(
                ErrorCode::Internal,
                format!("terrain cell id {:?} is not a `<log2>/<i>/<j>` id: {e}", c.id),
            )
        })?;
        let sha256 = if c.sha256.is_empty() { None } else { Some(parse_digest(&c.sha256, &c.id)?) };
        terrain_ids.push((id, sha256));
        terrain_sources.push(MemorySource(c.bytes));
    }

    let options = Options {
        name: opts.name.clone(),
        card_id: opts.card_id,
        target_shard_bytes: opts.target_shard_bytes,
        accept_holes: opts.accept_holes,
        accept_partial: opts.accept_partial,
        force_split: opts.force_split,
        // Never. See `BridgeOptions`.
        skip_verify: false,
    };

    let hand_off = hooks.wants_shards();
    let progress = RefCell::new(Progress {
        hooks,
        phase: Phase::Open,
        ticks: 0,
        written: 0,
        verified: 0,
        projected,
        last: -1.0,
        aborted: false,
        failure: None,
    });
    let clock = HookedClock { p: &progress };
    let mut store =
        HookedStore { shards: Vec::new(), manifest: Vec::new(), card_id: options.card_id, hand_off, p: &progress };
    // The terrain shard is written into an ordinary in-memory buffer, like every OBCM shard: the
    // engine's OBCT writer seeks (it back-patches its directory), which a `Cursor` gives for free.
    let mut terrain_sink = std::io::Cursor::new(Vec::<u8>::new());
    let job = terrain.map(|lattice| TerrainJob {
        params: TerrainParams { posting_log2: lattice.posting_log2, cell_log2: lattice.cell_log2 },
        cells: terrain_ids
            .iter()
            .zip(&terrain_sources)
            .map(|((id, sha256), src)| TerrainCellInput { id: *id, src, sha256: *sha256 })
            .collect(),
        sink: &mut terrain_sink,
    });
    let summary = match assemble_full(inputs, known_empty, job, &schema, &skin, &options, &mut store, &clock) {
        Ok(s) => s,
        Err(e) => {
            let p = progress.borrow();
            return Err(map_error(e, p.aborted, p.failure.clone()));
        }
    };
    {
        let mut p = progress.borrow_mut();
        p.emit(Phase::Done, 1.0);
    }

    let mut shards = store.shards;
    if shards.len() != summary.shards.len() {
        return Err(AssembleFailure::new(
            ErrorCode::Internal,
            format!(
                "the engine reported {} shard(s) and the store holds {} — the write loop and this store no longer \
                 agree on what a shard is.",
                summary.shards.len(),
                shards.len()
            ),
        ));
    }
    let mut files = Vec::with_capacity(shards.len() + 2);
    for (s, shard) in summary.shards.iter().zip(shards.drain(..)) {
        let sha256: String = s.sha256.iter().map(|b| format!("{b:02x}")).collect();
        // The hand-off names a file and digests it **mid-run**, from this store's own accounting,
        // because `Summary` does not exist yet. This is where that accounting meets the engine's:
        // a caller that already wrote `MS1S00.OBM` somewhere must have written the bytes the engine
        // says are in it. A disagreement is a defect in this bridge — and it is *reported* rather
        // than corrected, because the shard is already gone.
        if hand_off {
            check_handoff(s.index, (&shard.name, &shard.sha256), (&s.filename, &sha256))?;
        }
        if shard.handed_out {
            continue;
        }
        files.push(OutputFile { name: s.filename.clone(), role: s.role.as_str(), sha256, bytes: shard.src.bytes });
    }
    // The raster goes with the shards — before the manifest, because §5.4's rule is about the
    // manifest being last, and it names the terrain record too.
    if let Some(t) = &summary.terrain {
        files.push(OutputFile {
            name: t.filename.clone(),
            role: "terrain",
            sha256: t.sha256.iter().map(|b| format!("{b:02x}")).collect(),
            bytes: terrain_sink.into_inner(),
        });
    }
    files.push(OutputFile {
        name: summary.manifest_filename.clone(),
        role: "manifest",
        sha256: String::new(),
        bytes: store.manifest,
    });

    let summary_json = summary_json(&summary);
    Ok(Outcome { files, warnings: summary.warnings, summary_json })
}

/// The hand-off's identity check: what this store told the caller a shard was, against what the
/// engine says it wrote.
///
/// It exists because [`Hooks::take_shard`] runs **mid-run**, before `Summary` does, so the filename
/// and the digest it carries are this crate's own arithmetic — a derived §5.2 name and a SHA-256 of
/// the bytes the store actually accumulated. Being right about both is a correctness claim, not a
/// convenience: a caller writes that filename to a card and records that digest. This is where the
/// claim is checked, and a failure is [`ErrorCode::Internal`] because the only way to reach it is a
/// defect here — the engine's figures come from the same bytes by a different path.
///
/// It is a tripwire, not a gate: by the time it fires the shard has already been handed over. What
/// it buys is that the *run* fails loudly instead of a mislabelled file being reported as a map.
fn check_handoff(index: usize, got: (&str, &str), want: (&str, &str)) -> Result<(), AssembleFailure> {
    if got == want {
        return Ok(());
    }
    Err(AssembleFailure::new(
        ErrorCode::Internal,
        format!(
            "shard {index} was handed over as {:?} / {} but the engine wrote {:?} / {} — this bridge's own filename \
             or digest is wrong, and a caller may already have saved the shard under it.",
            got.0, got.1, want.0, want.1
        ),
    ))
}

/// A lowercase-hex SHA-256 as the 32 bytes the engine compares against. A malformed one is this
/// bridge's problem, not the catalog's format — the catalog parser already rejected a bad digest
/// long before the download started, so reaching here means the caller built the string wrongly.
fn parse_digest(hex: &str, what: &str) -> Result<[u8; 32], AssembleFailure> {
    let bad = || {
        AssembleFailure::new(
            ErrorCode::Internal,
            format!("terrain cell {what}: {hex:?} is not a 64-character lowercase-hex SHA-256"),
        )
    };
    if hex.len() != 64 {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(hex.get(k * 2..k * 2 + 2).ok_or_else(bad)?, 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// Map an engine refusal onto the bridge's vocabulary, keeping the engine's own message. The match
/// is exhaustive on purpose: a new [`Error`] variant must break this build rather than quietly
/// inherit someone else's code.
///
/// `aborted` is read **first**, before the error's own class. The abort path raises whatever error
/// channel it was standing in — the sink's `Error::Io` from a store call, but `Error::Verify` from a
/// [`VerifySource::read_at`] refusal, because `verify_shard` reports every read failure as a §4.8
/// defect. A cancelled run must never be reported as "the assembler wrote a set the reader cannot
/// read": that is the one code a caller is told never to retry past, and it would turn a cancel
/// button into a bug report.
///
/// `failure` is the hand-off path's own, raised through the same one-shaped `Error::Io` channel and
/// therefore just as unreadable without it. It is read next; it cannot coexist with an abort,
/// because every store entry point checks the abort flag before it hands anything out.
fn map_error(e: Error, aborted: bool, failure: Option<AssembleFailure>) -> AssembleFailure {
    if aborted {
        return AssembleFailure::new(
            ErrorCode::Aborted,
            "The assembly was cancelled. Nothing was written — the OBCS manifest is written last, so a cancelled run \
             leaves no set (OBCA §5.4).",
        );
    }
    if let Some(f) = failure {
        return f;
    }
    let message = e.to_string();
    let code = match e {
        Error::Input(_) => ErrorCode::Input,
        Error::Format(_) => ErrorCode::Format,
        Error::Capacity(_) => ErrorCode::Capacity,
        Error::Verify(_) => ErrorCode::Verify,
        Error::Io(_) => ErrorCode::Io,
    };
    AssembleFailure::new(code, message)
}

/// The summary, in the shape `obcm-assemble --json` prints. Restated here rather than shared because
/// the CLI's version names output *paths*, which do not exist in a browser — everything else is the
/// same document, deliberately, so the two hosts report one thing.
fn summary_json(s: &obcm_assemble::Summary) -> String {
    let shards: Vec<serde_json::Value> = s
        .shards
        .iter()
        .map(|sh| {
            serde_json::json!({
                "index": sh.index,
                "role": sh.role.as_str(),
                "file": sh.filename,
                "bytes": sh.bytes,
                "sha256": sh.sha256.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                "verified": sh.verify.as_ref().map(|v| serde_json::json!({
                    "chunks": v.chunks,
                    "features": v.features,
                    "nav_nodes": v.nav_nodes,
                    "largest_component_permille": v.largest_component_permille,
                })),
            })
        })
        .collect();
    let st = &s.stats;
    serde_json::to_string(&serde_json::json!({
        "assembly_bbox_udeg": {
            "min_lat": s.assembly_box.min_lat,
            "min_lon": s.assembly_box.min_lon,
            "span_log2": s.assembly_box.span_log2,
        },
        "cells": st.cells,
        "bytes": s.bytes,
        "manifest": s.manifest_filename,
        "shards": shards,
        "terrain": s.terrain.as_ref().map(|t| serde_json::json!({
            "file": t.filename,
            "bytes": t.bytes,
            "sha256": t.sha256.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "cells": t.cells,
            "slots": t.slots,
        })),
        "phases_us": {
            "open": st.open_us, "poi": st.poi_us, "nav": st.nav_us,
            "plan": st.plan_us, "write": st.write_us, "verify": st.verify_us, "total": st.total_us,
        },
        "geometry_bytes": st.geometry_bytes,
        "nav": {
            "section_bytes": st.nav_section_bytes,
            "cell_nodes": st.nav.cell_nodes,
            "nodes": st.nav.nodes,
            "edges": st.nav.edges,
            "unified": st.nav.unified,
            "duplicate_edges": st.nav.duplicate_edges,
            "components_found": st.nav.components_found,
            "components_kept": st.nav.components_kept,
            "pruned_nodes": st.nav.pruned_nodes,
            "pruned_edges": st.nav.pruned_edges,
            "largest_component_permille": st.nav.largest_component_permille,
            "degree_truncated": st.nav.degree_truncated,
            "dropped_nodes": st.nav.dropped_nodes,
        },
        "poi": {
            "records": st.poi_records,
            "duplicates": st.poi_duplicates,
            "dropped": st.poi_dropped,
            "section_bytes": st.poi_section_bytes,
        },
        "warnings": s.warnings,
    }))
    .unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four codes a caller actually branches on must be four different strings. The issue asks
    /// for exactly this: a §4.8 **verify** failure — the engine wrote a set the reader cannot read —
    /// must be distinguishable from a §4.1 **input** refusal (fix the selection) and a §5.7
    /// **capacity** refusal (reduce the coverage), because a caller's response to each is different
    /// and only one of the three is a bug report.
    #[test]
    fn every_refusal_class_maps_to_its_own_code() {
        let cases = [
            (Error::Input("mixed schemas".into()), ErrorCode::Input, "input"),
            (Error::Format("not an OBCM".into()), ErrorCode::Format, "format"),
            (Error::Capacity("past the 4 GiB ceiling".into()), ErrorCode::Capacity, "capacity"),
            (Error::Verify("chunk 3 does not decode".into()), ErrorCode::Verify, "verify"),
            (Error::Io(obc_formats::io::Error::BadOffset), ErrorCode::Io, "io"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (err, code, wire) in cases {
            let f = map_error(err, false, None);
            assert_eq!(f.code, code);
            assert_eq!(f.code.as_str(), wire, "the wire code is a contract with bridge.ts");
            assert!(seen.insert(wire), "two refusal classes share the code {wire:?}");
        }
    }

    /// The engine's own message must survive the crossing unchanged — it is the only thing that says
    /// *which* cell, *which* band, *which* ceiling.
    #[test]
    fn the_engines_message_is_not_rewritten() {
        let m = "band \"network\" is missing cell 18/1204/1053, which covers 18/1204/1053";
        assert_eq!(map_error(Error::Input(m.into()), false, None).message, m);
        // …except for `Verify`, whose `Display` prefixes it, exactly as the CLI prints it.
        assert_eq!(map_error(Error::Verify("chunk 3".into()), false, None).message, "verify failed: chunk 3");
    }

    /// An abort raises whatever error channel it was standing in; the driver is what knows it was a
    /// cancellation and says so.
    ///
    /// Both directions matter, and the second is the sharper one: an abort taken during the §4.8
    /// read-back comes back as `Error::Verify`, because `verify_shard` reports *any* read failure as
    /// a §4.8 defect. Reporting that as [`ErrorCode::Verify`] would tell the caller the assembler is
    /// broken — the one code the docs say never to retry past — because they pressed cancel.
    #[test]
    fn an_abort_is_not_reported_as_an_io_failure_or_a_verify_defect() {
        let f = map_error(Error::Io(obc_formats::io::Error::Io), true, None);
        assert_eq!(f.code, ErrorCode::Aborted);
        assert!(f.message.contains("cancelled"), "{}", f.message);
        let v = map_error(Error::Verify("the output does not parse: Io".into()), true, None);
        assert_eq!(v.code, ErrorCode::Aborted, "a cancelled read-back is a cancellation, not a §4.8 defect");
        // …and both classes with no abort pending still read as themselves.
        assert_eq!(map_error(Error::Io(obc_formats::io::Error::Io), false, None).code, ErrorCode::Io);
        assert_eq!(map_error(Error::Verify("chunk 3 does not decode".into()), false, None).code, ErrorCode::Verify);
    }

    /// The phase weights are a probability distribution over the run, or the bar goes backwards.
    #[test]
    fn the_phase_weights_sum_to_one_and_the_prefixes_ascend() {
        let order = [Phase::Open, Phase::Poi, Phase::Nav, Phase::Plan, Phase::Write, Phase::Verify];
        let total: f64 = order.iter().map(|p| p.weight()).sum();
        assert!((total - 1.0).abs() < 1e-9, "phase weights sum to {total}");
        let mut last = -1.0;
        for p in order {
            let at = p.prefix();
            assert!(at > last, "{p:?} starts at {at}, not after {last}");
            last = at;
        }
        assert_eq!(Phase::Manifest.prefix(), 1.0);
        assert_eq!(Phase::Done.prefix(), 1.0);
    }

    /// Every phase's wire name is distinct — `bridge.ts` switches on them.
    #[test]
    fn phase_names_are_distinct() {
        let all = [
            Phase::Open,
            Phase::Poi,
            Phase::Nav,
            Phase::Plan,
            Phase::Write,
            Phase::Verify,
            Phase::Manifest,
            Phase::Done,
        ];
        let names: std::collections::HashSet<&str> = all.iter().map(|p| p.as_str()).collect();
        assert_eq!(names.len(), all.len());
    }

    /// Options are read leniently: absent is the engine's default, unknown is ignored (a newer
    /// builder talking to an older module), wrong-typed is a refusal.
    #[test]
    fn options_parse_leniently_but_not_wrongly() {
        let d = BridgeOptions::parse("").expect("empty is the default");
        assert_eq!(d.card_id, Options::default().card_id);
        assert!(!d.force_split);

        let o = BridgeOptions::parse(r#"{"name":"Alps","cardId":7,"forceSplit":true,"somethingNew":42}"#)
            .expect("unknown keys are ignored");
        assert_eq!((o.name.as_str(), o.card_id, o.force_split), ("Alps", 7, true));

        assert!(BridgeOptions::parse(r#"{"cardId":"seven"}"#).is_err());
        assert!(BridgeOptions::parse("[1,2,3]").is_err());
        assert!(BridgeOptions::parse("not json").is_err());
    }

    /// `skip_verify` is not reachable from JS: OBCA §4.8 makes the read-back a precondition of
    /// writing a set, and this bridge exists to hand bytes to a device. The knob is ignored, not
    /// honoured, whichever way it is spelled.
    #[test]
    fn there_is_no_way_to_ask_for_an_unverified_set() {
        let o = BridgeOptions::parse(r#"{"skipVerify":true,"skip_verify":true}"#).expect("ignored, not honoured");
        let debug = format!("{o:?}");
        assert!(!debug.contains("skip"), "{debug}");
    }

    /// An empty selection is refused before anything is parsed, and as an *input* problem.
    #[test]
    fn an_empty_selection_is_an_input_refusal() {
        let e = assemble_cells(Vec::new(), "{}", "{}", &BridgeOptions::default(), &mut NoHooks)
            .expect_err("nothing to assemble");
        assert_eq!(e.code, ErrorCode::Input);
    }

    /// Known-empty coverage carries no binary tables, so it cannot make an
    /// otherwise artifact-free selection assembleable (§3.8).
    #[test]
    fn an_all_known_empty_selection_is_an_input_refusal() {
        let empty = vec![KnownEmptyCell { id: "18/1204/1055".into(), band: "fine".into() }];
        let e = assemble_cells_with_known_empty(Vec::new(), empty, "{}", "{}", &BridgeOptions::default(), &mut NoHooks)
            .expect_err("known-empty coverage cannot supply binary tables");
        assert_eq!(e.code, ErrorCode::Input);
        assert!(e.message.contains("at least one artifact"), "{}", e.message);
    }

    /// The hand-off's tripwire fires on either half of the identity, and says which shard.
    #[test]
    fn a_shard_handed_over_under_the_wrong_name_or_digest_is_an_internal_error() {
        let sha = "ab".repeat(32);
        let other = "cd".repeat(32);
        assert!(check_handoff(0, ("MS1S00.OBM", &sha), ("MS1S00.OBM", &sha)).is_ok());

        for (got, want) in [
            (("MS1S00.OBM", sha.as_str()), ("MS1S01.OBM", sha.as_str())),
            (("MS1S00.OBM", sha.as_str()), ("MS1S00.OBM", other.as_str())),
        ] {
            let e = check_handoff(3, got, want).expect_err("the identities differ");
            assert_eq!(e.code, ErrorCode::Internal);
            assert!(e.message.contains("shard 3"), "{}", e.message);
        }
    }

    /// A cell id that is not `<log2>/<i>/<j>` is this bridge's problem, not the catalog's format.
    #[test]
    fn a_malformed_cell_id_is_an_internal_error() {
        const SCHEMA: &str =
            r#"{"lods":[{"index":0}],"bands":[{"id":"fine","cell_log2":18,"lods":[0],"role":"core"}]}"#;
        const SKIN: &str = r#"{"marker_color":0,"styles":[]}"#;
        let cells = vec![CellBytes { id: "not-an-id".into(), band: "fine".into(), partial: false, bytes: vec![0; 8] }];
        let e = assemble_cells(cells, SCHEMA, SKIN, &BridgeOptions::default(), &mut NoHooks).expect_err("bad id");
        assert_eq!(e.code, ErrorCode::Internal);
        assert!(e.message.contains("not-an-id"), "{}", e.message);
    }
}
