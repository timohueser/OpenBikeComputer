//! The target-independent assembly driver: cell byte buffers in, one map file out, a progress/abort
//! seam, and a typed failure carrying the engine's own message.
//!
//! Progress is read from two seams the engine already calls. It calls the clock once per phase
//! boundary, in a fixed order, so ticks 1 to 5 name the phases `open`, `poi`, `nav`, `plan` and
//! `write`. From the write on, the store's own calls say which phase runs. The verify read-back
//! makes no store calls after [`MapStore::source`], and it is about two fifths of a run, so
//! [`VerifySource`] wraps the sealed map and reports from inside the read-back loop.
//!
//! [`Hooks::progress`] returns `true` to abort. The request takes effect at the next store call or
//! verify read. Inside the write or the verify pass that is prompt, and the refusal is always
//! [`ErrorCode::Aborted`], never an [`ErrorCode::Verify`] defect: `verify_map` turns any read
//! failure into `Error::Verify`, so [`map_error`] reads the abort flag first. The nav rewrite makes
//! no store calls, so an abort during it is honoured only when the phase ends.
//!
//! [`assemble_cells`] blocks. A Bundesland-scale assembly is about 16 s of straight-line compute, so
//! the browser must run it in a Web Worker: a main thread that is itself blocked cannot observe a
//! cooperative abort, and the UI's cancel is `worker.terminate()`.
//!
//! Neither the input cells nor the map have to be in wasm memory. [`SourceCell`] and [`CellReads`]
//! let a cell stay in host storage, and [`MapWrites`] takes the map: one country-scale map is a
//! single object of about 9 GiB, larger than a 32-bit address space. [`BlockCache`] stands between
//! both seams and the engine. It holds [`READ_CACHE_BLOCKS`] blocks across all sources, so residency
//! does not grow with the selection, and a read at least one block long bypasses it. With a sink the
//! map is reported by identity ([`SealedMap`]) and [`Outcome::bytes`] is `None`.

use std::cell::{Cell as StdCell, RefCell};

use obc_formats::io::{ByteSource, SliceSource};
use obcm_assemble::grid::CellId;
use obcm_assemble::schema::{MapStyles, Schema};
use obcm_assemble::{
    assemble_full, CellInput, Clock, Error, KnownEmptyInput, MapStore, MemoryScratch, MemorySource, Options, ScratchId,
    ScratchStore, TerrainCellInput, TerrainJob, TerrainParams,
};
use sha2::{Digest, Sha256};

/// One downloaded cell, as the caller hands it over: the catalog's identity plus the verified bytes.
///
/// `band` cannot be inferred from the bytes, because a legitimately empty cell and an out-of-band
/// cell look the same. The catalog states it.
pub struct CellBytes {
    /// The canonical cell id, `<log2>/<i>/<j>`.
    pub id: String,
    pub band: String,
    pub partial: bool,
    pub bytes: Vec<u8>,
}

/// One downloaded cell whose bytes are not in wasm memory: the identity [`CellBytes`] carries, the
/// length the catalog published, and an opaque host key.
///
/// `key` is never interpreted here. It only makes a read failure name the cell. Reads go through
/// [`CellReads`], which names a cell by its slot: its index in the `source_cells` list handed to
/// [`assemble`].
pub struct SourceCell {
    /// The canonical cell id, `<log2>/<i>/<j>`.
    pub id: String,
    pub band: String,
    pub partial: bool,
    /// The object's length, from the catalog. [`ByteSource::len`] answers it, so a wrong length is
    /// a format error at open and not a silent truncation.
    pub byte_length: u32,
    /// The host's own name for the bytes. Only ever shown in a message.
    pub key: String,
}

/// How a host serves the bytes of a [`SourceCell`].
///
/// Called from inside the synchronous assembly, with the engine blocked behind it, which is why the
/// seam is a blocking call and not a future. An implementation must not call back into the
/// assembler.
pub trait CellReads {
    /// Fill `buf` with `buf.len()` bytes at `offset` of the object in `slot`.
    ///
    /// `Err(message)` fails the run as [`ErrorCode::Io`], with the message quoted after the cell's
    /// own name. A short read is a failure: the buffer must be filled or the call must refuse.
    fn read(&self, slot: usize, offset: u64, buf: &mut [u8]) -> Result<(), String>;
}

/// How a host takes the bytes of the map, and gives them back.
///
/// The write-side twin of [`CellReads`]. There is one file, so nothing here is indexed.
///
/// Every method is called from inside the synchronous assembly, with the engine blocked behind it.
/// In the browser that is a `FileSystemSyncAccessHandle` opened before the run starts, because the
/// opener is async and the run cannot await.
///
/// The lifecycle is the engine's: `create` once, `write` many times in order, `seal` once, then any
/// number of [`MapWrites::read_at`] calls while the verify pass reads the map back. `Err(message)`
/// from any of them fails the run as [`ErrorCode::Io`], never as an [`ErrorCode::Verify`] defect,
/// which would tell a rider the assembler is broken because their disk filled up.
pub trait MapWrites {
    /// Begin the map. Anything already there is superseded: a host that reuses a file must truncate
    /// it here.
    fn create(&self) -> Result<(), String>;
    /// Append `bytes` to the map. A short write is a failure, not a partial success.
    fn write(&self, bytes: &[u8]) -> Result<(), String>;
    /// Fill `into` with `into.len()` bytes at `offset` of the sealed map, for the verify read-back.
    fn read_at(&self, offset: u64, into: &mut [u8]) -> Result<(), String>;
    /// No more bytes are coming. A host that buffers must flush here: the verify read-back is the
    /// very next thing that happens.
    fn seal(&self) -> Result<(), String>;
}

/// The map the host wrote itself, once the verify pass has read it back: an identity instead of
/// bytes, because the host already has the file.
///
/// The digest is taken from the bytes as they crossed into the sink, and it is checked against the
/// engine's before the caller is told anything. A map reported here has no [`Outcome::bytes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedMap {
    /// Lowercase-hex SHA-256 of the bytes the sink was handed.
    pub sha256: String,
    /// How many bytes the sink was handed — what the host's file must be long.
    pub byte_length: u64,
}

/// One selected cell the pinned catalog says has canonical empty content: an identity with no
/// payload buffer.
pub struct KnownEmptyCell {
    pub id: String,
    pub band: String,
}

/// How a host takes the engine's spill: anonymous files named by an id the host mints, append-only
/// writes, `u64` random-access reads, synchronous.
///
/// In the browser it is a pool of OPFS sync access handles opened before the run, so `create` can
/// refuse when the pool is exhausted. That means the budget is too small for the selection, and the
/// message should say so.
///
/// `Err(message)` from any method fails the run as [`ErrorCode::Io`] with that message, reported as
/// the working area failing and never as a broken input or a verify defect.
pub trait ScratchWrites {
    /// Mint an empty scratch file and answer its id. Ids are the host's, and a host must never
    /// re-issue an id it has removed.
    fn create(&self) -> Result<u32, String>;
    /// Append `bytes` to `id`'s current end. A short write is a failure.
    fn append(&self, id: u32, bytes: &[u8]) -> Result<(), String>;
    /// Fill `into` with exactly `into.len()` bytes at `offset`. A short read is a failure: a
    /// truncated spill read as data corrupts the merge silently.
    fn read_at(&self, id: u32, offset: u64, into: &mut [u8]) -> Result<(), String>;
    /// How many bytes have been appended to `id`.
    fn len(&self, id: u32) -> Result<u64, String>;
    /// Drop `id` and reclaim its bytes.
    fn remove(&self, id: u32) -> Result<(), String>;
}

/// [`ScratchWrites`] as the engine sees it: an [`obcm_assemble::ScratchStore`] whose every failure
/// is `Error::Scratch` with the host's own sentence in it.
///
/// Nothing is tracked here. The engine removes what it creates, and the host sweeps the rest when
/// the run ends.
struct HostScratch<'a>(&'a dyn ScratchWrites);

impl obcm_assemble::ScratchStore for HostScratch<'_> {
    fn create(&self) -> obcm_assemble::Result<ScratchId> {
        self.0.create().map(ScratchId).map_err(Error::Scratch)
    }

    fn append(&self, id: ScratchId, buf: &[u8]) -> obcm_assemble::Result<()> {
        self.0.append(id.0, buf).map_err(Error::Scratch)
    }

    fn read_at(&self, id: ScratchId, offset: u64, buf: &mut [u8]) -> obcm_assemble::Result<()> {
        self.0.read_at(id.0, offset, buf).map_err(Error::Scratch)
    }

    fn len(&self, id: ScratchId) -> obcm_assemble::Result<u64> {
        self.0.len(id.0).map_err(Error::Scratch)
    }

    fn remove(&self, id: ScratchId) -> obcm_assemble::Result<()> {
        self.0.remove(id.0).map_err(Error::Scratch)
    }
}

/// One downloaded terrain cell: its id on the terrain grid, the whole `.obcd` object, and the
/// `sha256` the pinned terrain index published for it.
///
/// A known-empty terrain square is not handed over at all. It has no object to fetch, and an absent
/// cell reads the same as an all-`NODATA` one.
pub struct TerrainCellBytes {
    /// The canonical cell id, `<cell_log2>/<i>/<j>`, on the terrain grid.
    pub id: String,
    /// Lowercase-hex SHA-256 from the terrain index. Empty means "no catalog", which the browser
    /// never has; it is here so the type can be built in a test.
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// The terrain store's lattice, verbatim from the catalog's `terrain` block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainLattice {
    pub posting_log2: u8,
    pub cell_log2: u8,
}

/// What an assembly can be told to do differently: [`obcm_assemble::Options`] without
/// `skip_verify`, plus the one knob that belongs to this crate.
///
/// `skip_verify` is not offered. Verification is a precondition of writing a map, and this bridge
/// exists to hand bytes to a device.
#[derive(Clone, Debug)]
pub struct BridgeOptions {
    /// Proceed although a selected cell is missing.
    pub accept_holes: bool,
    /// Proceed although a cell is `partial`.
    pub accept_partial: bool,
    /// The block the input cache fetches and evicts. Clamped to
    /// [`MIN_READ_BLOCK`]..=[`MAX_READ_BLOCK`]; cache residency is this times
    /// [`READ_CACHE_BLOCKS`]. A value of `1` turns the cache off: one host call per engine read.
    pub read_block_bytes: usize,
    /// The most memory the merge's sorted passes may hold. It changes what the merge holds, never
    /// what it writes.
    pub merge_budget_bytes: usize,
}

impl Default for BridgeOptions {
    fn default() -> Self {
        let d = Options::default();
        BridgeOptions {
            accept_holes: d.accept_holes,
            accept_partial: d.accept_partial,
            read_block_bytes: DEFAULT_READ_BLOCK,
            merge_budget_bytes: d.merge_budget_bytes,
        }
    }
}

impl BridgeOptions {
    /// Parse the options object the browser hands in: every field optional, defaults from
    /// [`Options::default`]. An unknown key is ignored, so a newer builder can talk to an older
    /// module.
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
                "acceptHoles" => o.accept_holes = value.as_bool().ok_or("options.acceptHoles must be a boolean")?,
                "acceptPartial" => {
                    o.accept_partial = value.as_bool().ok_or("options.acceptPartial must be a boolean")?
                }
                "readBlockBytes" => {
                    let n = value.as_u64().ok_or("options.readBlockBytes must be a number")?;
                    // Clamped rather than refused: it is a performance knob, so an absurd value
                    // must assemble the same map a little slower.
                    o.read_block_bytes = (n as usize).clamp(MIN_READ_BLOCK, MAX_READ_BLOCK);
                }
                // Clamped: a budget of zero is not a smaller merge, it is one that cannot make
                // progress.
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

/// Which stage of the assembly is running. The string form ([`Phase::as_str`]) is the wire contract
/// with the browser wrapper, so renaming one breaks the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Opening every cell through the real reader and checking the input preconditions.
    Open,
    /// Merging, deduplicating and re-binning the POI section.
    Poi,
    /// The nav rewrite, which is the memory peak.
    Nav,
    /// Laying the map out: every region's offset computed before a byte is written.
    Plan,
    /// Writing the file, where the geometry is grafted verbatim and the raster is spliced into the
    /// tail.
    Write,
    /// Reading the sealed map back through the real reader.
    Verify,
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
            Phase::Done => "done",
        }
    }

    /// This phase's share of the wall clock. Measured on a region-scale and a corridor-scale run,
    /// which agree to half a point on every phase, so one table serves both.
    const fn weight(self) -> f64 {
        match self {
            Phase::Open => 0.001,
            Phase::Poi => 0.001,
            Phase::Nav => 0.200,
            Phase::Plan => 0.001,
            Phase::Write => 0.363,
            Phase::Verify => 0.434,
            Phase::Done => 0.0,
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

/// Emit progress when the overall fraction has advanced by at least this much: about a hundred
/// callbacks over an assembly, plus one per phase boundary. It is also the abort poll interval,
/// because only a callback can set the flag.
const PROGRESS_STEP: f64 = 0.01;

/// Bytes fetched on a cache miss by default. Sequential reads reuse the block; scattered record
/// reads amplify traffic. Reads at least this large bypass the cache.
pub(crate) const DEFAULT_READ_BLOCK: usize = 4 * 1024;
/// Sealed-output verification has its own access pattern and cache geometry.
pub(crate) const VERIFY_READ_BLOCK: usize = 64 * 1024;
/// The floor a caller can ask for. `1` is not a small cache but no cache: every read takes the
/// bypass and becomes exactly one host call.
const MIN_READ_BLOCK: usize = 1;
/// The ceiling, so a mistyped option cannot reserve a quarter of the heap for read scratch.
const MAX_READ_BLOCK: usize = 4 * 1024 * 1024;
/// Fixed cache slot count shared by all sources. With the block size it bounds cache residency and
/// the linear hit/eviction scan. `estimate.rs` uses these constants to account for the input cache.
pub(crate) const READ_CACHE_BLOCKS: usize = 16;

/// The floor for [`BridgeOptions::merge_budget_bytes`]. Below it the merge produces the same map and
/// spends all its time in the k-way merge, so a mistyped option is slow rather than wrong.
const MIN_MERGE_BUDGET: usize = 64 * 1024;

/// The map's slot in its own read-back cache. There is one file, so one slot; the number exists
/// because [`BlockCache`] is shared with the input seam, where a slot names a cell.
const MAP_SLOT: usize = 0;

/// One resident block of one source.
struct CachedBlock {
    /// Which source. `slot == usize::MAX` marks a slot that was never filled, which no real source
    /// can collide with.
    slot: usize,
    /// Which block of it. File-relative, so `u64`: a large file has more blocks than a wasm32
    /// `usize` holds.
    index: u64,
    /// The block's bytes. Exactly `len` of them are valid, because the last block of a source is
    /// short.
    data: Vec<u8>,
    len: usize,
    /// The clock reading at the last hit, for the LRU eviction.
    used: u64,
}

/// One slotted, offset-addressed byte source on the host's side of the boundary. Both [`CellReads`]
/// and the read-back half of [`MapWrites`] have this shape, so one [`BlockCache`] serves either.
trait SlotReads {
    fn read_slot(&self, slot: usize, offset: u64, buf: &mut [u8]) -> Result<(), String>;
}

/// The input cells, as a cache reads them.
struct CellSlots<'r>(&'r dyn CellReads);

impl SlotReads for CellSlots<'_> {
    fn read_slot(&self, slot: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        self.0.read(slot, offset, buf)
    }
}

/// The sealed map, read back through the same handle it was written through. One file, so the slot
/// is always [`MAP_SLOT`] and is not passed on.
struct MapSlot<'w>(&'w dyn MapWrites);

impl SlotReads for MapSlot<'_> {
    fn read_slot(&self, _slot: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        self.0.read_at(offset, buf)
    }
}

/// The bridge between a [`SlotReads`] host and the engine's [`ByteSource`] reads: a fixed-size LRU
/// of blocks shared by every slot, so residency is a constant and not a per-source cost.
///
/// A read at least one block long bypasses it. The verbatim geometry copy is 256 KiB a time and
/// would otherwise evict the cache for bytes nothing reads twice.
///
/// [`ByteSource`]'s error type carries no message, so the host's own message is kept here and
/// [`assemble`] prefers it over the engine's. A cell that could not be read is [`ErrorCode::Io`]
/// with the cell's id in it, never a verify defect.
struct BlockCache<'r> {
    reads: &'r dyn SlotReads,
    block: usize,
    /// Per slot, how a message names that source. Set once, before the run, so the read path never
    /// formats.
    labels: Vec<String>,
    /// What a message calls a slot no label was set for. Only reachable through a defect.
    unnamed: &'static str,
    slots: RefCell<Vec<CachedBlock>>,
    clock: StdCell<u64>,
    /// The first host failure, kept for [`map_error`]. First rather than last, because everything
    /// after it is the engine unwinding.
    failure: RefCell<Option<String>>,
}

impl<'r> BlockCache<'r> {
    fn new(reads: &'r dyn SlotReads, block: usize, labels: Vec<String>, unnamed: &'static str) -> BlockCache<'r> {
        BlockCache {
            reads,
            block,
            labels,
            unnamed,
            slots: RefCell::new(Vec::new()),
            clock: StdCell::new(0),
            failure: RefCell::new(None),
        }
    }

    /// Record the host's own message and answer the engine in the only vocabulary the seam has.
    fn fail(&self, slot: usize, at: u64, message: String) -> obc_formats::io::Error {
        let named = self.labels.get(slot).filter(|l| !l.is_empty());
        let mut failure = self.failure.borrow_mut();
        if failure.is_none() {
            let what = named.map_or(self.unnamed, |l| l.as_str());
            *failure = Some(format!("{what} could not be read at byte {at}: {message}"));
        }
        obc_formats::io::Error::Io
    }

    /// One host read, straight into `buf`.
    ///
    /// Offsets stay `u64`: a sunk map is a host file that can be larger than this address space.
    /// The buffer stays `usize`-bounded, because it lives in wasm memory.
    fn fetch(&self, slot: usize, at: u64, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        self.reads.read_slot(slot, at, buf).map_err(|e| self.fail(slot, at, e))
    }

    /// The index of the cache slot holding block `index` of `slot`, filling it if it is not there.
    fn block_of(&self, slot: usize, index: u64, source_len: u64) -> Result<usize, obc_formats::io::Error> {
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
        let start = index * self.block as u64;
        let len = (self.block as u64).min(source_len.saturating_sub(start)) as usize;
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

    /// [`ByteSource::read_at`] for one source: whole blocks through the LRU, big reads around it.
    fn read_at(&self, slot: usize, offset: u64, source_len: u64, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        let end = offset.checked_add(buf.len() as u64).ok_or(obc_formats::io::Error::BadOffset)?;
        // Checked here rather than left to the host: a read past a source's declared end is a defect
        // in the engine or a wrong length in the catalog, and it must read as one.
        if end > source_len {
            return Err(obc_formats::io::Error::BadOffset);
        }
        if buf.is_empty() {
            return Ok(());
        }
        if buf.len() >= self.block {
            return self.fetch(slot, offset, buf);
        }
        let mut done = 0usize;
        while done < buf.len() {
            let cursor = offset + done as u64;
            let index = cursor / self.block as u64;
            let k = self.block_of(slot, index, source_len)?;
            let slots = self.slots.borrow();
            let b = &slots[k];
            // Inside one block, so the narrowing is against `self.block` and not against the file.
            let within = (cursor - index * self.block as u64) as usize;
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

/// The reader an assembly with no [`SourceCell`] inputs, or no [`MapWrites`] sink, is given, so
/// neither cache needs an `Option`. It is unreachable: [`assemble`] refuses a keyed source with no
/// reader, and a sunk map with no sink cannot be built.
struct NoReads;

impl SlotReads for NoReads {
    fn read_slot(&self, slot: usize, _offset: u64, _buf: &mut [u8]) -> Result<(), String> {
        Err(format!("slot {slot} has no reader — this assembly was wired without one"))
    }
}

/// One [`SourceCell`] as the engine reads it: a slot number, the catalog's length, and the shared
/// cache. No bytes.
struct KeyedSource<'a, 'r> {
    slot: usize,
    len: u64,
    cache: &'a BlockCache<'r>,
}

impl ByteSource for KeyedSource<'_, '_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        self.cache.read_at(self.slot, offset, self.len, buf)
    }

    fn len(&self) -> u64 {
        self.len
    }
}

/// The host's side of the two seams: a clock the engine can read, and a progress sink that doubles
/// as the abort signal.
pub trait Hooks {
    /// Monotonic microseconds. `0` is a legal implementation and only costs the reported phase
    /// split.
    fn now_us(&mut self) -> u64;
    /// Report `phase` with the overall completed `fraction` (0.0..=1.0). Return `true` to abort;
    /// see the module header for the granularity that request is honoured at.
    fn progress(&mut self, phase: Phase, fraction: f64) -> bool;

    /// The map the host's own [`MapWrites`] sink wrote has passed its read-back.
    ///
    /// Called once, after the engine returns, and only when a sink was wired. Without a sink the
    /// bytes are in [`Outcome::bytes`] and there is nothing to report.
    ///
    /// `Err(message)` fails the run as [`ErrorCode::Io`]: a map whose identity the caller could not
    /// record must not be reported as finished, because the digest is the only thing that says which
    /// bytes are in the file.
    fn map_sealed(&mut self, map: SealedMap) -> Result<(), String> {
        let _ = map;
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
/// unchanged. A caller that special-cases a cause branches on `code`; everyone else shows
/// `message`.
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

/// Why an assembly failed. The engine's refusal classes stay apart because a caller answers each
/// one differently: a verify failure is a defect, an input refusal is a selection to fix, and a
/// capacity refusal is coverage to reduce.
///
/// The string form ([`ErrorCode::as_str`]) is the wire contract with the browser wrapper. It lands
/// on the thrown JS `Error` as `.code`, so renaming one breaks the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Mixed schemas, an unaccepted hole, an unaccepted partial cell, a skin from another schema
    /// revision, a band the schema does not have. The selection is wrong; the cells are fine.
    Input,
    /// A cell does not honour the format or the cell contract. The download is corrupt or the
    /// catalog is serving something that is not a cell.
    Format,
    /// A ceiling: the file ceiling, the `HoursRef` pool, or the `uint32` index space. Coverage must
    /// be reduced; this bridge never drops any itself.
    Capacity,
    /// The verify pass rejected the output: the engine wrote a map the real reader cannot read. A
    /// defect in the assembler, and the one code a caller must never retry past.
    Verify,
    /// The byte source or sink failed.
    Io,
    /// The caller's own [`Hooks::progress`] asked to stop.
    Aborted,
    /// A defect in this bridge: an unparseable cell id, a schema or skin document that is not JSON,
    /// or a module that failed to load.
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

/// What an assembly produced: one map file, the engine's warnings, and the same summary document
/// `obcm-assemble --json` prints.
///
/// The file has no name here. Naming is the host's job, so what crosses is an identity and, when the
/// run buffered it, the bytes.
pub struct Outcome {
    /// Lowercase-hex SHA-256 of the whole file, the raster included.
    pub sha256: String,
    /// The file's length. `u64` because a sunk map may be larger than this address space.
    pub byte_length: u64,
    /// The bytes, when the run held them. `None` when a [`MapWrites`] sink wrote them and the host
    /// already has the file.
    pub bytes: Option<Vec<u8>>,
    /// What a producer reports rather than refuses: the size warning, dropped duplicate POIs,
    /// degree-cap truncations. A caller that ignores this ships the same bytes.
    pub warnings: Vec<String>,
    /// The summary as JSON, in the shape `obcm-assemble --json` prints (minus the CLI's output
    /// path, which does not exist here).
    pub summary_json: String,
}

/// Hand-written so a failing assertion prints the map's shape and not a megabyte of hex.
impl core::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Outcome")
            .field("sha256", &self.sha256)
            .field("byte_length", &self.byte_length)
            .field("resident", &self.bytes.is_some())
            .field("warnings", &self.warnings)
            .field("summary_json_len", &self.summary_json.len())
            .finish()
    }
}

/// Shared state behind the two seams. Both the clock and the store hold `&RefCell<Progress>`,
/// because the engine takes the store by `&mut` and the clock by `&`.
struct Progress<'h> {
    hooks: &'h mut dyn Hooks,
    phase: Phase,
    /// How many times the engine has read the clock.
    ticks: u32,
    /// Bytes handed to [`MapStore::write`] so far.
    written: u64,
    /// Bytes the read-back has pulled through [`VerifySource::read_at`] so far.
    verified: u64,
    /// Projected output size: the sum of the input cells' bytes. The map comes out about the size of
    /// the cells that went in, because geometry is copied verbatim and the nav section is rewritten
    /// to about the size the cells' own nav sections had.
    projected: u64,
    /// Overall fraction at the last emitted callback, so [`PROGRESS_STEP`] can throttle.
    last: f64,
    /// A [`Hooks::progress`] call asked to stop. Surfaced at the next store call.
    aborted: bool,
    /// The sink path's own failure. The engine's store contract has one refusal, `Error::Io`, which
    /// says nothing about why, so the real one is kept here and [`map_error`] prefers it.
    failure: Option<AssembleFailure>,
}

impl Progress<'_> {
    /// Emit `(phase, fraction)` unless it is within [`PROGRESS_STEP`] of the last one, and record an
    /// abort request. A phase change always emits.
    ///
    /// The reported fraction never goes below the last one. A bar that goes backwards is the one
    /// thing a caller may never be shown, so it is enforced here and not reasoned about.
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

    /// The write/verify span's bar. Two terms over one span, because a write counter alone stops
    /// moving the moment the verify pass starts, and that pass is two fifths of the run.
    ///
    /// Both terms use the same denominator, the input cells' byte count, because output is about the
    /// same size as input and the verify pass reads the output back in full. Each term is a growing
    /// counter over a constant, so both are monotone, and both are clamped: a projection that is a
    /// little wrong costs bar accuracy near the end of a phase and nothing else.
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

/// The clock the engine reads. Its call sequence is the phase seam; the value it returns is what
/// the summary's phase split is computed from.
struct HookedClock<'a, 'h> {
    p: &'a RefCell<Progress<'h>>,
}

impl Clock for HookedClock<'_, '_> {
    fn now_us(&self) -> u64 {
        let mut p = self.p.borrow_mut();
        p.ticks += 1;
        // Ticks 1..=5 each start the next phase. From tick 6 the engine is inside the write and
        // verify passes, where the store's own calls say which of the two runs.
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

/// Where the map's bytes are.
///
/// Both are the same file to the engine and to the format. Only whether this address space holds it
/// differs, so one enum serves and only `read_at` and `len` branch.
enum MapBody<'a> {
    /// In wasm memory: no sink, or a native caller that wants the bytes.
    Buffered(Vec<u8>),
    /// In the host's own storage: how many bytes were handed over, and the cache the read-back pulls
    /// them back through.
    Sunk { len: u64, cache: &'a BlockCache<'a> },
}

impl MapBody<'_> {
    /// The map's length. A buffered map is bounded by this address space; a sunk one is a host file
    /// that can be larger than it.
    fn len(&self) -> u64 {
        match self {
            MapBody::Buffered(bytes) => bytes.len() as u64,
            MapBody::Sunk { len, .. } => *len,
        }
    }
}

/// The sealed map as the verify pass reads it back: the bytes, wherever they are, plus the progress
/// and abort seam inside the read-back loop.
///
/// This crate keeps its own store instead of [`obcm_assemble::MemoryStore`] for those two lines.
/// [`MapStore::source`] hands the engine a `&dyn ByteSource` and the whole pass happens behind it,
/// so `read_at` is the only place a browser can learn that verify is moving, or stop it.
///
/// With a sink the read-back is a file read, so the pass re-reads the bytes that were written and
/// not the ones this process still holds.
struct VerifySource<'a, 'h> {
    body: MapBody<'a>,
    p: &'a RefCell<Progress<'h>>,
}

impl ByteSource for VerifySource<'_, '_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        {
            let mut p = self.p.borrow_mut();
            p.verified += buf.len() as u64;
            let at = p.write_verify_fraction();
            // Throttled by `emit`, so the callback fires about a hundred times over the pass
            // however many reads the reader makes.
            p.emit(Phase::Verify, at);
            if p.aborted {
                // The read-back's own error channel is all there is here. `verify_map` turns it
                // into `Error::Verify`, which is why `map_error` reads the abort flag first.
                // Checked before the read is issued, so a cancel costs at most one host call.
                return Err(obc_formats::io::Error::Io);
            }
        }
        match &self.body {
            MapBody::Buffered(bytes) => SliceSource(bytes).read_at(offset, buf),
            MapBody::Sunk { len, cache } => cache.read_at(MAP_SLOT, offset, *len, buf),
        }
    }

    fn len(&self) -> u64 {
        self.body.len()
    }
}

/// The [`MapStore`] the browser assembles through, plus the progress and abort seam.
///
/// A sealed map must be randomly addressable, because it is read back through the real reader before
/// it is handed on. It can be addressable from wasm memory or from the host's own storage through a
/// [`MapWrites`] sink. A country-scale map needs the second: it is one file a browser cannot hold.
struct HookedStore<'a, 'h> {
    src: VerifySource<'a, 'h>,
    /// Fed by every [`MapStore::write`] and finalized at [`MapStore::seal`], but only with a sink:
    /// a second SHA-256 pass over a gigabyte-scale file costs real time, and a buffered caller has
    /// the bytes anyway.
    hasher: Sha256,
    /// Lowercase hex, once sealed. Empty for a run with no sink.
    sha256: String,
    /// Where the map's bytes go, when they do not go into this address space. `None` is the
    /// buffered store.
    sink: Option<&'a dyn MapWrites>,
    /// How the verify pass reads them back. Present whether or not `sink` is, so the borrow graph
    /// needs no `Option`; unreachable without one, because only a `Sunk` body names it.
    cache: &'a BlockCache<'a>,
    p: &'a RefCell<Progress<'h>>,
}

/// Park a sink failure where [`map_error`] will find it, and answer the engine in the only
/// vocabulary its store contract has.
///
/// The `Error::Io` this returns says nothing about why, so the host's own sentence is kept here and
/// preferred over the engine's when the run unwinds.
fn sink_failed(p: &RefCell<Progress<'_>>, what: &str, message: String) -> Error {
    let mut p = p.borrow_mut();
    if p.failure.is_none() {
        p.failure = Some(AssembleFailure::new(ErrorCode::Io, format!("the map could not be {what}: {message}")));
    }
    Error::Io(obc_formats::io::Error::Io)
}

impl HookedStore<'_, '_> {
    /// The abort check every store entry point runs first. `Error::Io` is the only refusal the
    /// engine's store contract has; [`assemble`] turns it back into [`ErrorCode::Aborted`].
    fn check_abort(&self) -> obcm_assemble::Result<()> {
        if self.p.borrow().aborted {
            return Err(Error::Io(obc_formats::io::Error::Io));
        }
        Ok(())
    }
}

impl MapStore for HookedStore<'_, '_> {
    fn begin(&mut self) -> obcm_assemble::Result<()> {
        self.check_abort()?;
        {
            let mut p = self.p.borrow_mut();
            let at = p.write_verify_fraction();
            p.emit(Phase::Write, at);
        }
        match self.sink {
            // The host takes the bytes. Nothing map-sized is reserved here: the plan may be a
            // 9 GiB file.
            Some(sink) => {
                sink.create().map_err(|e| sink_failed(self.p, "created", e))?;
                self.src.body = MapBody::Sunk { len: 0, cache: self.cache };
            }
            None => {
                let mut bytes = Vec::new();
                // The map comes out about the size of the cells that went in, so reserving the
                // projection gets the buffer in one allocation. A doubling ladder would instead
                // hold 1.5x a gigabyte-scale file at its last step, which a tab may not have
                // contiguously. `try_` because a refusal is recoverable: the write path then grows
                // the vector itself, where `reserve_exact` would abort the whole module.
                let _ = bytes.try_reserve_exact(usize::try_from(self.p.borrow().projected).unwrap_or(0));
                self.src.body = MapBody::Buffered(bytes);
            }
        }
        Ok(())
    }

    fn write(&mut self, buf: &[u8]) -> obcm_assemble::Result<()> {
        {
            let mut p = self.p.borrow_mut();
            p.written += buf.len() as u64;
            let at = p.write_verify_fraction();
            p.emit(Phase::Write, at);
        }
        // Checked after accounting, so the callback that observes the abort also reports where it
        // got to, and before the sink is touched, so a cancel costs at most the write in flight.
        self.check_abort()?;
        // The engine keeps its own digest to itself until the run ends. Hashing here makes the
        // end-of-run comparison a real check that the bytes the host saved are the bytes the engine
        // wrote.
        if self.sink.is_some() {
            self.hasher.update(buf);
        }
        match &mut self.src.body {
            MapBody::Buffered(bytes) => bytes.extend_from_slice(buf),
            MapBody::Sunk { len, .. } => {
                let sink = self.sink.expect("a sunk map has a sink");
                sink.write(buf).map_err(|e| sink_failed(self.p, "written", e))?;
                *len += buf.len() as u64;
            }
        }
        Ok(())
    }

    fn seal(&mut self) -> obcm_assemble::Result<()> {
        if let Some(sink) = self.sink {
            let digest = core::mem::take(&mut self.hasher).finalize();
            self.sha256 = digest.iter().map(|b| format!("{b:02x}")).collect();
            // Before the abort check: the very next thing the engine does is read this file back,
            // so a host that buffers must have flushed by now.
            sink.seal().map_err(|e| sink_failed(self.p, "sealed", e))?;
        }
        self.check_abort()
    }

    fn source(&self) -> obcm_assemble::Result<&dyn ByteSource> {
        // The engine asks for the sealed map only for the verify pass. This is the phase boundary;
        // the reads that follow report the pass's own progress.
        {
            let mut p = self.p.borrow_mut();
            let at = p.write_verify_fraction();
            p.emit(Phase::Verify, at);
        }
        self.check_abort()?;
        Ok(&self.src as &dyn ByteSource)
    }
}

/// Assemble `cells` into one `.obcm`, reporting through `hooks`.
///
/// Byte-for-byte the same output as the native CLI on the same inputs: this function contributes no
/// format knowledge, only the buffer adapter and the seams above.
pub fn assemble_cells(
    cells: Vec<CellBytes>,
    schema_json: &str,
    light_skin_json: &str,
    dark_skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    assemble_cells_with_known_empty(cells, Vec::new(), schema_json, light_skin_json, dark_skin_json, opts, hooks)
}

/// Assemble downloaded artifacts while retaining selected canonical-empty
/// cells in coverage and bbox arithmetic.
pub fn assemble_cells_with_known_empty(
    cells: Vec<CellBytes>,
    known_empty: Vec<KnownEmptyCell>,
    schema_json: &str,
    light_skin_json: &str,
    dark_skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    assemble_everything(cells, known_empty, None, Vec::new(), schema_json, light_skin_json, dark_skin_json, opts, hooks)
}

/// The full assembly, raster included.
///
/// `terrain` is the catalog's lattice. `None` means the catalog publishes no terrain block, or the
/// selection covers no terrain object, and the map is written with an empty raster region: an
/// ordinary map whose profiles are flat.
///
/// This is [`assemble`] with every cell's bytes in hand. A caller whose cells or map live outside
/// wasm memory builds a [`Wiring`] instead.
// One assembly is exactly these eight things; a struct would restate the signature.
#[allow(clippy::too_many_arguments)]
pub fn assemble_everything(
    cells: Vec<CellBytes>,
    known_empty: Vec<KnownEmptyCell>,
    terrain: Option<TerrainLattice>,
    terrain_cells: Vec<TerrainCellBytes>,
    schema_json: &str,
    light_skin_json: &str,
    dark_skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    assemble(
        Wiring { cells, known_empty, terrain, terrain_cells, ..Wiring::default() },
        schema_json,
        light_skin_json,
        dark_skin_json,
        opts,
        hooks,
    )
}

/// Everything one assembly is wired to: the cells in whichever of the two forms the host has them,
/// the selection's canonical-empty coverage, the raster, and where the map goes.
///
/// The two cell lists are alternatives, but both may be present, and the engine sees one list: the
/// buffered cells first, then the source-backed ones. Order does not reach the output, because the
/// engine renumbers by content and lays the file out by ladder level.
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
    /// Where the map is written. `None` keeps it in wasm memory, which is what a native caller
    /// wants and what the browser falls back to. With a sink the file is never resident and
    /// [`Outcome::bytes`] is `None`: its identity is reported to [`Hooks::map_sealed`].
    pub sink: Option<&'r dyn MapWrites>,
    /// Where the engine's spill goes. `None` keeps it in wasm memory ([`MemoryScratch`]), which is
    /// the browser's fallback when OPFS is not usable, at exactly the residency the spill exists to
    /// remove. A browser that can serve sync reads should always wire this.
    pub scratch: Option<&'r dyn ScratchWrites>,
}

/// Assemble one selection into a `.obcm`, reporting through `hooks`.
pub fn assemble(
    wiring: Wiring<'_>,
    schema_json: &str,
    light_skin_json: &str,
    dark_skin_json: &str,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<Outcome, AssembleFailure> {
    let Wiring { cells, source_cells, reads, known_empty, terrain, terrain_cells, sink, scratch } = wiring;
    if cells.is_empty() && source_cells.is_empty() {
        return Err(AssembleFailure::new(
            ErrorCode::Input,
            "No OBCM cell artifact was handed to the assembler. An assembly needs at least one artifact to verify \
             the schema revision's binary style and routing-profile tables (OBCA §3.8).",
        ));
    }
    // A key with no way to resolve it is a host that wired half of the streamed path. Refused up
    // front, because at the first read it would look like a storage failure.
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
    let map_styles =
        MapStyles::parse(light_skin_json, dark_skin_json).map_err(|e| AssembleFailure::new(ErrorCode::Internal, e))?;

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
    // A cache is built even with nothing to read through it, to keep the borrow graph below free of
    // an `Option`. `NoReads` is never reached: the loop that would call it has no iterations.
    let no_reads = NoReads;
    let cell_slots = reads.map(CellSlots);
    let cache = BlockCache::new(
        cell_slots.as_ref().map_or(&no_reads as &dyn SlotReads, |c| c as &dyn SlotReads),
        opts.read_block_bytes,
        labels,
        "an unknown cell",
    );
    // The same on the write side, for the read-back of a map the host wrote itself. One file, so
    // its label is known before the run.
    let map_slot = sink.map(MapSlot);
    let sink_cache = BlockCache::new(
        map_slot.as_ref().map_or(&no_reads as &dyn SlotReads, |s| s as &dyn SlotReads),
        VERIFY_READ_BLOCK,
        vec!["the map".to_string()],
        "the map",
    );
    let keyed: Vec<KeyedSource<'_, '_>> = keyed_ids
        .iter()
        .enumerate()
        .map(|(slot, (_, _, _, len))| KeyedSource { slot, len: u64::from(*len), cache: &cache })
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
        accept_holes: opts.accept_holes,
        accept_partial: opts.accept_partial,
        // Never. See `BridgeOptions`.
        skip_verify: false,
        merge_budget_bytes: opts.merge_budget_bytes,
    };

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
        // Replaced at `begin`; an assembly that never got that far has nothing to read back.
        src: VerifySource { body: MapBody::Buffered(Vec::new()), p: &progress },
        hasher: Sha256::new(),
        sha256: String::new(),
        sink,
        cache: &sink_cache,
        p: &progress,
    };
    let job = terrain.map(|lattice| TerrainJob {
        params: TerrainParams { posting_log2: lattice.posting_log2, cell_log2: lattice.cell_log2 },
        cells: terrain_ids
            .iter()
            .zip(&terrain_sources)
            .map(|((id, sha256), src)| TerrainCellInput { id: *id, src, sha256: *sha256 })
            .collect(),
    });
    // The host's own storage when it wired one, and wasm linear memory otherwise. Two bindings so
    // the trait object can borrow whichever exists.
    let host_scratch;
    let memory_scratch;
    let scratch: &dyn ScratchStore = match scratch {
        Some(host) => {
            host_scratch = HostScratch(host);
            &host_scratch
        }
        None => {
            memory_scratch = MemoryScratch::new();
            &memory_scratch
        }
    };
    let summary =
        match assemble_full(inputs, known_empty, job, &schema, &map_styles, &options, &mut store, &clock, scratch) {
            Ok(s) => s,
            Err(e) => {
                let p = progress.borrow();
                // A cell that could not be read is the root cause of whatever the engine reported:
                // `Cell::open` turns a failed read into "not a readable OBCM", which blames the catalog
                // for the browser's storage. The host's own message wins. The map read-back matters
                // more, because the verify pass reports every read failure as a defect, so a full disk
                // would otherwise tell a rider the assembler is broken.
                let read_failure = cache
                    .failure
                    .borrow()
                    .clone()
                    .or_else(|| sink_cache.failure.borrow().clone())
                    .map(|message| AssembleFailure::new(ErrorCode::Io, message));
                return Err(map_error(e, p.aborted, read_failure.or_else(|| p.failure.clone())));
            }
        };

    let sha256: String = summary.sha256.iter().map(|b| format!("{b:02x}")).collect();
    let bytes = match core::mem::replace(&mut store.src.body, MapBody::Buffered(Vec::new())) {
        MapBody::Buffered(bytes) => Some(bytes),
        MapBody::Sunk { .. } => None,
    };
    // The host saved these bytes without seeing them, so this equality is the only thing between a
    // mislabelled file and a card. It runs before the caller is told anything.
    if sink.is_some() {
        check_sealed_identity(&store.sha256, &sha256)?;
        let sealed = SealedMap { sha256: sha256.clone(), byte_length: summary.bytes };
        if let Err(message) = progress.borrow_mut().hooks.map_sealed(sealed) {
            return Err(AssembleFailure::new(ErrorCode::Io, message));
        }
    }
    {
        let mut p = progress.borrow_mut();
        p.emit(Phase::Done, 1.0);
    }

    let summary_json = summary_json(&summary);
    Ok(Outcome { sha256, byte_length: summary.bytes, bytes, warnings: summary.warnings, summary_json })
}

/// What this store hashed on the way into the sink, against what the engine says it wrote.
///
/// The two digests come from the same bytes by different paths, and a caller records this digest
/// against a file it can no longer inspect. A failure is [`ErrorCode::Internal`], because the only
/// way to reach it is a defect here.
fn check_sealed_identity(got: &str, want: &str) -> Result<(), AssembleFailure> {
    if got == want {
        return Ok(());
    }
    Err(AssembleFailure::new(
        ErrorCode::Internal,
        format!(
            "the map was written through the sink as {got} but the engine wrote {want} — this bridge's own digest is \
             wrong, and the host's file cannot be identified from it."
        ),
    ))
}

/// A lowercase-hex SHA-256 as the 32 bytes the engine compares against. A malformed one is this
/// bridge's problem: the catalog parser rejects a bad digest before the download starts, so reaching
/// here means the caller built the string wrongly.
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
/// is exhaustive on purpose: a new [`Error`] variant must break this build.
///
/// `aborted` is read first, before the error's own class, because the abort path raises whatever
/// error channel it was standing in. A [`VerifySource::read_at`] refusal arrives as `Error::Verify`,
/// and a cancelled run must never be reported as a verify defect.
///
/// `failure` is the sink path's own, raised through the same `Error::Io` channel. It is read next.
/// It cannot coexist with an abort, because every store entry point checks the abort flag before it
/// touches the sink.
fn map_error(e: Error, aborted: bool, failure: Option<AssembleFailure>) -> AssembleFailure {
    if aborted {
        return AssembleFailure::new(
            ErrorCode::Aborted,
            "The assembly was cancelled. Whatever reached storage is a partial file, not a map — a map is only a map \
             once §4.8 has read the whole of it back — so discard it and re-run.",
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
        // The scratch seam is storage like any other, so a caller sees an `io` problem and the
        // engine's message says which one.
        Error::Scratch(_) => ErrorCode::Io,
    };
    AssembleFailure::new(code, message)
}

/// The summary, in the shape `obcm-assemble --json` prints. Restated here because the CLI's version
/// names an output path, which a browser does not have.
fn summary_json(s: &obcm_assemble::Summary) -> String {
    let st = &s.stats;
    serde_json::to_string(&serde_json::json!({
        "assembly_bbox_udeg": {
            "min_lat": s.assembly_box.min_lat,
            "min_lon": s.assembly_box.min_lon,
            "span_log2": s.assembly_box.span_log2,
        },
        "cells": st.cells,
        "bytes": s.bytes,
        "sha256": s.sha256.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "verified": s.verify.as_ref().map(|v| serde_json::json!({
            "chunks": v.chunks,
            "features": v.features,
            "nav_nodes": v.nav_nodes,
            "largest_component_permille": v.largest_component_permille,
        })),
        "terrain": s.terrain.as_ref().map(|t| serde_json::json!({
            "bytes": t.bytes,
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

    #[test]
    fn sequential_sources_trade_small_read_calls_for_bounded_cache_residency() {
        struct SequentialReads {
            calls: StdCell<usize>,
            bytes: StdCell<usize>,
        }
        impl SlotReads for SequentialReads {
            fn read_slot(&self, _slot: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
                self.calls.set(self.calls.get() + 1);
                self.bytes.set(self.bytes.get() + buf.len());
                for (i, byte) in buf.iter_mut().enumerate() {
                    *byte = (offset + i as u64) as u8;
                }
                Ok(())
            }
        }
        let source_len = 1024 * 1024;
        for read_len in [17, 256 * 1024] {
            let mut calls = Vec::new();
            for block in [DEFAULT_READ_BLOCK, 64 * 1024] {
                let source = SequentialReads { calls: StdCell::new(0), bytes: StdCell::new(0) };
                let cache = BlockCache::new(&source, block, vec!["sequential".into()], "source");
                let mut buf = vec![0; read_len];
                for offset in (0..source_len).step_by(read_len) {
                    let buf = &mut buf[..read_len.min(source_len - offset)];
                    cache.read_at(0, offset as u64, source_len as u64, buf).expect("sequential read");
                    assert!(buf.iter().enumerate().all(|(i, byte)| *byte == (offset + i) as u8));
                }
                assert_eq!(source.bytes.get(), source_len, "no sequential read amplification");
                assert!(cache.slots.borrow().iter().map(|s| s.data.len()).sum::<usize>() <= READ_CACHE_BLOCKS * block);
                calls.push(source.calls.get());
            }
            if read_len == 17 {
                assert_eq!(calls, [256, 16], "smaller windows need more sequential record calls");
            } else {
                assert_eq!(calls, [4, 4], "bulk copies bypass both cache sizes");
            }
        }
    }

    #[test]
    fn every_refusal_class_maps_to_its_own_code() {
        let cases = [
            (Error::Input("mixed schemas".into()), ErrorCode::Input, "input"),
            (Error::Format("not an OBCM".into()), ErrorCode::Format, "format"),
            (Error::Capacity("past the interior the scale addresses".into()), ErrorCode::Capacity, "capacity"),
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

    /// The engine's message is the only thing that says which cell, which band, which ceiling.
    #[test]
    fn the_engines_message_is_not_rewritten() {
        let m = "band \"network\" is missing cell 18/1204/1053, which covers 18/1204/1053";
        assert_eq!(map_error(Error::Input(m.into()), false, None).message, m);
        // `Verify` is the exception: its `Display` prefixes the message, as the CLI prints it.
        assert_eq!(map_error(Error::Verify("chunk 3".into()), false, None).message, "verify failed: chunk 3");
    }

    /// An abort raises whatever error channel it was standing in; the driver knows it was a
    /// cancellation and says so.
    #[test]
    fn an_abort_is_not_reported_as_an_io_failure_or_a_verify_defect() {
        let f = map_error(Error::Io(obc_formats::io::Error::Io), true, None);
        assert_eq!(f.code, ErrorCode::Aborted);
        assert!(f.message.contains("cancelled"), "{}", f.message);
        let v = map_error(Error::Verify("the output does not parse: Io".into()), true, None);
        assert_eq!(v.code, ErrorCode::Aborted, "a cancelled read-back is a cancellation, not a §4.8 defect");
        // Both classes with no abort pending still read as themselves.
        assert_eq!(map_error(Error::Io(obc_formats::io::Error::Io), false, None).code, ErrorCode::Io);
        assert_eq!(map_error(Error::Verify("chunk 3 does not decode".into()), false, None).code, ErrorCode::Verify);
    }

    /// The phase weights are a distribution over the run, or the bar goes backwards.
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
        assert_eq!(Phase::Done.prefix(), 1.0);
    }

    /// `bridge.ts` switches on the wire names.
    #[test]
    fn phase_names_are_distinct() {
        let all = [Phase::Open, Phase::Poi, Phase::Nav, Phase::Plan, Phase::Write, Phase::Verify, Phase::Done];
        let names: std::collections::HashSet<&str> = all.iter().map(|p| p.as_str()).collect();
        assert_eq!(names.len(), all.len());
    }

    #[test]
    fn options_parse_leniently_but_not_wrongly() {
        let d = BridgeOptions::parse("").expect("empty is the default");
        assert!(!d.accept_partial);
        assert_eq!(d.merge_budget_bytes, Options::default().merge_budget_bytes);

        let o = BridgeOptions::parse(r#"{"acceptPartial":true,"mergeBudgetBytes":1048576,"somethingNew":42}"#)
            .expect("unknown keys are ignored");
        assert_eq!((o.accept_partial, o.merge_budget_bytes), (true, 1048576));

        assert!(BridgeOptions::parse(r#"{"acceptPartial":"yes"}"#).is_err());
        assert!(BridgeOptions::parse("[1,2,3]").is_err());
        assert!(BridgeOptions::parse("not json").is_err());
    }

    /// `skip_verify` is not reachable from JS, whichever way it is spelled.
    #[test]
    fn there_is_no_way_to_ask_for_an_unverified_map() {
        let o = BridgeOptions::parse(r#"{"skipVerify":true,"skip_verify":true}"#).expect("ignored, not honoured");
        let debug = format!("{o:?}");
        assert!(!debug.contains("skip"), "{debug}");
    }

    #[test]
    fn an_empty_selection_is_an_input_refusal() {
        let e = assemble_cells(Vec::new(), "{}", "{}", "{}", &BridgeOptions::default(), &mut NoHooks)
            .expect_err("nothing to assemble");
        assert_eq!(e.code, ErrorCode::Input);
    }

    /// Known-empty coverage carries no binary tables, so it cannot make an artifact-free selection
    /// assembleable.
    #[test]
    fn an_all_known_empty_selection_is_an_input_refusal() {
        let empty = vec![KnownEmptyCell { id: "18/1204/1055".into(), band: "fine".into() }];
        let e = assemble_cells_with_known_empty(
            Vec::new(),
            empty,
            "{}",
            "{}",
            "{}",
            &BridgeOptions::default(),
            &mut NoHooks,
        )
        .expect_err("known-empty coverage cannot supply binary tables");
        assert_eq!(e.code, ErrorCode::Input);
        assert!(e.message.contains("at least one artifact"), "{}", e.message);
    }

    #[test]
    fn a_map_sunk_under_the_wrong_digest_is_an_internal_error() {
        let sha = "ab".repeat(32);
        let other = "cd".repeat(32);
        assert!(check_sealed_identity(&sha, &sha).is_ok());
        let e = check_sealed_identity(&sha, &other).expect_err("the digests differ");
        assert_eq!(e.code, ErrorCode::Internal);
        assert!(e.message.contains(&sha) && e.message.contains(&other), "{}", e.message);
    }

    #[test]
    fn a_malformed_cell_id_is_an_internal_error() {
        const SCHEMA: &str =
            r#"{"lods":[{"index":0}],"bands":[{"id":"fine","cell_log2":18,"lods":[0],"role":"core"}]}"#;
        const SKIN: &str = r#"{"marker_color":0,"styles":[]}"#;
        let cells = vec![CellBytes { id: "not-an-id".into(), band: "fine".into(), partial: false, bytes: vec![0; 8] }];
        let e = assemble_cells(cells, SCHEMA, SKIN, SKIN, &BridgeOptions::default(), &mut NoHooks).expect_err("bad id");
        assert_eq!(e.code, ErrorCode::Internal);
        assert!(e.message.contains("not-an-id"), "{}", e.message);
    }
}
