//! Fixed-size record streams over the [scratch seam](crate::scratch), and the external sort every
//! whole-map pass in the merge is built around.
//!
//! The merge's blocker at country scale is bookkeeping held as whole-map random-access arrays. The
//! replacement for random access is a sorted pass, and a sorted pass at DACH scale does not fit in
//! a browser tab's heap — so it is generated in runs bounded by an explicit budget and merged back
//! k-way.
//!
//! [`ExternalSort::new`] takes a byte budget and never exceeds it: run generation fills a buffer of
//! exactly `budget / R` records and spills when it is full, and the k-way merge divides the same
//! budget among the runs' read buffers. The buffer grows in steps rather than being reserved up
//! front, so a sort of six records costs six records while its capacity never passes the ceiling; a
//! doubling `Vec` would sail through it by up to 2× at the worst possible moment.
//! `tests/sort_budget.rs` measures the peak rather than asserting it.
//!
//! Everything downstream of a sorted pass — dense node ids, the edge pool's layout, the adjacency
//! walk order — is byte-visible in the map, so the sort has to produce one answer. It is stable:
//! records that compare `Equal` come out in push order, across the whole sort and not only inside a
//! run, because runs are generated with a stable sort, merged with ties broken by lowest run index,
//! and numbered in push order.
//!
//! Callers should still prefer a comparator that is a total order, because then the result does not
//! depend on stability at all. What a comparator may not be is inconsistent: `sort_by` guarantees
//! nothing beyond "does not panic, does not lose records" for one, and the k-way merge would
//! produce an unsorted stream, so determinism would be lost silently. Comparators here are `fn`
//! pointers over plain byte arrays precisely so they are easy to keep pure.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::scratch::{ScratchId, ScratchStore};
use crate::Result;

/// How two `R`-byte records order. See the module header for the contract.
pub type Comparator<const R: usize> = fn(&[u8; R], &[u8; R]) -> Ordering;

/// How many `R`-byte records `budget` bytes hold — at least one, or a sort could never make
/// progress.
fn records_in(budget: usize, r: usize) -> usize {
    (budget / r.max(1)).max(1)
}

/// The share of the budget [`ExternalSort`]'s run buffer gets, as a divisor.
///
/// Half, because a stable sort of that buffer allocates an auxiliary one beside it: Rust's
/// `slice::sort_by` reserves up to the slice's own size, so a run buffer given the whole budget
/// peaks at twice it. The merge half has no such companion and divides the full budget among its
/// cursors.
const RUN_SHARE: usize = 2;

/// Grow `buf` towards `cap` records without ever passing it, in ×4 steps from a small floor.
///
/// A plain `push` doubles, which would leave a buffer of `2 × cap` capacity the instant it reached
/// `cap`, and reserving `cap` up front would make a six-record sort allocate the whole ceiling.
fn grow_to<const R: usize>(buf: &mut Vec<[u8; R]>, cap: usize) {
    if buf.len() < buf.capacity() {
        return;
    }
    let want = buf.capacity().saturating_mul(4).max(64).min(cap);
    if want > buf.capacity() {
        buf.reserve_exact(want - buf.capacity());
    }
}

/// A stream of `R`-byte records being appended to a scratch file, through a buffer of at most
/// `budget` bytes.
pub struct SpillWriter<'s, const R: usize> {
    scratch: &'s dyn ScratchStore,
    id: ScratchId,
    buf: Vec<[u8; R]>,
    cap: usize,
    /// Records handed over so far, flushed or not.
    written: u64,
}

impl<'s, const R: usize> SpillWriter<'s, R> {
    /// Open a new scratch file to spill into.
    pub fn create(scratch: &'s dyn ScratchStore, budget: usize) -> Result<SpillWriter<'s, R>> {
        let id = scratch.create()?;
        Ok(SpillWriter { scratch, id, buf: Vec::new(), cap: records_in(budget, R), written: 0 })
    }

    pub fn push(&mut self, rec: [u8; R]) -> Result<()> {
        grow_to(&mut self.buf, self.cap);
        self.buf.push(rec);
        self.written += 1;
        if self.buf.len() >= self.cap {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if !self.buf.is_empty() {
            self.scratch.append(self.id, self.buf.as_flattened())?;
            self.buf.clear();
        }
        Ok(())
    }

    /// Flush and hand back the file, which the caller now owns (and must [`ScratchStore::remove`]).
    pub fn seal(mut self) -> Result<(ScratchId, u64)> {
        self.flush()?;
        // Release the buffer before the caller starts reading the file back: at DACH scale this is
        // the difference between the write buffer and the read buffer coexisting and not.
        self.buf = Vec::new();
        Ok((self.id, self.written))
    }
}

/// One `R`-byte record stream being read front to back, through a buffer of at most `budget` bytes.
pub struct SpillReader<'s, const R: usize> {
    inner: BlockReader<'s, R>,
}

impl<'s, const R: usize> SpillReader<'s, R> {
    /// Read `id` from its first byte to its last. The file's length must be a whole number of
    /// records; a partial tail is a defect in whatever wrote it.
    pub fn open(scratch: &'s dyn ScratchStore, id: ScratchId, budget: usize) -> Result<SpillReader<'s, R>> {
        let end = scratch.len(id)?;
        if end % R as u64 != 0 {
            return Err(crate::Error::Scratch(format!(
                "{id} is {end} bytes, which is not a whole number of {R}-byte records"
            )));
        }
        Ok(SpillReader { inner: BlockReader::new(scratch, id, 0, end, records_in(budget, R)) })
    }
}

impl<const R: usize> Iterator for SpillReader<'_, R> {
    type Item = Result<[u8; R]>;
    fn next(&mut self) -> Option<Result<[u8; R]>> {
        self.inner.next()
    }
}

/// A window of one scratch file, read forward through a fixed record buffer.
struct BlockReader<'s, const R: usize> {
    scratch: &'s dyn ScratchStore,
    id: ScratchId,
    /// Next byte of the file to fetch.
    at: u64,
    /// One past this window's last byte.
    end: u64,
    /// The fetched block. `pos` records are already handed out.
    buf: Vec<u8>,
    pos: usize,
    /// Records per fetch.
    cap: usize,
}

impl<'s, const R: usize> BlockReader<'s, R> {
    fn new(scratch: &'s dyn ScratchStore, id: ScratchId, at: u64, end: u64, cap: usize) -> BlockReader<'s, R> {
        BlockReader { scratch, id, at, end, buf: Vec::new(), pos: 0, cap: cap.max(1) }
    }

    fn refill(&mut self) -> Result<bool> {
        let left = self.end - self.at;
        if left == 0 {
            return Ok(false);
        }
        let want = (self.cap as u64 * R as u64).min(left) as usize;
        self.buf.resize(want, 0);
        self.scratch.read_at(self.id, self.at, &mut self.buf)?;
        self.at += want as u64;
        self.pos = 0;
        Ok(true)
    }
}

impl<const R: usize> Iterator for BlockReader<'_, R> {
    type Item = Result<[u8; R]>;

    fn next(&mut self) -> Option<Result<[u8; R]>> {
        if self.pos * R >= self.buf.len() {
            match self.refill() {
                Ok(false) => return None,
                Ok(true) => {}
                Err(e) => return Some(Err(e)),
            }
        }
        let at = self.pos * R;
        self.pos += 1;
        Some(Ok(self.buf[at..at + R].try_into().expect("R bytes")))
    }
}

/// One run's next record, in the k-way merge's heap.
///
/// `Ord` is reversed so [`BinaryHeap`]'s max-heap pops the smallest record, and ties go to the
/// lowest run index — runs are numbered in push order, which is what makes the whole sort stable.
struct Head<const R: usize> {
    rec: [u8; R],
    run: usize,
    order: Comparator<R>,
}

impl<const R: usize> Ord for Head<R> {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.order)(&other.rec, &self.rec).then_with(|| other.run.cmp(&self.run))
    }
}

impl<const R: usize> PartialOrd for Head<R> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<const R: usize> PartialEq for Head<R> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<const R: usize> Eq for Head<R> {}

/// A sort of `R`-byte records that never holds more than its budget.
///
/// Push everything, then [`ExternalSort::finish`] for the sorted stream. Dropping an unfinished
/// sort removes its runs.
pub struct ExternalSort<'s, const R: usize> {
    scratch: &'s dyn ScratchStore,
    budget: usize,
    /// Records the run buffer may hold — `budget / R`.
    cap: usize,
    buf: Vec<[u8; R]>,
    runs: Vec<(ScratchId, u64)>,
    order: Comparator<R>,
}

impl<'s, const R: usize> ExternalSort<'s, R> {
    pub fn new(scratch: &'s dyn ScratchStore, budget: usize, order: Comparator<R>) -> ExternalSort<'s, R> {
        let cap = records_in(budget / RUN_SHARE, R);
        ExternalSort { scratch, budget, cap, buf: Vec::new(), runs: Vec::new(), order }
    }

    pub fn push(&mut self, rec: [u8; R]) -> Result<()> {
        grow_to(&mut self.buf, self.cap);
        self.buf.push(rec);
        if self.buf.len() >= self.cap {
            self.spill()?;
        }
        Ok(())
    }

    /// Runs written so far — what a budget test counts.
    pub fn runs(&self) -> usize {
        self.runs.len()
    }

    /// Bytes the run buffer is holding right now — [`RUN_SHARE`]'s share of the budget at most,
    /// which with the stable sort's companion buffer is the budget.
    pub fn resident_bytes(&self) -> usize {
        self.buf.capacity() * R
    }

    /// Sort the buffer and write it out as one run.
    fn spill(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        // Stable, so equal records keep push order inside the run.
        self.buf.sort_by(self.order);
        let id = self.scratch.create()?;
        if let Err(error) = self.scratch.append(id, self.buf.as_flattened()) {
            let _ = self.scratch.remove(id);
            return Err(error);
        }
        self.runs.push((id, self.buf.len() as u64));
        self.buf.clear();
        Ok(())
    }

    /// The sorted records. Nothing is materialized: a single run that never spilled is served from
    /// the buffer that already holds it, and anything larger is merged as it is read.
    pub fn finish(mut self) -> Result<SortedRecords<'s, R>> {
        if self.runs.is_empty() {
            self.buf.sort_by(self.order);
            return Ok(SortedRecords {
                source: Source::Memory { buf: std::mem::take(&mut self.buf), at: 0 },
                scratch: self.scratch,
            });
        }
        self.spill()?;
        // The run buffer is dead the moment the last run is on disk, and the read buffers below are
        // about to be allocated.
        self.buf = Vec::new();

        // The budget, split over the runs plus one share of slack for the heap and the caller's own
        // record. A run always gets at least one record's worth, so a merge of more runs than the
        // budget has records still runs, slower and correct.
        let per = records_in(self.budget / (self.runs.len() + 1), R);
        let mut cursors: Vec<BlockReader<'s, R>> = Vec::with_capacity(self.runs.len());
        let mut heap: BinaryHeap<Head<R>> = BinaryHeap::with_capacity(self.runs.len());
        for (run, &(id, count)) in self.runs.iter().enumerate() {
            let mut cursor = BlockReader::new(self.scratch, id, 0, count * R as u64, per);
            if let Some(rec) = cursor.next() {
                heap.push(Head { rec: rec?, run, order: self.order });
            }
            cursors.push(cursor);
        }
        let runs = std::mem::take(&mut self.runs).into_iter().map(|(id, _)| Some(id)).collect();
        Ok(SortedRecords { source: Source::Merge { cursors, heap, runs }, scratch: self.scratch })
    }
}

impl<const R: usize> Drop for ExternalSort<'_, R> {
    fn drop(&mut self) {
        for &(id, _) in &self.runs {
            let _ = self.scratch.remove(id);
        }
    }
}

enum Source<'s, const R: usize> {
    Memory {
        buf: Vec<[u8; R]>,
        at: usize,
    },
    Merge {
        cursors: Vec<BlockReader<'s, R>>,
        heap: BinaryHeap<Head<R>>,
        /// `None` once a run's file is already deleted, which happens the moment its cursor
        /// exhausts rather than when the stream drops. On the merge's workloads push order
        /// correlates with key order, so runs drain one after another and the spill shrinks while
        /// the next pass's spill grows; without this, two chained passes peak at twice the data.
        runs: Vec<Option<ScratchId>>,
    },
}

/// The sorted stream. Read it once, front to back; the runs behind it are deleted when it drops.
pub struct SortedRecords<'s, const R: usize> {
    source: Source<'s, R>,
    scratch: &'s dyn ScratchStore,
}

impl<const R: usize> Iterator for SortedRecords<'_, R> {
    type Item = Result<[u8; R]>;

    fn next(&mut self) -> Option<Result<[u8; R]>> {
        match &mut self.source {
            Source::Memory { buf, at } => {
                let rec = *buf.get(*at)?;
                *at += 1;
                Some(Ok(rec))
            }
            Source::Merge { cursors, heap, runs } => {
                let head = heap.pop()?;
                match cursors[head.run].next() {
                    Some(Ok(rec)) => heap.push(Head { rec, run: head.run, order: head.order }),
                    Some(Err(e)) => return Some(Err(e)),
                    None => {
                        // This run's last record is the one being handed out, so its file is dead
                        // now. Best-effort for the same reason `Drop` is.
                        if let Some(id) = runs[head.run].take() {
                            let _ = self.scratch.remove(id);
                        }
                    }
                }
                Some(Ok(head.rec))
            }
        }
    }
}

/// The runs die with the stream that reads them.
///
/// Best-effort by design: a scratch file that cannot be deleted is bytes nothing can reach any
/// more, on a host that is about to drop its whole scratch area. Failing an assembly over it would
/// turn a storage hiccup into a map that was never written.
impl<const R: usize> Drop for SortedRecords<'_, R> {
    fn drop(&mut self) {
        if let Source::Merge { runs, cursors, heap } = &mut self.source {
            cursors.clear();
            heap.clear();
            // Only what the merge walk has not already deleted.
            for id in runs.drain(..).flatten() {
                let _ = self.scratch.remove(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::MemoryScratch;

    /// Records are `[key u32 LE][tag u32 LE]`; the comparator reads the key only, so the tag is
    /// what a stability claim is visible in.
    const R: usize = 8;

    fn rec(key: u32, tag: u32) -> [u8; R] {
        let mut r = [0u8; R];
        r[0..4].copy_from_slice(&key.to_le_bytes());
        r[4..8].copy_from_slice(&tag.to_le_bytes());
        r
    }

    fn key(r: &[u8; R]) -> u32 {
        u32::from_le_bytes(r[0..4].try_into().expect("4 bytes"))
    }

    fn tag(r: &[u8; R]) -> u32 {
        u32::from_le_bytes(r[4..8].try_into().expect("4 bytes"))
    }

    fn by_key(a: &[u8; R], b: &[u8; R]) -> Ordering {
        key(a).cmp(&key(b))
    }

    /// A deterministic shuffle — a 32-bit xorshift, so the fixture is the same on every machine.
    fn scrambled(n: u32) -> Vec<u32> {
        let mut x = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x % (n * 2)
            })
            .collect()
    }

    fn sorted_with(budget: usize, input: &[(u32, u32)]) -> (Vec<(u32, u32)>, usize) {
        let scratch = MemoryScratch::new();
        let mut sort = ExternalSort::<R>::new(&scratch, budget, by_key);
        for &(k, t) in input {
            sort.push(rec(k, t)).expect("push");
            assert!(sort.resident_bytes() <= (budget / RUN_SHARE).max(R), "the run buffer passed its share");
        }
        // Read before `finish`, which spills whatever is left as one last run.
        let runs = sort.runs();
        let out: Vec<(u32, u32)> =
            sort.finish().expect("finish").map(|r| r.expect("a record")).map(|r| (key(&r), tag(&r))).collect();
        assert_eq!(scratch.resident_bytes(), 0, "every run is deleted when the stream drops");
        (out, runs)
    }

    #[test]
    fn the_sort_is_the_same_answer_at_every_budget() {
        let input: Vec<(u32, u32)> = scrambled(4000).into_iter().enumerate().map(|(i, k)| (k, i as u32)).collect();
        let mut want: Vec<(u32, u32)> = input.clone();
        want.sort_by_key(|&(k, _)| k); // stable: equal keys keep push order

        // One record per run, a handful, everything at once, and a budget that is not a multiple of
        // the record size.
        for budget in [R, 3 * R, 100 * R, 1000 * R, 1 << 20, 7 * R + 3] {
            let (got, runs) = sorted_with(budget, &input);
            assert_eq!(got, want, "budget {budget} sorted differently");
            let cap = (budget / RUN_SHARE / R).max(1);
            assert_eq!(runs, input.len() / cap, "budget {budget} generated the wrong number of runs");
        }
    }

    #[test]
    fn equal_keys_come_out_in_push_order_however_many_runs_they_land_in() {
        // Ten distinct keys, four hundred records: every key spans many runs at this budget, so a
        // merge that broke ties by anything but the run index would interleave them.
        let input: Vec<(u32, u32)> = (0..400u32).map(|i| (i % 10, i)).collect();
        let (got, runs) = sorted_with(16 * R, &input);
        assert!(runs > 20, "the fixture is meant to generate many runs, got {runs}");
        for k in 0..10u32 {
            let tags: Vec<u32> = got.iter().filter(|&&(gk, _)| gk == k).map(|&(_, t)| t).collect();
            assert!(tags.windows(2).all(|w| w[0] < w[1]), "key {k} came out of order: {tags:?}");
        }
    }

    #[test]
    fn an_empty_sort_yields_nothing_and_touches_no_scratch() {
        let scratch = MemoryScratch::new();
        let sort = ExternalSort::<R>::new(&scratch, 1 << 20, by_key);
        assert_eq!(sort.finish().expect("finish").count(), 0);
        assert_eq!(scratch.resident_bytes(), 0);
    }

    #[test]
    fn a_spilled_stream_reads_back_exactly_what_was_written() {
        let scratch = MemoryScratch::new();
        let mut w = SpillWriter::<R>::create(&scratch, 5 * R).expect("create");
        for i in 0..97u32 {
            w.push(rec(i, i * 3)).expect("push");
        }
        let (id, count) = w.seal().expect("seal");
        assert_eq!(count, 97);
        assert_eq!(scratch.len(id).expect("len"), 97 * R as u64);
        let got: Vec<(u32, u32)> = SpillReader::<R>::open(&scratch, id, 7 * R)
            .expect("open")
            .map(|r| r.expect("a record"))
            .map(|r| (key(&r), tag(&r)))
            .collect();
        assert_eq!(got, (0..97u32).map(|i| (i, i * 3)).collect::<Vec<_>>());
        scratch.remove(id).expect("remove");
    }

    #[test]
    fn a_stream_whose_length_is_not_a_whole_number_of_records_is_refused() {
        let scratch = MemoryScratch::new();
        let id = scratch.create().expect("create");
        scratch.append(id, &[0u8; R + 3]).expect("append");
        let Err(err) = SpillReader::<R>::open(&scratch, id, 1 << 10) else {
            panic!("a stream of one and a bit records must be refused");
        };
        assert!(format!("{err}").contains("whole number"), "got: {err}");
    }

    /// A [`ScratchStore`] that counts what is alive, so a spill-residency claim is measured rather
    /// than assumed.
    struct Counting {
        inner: MemoryScratch,
        live: std::cell::Cell<usize>,
        peak: std::cell::Cell<usize>,
        fail_append: std::cell::Cell<bool>,
        fail_read: std::cell::Cell<bool>,
    }

    impl Counting {
        fn new() -> Counting {
            Counting {
                inner: MemoryScratch::new(),
                live: 0.into(),
                peak: 0.into(),
                fail_append: false.into(),
                fail_read: false.into(),
            }
        }
    }

    impl ScratchStore for Counting {
        fn create(&self) -> Result<ScratchId> {
            self.live.set(self.live.get() + 1);
            self.peak.set(self.peak.get().max(self.live.get()));
            self.inner.create()
        }
        fn append(&self, id: ScratchId, buf: &[u8]) -> Result<()> {
            if self.fail_append.get() {
                self.inner.append(id, &buf[..1])?;
                return Err(crate::Error::Scratch("append refused after a partial write".into()));
            }
            self.inner.append(id, buf)
        }
        fn read_at(&self, id: ScratchId, offset: u64, buf: &mut [u8]) -> Result<()> {
            if self.fail_read.get() {
                return Err(crate::Error::Scratch("read refused".into()));
            }
            self.inner.read_at(id, offset, buf)
        }
        fn len(&self, id: ScratchId) -> Result<u64> {
            self.inner.len(id)
        }
        fn remove(&self, id: ScratchId) -> Result<()> {
            self.live.set(self.live.get() - 1);
            self.inner.remove(id)
        }
    }

    #[test]
    fn abandoned_runs_die_with_the_sort_or_its_transferred_stream() {
        for finish in [false, true] {
            let scratch = Counting::new();
            let mut sort = ExternalSort::<R>::new(&scratch, 4 * R, by_key);
            for i in 0..6 {
                sort.push(rec(i, i)).expect("push");
            }
            assert_eq!(scratch.live.get(), 3);
            if finish {
                let mut stream = sort.finish().expect("finish");
                assert_eq!(key(&stream.next().expect("record").expect("read")), 0);
                assert!(scratch.live.get() > 0, "unfinished runs belong to the stream");
                drop(stream);
            } else {
                drop(sort);
            }
            assert_eq!(scratch.live.get(), 0);
            assert_eq!(scratch.inner.resident_bytes(), 0);
        }
    }

    #[test]
    fn scratch_failures_remove_existing_and_partially_written_runs() {
        for failure in ["push", "finish write", "finish read"] {
            let scratch = Counting::new();
            let mut sort = ExternalSort::<R>::new(&scratch, 4 * R, by_key);
            for i in 0..3 {
                sort.push(rec(i, i)).expect("push");
            }
            assert_eq!(scratch.live.get(), 1, "one run and one buffered record");
            scratch.fail_append.set(failure != "finish read");
            scratch.fail_read.set(failure == "finish read");
            let result = if failure == "push" {
                let result = sort.push(rec(3, 3));
                assert_eq!(scratch.live.get(), 1, "the partial run is removed immediately");
                drop(sort);
                result
            } else {
                sort.finish().map(drop)
            };
            let error = result.expect_err(failure).to_string();
            assert!(error.contains(if failure == "finish read" { "read refused" } else { "append refused" }));
            assert_eq!(scratch.live.get(), 0, "{failure} left a run behind");
            assert_eq!(scratch.inner.resident_bytes(), 0);
        }
    }

    /// The mid-stream eviction. On a sequential workload — push order equals key order, which is
    /// what a collection-index key is exactly and a spatial key nearly — runs drain one after
    /// another, so their files must die during the merge and not when the stream drops. Two chained
    /// sorts otherwise hold both passes' spill at once.
    #[test]
    fn a_drained_runs_file_dies_mid_stream_not_at_drop() {
        let scratch = Counting::new();
        // A budget of 8 records → runs of 4 → 64 sequential records spill 16 runs.
        let mut sort = ExternalSort::<R>::new(&scratch, 8 * R * RUN_SHARE, by_key);
        for i in 0..64u32 {
            sort.push(rec(i, i)).expect("push");
        }
        let mut stream = sort.finish().expect("finish");
        let total = scratch.live.get();
        assert!(total >= 8, "the fixture must actually spill ({total} runs)");
        // Halfway through a sequential stream, about half the runs are drained — and drained means
        // deleted. One run of slack for the record in flight at the boundary.
        for _ in 0..32 {
            stream.next().expect("a record").expect("ok");
        }
        assert!(
            scratch.live.get() <= total / 2 + 1,
            "{} of {total} runs still alive at the halfway point — drained runs are not being evicted",
            scratch.live.get()
        );
        // The rest of the stream still reads correctly after its predecessors' files are gone…
        let rest: Vec<u32> = stream.by_ref().map(|r| key(&r.expect("ok"))).collect();
        assert_eq!(rest, (32..64u32).collect::<Vec<_>>());
        // …a fully consumed stream has deleted everything itself…
        assert_eq!(scratch.live.get(), 0, "a consumed stream left runs behind");
        // …and the drop after full consumption double-removes nothing. The seam allows a second
        // remove to refuse, but the eviction must not rely on that.
        drop(stream);
        assert_eq!(scratch.live.get(), 0);
    }
}
