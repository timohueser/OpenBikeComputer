//! `obcm-assemble` — the native driver for the assembly engine.
//!
//! Everything here is I/O and argument parsing. The crate's rule is that the **engine** never
//! touches a filesystem (it has to run in a browser tab, #1024/P4), so this file owns every
//! `std::fs` call: it opens cell artifacts as [`ByteSource`]s, implements the [`MapStore`] over
//! real files, and prints what the engine reports.
//!
//! ```text
//! obcm-assemble --cells <cells.json> --skin <skin.json> --out <path.obcm> [options]
//! ```
//!
//! `cells.json` is the cutter's provenance sidecar (`obc-pack cut`), which already states every
//! cell's band, path and `partial` flag plus the schema they were baked at — so the common case
//! needs no second document. `--schema` overrides it (an OBCC v2 root or a bare `SchemaEntry`),
//! which is what a hosted catalog hands in.
//!
//! # Measuring the assembler's memory (`--features mem-profile`)
//!
//! The engine's peak heap is the thing a hosted (wasm) assembly is rationed on, and it is not
//! something a benchmark can guess: it is dominated by one phase (the nav rewrite) at a multiple of
//! the nav section it produces. The `mem-profile` feature — **off by default, native only** — wraps
//! the global allocator and the CLI's [`Clock`], so every phase boundary the engine ticks also
//! snapshots the peak since the previous one. It prints a table to **stderr**; the summary on stdout
//! (including `--json`) is byte-for-byte unchanged.
//!
//! Two commands, the second of which is the measurement:
//!
//! ```text
//! # 1. fetch a real region from the published catalog (resumable, stdlib python3)
//! python3 host/obcm-assemble/dev/fetch_region.py \
//!     europe/germany/baden-wuerttemberg/freiburg-regbez /tmp/obca/freiburg
//!
//! # 2. assemble it under the harness
//! cargo run --release -p obcm-assemble --features mem-profile -- \
//!     --cells /tmp/obca/freiburg/cells.json \
//!     --skin  /tmp/obca/freiburg/skin.json \
//!     --out   /tmp/obca/freiburg.obcm --accept-holes
//! ```

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use obc_formats::io::{ByteSource, Error as IoError};
use obcm_assemble::grid::CellId;
use obcm_assemble::schema::{Schema, Skin};
use obcm_assemble::{
    assemble_full, CellInput, Clock, Error, MapStore, Options, Result, ScratchId, ScratchStore, TerrainCellInput,
    TerrainJob, TerrainParams,
};

/// A cell artifact read on demand. Cell regions are copied in 256 KB blocks, so the whole tree never
/// has to be resident — which is what keeps a country assembly's memory about the nav graph rather
/// than about the geometry.
struct FileSource {
    file: RefCell<File>,
    len: u64,
}

impl FileSource {
    fn open(path: &Path) -> std::io::Result<FileSource> {
        let file = File::open(path)?;
        // No narrowing: the read seam is `u64`, so a file's length is simply its length. This used
        // to refuse anything past 4 GiB − 1, because a `uint32` offset could not name the bytes and
        // a truncating cast would have presented the low 32 bits as the whole file.
        let len = file.metadata()?.len();
        Ok(FileSource { file: RefCell::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::result::Result<(), IoError> {
        let mut f = self.file.borrow_mut();
        f.seek(SeekFrom::Start(offset)).map_err(|_| IoError::Io)?;
        f.read_exact(buf).map_err(|_| IoError::Io)
    }
    fn len(&self) -> u64 {
        self.len
    }
}

/// The map as one file at a path the caller named.
///
/// It used to be a directory of derived `MS<id>S<kk>.OBM` names plus an `MS<id>.OBS` manifest
/// written last, plus an orphan sweep for the shards a smaller re-assembly left behind. One file
/// needs none of that: the name is the caller's, and replacing a map is truncating it.
struct FileStore {
    path: PathBuf,
    open: Option<std::io::BufWriter<File>>,
    sealed: Option<FileSource>,
}

impl FileStore {
    fn new(path: &Path) -> Result<FileStore> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|_| Error::Io(IoError::Io))?;
        }
        Ok(FileStore { path: path.to_path_buf(), open: None, sealed: None })
    }
}

impl MapStore for FileStore {
    fn begin(&mut self) -> Result<()> {
        self.sealed = None;
        let file = File::create(&self.path).map_err(|_| Error::Io(IoError::Io))?;
        self.open = Some(std::io::BufWriter::new(file));
        Ok(())
    }
    fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.open.as_mut().expect("the map is open").write_all(buf).map_err(|_| Error::Io(IoError::Io))
    }
    fn seal(&mut self) -> Result<()> {
        let mut w = self.open.take().expect("the map is open");
        w.flush().map_err(|_| Error::Io(IoError::Io))?;
        drop(w);
        self.sealed = Some(FileSource::open(&self.path).map_err(|_| Error::Io(IoError::Io))?);
        Ok(())
    }
    fn source(&self) -> Result<&dyn ByteSource> {
        self.sealed.as_ref().map(|s| s as &dyn ByteSource).ok_or(Error::Io(IoError::BadOffset))
    }
}

/// The engine's spill area, as ordinary files under a directory of this run's own (#1116 D2).
///
/// Anonymous in the sense the seam means: the names are ordinals nothing outside this store knows,
/// the directory is created per process and per invocation, and it is removed whole when the store
/// drops — including after a failed assembly, which is the case a bare `remove_file` per file would
/// miss. A file is also removed as soon as the engine says it is done with it, so a country-scale
/// run's *live* scratch is one or two streams rather than all of them.
struct FileScratch {
    dir: PathBuf,
    /// Open handles by [`ScratchId`], with each one's length so an append never has to seek to find
    /// the end. `None` is a removed file, so an id is never reused.
    files: RefCell<Vec<Option<(File, u64)>>>,
}

impl FileScratch {
    fn new() -> std::result::Result<FileScratch, String> {
        // The pid keeps two concurrent assemblies apart, and every file is opened `truncate`, so a
        // directory left behind by a crashed run with a recycled pid is overwritten rather than read.
        let dir = std::env::temp_dir().join(format!("obcm-assemble-scratch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("create scratch dir {}: {e}", dir.display()))?;
        Ok(FileScratch { dir, files: RefCell::new(Vec::new()) })
    }

    /// Run `f` against the open handle, or refuse — never silently against the wrong file.
    fn with<T>(&self, id: ScratchId, f: impl FnOnce(&mut (File, u64)) -> Result<T>) -> Result<T> {
        let mut files = self.files.borrow_mut();
        match files.get_mut(id.0 as usize).and_then(Option::as_mut) {
            Some(entry) => f(entry),
            None => Err(Error::Scratch(format!("{id} is not open"))),
        }
    }
}

impl ScratchStore for FileScratch {
    fn create(&self) -> Result<ScratchId> {
        let mut files = self.files.borrow_mut();
        let id = ScratchId(u32::try_from(files.len()).map_err(|_| Error::Scratch("too many scratch files".into()))?);
        let path = self.dir.join(format!("{}.spill", id.0));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| Error::Scratch(format!("create {}: {e}", path.display())))?;
        files.push(Some((file, 0)));
        Ok(id)
    }

    fn append(&self, id: ScratchId, buf: &[u8]) -> Result<()> {
        self.with(id, |(file, len)| {
            file.seek(SeekFrom::Start(*len)).map_err(|e| Error::Scratch(format!("{id}: seek: {e}")))?;
            file.write_all(buf).map_err(|e| Error::Scratch(format!("{id}: write: {e}")))?;
            *len += buf.len() as u64;
            Ok(())
        })
    }

    fn read_at(&self, id: ScratchId, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.with(id, |(file, len)| {
            let end = offset.saturating_add(buf.len() as u64);
            if end > *len {
                return Err(Error::Scratch(format!(
                    "{id}: a read of {} byte(s) at {offset} runs past the {len}-byte end",
                    buf.len()
                )));
            }
            file.seek(SeekFrom::Start(offset)).map_err(|e| Error::Scratch(format!("{id}: seek: {e}")))?;
            file.read_exact(buf).map_err(|e| Error::Scratch(format!("{id}: read: {e}")))
        })
    }

    fn len(&self, id: ScratchId) -> Result<u64> {
        self.with(id, |(_, len)| Ok(*len))
    }

    fn remove(&self, id: ScratchId) -> Result<()> {
        let mut files = self.files.borrow_mut();
        match files.get_mut(id.0 as usize) {
            Some(slot) => {
                *slot = None; // closes the handle
                let _ = std::fs::remove_file(self.dir.join(format!("{}.spill", id.0)));
                Ok(())
            }
            None => Err(Error::Scratch(format!("{id} is not open"))),
        }
    }
}

impl Drop for FileScratch {
    fn drop(&mut self) {
        self.files.borrow_mut().clear();
        // Best effort: the run is over either way, and a temp directory that could not be removed is
        // not a reason to turn a written set into a failure.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The terrain sidecar the CLI is driven by — the catalog's §13.1 lattice plus the downloaded
/// cells, in the same shape `cells.json` states the OBCM ones.
struct TerrainSidecar {
    params: TerrainParams,
    cells: Vec<(CellId, String, Option<[u8; 32]>)>,
}

fn parse_terrain_sidecar(text: &str) -> std::result::Result<TerrainSidecar, String> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("terrain.json: {e}"))?;
    let byte = |key: &str| -> std::result::Result<u8, String> {
        doc.get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| u8::try_from(v).ok())
            .ok_or_else(|| format!("terrain.json has no `{key}`"))
    };
    let params = TerrainParams { posting_log2: byte("posting_log2")?, cell_log2: byte("cell_log2")? };
    let listed = doc.get("cells").and_then(|c| c.as_array()).ok_or("terrain.json has no `cells` array")?;
    let mut cells = Vec::with_capacity(listed.len());
    for c in listed {
        let id = c.get("id").and_then(|v| v.as_str()).ok_or("a terrain cell has no `id`")?;
        let path = c.get("path").and_then(|v| v.as_str()).ok_or("a terrain cell has no `path`")?;
        let sha256 = match c.get("sha256").and_then(|v| v.as_str()) {
            None => None,
            Some(hex) => {
                let raw: std::result::Result<Vec<u8>, String> = (0..hex.len())
                    .step_by(2)
                    .map(|k| u8::from_str_radix(&hex[k..k + 2], 16).map_err(|_| format!("bad sha256 {hex:?}")))
                    .collect();
                let raw = raw?;
                Some(<[u8; 32]>::try_from(raw.as_slice()).map_err(|_| format!("sha256 {hex:?} is not 32 bytes"))?)
            }
        };
        cells.push((CellId::parse(id)?, path.to_string(), sha256));
    }
    Ok(TerrainSidecar { params, cells })
}

/// Wall-clock microseconds, for the phase split the engine reports.
struct StdClock(Instant);

impl Clock for StdClock {
    fn now_us(&self) -> u64 {
        self.0.elapsed().as_micros() as u64
    }
}

/// The peak-allocation harness (`--features mem-profile`). See this file's module docs for the
/// two-command workflow; everything here is compiled out by default.
#[cfg(feature = "mem-profile")]
mod mem_profile {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use obcm_assemble::{Clock, Summary};

    /// Bytes currently owned by the process, as `Layout`s state them.
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    /// The high-water mark since the last phase boundary; [`ProfilingClock`] swaps it on every tick.
    static WINDOW: AtomicUsize = AtomicUsize::new(0);
    /// The high-water mark over the whole run, never reset.
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    /// Raise `slot` to `now` if `now` is higher. Relaxed throughout: each counter is a statistic, not
    /// a lock — no other memory is published through it, and the assembly is single-threaded, so the
    /// only contention is with whatever the runtime allocates on its own threads.
    #[inline]
    fn raise(slot: &AtomicUsize, now: usize) {
        let mut seen = slot.load(Ordering::Relaxed);
        while now > seen {
            match slot.compare_exchange_weak(seen, now, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(actual) => seen = actual,
            }
        }
    }

    #[inline]
    fn grew(now: usize) {
        raise(&WINDOW, now);
        raise(&PEAK, now);
    }

    /// A pass-through over [`System`] that counts. It charges the **net** size change of every
    /// allocation, so a `Vec` growth that the system realloc satisfies by copying into a fresh block
    /// is charged its increment, not `old + new` for the instant both exist. That undercount is
    /// bounded by the largest single buffer and is why a run worth publishing is cross-checked
    /// against `/usr/bin/time -l`'s peak RSS.
    pub struct Tracking;

    unsafe impl GlobalAlloc for Tracking {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(layout) };
            if !p.is_null() {
                grew(LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size());
            }
            p
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(layout) };
            if !p.is_null() {
                grew(LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size());
            }
            p
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let p = unsafe { System.realloc(ptr, layout, new_size) };
            if !p.is_null() {
                let old = layout.size();
                if new_size >= old {
                    grew(LIVE.fetch_add(new_size - old, Ordering::Relaxed) + (new_size - old));
                } else {
                    LIVE.fetch_sub(old - new_size, Ordering::Relaxed);
                }
            }
            p
        }
    }

    /// One phase boundary: when the engine ticked, how high the heap got since the previous tick,
    /// and how much of it was still owned at the tick itself.
    struct Sample {
        us: u64,
        window_peak: usize,
        live: usize,
    }

    /// The CLI's clock, plus a snapshot at every tick.
    ///
    /// The engine calls its [`Clock`] exactly once per phase boundary, in a documented order
    /// (`assemble_full`): start, open, poi, nav, plan, then write, verify and the total.
    /// [`ProfilingClock::report`] labels
    /// the samples against the summary it is handed, and falls back to positional labels if the
    /// engine ever ticks a different number of times than that arithmetic predicts — a wrong label
    /// on a real number is worse than an honest `tick N`.
    pub struct ProfilingClock<C: Clock> {
        inner: C,
        samples: RefCell<Vec<Sample>>,
    }

    impl<C: Clock> ProfilingClock<C> {
        pub fn new(inner: C) -> Self {
            // Reserved up front so the recording itself does not allocate mid-phase.
            ProfilingClock { inner, samples: RefCell::new(Vec::with_capacity(256)) }
        }

        /// Print the per-phase table to stderr. `nav_section_bytes` is the assembly's one natural
        /// yardstick: the section the whole rewrite exists to produce.
        pub fn report(&self, summary: &Summary) {
            let samples = self.samples.borrow();
            let labels = labels_for(samples.len());
            eprintln!("\nmem-profile — peak heap per phase (the window resets at every phase boundary)");
            eprintln!("  {:<28}{:>14}{:>14}{:>12}", "phase", "peak", "live after", "at");
            for (s, label) in samples.iter().zip(&labels) {
                eprintln!(
                    "  {:<28}{:>14}{:>14}{:>11.1}s",
                    label,
                    mib(s.window_peak),
                    mib(s.live),
                    s.us as f64 / 1_000_000.0
                );
            }
            let peak = PEAK.load(Ordering::Relaxed);
            let nav = summary.stats.nav_section_bytes;
            eprintln!("  {:-<68}", "");
            eprintln!("  {:<28}{:>14}  ({peak} bytes)", "overall peak", mib(peak));
            eprintln!("  {:<28}{:>14}  ({nav} bytes)", "nav section written", mib(nav as usize));
            if nav > 0 {
                eprintln!("  {:<28}{:>13.2}x", "overall peak / nav section", peak as f64 / nav as f64);
            }
        }
    }

    impl<C: Clock> Clock for ProfilingClock<C> {
        fn now_us(&self) -> u64 {
            let us = self.inner.now_us();
            let live = LIVE.load(Ordering::Relaxed);
            // Close the window at the level the heap is actually sitting at, so the next phase's
            // number is what *that* phase added rather than what a previous one left behind.
            let window_peak = WINDOW.swap(live, Ordering::Relaxed);
            self.samples.borrow_mut().push(Sample { us, window_peak, live });
            us
        }
    }

    fn mib(bytes: usize) -> String {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }

    /// The tick order `assemble_full` documents. It is a fixed list now — one file means one write
    /// and one verify, where a set meant four ticks per shard and two more for the raster.
    fn labels_for(ticks: usize) -> Vec<String> {
        let mut out: Vec<String> = [
            "start (CLI: sidecar + open)",
            "open cells",
            "merge POIs",
            "merge nav",
            "plan map",
            "(pre-write)",
            "write",
            "(pre-verify)",
            "verify",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if out.len() != ticks {
            // The engine's tick order changed under us: number them rather than mislabel them.
            out = (0..ticks).map(|k| format!("tick {k}")).collect();
        }
        out
    }
}

#[cfg(feature = "mem-profile")]
#[global_allocator]
static TRACKING_ALLOCATOR: mem_profile::Tracking = mem_profile::Tracking;

/// One cell as the cutter's sidecar states it.
struct SidecarCell {
    id: CellId,
    band: String,
    path: String,
    partial: bool,
}

fn parse_sidecar(text: &str) -> std::result::Result<(Schema, Vec<SidecarCell>), String> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("cells.json: {e}"))?;
    let schema: Schema = serde_json::from_value(doc.get("schema").cloned().unwrap_or_default())
        .map_err(|e| format!("cells.json schema: {e}"))?;
    let cells = doc.get("cells").and_then(|c| c.as_array()).ok_or("cells.json has no `cells` array")?;
    let mut out = Vec::with_capacity(cells.len());
    for c in cells {
        let id = c.get("id").and_then(|v| v.as_str()).ok_or("a cell entry has no `id`")?;
        out.push(SidecarCell {
            id: CellId::parse(id)?,
            band: c.get("band").and_then(|v| v.as_str()).ok_or("a cell entry has no `band`")?.to_string(),
            path: c.get("path").and_then(|v| v.as_str()).ok_or("a cell entry has no `path`")?.to_string(),
            partial: c.get("partial").and_then(|v| v.as_bool()).unwrap_or(false),
        });
    }
    Ok((schema, out))
}

const USAGE: &str = "\
obcm-assemble — assemble baked OBCA cells into one .obcm

USAGE:
    obcm-assemble --cells <cells.json> --skin <skin.json> --out <path.obcm> [OPTIONS]

REQUIRED:
    --cells <path>          the cutter's provenance sidecar (obc-pack cut)
    --skin <path>           the skin to stamp (OBCC §5, or an id-keyed style table)
    --out <path>            the map file to write

OPTIONS:
    --schema <path>         schema document (OBCC v2 root or SchemaEntry); default: the sidecar's
    --terrain <path>        terrain sidecar: {posting_log2, cell_log2, cells:[{id, path, sha256?}]}.
                            Splices the raster into the map's OBCM §1.3 terrain region. Squares the
                            selection covers but the list omits are canonically void and cost four
                            directory bytes each (OBCC §13.6)
    --band <id>             only assemble these bands (repeatable)
    --cell <id>             only assemble these cells (repeatable, `<log2>/<i>/<j>`)
    --merge-budget-bytes <n>  the most memory the §4.6 merge's sorted passes may hold (default 64
                            MiB). Everything above it spills to scratch files under the system temp
                            directory, which are removed as the merge finishes with them. Lower it to
                            assemble a region on a small machine — or to prove that the map is the
                            same bytes at every budget, which is what it is here for
    --accept-holes          proceed although the selection has missing cells. The hole is legal — the
                            quadtree writes an empty leaf and the renderer paints backdrop — but
                            OBCA §4.1 requires the caller to say so rather than discover it
    --accept-partial        proceed although a cell is `partial`: its sources did not cover its whole
                            square, so the map ends inside that cell rather than at its edge, with no
                            visible seam to warn the rider (OBCA §3.7)
    --skip-verify           skip the §4.8 verify pass — BENCHMARKS ONLY, never for a device
    --json                  print the summary as JSON
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> std::result::Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(());
    }
    let mut cells_path: Option<PathBuf> = None;
    let mut terrain_path: Option<PathBuf> = None;
    let mut schema_path: Option<PathBuf> = None;
    let mut skin_path: Option<PathBuf> = None;
    let mut out_path: Option<PathBuf> = None;
    let mut only_bands: Vec<String> = Vec::new();
    let mut only_cells: Vec<CellId> = Vec::new();
    let mut json = false;
    let mut opts = Options::default();

    let mut i = 0;
    while i < args.len() {
        let value = |i: &mut usize| -> std::result::Result<String, String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", args[*i - 1]))
        };
        match args[i].as_str() {
            "--cells" => cells_path = Some(PathBuf::from(value(&mut i)?)),
            "--terrain" => terrain_path = Some(PathBuf::from(value(&mut i)?)),
            "--schema" => schema_path = Some(PathBuf::from(value(&mut i)?)),
            "--skin" => skin_path = Some(PathBuf::from(value(&mut i)?)),
            "--out" => out_path = Some(PathBuf::from(value(&mut i)?)),
            "--band" => only_bands.push(value(&mut i)?),
            "--cell" => only_cells.push(CellId::parse(&value(&mut i)?)?),
            "--merge-budget-bytes" => {
                opts.merge_budget_bytes = value(&mut i)?.parse().map_err(|_| "--merge-budget-bytes takes a number")?;
                if opts.merge_budget_bytes == 0 {
                    return Err("--merge-budget-bytes must be at least one record".into());
                }
            }
            "--accept-holes" => opts.accept_holes = true,
            "--accept-partial" => opts.accept_partial = true,
            "--skip-verify" => opts.skip_verify = true,
            "--json" => json = true,
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
        i += 1;
    }

    let cells_path = cells_path.ok_or("--cells is required")?;
    let skin_path = skin_path.ok_or("--skin is required")?;
    let out_path = out_path.ok_or("--out is required")?;
    let root = cells_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let sidecar = std::fs::read_to_string(&cells_path).map_err(|e| format!("read {}: {e}", cells_path.display()))?;
    let (sidecar_schema, listed) = parse_sidecar(&sidecar)?;
    let schema = match &schema_path {
        None => sidecar_schema,
        Some(p) => Schema::parse(&std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?)?,
    };
    let skin =
        Skin::parse(&std::fs::read_to_string(&skin_path).map_err(|e| format!("read {}: {e}", skin_path.display()))?)?;

    // Open every selected cell. The sources outlive the assembly, so they are held here.
    let selected: Vec<&SidecarCell> = listed
        .iter()
        .filter(|c| only_bands.is_empty() || only_bands.contains(&c.band))
        .filter(|c| only_cells.is_empty() || only_cells.contains(&c.id))
        .collect();
    if selected.is_empty() {
        return Err("the selection is empty".into());
    }
    let mut sources: Vec<FileSource> = Vec::with_capacity(selected.len());
    for c in &selected {
        let path = root.join(&c.path);
        sources.push(FileSource::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?);
    }
    let inputs: Vec<CellInput<'_>> = selected
        .iter()
        .zip(&sources)
        .map(|(c, src)| CellInput { id: c.id, band: c.band.clone(), src, partial: c.partial })
        .collect();

    // The raster, if this assembly has one. Read as file sources exactly like the OBCM cells: the
    // engine copies one block at a time, so a continental terrain tree is never resident.
    let terrain_sidecar = match &terrain_path {
        None => None,
        Some(p) => {
            Some(parse_terrain_sidecar(&std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?)?)
        }
    };
    let terrain_root = terrain_path.as_ref().and_then(|p| p.parent()).unwrap_or(Path::new(".")).to_path_buf();
    let mut terrain_sources: Vec<FileSource> = Vec::new();
    if let Some(sidecar) = &terrain_sidecar {
        for (_, path, _) in &sidecar.cells {
            let path = terrain_root.join(path);
            terrain_sources.push(FileSource::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?);
        }
    }

    let mut store = FileStore::new(&out_path).map_err(|e| e.to_string())?;
    let job = terrain_sidecar.as_ref().map(|sidecar| TerrainJob {
        params: sidecar.params,
        cells: sidecar
            .cells
            .iter()
            .zip(&terrain_sources)
            .map(|((id, _, sha256), src)| TerrainCellInput { id: *id, src, sha256: *sha256 })
            .collect(),
    });

    #[cfg(not(feature = "mem-profile"))]
    let clock = StdClock(Instant::now());
    #[cfg(feature = "mem-profile")]
    let clock = mem_profile::ProfilingClock::new(StdClock(Instant::now()));
    // The engine's spill area. Real files, so the merge's sorted passes are genuinely off-heap —
    // which is also what makes the `mem-profile` numbers mean anything.
    let scratch = FileScratch::new()?;
    let summary = assemble_full(inputs, Vec::new(), job, &schema, &skin, &opts, &mut store, &clock, &scratch)
        .map_err(|e| e.to_string())?;

    // The engine returns what the spec says to report; the CLI is what has a stderr (§4.5.2, §5.7,
    // `OBCM_Spec.md` §8.3). Printed before the summary so a long JSON blob cannot bury them.
    for w in &summary.warnings {
        eprintln!("warning: {w}");
    }
    if json {
        println!("{}", summary_json(&summary, &out_path));
    } else {
        print_summary(&summary, &out_path);
    }
    // stderr, and last: the summary above is pinned by tests and by the builder's parser.
    #[cfg(feature = "mem-profile")]
    clock.report(&summary);
    Ok(())
}

fn print_summary(s: &obcm_assemble::Summary, out_path: &Path) {
    let (min_lon, min_lat, max_lon, max_lat) = s.assembly_box.ubox();
    println!(
        "assembly bbox: lat [{min_lat}, {max_lat}) × lon [{min_lon}, {max_lon})  (2^{} µdeg square)",
        s.assembly_box.span_log2
    );
    println!("{} cell(s) → {} bytes", s.stats.cells, s.bytes);
    let v = s
        .verify
        .as_ref()
        .map(|r| {
            format!(
                "  verified: {} chunk(s), {} feature(s), {} nav node(s), largest component {:.1}%",
                r.chunks,
                r.features,
                r.nav_nodes,
                r.largest_component_permille as f64 / 10.0
            )
        })
        .unwrap_or_else(|| "  NOT VERIFIED (--skip-verify)".into());
    println!("  {}\n{v}", out_path.display());
    if let Some(t) = &s.terrain {
        println!(
            "  terrain region: {} B, {} of {} square(s) present, the rest canonically void",
            t.bytes, t.cells, t.slots
        );
    }
    let st = &s.stats;
    println!(
        "phases (ms): open {:.1} · poi {:.1} · nav {:.1} · plan {:.1} · write(+graft) {:.1} · verify {:.1} · total \
         {:.1}",
        st.open_us as f64 / 1000.0,
        st.poi_us as f64 / 1000.0,
        st.nav_us as f64 / 1000.0,
        st.plan_us as f64 / 1000.0,
        st.write_us as f64 / 1000.0,
        st.verify_us as f64 / 1000.0,
        st.total_us as f64 / 1000.0
    );
    println!(
        "geometry copied: {} B · nav section: {} B ({} node(s), {} edge(s), {} unified at seams, {} island node(s) \
         pruned, {} adjacency entrie(s) at the degree cap, {} node record(s) dropped) · POIs: {} ({} duplicate(s), {} \
         dropped)",
        st.geometry_bytes,
        st.nav_section_bytes,
        st.nav.nodes,
        st.nav.edges,
        st.nav.unified,
        st.nav.pruned_nodes,
        st.nav.degree_truncated,
        st.nav.dropped_nodes,
        st.poi_records,
        st.poi_duplicates,
        st.poi_dropped
    );
}

fn summary_json(s: &obcm_assemble::Summary, out_path: &Path) -> String {
    let st = &s.stats;
    serde_json::to_string_pretty(&serde_json::json!({
        "assembly_bbox_udeg": {
            "min_lat": s.assembly_box.min_lat,
            "min_lon": s.assembly_box.min_lon,
            "span_log2": s.assembly_box.span_log2,
        },
        "cells": st.cells,
        "file": out_path.display().to_string(),
        "bytes": s.bytes,
        "sha256": s.sha256.iter().map(|b| format!("{b:02x}")).collect::<String>(),
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
