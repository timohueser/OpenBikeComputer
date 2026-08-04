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
//!
//! # Reading the input cells from outside wasm memory (#1116 B2)
//!
//! The other half of a browser's peak is the **input**: [`CellBytes`] carries a whole downloaded
//! cell into linear memory and it stays there for the run — ~795 MB at country scale, resident from
//! the first `addCell` to the last file taken. It does not have to be there either. Everything the
//! engine does with a cell it does through [`ByteSource::read_at`], so a cell can live anywhere the
//! host can serve a byte range from — OPFS, in the browser's case.
//!
//! [`SourceCell`] is that input: identity, declared length, and an opaque host key. [`CellReads`] is
//! the seam the host implements, and [`BlockCache`] is what stands between it and the engine.
//!
//! **The cache is not an optimisation, it is what makes the seam affordable.** §4.6.6 emits the
//! merged edge pool one record at a time ([`obcm_assemble`]'s `nav::serialize`, a `read_into` per
//! record — 17.5 M of them at country scale, ~13–100 bytes each), and the §4.6 merge walks every
//! cell's node chunks 512 bytes at a time. Handed straight to a JS callback that does one OPFS read
//! each, that is millions of boundary crossings and millions of syscalls. Both walks are strongly
//! sequential *within* a cell, though, so a small LRU of [`DEFAULT_READ_BLOCK`]-sized blocks turns
//! them into one host read per block — roughly `bytes / 64 KiB` calls for a whole pass, which is
//! four orders of magnitude fewer. Reads at least a block long (§2.3's 256 KiB verbatim geometry
//! copy, the merge's whole-edge-pool read) bypass the cache and land straight in the caller's
//! buffer, so the big copies pay neither an extra copy nor an eviction.
//!
//! One crossing measured **~0.4 µs** in Node (V8), callback and memory view included, against the
//! fixture with the cache switched off — so BW's nav emission alone would spend ~7 s crossing the
//! boundary before a single byte is read, and each of those crossings is also a file read. Cached,
//! the same pass asks the host about `795 MB / 64 KiB ≈ 12 k` times. The cache's own residency is
//! [`READ_CACHE_BLOCKS`] × the block size — 1 MiB by default, independent of how many cells the
//! selection has.
//!
//! # Writing the shards outside wasm memory (#1116 D1)
//!
//! B1 above bounds the *output* at one shard, which is the right answer only while a shard is small
//! enough to be one. It is not: the **core shard cannot be split** — one nav graph, one file (OBCA
//! §5.1) — so a DACH-scale core is a single ~3 GiB `Vec<u8>` in a 4 GiB address space, and no amount
//! of streaming the merge helps while the sink is that vector.
//!
//! [`ShardWrites`] is the write-side twin of [`CellReads`], and it is the same four ideas: the host
//! is handed a **slot** (the shard's index), the calls are synchronous so they can be made from
//! inside the one blocking assembly, the bytes cross as a view rather than a copy, and everything
//! the engine reads back comes through a [`BlockCache`]. In the browser it is one OPFS
//! `FileSystemSyncAccessHandle` per shard, opened in the assembly worker.
//!
//! What changes inside this module is small on purpose, because the format knowledge must not move:
//!
//! * [`ShardStore::write`] forwards to the host instead of extending a vector.
//! * [`ShardStore::source`] hands §4.8 a [`VerifySource`] whose body is a *file*, so the read-back
//!   genuinely re-reads what was written — the property the whole verify pass exists for, and one
//!   that an in-memory store can only assert. `a_shard_the_sink_corrupts_on_disk_fails_verify` is
//!   the proof: flip a byte behind the driver's back and §4.8 must reject the set.
//! * A sealed, verified shard is reported by identity ([`SealedShard`]) rather than handed over as
//!   bytes — [`Hooks::shard_sealed`] — because the host already has the file. Nothing shard-sized
//!   is ever in linear memory, so there is nothing to evict and nothing to `postMessage`.
//!
//! The buffered path stays exactly as it was, to the byte: no sink means [`ShardBody::Buffered`],
//! `Vec<u8>`, [`Hooks::take_shard`], and every test in `tests/determinism.rs` that was written
//! before any of this.

use std::cell::{Cell as StdCell, RefCell};

use obc_formats::io::{ByteSource, SliceSource};
use obcm_assemble::grid::CellId;
use obcm_assemble::schema::{Schema, Skin};
use obcm_assemble::{
    assemble_full, CellInput, Clock, Error, KnownEmptyInput, MemoryScratch, MemorySource, Options, ShardPlan,
    ShardStore, TerrainCellInput, TerrainJob, TerrainParams,
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

/// One downloaded cell whose bytes are **not** in wasm memory (#1116 B2): the same identity
/// [`CellBytes`] carries, plus the length the catalog published and an opaque key the host resolves
/// reads against.
///
/// `key` is never interpreted here — it is the browser's OPFS filename (a content digest), and to
/// this crate it is a label that makes a read failure say *which* cell. Reads themselves go through
/// [`CellReads`] and name the cell by its **slot**: its index in the `source_cells` list handed to
/// [`assemble`], which is a number rather than a string precisely because it crosses the wasm
/// boundary once per host read.
pub struct SourceCell {
    /// The canonical cell id, `<log2>/<i>/<j>`.
    pub id: String,
    pub band: String,
    /// The catalog's `partial` flag (OBCA §3.7).
    pub partial: bool,
    /// The object's length, as the catalog pins it. It is what [`ByteSource::len`] answers, so a
    /// wrong one is a format error at open rather than a silent truncation.
    pub byte_length: u32,
    /// The host's own name for the bytes. Only ever shown in a message.
    pub key: String,
}

/// How a host serves the bytes of a [`SourceCell`].
///
/// One method, called from **inside** the synchronous assembly, with the engine blocked behind it —
/// in the browser that is a `FileSystemSyncAccessHandle.read()` from the assembly worker, which is
/// the whole reason this seam is shaped as a blocking call rather than a future.
///
/// It is called far less often than the engine reads: [`BlockCache`] serves the record-at-a-time
/// walks out of a small LRU and only misses reach here (module header). Implementations should
/// still be cheap and must be re-entrant-free — nothing may call back into the assembler.
pub trait CellReads {
    /// Fill `buf` with `buf.len()` bytes at `offset` of the object in `slot`.
    ///
    /// `Err(message)` fails the run as [`ErrorCode::Io`], with the message quoted after the cell's
    /// own name. A short read is a failure: the buffer must be filled or the call must refuse.
    fn read(&self, slot: usize, offset: u32, buf: &mut [u8]) -> Result<(), String>;
}

/// How a host takes the bytes of a shard — and gives them back (#1116 D1).
///
/// The write-side twin of [`CellReads`], and the reason the unsplittable core shard does not have to
/// be a `Vec<u8>` in a 4 GiB address space. A shard is named by its **slot**, which is its index in
/// the set: the number the engine plans with, so nothing has to be looked up per call.
///
/// Every method is called from **inside** the synchronous assembly, with the engine blocked behind
/// it. In the browser that is a `FileSystemSyncAccessHandle` per shard, opened in the assembly
/// worker before the run starts (they cannot be opened during it — the opener is async and the run
/// cannot await), which is why the seam is blocking calls rather than futures.
///
/// The lifecycle per shard is fixed and the engine's own: `create` once, `write` many times in
/// order, `seal` once, then any number of [`ShardWrites::read_at`] calls while §4.8 reads the shard
/// back. `Err(message)` from any of them fails the run as [`ErrorCode::Io`] carrying that message —
/// never as a §4.8 [`ErrorCode::Verify`] defect, which would tell a rider the assembler is broken
/// because their disk filled up.
pub trait ShardWrites {
    /// Begin shard `slot`, which the set will call `name` (the derived §5.2 filename). Anything
    /// already at that slot is superseded: a host that reuses a file must truncate it here.
    fn create(&self, slot: usize, name: &str) -> Result<(), String>;
    /// Append `bytes` to shard `slot`. A short write is a failure, not a partial success.
    fn write(&self, slot: usize, bytes: &[u8]) -> Result<(), String>;
    /// Fill `into` with `into.len()` bytes at `offset` of the **sealed** shard in `slot`, for the
    /// §4.8 read-back. Served through a [`BlockCache`], so this is called on the order of once per
    /// [`DEFAULT_READ_BLOCK`] rather than once per engine read.
    fn read_at(&self, slot: usize, offset: u32, into: &mut [u8]) -> Result<(), String>;
    /// No more bytes are coming for `slot`. A host that buffers must flush here: the very next thing
    /// that happens is §4.8 reading the shard back.
    fn seal(&self, slot: usize) -> Result<(), String>;
}

/// One shard the host wrote itself, once §4.8 has read it back and passed it (#1116 D1) — the
/// write-side answer to [`Hooks::take_shard`], carrying an **identity instead of bytes** because the
/// host already has the file.
///
/// The name and the digest are this crate's own arithmetic, computed mid-run because `Summary` does
/// not exist yet, and both are cross-checked against the engine's when the run ends (see
/// [`check_handoff`]). A shard reported here is **not** in [`Outcome::files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedShard {
    /// The slot it was written to — the `create`/`write`/`seal` argument, and the set index.
    pub slot: usize,
    /// The derived 8.3 filename (`MS<id>S<kk>.OBM`) the set calls it.
    pub name: String,
    /// `"core"`, `"coarse"` or `"geometry"`.
    pub role: &'static str,
    /// Lowercase-hex SHA-256 of the bytes the sink was handed, as the manifest will record it.
    pub sha256: String,
    /// How many bytes the sink was handed — what the host's file must be long.
    pub byte_length: u64,
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
    /// The block the input read cache fetches and evicts in, for [`SourceCell`] inputs only
    /// (#1116 B2). Clamped to [`MIN_READ_BLOCK`]..=[`MAX_READ_BLOCK`]; the cache's whole residency
    /// is this times [`READ_CACHE_BLOCKS`].
    ///
    /// Exposed because it is the one number that trades host calls against read amplification, and
    /// because `1` turns the cache **off** — one host call per engine read — which is what both its
    /// transparency (the same bytes either way) and its cost are measured against.
    pub read_block_bytes: usize,
    /// The most memory the §4.6 merge's sorted passes may hold (#1116 D2). A rationed host sets it
    /// from what the tab is allowed to use; it changes what the merge holds, never what it writes.
    pub merge_budget_bytes: usize,
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
            read_block_bytes: DEFAULT_READ_BLOCK,
            merge_budget_bytes: d.merge_budget_bytes,
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
                "readBlockBytes" => {
                    let n = value.as_u64().ok_or("options.readBlockBytes must be a number")?;
                    // Clamped rather than refused: it is a performance knob, and a browser that
                    // asks for something absurd should assemble the same map a little slower.
                    o.read_block_bytes = (n as usize).clamp(MIN_READ_BLOCK, MAX_READ_BLOCK);
                }
                // Clamped for the same reason the read block is: a budget of zero is not a smaller
                // merge, it is one that cannot make progress, and the floor is one record.
                "mergeBudgetBytes" => {
                    let n = value.as_u64().ok_or("options.mergeBudgetBytes must be a number")?;
                    o.merge_budget_bytes = (n as usize).max(MIN_MERGE_BUDGET);
                }
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

/// How much of a [`SourceCell`] one host read brings back by default (#1116 B2).
///
/// 64 KiB against the engine's two access patterns: §4.6.6's per-record emission and the §4.6
/// merge's 512-byte node chunks are sequential inside a cell, so one block serves ~128 of them and
/// the amplification of over-reading is nil; §2.3's verbatim geometry copy asks for 256 KiB at a
/// time and skips the cache entirely. Smaller would cost calls for nothing; larger would make the
/// first read of a small cell fetch most of it.
pub(crate) const DEFAULT_READ_BLOCK: usize = 64 * 1024;
/// The floor a caller can ask for: `1`, which is not a small cache but **no cache** — every read is
/// at least one byte, so every read takes the bypass and becomes exactly one host call. That is the
/// configuration the cache is measured against (`the_read_block_size_changes_the_call_count_and_not_
/// the_bytes` natively, and `bridge.test.ts`'s per-crossing cost in Node), which is why the floor is
/// a degenerate setting rather than a merely small one.
const MIN_READ_BLOCK: usize = 1;
/// …and the ceiling, so a mistyped option cannot reserve a quarter of the heap for read scratch.
const MAX_READ_BLOCK: usize = 4 * 1024 * 1024;
/// How many blocks the input read cache holds. The engine reads one cell at a time within a phase,
/// so the working set is one or two blocks; the rest is slack for the seams where §4.6.6 crosses
/// from one source cell to the next. Sixteen keeps the whole cache at 1 MiB and the miss scan at
/// sixteen comparisons — which matters, because that scan runs on every engine read.
/// `pub(crate)` with [`DEFAULT_READ_BLOCK`] because `estimate.rs` prices the streamed input path
/// as this cache — restating the product there would let the two drift.
pub(crate) const READ_CACHE_BLOCKS: usize = 16;

/// The floor for [`BridgeOptions::merge_budget_bytes`]: 64 KiB. Below it the merge still produces
/// the same map, one run per few thousand records, and spends all its time in the k-way merge — so
/// a mistyped option is slow rather than wrong.
const MIN_MERGE_BUDGET: usize = 64 * 1024;

/// One resident block of one [`SourceCell`].
struct CachedBlock {
    /// Which source, and which block of it. `slot == usize::MAX` marks a slot that has never been
    /// filled, which no real source can collide with.
    slot: usize,
    index: usize,
    /// The block's bytes. Exactly `len` of them are valid — the last block of a cell is short.
    data: Vec<u8>,
    len: usize,
    /// The clock reading at the last hit, for the LRU eviction.
    used: u64,
}

/// One slotted, offset-addressed byte source on the **host's** side of the boundary.
///
/// Both seams have this shape — [`CellReads`] for the input cells (#1116 B2) and the read-back half
/// of [`ShardWrites`] for the output shards (#1116 D1) — so one [`BlockCache`] serves either, and
/// the argument for the cache (module header) is made once rather than twice.
trait SlotReads {
    fn read_slot(&self, slot: usize, offset: u32, buf: &mut [u8]) -> Result<(), String>;
}

/// The input cells, as a cache reads them.
struct CellSlots<'r>(&'r dyn CellReads);

impl SlotReads for CellSlots<'_> {
    fn read_slot(&self, slot: usize, offset: u32, buf: &mut [u8]) -> Result<(), String> {
        self.0.read(slot, offset, buf)
    }
}

/// …and the sealed shards, which the §4.8 pass reads back through the same handle they were written
/// through.
struct SinkSlots<'w>(&'w dyn ShardWrites);

impl SlotReads for SinkSlots<'_> {
    fn read_slot(&self, slot: usize, offset: u32, buf: &mut [u8]) -> Result<(), String> {
        self.0.read_at(slot, offset, buf)
    }
}

/// The bridge between a [`SlotReads`] host and the engine's [`ByteSource`] reads: a fixed-size LRU
/// of blocks shared by **every** slot, so residency is a constant rather than a per-cell cost.
///
/// See the module header for why this exists at all. Two rules are load-bearing:
///
/// * **A read at least one block long bypasses it**, going straight from the host into the caller's
///   buffer. The verbatim geometry copy is 256 KiB a time and would otherwise evict the cache on
///   every block for bytes nothing reads twice.
/// * **A miss is a failure the run can name.** [`ByteSource`]'s error type carries no message, so
///   the host's own is parked here and [`assemble`] prefers it over the engine's — a cell that could
///   not be read is [`ErrorCode::Io`] with the cell's id in it, never a panic across the FFI seam
///   and never a §4.8 "the assembler wrote a set the reader cannot read".
struct BlockCache<'r> {
    reads: &'r dyn SlotReads,
    block: usize,
    /// Per slot, how a message names that source. Set once per slot — the input's are all known
    /// before the run, the sink's as each shard is created — so the read path never formats.
    labels: RefCell<Vec<String>>,
    /// What a message calls a slot no label was ever set for. Only reachable through a defect, but
    /// the two seams' defects read very differently.
    unnamed: &'static str,
    slots: RefCell<Vec<CachedBlock>>,
    clock: StdCell<u64>,
    /// The first host failure, kept for [`map_error`]. First rather than last: everything after it
    /// is the engine unwinding.
    failure: RefCell<Option<String>>,
}

impl<'r> BlockCache<'r> {
    fn new(reads: &'r dyn SlotReads, block: usize, labels: Vec<String>, unnamed: &'static str) -> BlockCache<'r> {
        BlockCache {
            reads,
            block,
            labels: RefCell::new(labels),
            unnamed,
            slots: RefCell::new(Vec::new()),
            clock: StdCell::new(0),
            failure: RefCell::new(None),
        }
    }

    /// Name a slot that did not exist when the cache was built — the sink's, whose filenames are
    /// derived one shard at a time as the engine plans them.
    fn name_slot(&self, slot: usize, label: String) {
        let mut labels = self.labels.borrow_mut();
        if labels.len() <= slot {
            labels.resize(slot + 1, String::new());
        }
        labels[slot] = label;
    }

    /// Drop everything cached for `slot`, because whatever is behind it is about to change.
    ///
    /// Only the sink needs this, and only in principle: a slot is a shard index, so a slot is
    /// written once and read back once. It runs at `create` anyway, because a cache over a file that
    /// something else may rewrite is the kind of thing that is correct until it silently is not.
    fn forget_slot(&self, slot: usize) {
        self.slots.borrow_mut().retain(|b| b.slot != slot);
    }

    /// Record the host's own message and answer the engine in the only vocabulary the seam has.
    fn fail(&self, slot: usize, at: usize, message: String) -> obc_formats::io::Error {
        let named = {
            let labels = self.labels.borrow();
            labels.get(slot).filter(|l| !l.is_empty()).cloned()
        };
        let mut failure = self.failure.borrow_mut();
        if failure.is_none() {
            let what = named.unwrap_or_else(|| self.unnamed.to_string());
            *failure = Some(format!("{what} could not be read at byte {at}: {message}"));
        }
        obc_formats::io::Error::Io
    }

    /// One host read, straight into `buf`.
    fn fetch(&self, slot: usize, at: usize, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        let offset = u32::try_from(at).map_err(|_| obc_formats::io::Error::BadOffset)?;
        self.reads.read_slot(slot, offset, buf).map_err(|e| self.fail(slot, at, e))
    }

    /// The index of the cache slot holding block `index` of `slot`, filling it if it is not there.
    fn block_of(&self, slot: usize, index: usize, source_len: usize) -> Result<usize, obc_formats::io::Error> {
        let now = self.clock.get().wrapping_add(1);
        self.clock.set(now);
        {
            let mut slots = self.slots.borrow_mut();
            if let Some(k) = slots.iter().position(|b| b.slot == slot && b.index == index) {
                slots[k].used = now;
                return Ok(k);
            }
        }
        // Fill outside the borrow: the host call is arbitrary code, and holding a `RefCell` across
        // it would turn a re-entrant caller into a panic instead of a refusal.
        let start = index * self.block;
        let len = self.block.min(source_len.saturating_sub(start));
        if len == 0 {
            return Err(obc_formats::io::Error::BadOffset);
        }
        let mut data = vec![0u8; len];
        self.fetch(slot, start, &mut data)?;

        let mut slots = self.slots.borrow_mut();
        let k = if slots.len() < READ_CACHE_BLOCKS {
            slots.push(CachedBlock { slot: usize::MAX, index: 0, data: Vec::new(), len: 0, used: 0 });
            slots.len() - 1
        } else {
            // Least recently used. Sixteen entries, so a scan beats keeping an order.
            slots.iter().enumerate().min_by_key(|(_, b)| b.used).map(|(k, _)| k).expect("the cache is not empty")
        };
        slots[k] = CachedBlock { slot, index, len: data.len(), data, used: now };
        Ok(k)
    }

    /// [`ByteSource::read_at`] for one source cell: whole blocks through the LRU, big reads around
    /// it.
    fn read_at(&self, slot: usize, offset: u32, source_len: u32, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        let at = offset as usize;
        let end = at.checked_add(buf.len()).ok_or(obc_formats::io::Error::BadOffset)?;
        // Checked here rather than left to the host: a read past a cell's declared end is a defect
        // in the engine or a wrong length in the catalog, and it must read as one — not as whatever
        // a short host read happens to leave in the buffer.
        if end > source_len as usize {
            return Err(obc_formats::io::Error::BadOffset);
        }
        if buf.is_empty() {
            return Ok(());
        }
        if buf.len() >= self.block {
            return self.fetch(slot, at, buf);
        }
        let mut done = 0usize;
        while done < buf.len() {
            let cursor = at + done;
            let index = cursor / self.block;
            let k = self.block_of(slot, index, source_len as usize)?;
            let slots = self.slots.borrow();
            let b = &slots[k];
            let within = cursor - index * self.block;
            let n = (b.len - within).min(buf.len() - done);
            if n == 0 {
                return Err(obc_formats::io::Error::BadOffset);
            }
            buf[done..done + n].copy_from_slice(&b.data[within..within + n]);
            done += n;
        }
        Ok(())
    }
}

/// The reader an assembly with no [`SourceCell`] inputs — or no [`ShardWrites`] sink — is given, so
/// neither cache needs an `Option`. Reaching it means a `KeyedSource` exists without a reader (which
/// [`assemble`] refuses first) or a sunk shard exists without a sink (which cannot be constructed).
struct NoReads;

impl SlotReads for NoReads {
    fn read_slot(&self, slot: usize, _offset: u32, _buf: &mut [u8]) -> Result<(), String> {
        Err(format!("slot {slot} has no reader — this assembly was wired without one"))
    }
}

/// One [`SourceCell`] as the engine reads it: a slot number, the catalog's length, and the shared
/// cache. No bytes — that is the entire point.
struct KeyedSource<'a, 'r> {
    slot: usize,
    len: u32,
    cache: &'a BlockCache<'r>,
}

impl ByteSource for KeyedSource<'_, '_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        self.cache.read_at(self.slot, offset, self.len, buf)
    }

    fn len(&self) -> u32 {
        self.len
    }
}

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

    /// One shard the host's own [`ShardWrites`] sink wrote has passed its §4.8 read-back (#1116 D1).
    ///
    /// The same moment as [`Hooks::take_shard`] and the same consequences — the file is not in
    /// [`Outcome::files`], a failed or cancelled run may already have reported some, and §5.4 makes
    /// what it reported invisible as a map until the manifest exists. What differs is that there are
    /// no bytes to hand over: the host wrote them, so it is told *what it has*.
    ///
    /// It is not gated on [`Hooks::wants_shards`]. The sink **is** the hand-off: bytes that never
    /// entered linear memory cannot be kept there, so a caller that wires a sink is told about every
    /// shard whether or not it also asked for the buffered eviction path.
    ///
    /// `Err(message)` stops the run as [`ErrorCode::Io`] with that message, because a set whose
    /// files the caller could not record must not be reported as finished.
    fn shard_sealed(&mut self, shard: SealedShard) -> Result<(), String> {
        let _ = shard;
        Ok(())
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

/// Where one shard's bytes actually are.
///
/// The two are the same shard to the engine and to the format — the difference is only whether this
/// address space is holding it. That is why the enum is here rather than two stores: everything
/// about §4.8, the digest, the filename and the hand-off moment is shared, and only `read_at` and
/// `len` branch.
enum ShardBody<'a> {
    /// In wasm memory, as it always was: no sink, or a native caller that wants the bytes.
    Buffered(Vec<u8>),
    /// In the host's own storage (#1116 D1) — a slot number, how many bytes were handed over, and
    /// the cache the §4.8 read-back pulls them back through.
    Sunk { slot: usize, len: u64, cache: &'a BlockCache<'a> },
}

impl ShardBody<'_> {
    /// The shard's length. A shard is `< 4 GiB` by OBCA §5.7, which the engine refuses past long
    /// before here; the clamp is so a defect reads as a truncated file rather than a wrapped one.
    fn len(&self) -> u32 {
        match self {
            ShardBody::Buffered(bytes) => bytes.len() as u32,
            ShardBody::Sunk { len, .. } => u32::try_from(*len).unwrap_or(u32::MAX),
        }
    }
}

/// A sealed shard as the §4.8 verify pass reads it back — the bytes, wherever they are, plus the
/// progress and abort seam **inside the read-back's own loop**.
///
/// This is [`obcm_assemble::MemorySource`] with two lines added, and those two lines are the reason
/// this crate keeps its own store instead of delegating to [`obcm_assemble::MemoryStore`]:
/// [`ShardStore::source`] hands the engine a `&dyn ByteSource` and everything §4.8 does happens
/// behind it, so `read_at` is the only place a browser can learn that verify is moving — or tell it
/// to stop.
///
/// With a sink the read-back is a *file* read (#1116 D1), which is what makes §4.8 the check it
/// claims to be: the pass re-reads the bytes that were written rather than the ones this process
/// still happens to be holding.
struct VerifySource<'a, 'h> {
    body: ShardBody<'a>,
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
                // Checked *before* the read is issued, so a cancel during §4.8 costs at most one
                // outstanding host call — and never reaches the sink at all.
                return Err(obc_formats::io::Error::Io);
            }
        }
        match &self.body {
            ShardBody::Buffered(bytes) => SliceSource(bytes).read_at(offset, buf),
            ShardBody::Sunk { slot, len, cache } => {
                cache.read_at(*slot, offset, u32::try_from(*len).unwrap_or(u32::MAX), buf)
            }
        }
    }

    fn len(&self) -> u32 {
        self.body.len()
    }
}

/// One shard's slot in the store: the bytes §4.8 reads back, the identity the hand-off needs before
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

/// The [`ShardStore`] the browser assembles through, plus the progress and abort seam.
///
/// A sealed shard has to be **randomly addressable**: OBCA §4.8 requires every one to be read back
/// through the real reader before the manifest is written. Where it is addressable *from* is this
/// store's one degree of freedom, and both answers are here — wasm memory, or the host's own storage
/// through a [`ShardWrites`] sink (#1116 D1). The second is what a country-scale core shard needs,
/// because it cannot be split and a browser cannot hold it.
struct HookedStore<'a, 'h> {
    shards: Vec<StoredShard<'a, 'h>>,
    manifest: Vec<u8>,
    /// Needed to derive a shard's filename at hand-off time, mid-run.
    card_id: u16,
    /// [`Hooks::wants_shards`], read once before the assembly starts.
    hand_off: bool,
    /// Where the shards' bytes go, when they do not go into this address space (#1116 D1). `None`
    /// is the buffered store, unchanged to the byte.
    sink: Option<&'a dyn ShardWrites>,
    /// …and how §4.8 reads them back. Present whether or not `sink` is, so the borrow graph needs no
    /// `Option`; unreachable without one, because only a `Sunk` body names it.
    sink_cache: &'a BlockCache<'a>,
    p: &'a RefCell<Progress<'h>>,
}

/// Park a sink failure where [`map_error`] will find it, and answer the engine in the only
/// vocabulary its sink contract has.
///
/// The `Error::Io` this returns says nothing about *why*; the host's own sentence is the whole value
/// of the seam ("the disk is full", "the handle is closed"), so it is kept here and preferred over
/// the engine's when the run unwinds. Free-standing rather than a method so a caller that is already
/// holding a shard mutably can still raise it.
fn sink_failed(p: &RefCell<Progress<'_>>, what: &str, name: &str, message: String) -> Error {
    let mut p = p.borrow_mut();
    if p.failure.is_none() {
        p.failure = Some(AssembleFailure::new(ErrorCode::Io, format!("{name} could not be {what}: {message}")));
    }
    Error::Io(obc_formats::io::Error::Io)
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

    /// Report every sealed-and-verified shard exactly once — to [`Hooks::take_shard`] if its bytes
    /// are here, to [`Hooks::shard_sealed`] if the host wrote them itself.
    ///
    /// Called from [`ShardStore::begin`] and [`ShardStore::manifest`] — the two observable moments
    /// at which the engine has finished with the previous shard (module header). Every shard in
    /// `self.shards` at those points has been through §4.8: `begin` is called before the new slot is
    /// pushed, and the manifest is written after the whole loop.
    ///
    /// Runs **after** each caller's `check_abort`, so a cancelled run hands nothing further out and
    /// a sink failure cannot be confused with a cancellation.
    fn hand_out_verified(&mut self) -> obcm_assemble::Result<()> {
        if !self.hand_off && self.sink.is_none() {
            return Ok(());
        }
        // A copy of the shared handle, so the loop below can borrow `self.shards` mutably.
        let p = self.p;
        for shard in self.shards.iter_mut().filter(|s| !s.offered) {
            shard.offered = true;
            if let ShardBody::Sunk { slot, len, .. } = shard.src.body {
                // No bytes to hand over — the host has the file. What it is told is what it has.
                let sealed = SealedShard {
                    slot,
                    name: shard.name.clone(),
                    role: shard.role,
                    sha256: shard.sha256.clone(),
                    byte_length: len,
                };
                // Bound first: a `match` on the call itself would hold the borrow through the arm
                // that takes it again.
                let recorded = p.borrow_mut().hooks.shard_sealed(sealed);
                match recorded {
                    Ok(()) => shard.handed_out = true,
                    Err(message) => {
                        p.borrow_mut().failure = Some(AssembleFailure::new(ErrorCode::Io, message));
                        return Err(Error::Io(obc_formats::io::Error::Io));
                    }
                }
                continue;
            }
            if !self.hand_off {
                continue;
            }
            let ShardBody::Buffered(bytes) = &mut shard.src.body else { unreachable!("the sunk case returned above") };
            let file = OutputFile {
                name: shard.name.clone(),
                role: shard.role,
                sha256: shard.sha256.clone(),
                bytes: core::mem::take(bytes),
            };
            let taken = p.borrow_mut().hooks.take_shard(file);
            match taken {
                Ok(None) => shard.handed_out = true,
                Ok(Some(back)) => shard.src.body = ShardBody::Buffered(back.bytes),
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
        let name = obcm_assemble::shard::shard_filename(self.card_id, plan.index);
        let body = match self.sink {
            // The host takes the bytes. Nothing shard-sized is reserved here, which is the whole
            // point: §5 may have just planned a 3 GiB core shard.
            Some(sink) => {
                sink.create(plan.index, &name).map_err(|e| sink_failed(self.p, "created", &name, e))?;
                // So a failed read can say *which* shard, and so a slot that is being written for
                // the first time cannot serve a block from anything else.
                self.sink_cache.name_slot(plan.index, format!("shard {name}"));
                self.sink_cache.forget_slot(plan.index);
                ShardBody::Sunk { slot: plan.index, len: 0, cache: self.sink_cache }
            }
            None => {
                let mut bytes = Vec::new();
                // §5 computes a shard's exact size before a byte of it is written, so the browser
                // can have the buffer it needs in one allocation instead of a doubling ladder whose
                // last step transiently holds 1.5× a gigabyte-scale shard — memory the estimate does
                // not model and a tab may not have contiguously. `try_` because a refusal here is
                // recoverable: the write path would then grow the vector itself and fail (or not)
                // exactly as it used to, whereas a plain `reserve_exact` would abort the whole
                // module on a capacity a shard might never actually reach.
                let _ = bytes.try_reserve_exact(usize::try_from(plan.bytes).unwrap_or(0));
                ShardBody::Buffered(bytes)
            }
        };
        self.shards.push(StoredShard {
            src: VerifySource { body, p: self.p },
            name,
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
        // that also reports where it got to — and *before* the sink is touched, so a cancel costs at
        // most the write already in flight.
        self.check_abort()?;
        let digesting = self.hand_off || self.sink.is_some();
        let sink = self.sink;
        let p = self.p;
        let shard = self.shards.last_mut().expect("a shard is open");
        // The engine hashes the same bytes on its way through `shard::write`, but it keeps that
        // digest to itself (it lands in the plan *after* `seal`). Hashing here is therefore what
        // makes a mid-run hand-off able to name its own file — and the end-of-run comparison against
        // the engine's figure is then a real check that the bytes handed out (or written to the
        // host's own file) are the bytes the engine wrote, not a restatement.
        if digesting {
            shard.hasher.update(buf);
        }
        match &mut shard.src.body {
            ShardBody::Buffered(bytes) => bytes.extend_from_slice(buf),
            ShardBody::Sunk { slot, len, .. } => {
                let sink = sink.expect("a sunk shard has a sink");
                sink.write(*slot, buf).map_err(|e| sink_failed(p, "written", &shard.name, e))?;
                *len += buf.len() as u64;
            }
        }
        Ok(())
    }

    fn seal(&mut self) -> obcm_assemble::Result<()> {
        let digesting = self.hand_off || self.sink.is_some();
        let sink = self.sink;
        let p = self.p;
        let shard = self.shards.last_mut().expect("a shard is open");
        if digesting {
            let digest = core::mem::take(&mut shard.hasher).finalize();
            shard.sha256 = digest.iter().map(|b| format!("{b:02x}")).collect();
        }
        if let (Some(sink), ShardBody::Sunk { slot, .. }) = (sink, &shard.src.body) {
            // Before the abort check and before §4.8: the very next thing the engine does is read
            // this shard back, so a host that buffers has to have flushed by now.
            sink.seal(*slot).map_err(|e| sink_failed(p, "sealed", &shard.name, e))?;
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
        // ever does, it is this bridge that broke the premise, and it says so. A **sunk** shard has
        // no such failure mode: reporting it to the host took nothing away, and the file is still
        // there to read.
        match self.shards.get(index) {
            Some(s) if s.handed_out && matches!(s.src.body, ShardBody::Buffered(_)) => {
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
///
/// This is [`assemble`] with every cell's bytes in hand. A caller whose cells live outside wasm
/// memory (#1116 B2), or one that would rather the shards did not either (#1116 D1), builds a
/// [`Wiring`] instead.
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
    assemble(
        Wiring { cells, known_empty, terrain, terrain_cells, ..Wiring::default() },
        schema_json,
        skin_json,
        opts,
        hooks,
    )
}

/// Everything one assembly is wired to: the cells, in whichever of the two forms the host has them,
/// the selection's canonical-empty coverage, the raster — and, if the host would rather keep the
/// output than have this address space hold it, where the shards go.
///
/// The two cell lists are alternatives rather than halves — a host uses buffers ([`CellBytes`]) or
/// keys ([`SourceCell`]), and the browser picks by capability — but both may be present, and the
/// engine sees one list: the buffered cells first, then the source-backed ones. Order is not part of
/// the output: §4.6.5 renumbers by content and §5 plans by role, which is what makes an assembly
/// deterministic however the downloads finished.
#[derive(Default)]
pub struct Wiring<'r> {
    /// Cells whose bytes are already in wasm memory.
    pub cells: Vec<CellBytes>,
    /// Cells the host serves through [`Wiring::reads`], by slot — their index in **this** list.
    pub source_cells: Vec<SourceCell>,
    /// Required if `source_cells` is non-empty, and unused otherwise.
    pub reads: Option<&'r dyn CellReads>,
    pub known_empty: Vec<KnownEmptyCell>,
    pub terrain: Option<TerrainLattice>,
    pub terrain_cells: Vec<TerrainCellBytes>,
    /// Where the OBCM shards are written, by slot — their index in the set (#1116 D1). `None` keeps
    /// them in wasm memory, which is what every native caller wants and what the browser falls back
    /// to. With one, no shard is ever resident and none appears in [`Outcome::files`]: each is
    /// reported by identity to [`Hooks::shard_sealed`] as it passes §4.8.
    ///
    /// The **terrain** shard and the OBCS manifest are not affected — terrain is written through the
    /// engine's own sink and the manifest is a few hundred bytes, so both stay in `files`.
    pub sink: Option<&'r dyn ShardWrites>,
}

/// Assemble one selection into a `.obcm` or an OBCA volume set, reporting through `hooks`.
pub fn assemble(
    wiring: Wiring<'_>,
    schema_json: &str,
    skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    let Wiring { cells, source_cells, reads, known_empty, terrain, terrain_cells, sink } = wiring;
    if cells.is_empty() && source_cells.is_empty() {
        return Err(AssembleFailure::new(
            ErrorCode::Input,
            "No OBCM cell artifact was handed to the assembler. An assembly needs at least one artifact to verify \
             the schema revision's binary style and routing-profile tables (OBCA §3.8).",
        ));
    }
    // A key with no way to resolve it is a host that wired half of the streamed path. Refused up
    // front rather than at the first read, where it would arrive as an I/O failure and read like a
    // storage problem.
    let reads = match (source_cells.is_empty(), reads) {
        (false, None) => {
            return Err(AssembleFailure::new(
                ErrorCode::Internal,
                format!(
                    "{} cell(s) were handed over by key, but no read callback was supplied — there is no way to \
                     fetch their bytes.",
                    source_cells.len()
                ),
            ))
        }
        (_, r) => r,
    };
    let schema = Schema::parse(schema_json).map_err(|e| AssembleFailure::new(ErrorCode::Internal, e))?;
    let skin = Skin::parse(skin_json).map_err(|e| AssembleFailure::new(ErrorCode::Internal, e))?;

    let projected: u64 = cells.iter().map(|c| c.bytes.len() as u64).sum::<u64>()
        + source_cells.iter().map(|c| c.byte_length as u64).sum::<u64>();
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
    // …and the same for the cells that have no payload here at all: identity now, bytes on demand.
    let mut keyed_ids = Vec::with_capacity(source_cells.len());
    let mut labels = Vec::with_capacity(source_cells.len());
    for c in &source_cells {
        let id = CellId::parse(&c.id).map_err(|e| {
            AssembleFailure::new(ErrorCode::Internal, format!("cell id {:?} is not a `<log2>/<i>/<j>` id: {e}", c.id))
        })?;
        keyed_ids.push((id, c.band.clone(), c.partial, c.byte_length));
        labels.push(format!("cell {} of band {:?} ({})", c.id, c.band, c.key));
    }
    // A cache is built even with nothing to read through it: it is one empty `Vec` and it keeps the
    // borrow graph below free of an `Option`. `NoReads` is never reached in that case — the loop
    // that would call it has no iterations.
    let no_reads = NoReads;
    let cell_slots = reads.map(CellSlots);
    let cache = BlockCache::new(
        cell_slots.as_ref().map_or(&no_reads as &dyn SlotReads, |c| c as &dyn SlotReads),
        opts.read_block_bytes,
        labels,
        "an unknown cell",
    );
    // …and the same on the write side, for the §4.8 read-back of shards the host wrote itself. Its
    // labels are filled in as the engine plans each shard, because a shard's derived filename does
    // not exist until then.
    let sink_slots = sink.map(SinkSlots);
    let sink_cache = BlockCache::new(
        sink_slots.as_ref().map_or(&no_reads as &dyn SlotReads, |s| s as &dyn SlotReads),
        opts.read_block_bytes,
        Vec::new(),
        "an unnamed shard",
    );
    let keyed: Vec<KeyedSource<'_, '_>> = keyed_ids
        .iter()
        .enumerate()
        .map(|(slot, (_, _, _, len))| KeyedSource { slot, len: *len, cache: &cache })
        .collect();
    let inputs: Vec<CellInput<'_>> = ids
        .iter()
        .zip(&sources)
        .map(|((id, band, partial), src)| CellInput {
            id: *id,
            band: band.clone(),
            src: src as &dyn ByteSource,
            partial: *partial,
        })
        .chain(keyed_ids.iter().zip(&keyed).map(|((id, band, partial, _), src)| CellInput {
            id: *id,
            band: band.clone(),
            src: src as &dyn ByteSource,
            partial: *partial,
        }))
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
        merge_budget_bytes: opts.merge_budget_bytes,
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
    let mut store = HookedStore {
        shards: Vec::new(),
        manifest: Vec::new(),
        card_id: options.card_id,
        hand_off,
        sink,
        sink_cache: &sink_cache,
        p: &progress,
    };
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
    // The engine's third seam (#1116 D2). In wasm linear memory for now: the browser's own scratch
    // is an OPFS-backed store, and it lands with the passes that make it pay for itself — until
    // then this is the same bytes at a peak that is a wash against the arrays it replaced.
    let scratch = MemoryScratch::new();
    let summary = match assemble_full(inputs, known_empty, job, &schema, &skin, &options, &mut store, &clock, &scratch)
    {
        Ok(s) => s,
        Err(e) => {
            let p = progress.borrow();
            // An input cell that could not be read is the root cause of whatever the engine went on
            // to report — `Cell::open` turns a failed read into "not a readable OBCM", which would
            // blame the catalog for the browser's storage. The host's own message wins. A shard that
            // could not be read *back* is the same story one seam over, and it matters more: §4.8
            // reports every read failure as a verify defect, so without this a full disk would tell
            // a rider the assembler is broken.
            let read_failure = cache
                .failure
                .borrow()
                .clone()
                .or_else(|| sink_cache.failure.borrow().clone())
                .map(|message| AssembleFailure::new(ErrorCode::Io, message));
            return Err(map_error(e, p.aborted, read_failure.or_else(|| p.failure.clone())));
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
        // …and the same claim, made harder, when the host wrote the file itself: it saved bytes
        // under this name and recorded this digest without ever seeing them, so the equality below
        // is the only thing standing between a mislabelled file and a card.
        if hand_off || sink.is_some() {
            check_handoff(s.index, (&shard.name, &shard.sha256), (&s.filename, &sha256))?;
        }
        if shard.handed_out {
            continue;
        }
        let ShardBody::Buffered(bytes) = shard.src.body else {
            // A sunk shard is always reported and always `handed_out`; reaching here would mean the
            // hand-out loop skipped one, and the alternative to saying so is an empty `.OBM`.
            return Err(AssembleFailure::new(
                ErrorCode::Internal,
                format!(
                    "shard {} ({}) was written to the host's own sink but never reported as sealed — this bridge \
                     would have handed back an empty file.",
                    s.index, s.filename
                ),
            ));
        };
        files.push(OutputFile { name: s.filename.clone(), role: s.role.as_str(), sha256, bytes });
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
        // The scratch seam is storage like any other, and a browser's is OPFS — "the working area
        // failed" is an `io` problem to a caller, and the engine's message says which one.
        Error::Scratch(_) => ErrorCode::Io,
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
