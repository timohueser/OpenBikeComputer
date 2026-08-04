//! **The budget is measured, not asserted** (#1116 D2).
//!
//! [`obcm_assemble::extsort::ExternalSort`]'s whole reason to exist is that it sorts more records
//! than it can hold. A unit test can check that it produces the right order and that its own buffer
//! reports a bounded capacity, but neither of those would notice a k-way merge that quietly
//! allocated a cursor per run without dividing the budget, or a `Vec` that doubled past the ceiling
//! at the worst moment. What notices is counting every allocation the process makes.
//!
//! So this file installs a counting global allocator — process-wide, which is why it is a test
//! binary of its own with exactly **one** test in it (two would interleave on the counters) — and
//! sorts the same shape of input at two sizes and two budgets:
//!
//! * 16× the records at the same budget must cost the **same peak**. That is the claim: the sort's
//!   footprint is the budget, not the input.
//! * 16× the budget at the same records must cost a visibly larger one. That is the other half:
//!   the knob is real, not decoration.
//!
//! The scratch is backed by **files**, not [`obcm_assemble::MemoryScratch`], for the same reason:
//! an in-memory scratch would put the spill back on the heap and every number here would measure it
//! instead of the sorter.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering as Memory};

use obcm_assemble::extsort::ExternalSort;
use obcm_assemble::{Error, Result, ScratchId, ScratchStore};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

/// Charges the **net** change of every allocation, exactly as the CLI's `mem-profile` harness does,
/// so the two report the same kind of number.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(LIVE.fetch_add(layout.size(), Memory::Relaxed) + layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(LIVE.fetch_add(layout.size(), Memory::Relaxed) + layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Memory::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            let old = layout.size();
            if new_size >= old {
                grew(LIVE.fetch_add(new_size - old, Memory::Relaxed) + (new_size - old));
            } else {
                LIVE.fetch_sub(old - new_size, Memory::Relaxed);
            }
        }
        p
    }
}

fn grew(now: usize) {
    let mut seen = PEAK.load(Memory::Relaxed);
    while now > seen {
        match PEAK.compare_exchange_weak(seen, now, Memory::Relaxed, Memory::Relaxed) {
            Ok(_) => break,
            Err(actual) => seen = actual,
        }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Bytes allocated above where the heap was standing when `f` started.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = LIVE.load(Memory::Relaxed);
    PEAK.store(before, Memory::Relaxed);
    let out = f();
    (out, PEAK.load(Memory::Relaxed).saturating_sub(before))
}

/// Scratch as real files, so nothing the sorter spills is counted as heap.
struct FileScratch {
    dir: PathBuf,
    files: RefCell<Vec<Option<(File, u64)>>>,
}

impl FileScratch {
    fn new() -> FileScratch {
        let dir = std::env::temp_dir().join(format!("obcm-sort-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp directory");
        FileScratch { dir, files: RefCell::new(Vec::new()) }
    }

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
        let id = ScratchId(files.len() as u32);
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(self.dir.join(format!("{}.spill", id.0)))
            .map_err(|e| Error::Scratch(format!("{id}: {e}")))?;
        files.push(Some((file, 0)));
        Ok(id)
    }
    fn append(&self, id: ScratchId, buf: &[u8]) -> Result<()> {
        self.with(id, |(file, len)| {
            file.seek(SeekFrom::Start(*len))
                .and_then(|_| file.write_all(buf))
                .map_err(|e| Error::Scratch(e.to_string()))?;
            *len += buf.len() as u64;
            Ok(())
        })
    }
    fn read_at(&self, id: ScratchId, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.with(id, |(file, _)| {
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(buf))
                .map_err(|e| Error::Scratch(e.to_string()))
        })
    }
    fn len(&self, id: ScratchId) -> Result<u64> {
        self.with(id, |(_, len)| Ok(*len))
    }
    fn remove(&self, id: ScratchId) -> Result<()> {
        let mut files = self.files.borrow_mut();
        if let Some(slot) = files.get_mut(id.0 as usize) {
            *slot = None;
            let _ = std::fs::remove_file(self.dir.join(format!("{}.spill", id.0)));
        }
        Ok(())
    }
}

impl Drop for FileScratch {
    fn drop(&mut self) {
        self.files.borrow_mut().clear();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Sixteen bytes, the shape of the merge's own node record: a key and a payload the comparator does
/// not read.
const R: usize = 16;

fn key(r: &[u8; R]) -> u64 {
    u64::from_le_bytes(r[0..8].try_into().expect("8 bytes"))
}

fn by_key(a: &[u8; R], b: &[u8; R]) -> Ordering {
    key(a).cmp(&key(b))
}

/// Sort `n` records at `budget` and return the peak heap the whole pass cost, output checked.
fn sort_peak(scratch: &FileScratch, n: u64, budget: usize) -> usize {
    let ((), peak) = peak_of(|| {
        let mut sort = ExternalSort::<R>::new(scratch, budget, by_key);
        // A deterministic scramble, so the runs are genuinely unordered relative to each other.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for i in 0..n {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let mut rec = [0u8; R];
            rec[0..8].copy_from_slice(&x.to_le_bytes());
            rec[8..16].copy_from_slice(&i.to_le_bytes());
            sort.push(rec).expect("push");
        }
        let mut seen = 0u64;
        let mut last = 0u64;
        for rec in sort.finish().expect("finish") {
            let k = key(&rec.expect("a record"));
            assert!(k >= last, "the merged stream is out of order at record {seen}");
            last = k;
            seen += 1;
        }
        assert_eq!(seen, n, "the sort lost records");
    });
    peak
}

/// The one test in this binary — see the module header for why it is one.
#[test]
fn the_sort_costs_its_budget_and_not_its_input() {
    let scratch = FileScratch::new();
    const SMALL: usize = 256 * 1024;
    const LARGE: usize = 16 * SMALL;

    // Warm up: the first sort pays for whatever the runtime lazily allocates on first use (the
    // formatting machinery, the io buffers), and that is not the sorter's cost.
    sort_peak(&scratch, 10_000, SMALL);

    let few = sort_peak(&scratch, 50_000, SMALL);
    let many = sort_peak(&scratch, 800_000, SMALL);
    let generous = sort_peak(&scratch, 800_000, LARGE);

    // 16× the records, same budget: the same peak. A quarter of the budget is the slack — the input
    // grew by 12 MB of records and 45 runs' worth of cursors, so anything proportional to `n` blows
    // straight through this.
    assert!(
        many <= few + SMALL / 4,
        "16× the records cost {many} B against {few} B at the same {SMALL} B budget — the sort's footprint is \
         tracking its input, not its budget"
    );
    // …and neither of them is above the budget by more than the merge's own fixed overhead.
    for (what, peak) in [("50 000", few), ("800 000", many)] {
        assert!(peak < SMALL * 2, "{what} records peaked at {peak} B, past twice the {SMALL} B budget");
    }
    // 16× the budget, same records: a visibly bigger footprint, or the parameter is decoration.
    assert!(
        generous > SMALL * 4,
        "raising the budget from {SMALL} B to {LARGE} B moved the peak to only {generous} B — the budget is not what \
         the sort is sizing itself from"
    );
    assert!(generous < LARGE * 2, "the {LARGE} B budget peaked at {generous} B, past twice what it was given");
}
