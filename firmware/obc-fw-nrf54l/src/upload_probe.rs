//! **Diagnostic probe for the staged USB map upload** — investigation scaffolding, not product.
//!
//! One atomic scoreboard, filled from three places while a staged upload is active and printed
//! **once** per upload from the v4 driver's end-of-upload rate line:
//!
//! - the USB driver loop (receive wait / arena copy / storage round-trip split, endpoint read
//!   size histogram),
//! - `FlatCard::write`'s staged branch (deferred-DMA join time vs. CMD25 setup time, per-write
//!   size histogram),
//! - every *other* card command that runs during the window (each one force-joins the in-flight
//!   write DMA, so their count is pipeline-break evidence).
//!
//! Everything is `AtomicU32` because thumbv8m has no 64-bit atomics; per-upload sums fit easily
//! (µs sums cap at ~71 minutes, byte counts at 4 GiB). All accesses are `Relaxed` — the numbers
//! are a report, not a synchronization edge.
//!
//! **Death trigger:** this module dies with the USB throughput investigation (see the branch's
//! PR); nothing but the probe summary may ever depend on it.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use embassy_time::Instant;

/// Whether a staged USB map upload is in flight — the gate for every passive hook.
static ACTIVE: AtomicBool = AtomicBool::new(false);

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        $(static $name: AtomicU32 = AtomicU32::new(0);)*
        fn reset_counters() {
            $($name.store(0, Relaxed);)*
        }
    };
}

counters!(
    // ── storage side: the staged (deferred-DMA) card writes ──
    STAGED_WRITES,
    STAGED_BYTES,
    STAGED_64K,
    STAGED_PARTIAL,
    FINISH_US,
    FINISH_MAX_US,
    START_US,
    // ── storage side: everything else that touched the card during the window ──
    OTHER_WRITES,
    OTHER_WRITE_BYTES,
    READS,
    READ_BYTES,
    READ_US,
    // ── engine side ──
    BATCHES,
    BATCH_SERVE_US,
    // ── USB driver side ──
    RECORDS,
    RECV_WAIT_US,
    COPY_US,
    BATCH_CALL_US,
    // ── endpoint read sizes ──
    EP_READS,
    EP_READ_BYTES,
    EP_READ_8192,
    EP_READ_BLOCKS,
    EP_READ_SHORT,
    EP_READ_AWAIT_US,
    // ── endpoint read await distribution (full 8,192 B reads only) ──
    EP_AWAIT_LT_300US,
    EP_AWAIT_LT_700US,
    EP_AWAIT_LT_1500US,
    EP_AWAIT_LT_3MS,
    EP_AWAIT_GE_3MS,
    EP_AWAIT_MAX_US,
    // ── maxima for the driver-side sums ──
    RECV_WAIT_MAX_US,
    BATCH_CALL_MAX_US,
    COPY_MAX_US,
    // ── the in-upload memcpy calibration ──
    CAL_COPIES,
    CAL_US,
    // ── copy triangulation: same payload, split by which side is the real one ──
    CAL_SRC_US,
    CAL_DST_US,
    CAL_TRI_N,
    SRC_PTR_ALIGN4,
    SRC_PTR_UNALIGNED,
);

/// Arm the probe at the instant the stage is granted. Clears the scoreboard.
pub(crate) fn begin() {
    reset_counters();
    ACTIVE.store(true, Relaxed);
}

fn active() -> bool {
    ACTIVE.load(Relaxed)
}

fn add(counter: &AtomicU32, value: u32) {
    counter.fetch_add(value, Relaxed);
}

fn max(counter: &AtomicU32, value: u32) {
    counter.fetch_max(value, Relaxed);
}

fn us(since: Instant) -> u32 {
    u32::try_from(since.elapsed().as_micros()).unwrap_or(u32::MAX)
}

/// One staged (deferred-DMA) card write: `finish_write_blocks` join time, CMD25 setup time, size.
pub(crate) fn staged_write(bytes: usize, finish_us: u32, start_us: u32) {
    add(&STAGED_WRITES, 1);
    add(&STAGED_BYTES, bytes as u32);
    add(if bytes == crate::usb::STAGE_HALF_LEN { &STAGED_64K } else { &STAGED_PARTIAL }, 1);
    add(&FINISH_US, finish_us);
    max(&FINISH_MAX_US, finish_us);
    add(&START_US, start_us);
}

/// A synchronous (non-staged) card write during the window.
pub(crate) fn other_write(bytes: usize) {
    if active() {
        add(&OTHER_WRITES, 1);
        add(&OTHER_WRITE_BYTES, bytes as u32);
    }
}

/// A card read during the window. Every one of these joined the in-flight write DMA first.
pub(crate) fn read(bytes: usize, started: Instant) {
    if active() {
        add(&READS, 1);
        add(&READ_BYTES, bytes as u32);
        add(&READ_US, us(started));
    }
}

/// One `StreamStagedBatch` served, and the time `serve` spent on it (storage-task side).
pub(crate) fn batch_served(started: Instant) {
    add(&BATCHES, 1);
    add(&BATCH_SERVE_US, us(started));
}

/// Time the driver spent awaiting the next record while a staged upload was live.
pub(crate) fn recv_wait(started: Instant) {
    if active() {
        let wait = us(started);
        add(&RECV_WAIT_US, wait);
        max(&RECV_WAIT_MAX_US, wait);
    }
}

/// Time the driver waited to collect a deferred batch's answer at a bank's first record.
pub(crate) fn batch_wait(started: Instant) {
    let wait = us(started);
    add(&BATCH_CALL_US, wait);
    max(&BATCH_CALL_MAX_US, wait);
}

/// Driver-side accounting for one staged stream record: arena copy time and, on a bank boundary,
/// the `StreamStagedBatch` hand-off (send for a deferred mid batch, the full round trip for the
/// final one; the deferred collect is [`batch_wait`]).
pub(crate) fn staged_record(copy_us: u32, batch_call_us: u32) {
    let n = RECORDS.fetch_add(1, Relaxed);
    add(&COPY_US, copy_us);
    max(&COPY_MAX_US, copy_us);
    add(&BATCH_CALL_US, batch_call_us);
    max(&BATCH_CALL_MAX_US, batch_call_us);
    if n.is_multiple_of(128) {
        calibrate_copy();
    }
}

/// Triangulate one real record copy, sampled by the caller: time `payload` into the probe's own
/// scratch (real source, plain destination), then scratch into the arena slot the record is about
/// to overwrite anyway (plain source, real destination). Also tally the payload pointer's
/// alignment on every record. The bank slot is garbage after this — the caller's real copy is what
/// fills it, so ordering (triangulate first) is what makes this safe.
pub(crate) fn copy_triangulate(payload: &[u8], bank: usize, fill: usize, sample: bool) {
    add(if (payload.as_ptr() as usize).is_multiple_of(4) { &SRC_PTR_ALIGN4 } else { &SRC_PTR_UNALIGNED }, 1);
    if !sample || payload.len() != 8_192 {
        return;
    }
    static mut SCRATCH: [u8; 8_192] = [0; 8_192];
    // SAFETY: one caller, the USB driver task; the scratch exists for nothing else.
    let scratch = unsafe { &mut *core::ptr::addr_of_mut!(SCRATCH) };
    let started = Instant::now();
    scratch.copy_from_slice(payload);
    core::hint::black_box(&mut scratch[0]);
    let src_us = us(started);
    let started = Instant::now();
    let dst_us = crate::arena::with_usb_stage_bank(bank, |slot| {
        slot[fill..fill + 8_192].copy_from_slice(&*scratch);
        core::hint::black_box(&mut slot[fill]);
        us(started)
    })
    .unwrap_or(0);
    add(&CAL_SRC_US, src_us);
    add(&CAL_DST_US, dst_us);
    add(&CAL_TRI_N, 1);
}

/// One completed bulk OUT endpoint read, and how long the pump awaited it.
pub(crate) fn ep_read(bytes: usize, started: Instant) {
    if !active() {
        return;
    }
    add(&EP_READS, 1);
    add(&EP_READ_BYTES, bytes as u32);
    add(
        match bytes {
            8_192 => &EP_READ_8192,
            n if n.is_multiple_of(512) => &EP_READ_BLOCKS,
            _ => &EP_READ_SHORT,
        },
        1,
    );
    let await_us = us(started);
    add(&EP_READ_AWAIT_US, await_us);
    if bytes == 8_192 {
        add(
            match await_us {
                0..=299 => &EP_AWAIT_LT_300US,
                300..=699 => &EP_AWAIT_LT_700US,
                700..=1_499 => &EP_AWAIT_LT_1500US,
                1_500..=2_999 => &EP_AWAIT_LT_3MS,
                _ => &EP_AWAIT_GE_3MS,
            },
            1,
        );
        max(&EP_AWAIT_MAX_US, await_us);
    }
}

/// One 8 KiB memcpy between two probe-owned scratch buffers, timed under whatever interrupt load
/// is live at that moment of the upload. Calibrates the staged-record copy times: a slow
/// *calibration* means ambient ISR/DMA load, a slow staged copy over a fast calibration means the
/// staged path itself.
fn calibrate_copy() {
    static mut SCRATCH_A: [u8; 8_192] = [0; 8_192];
    static mut SCRATCH_B: [u8; 8_192] = [0; 8_192];
    let started = Instant::now();
    // SAFETY: only ever entered from the one USB driver task (via `staged_record`), never
    // reentrantly; the buffers exist for nothing else.
    unsafe {
        let a = &*core::ptr::addr_of!(SCRATCH_A);
        let b = &mut *core::ptr::addr_of_mut!(SCRATCH_B);
        b.copy_from_slice(a);
        core::hint::black_box(&mut b[0]);
    }
    add(&CAL_COPIES, 1);
    add(&CAL_US, us(started));
}

/// Print the whole scoreboard — once, from the driver's end-of-upload rate line — and disarm.
pub(crate) fn finish() {
    if !ACTIVE.swap(false, Relaxed) {
        return;
    }
    let get = |c: &AtomicU32| c.load(Relaxed);
    defmt::info!(
        "usb: [probe] card: {=u32} staged writes ({=u32} full 64K, {=u32} partial, {=u32} B), \
         finish {=u32} ms (max {=u32} us), start {=u32} ms; other writes {=u32} ({=u32} B); \
         reads {=u32} ({=u32} B, {=u32} ms)",
        get(&STAGED_WRITES),
        get(&STAGED_64K),
        get(&STAGED_PARTIAL),
        get(&STAGED_BYTES),
        get(&FINISH_US) / 1_000,
        get(&FINISH_MAX_US),
        get(&START_US) / 1_000,
        get(&OTHER_WRITES),
        get(&OTHER_WRITE_BYTES),
        get(&READS),
        get(&READ_BYTES),
        get(&READ_US) / 1_000,
    );
    defmt::info!(
        "usb: [probe] engine: {=u32} batches, serve {=u32} ms; driver: {=u32} records, \
         recv-wait {=u32} ms (max {=u32} us), copy {=u32} ms (max {=u32} us), \
         batch-call {=u32} ms (max {=u32} us); cal {=u32} copies avg {=u32} us",
        get(&BATCHES),
        get(&BATCH_SERVE_US) / 1_000,
        get(&RECORDS),
        get(&RECV_WAIT_US) / 1_000,
        get(&RECV_WAIT_MAX_US),
        get(&COPY_US) / 1_000,
        get(&COPY_MAX_US),
        get(&BATCH_CALL_US) / 1_000,
        get(&BATCH_CALL_MAX_US),
        get(&CAL_COPIES),
        get(&CAL_US) / get(&CAL_COPIES).max(1),
    );
    defmt::info!(
        "usb: [probe] copy triangulation: {=u32} samples, payload->scratch avg {=u32} us, \
         scratch->arena avg {=u32} us; src ptr aligned4={=u32} unaligned={=u32}",
        get(&CAL_TRI_N),
        get(&CAL_SRC_US) / get(&CAL_TRI_N).max(1),
        get(&CAL_DST_US) / get(&CAL_TRI_N).max(1),
        get(&SRC_PTR_ALIGN4),
        get(&SRC_PTR_UNALIGNED),
    );
    defmt::info!(
        "usb: [probe] ep: {=u32} reads ({=u32} B): 8192={=u32} block-multiple={=u32} short={=u32}; \
         await {=u32} ms",
        get(&EP_READS),
        get(&EP_READ_BYTES),
        get(&EP_READ_8192),
        get(&EP_READ_BLOCKS),
        get(&EP_READ_SHORT),
        get(&EP_READ_AWAIT_US) / 1_000,
    );
    defmt::info!(
        "usb: [probe] ep await (8192 reads): <300us={=u32} <700us={=u32} <1.5ms={=u32} <3ms={=u32} \
         >=3ms={=u32} max={=u32} us",
        get(&EP_AWAIT_LT_300US),
        get(&EP_AWAIT_LT_700US),
        get(&EP_AWAIT_LT_1500US),
        get(&EP_AWAIT_LT_3MS),
        get(&EP_AWAIT_GE_3MS),
        get(&EP_AWAIT_MAX_US),
    );
    let (woken, timeout_published, timeout_fires, isr_xfrc_out) = embassy_usb_synopsys_otg::read_probe::take();
    defmt::info!(
        "usb: [probe] driver completions: woken={=u32} timeout-published={=u32} timeout-fires={=u32} \
         isr-xfrc-out={=u32}",
        woken,
        timeout_published,
        timeout_fires,
        isr_xfrc_out,
    );
}
