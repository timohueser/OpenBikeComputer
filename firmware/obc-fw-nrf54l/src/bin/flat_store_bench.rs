//! **Flat store bench** — the whole of `obc_storage::flat` on the real card (FS4, #1386).
//!
//!     cargo run --release --bin flat_store_bench
//!
//! `FLAT_Store_Format.md` states five figures the board is the only place to check: a first boot
//! "well under a second", a mount "about 100 ms", a commit "about 15–20 ms at a few hundred
//! entries", a read path that is arithmetic rather than a chain walk, and a resident cost of the
//! 8 KiB free bitmap plus the ride tail. This binary produces a measured number for each of them
//! against a card in the slot, in the shape #1375 and #1379 established for OBC2: RTT-driven,
//! destructive, self-reporting, and never shipped — the app image does not link it.
//!
//! It brings up only the sEMMC card: no display, no app, no BLE, no sensors. The store owns the
//! **raw card from LBA 0** — there is no partition table and no filesystem — so a run destroys
//! whatever was on it, including a FAT volume. It refuses to touch a card that already carries a
//! flat store under someone else's `StoreId`.
//!
//! ## What it measures, in the order it runs
//!
//! Phase one, on a card this bench may destroy:
//!
//! 1. **Initialization (§8).** Two superblocks, one gate, sixteen slot headers, one empty catalog.
//! 2. **Mount (§5.6)**, at four catalog shapes: empty, [`LADDER_MID`] entries, [`LADDER_TOP`]
//!    entries, and with a ride recording (which is the only shape that reads the journal).
//! 3. **The commit ladder (§5.5).** One create per step, and at 0, [`LADDER_MID`] and
//!    [`LADDER_TOP`] entries a create/remove pair repeated [`COMMIT_SAMPLES`] times, so the reported
//!    figure carries a spread rather than being one observation. Also [`measure_opens`]: what §5.3's
//!    lookup costs twelve times over, which is a rendered set coming up.
//! 4. **The ride journal (§7.2).** One checkpoint every [`RIDE_GROWTH`] bytes, timed, split by
//!    whether it flushed a payload page.
//! 5. **The read path (§6.1)** into a multi-GiB object: one sequential sweep and three random
//!    passes, each with the **read amplification** — device blocks read over payload blocks
//!    required — which is the flat store's version of #1379's read-ratio check.
//! 6. **Resident cost** as a build assertion (see [`RESIDENT`]), plus the stack high-water the whole
//!    run reached.
//!
//! Every timed figure is reported through [`report_split`]: the card's write half, its read half and
//! the M33's residue, measured *inside* the block-device adapter. A commit is not one number — at 300
//! entries it writes 79 blocks and reads 156, and attributing the reads to the program cycle is how
//! the first round of this bench got its headline wrong.
//!
//! Phase two, after `probe-rs reset`, on the ride phase one left recording:
//!
//! 7. **Recovery (§7.3)**, through the store's own [`FlatStore::recovered_ride`] — not a
//!    reimplementation of it — and then §7.2's ride end, after which the ride is read back byte for
//!    byte against the payload phase one generated.
//!
//! ## What it does NOT prove — read this before quoting the results
//!
//! A `probe-rs reset` is a **CPU reset, not a power cut**. The card keeps its supply and never sees
//! the mid-page interruption §1's fault model is about, so nothing here says anything about tearing;
//! that is FS1's rig (#1383). What the reset loop validates is that a mount reconstructs the catalog
//! and the ride from the card alone, which is a different claim and the one this bench is for.
//!
//! ## Bring-up
//!
//! `semmc.rs` is pulled in by path and has no `crate::` dependencies, so this binary owns its own
//! host instance and never touches the display mux. The M33 must be at CK128 and `VPR00` bound,
//! exactly as in `obc2_store_bench`.
//!
//! # Serial map ingest — the board-acceptance path (FS7.5)
//!
//! A board session needs a **real packed map** on a real flat store, and on this rig there is no
//! transport that can put one there: USB is still protocol v2, BLE v4's phone client is not ready,
//! and the host has no card reader. So this bench carries one, in the only place the
//! bench-separation rule allows it — here, in a binary the app image never links.
//!
//! It is a **mode, not a phase**: before any measurement runs, the bench advertises on the DK's
//! VCOM UART for [`INGEST_WINDOW_MS`]. If a host answers, it ingests objects until the host stops
//! and then parks — the destructive measurement suite never runs, which is the point (it would wipe
//! the map that was just written). With nothing on the other end the window expires and the bench is
//! exactly what it was.
//!
//! ## The wire, in full
//!
//! Little-endian, CRC-32/IEEE, four frames. `OBCI` in both directions.
//!
//! | Frame | Dir | Bytes | Layout |
//! | :-- | :-- | --: | :-- |
//! | READY | D→H | 14 | magic, version, `'R'`, chunk size `u32`, CRC over `0..10` |
//! | GONE | D→H | 14 | magic, version, `'G'`, reason `u32` ([`gone`]), CRC over `0..10` |
//! | HEADER | H→D | 72 | magic, version, `'H'`, kind (§3.1), name len, payload len `u64`, payload CRC `u32`, 48-byte zero-padded name, CRC over `0..68` |
//! | STATUS | D→H | 2 | `0x06` ACK / `0x15` NAK, then a reason byte ([`reason`]) |
//! | RESULT | D→H | 42 | magic, version, `'D'`/`'E'`, reason, pad, `ObjectId` `u64`, `Revision` `u64`, payload len `u64`, device-computed CRC `u32`, entry count `u16`, CRC over `0..38` |
//!
//! GONE is READY's shape with another tag, so a host blocked waiting for a READY decodes it with the
//! code it already has. Every path out of the ingest sends one, including the path into the
//! destructive measurement run — a host that arrived a second late learns that from the device
//! rather than from a timeout it has to attribute itself.
//!
//! READY repeats about twice a second, and **only after the line has been quiet** for one interval,
//! so an advertisement can never land on top of a host's burst. A STATUS answers the HEADER and
//! every chunk; a RESULT closes the object out after the last one, so exactly one frame ends a
//! transfer and the host never has to guess which. The payload is `ceil(len / chunk)` chunks — every
//! one full but the last, both sides computing the same lengths, so no chunk carries a length field.
//! Each chunk is acked *after* it is in the reservation, which is what paces the host: there is no
//! flow control on this cable and none is gambled on. The device folds its own CRC as it goes and
//! compares it with the header's **before** committing, which is the last moment it still holds the
//! `Allocation`: a mismatch, a link fault or a refused chunk all `cancel`, so the attempt publishes
//! nothing, gives its extents back, and the next HEADER is a fresh put with a new `ObjectId`.
//!
//! **One failure is not like the others.** A refused *commit* has already taken the `Allocation` by
//! value, and §5.5 clears the reservation row only once its gate write lands — so those extents stay
//! held with nothing left to cancel them, and only a remount frees them. `MAX_RESERVATIONS` is 2, so
//! a session that carried on would wedge on its third object. The device therefore **ends the
//! session** after a commit refusal and says so on both the wire and RTT: reset, and the mount
//! reclaims.
//!
//! ## When it refuses to fall through
//!
//! A window that ends with the UARTE reporting errors is **not** a quiet line, and the bench will not
//! run the measurements after one. Something is transmitting bytes this device cannot decode — a
//! baud mismatch is by far the likeliest — which means a host is probably at the other end believing
//! it is sending a map, and phase one would destroy the card it is aimed at. The RTT log names the
//! configured baud so the mismatch is one line to spot.
//!
//! ## The board session
//!
//! ```text
//! # 1. the host waits for the device (start it first — it blocks on READY)
//! python3 tools/bench_ingest.py --port /dev/cu.usbmodem*133 \
//!     --file "$(python3 tools/fixtures.py resolve monaco-upahead | awk '/^map/ {print $2}')" \
//!     --kind map --name monaco.obcm
//!
//! # 2. flash + run the bench (a second shell). The committed runner already carries `--verify`.
//! pkill probe-rs
//! cd firmware/obc-fw-nrf54l
//! cargo run --release --bin flat_store_bench
//! ```
//!
//! Keep the read-back check: `.cargo/config.toml` sets the runner to `probe-rs run --chip
//! nRF54LM20A --verify` because probe-rs 0.31 corrupts the first RRAM write after a code change
//! often enough to matter, and on this part that is a boot HardFault at a random PC. (`cargo run
//! --verify` is not a thing — the flag is in the runner, not in cargo's arguments.) To flash and
//! attach separately, `probe-rs download --chip nRF54LM20A --verify <elf>` then `probe-rs run`.
//!
//! `sim-monaco`'s `monaco.obcm` is 718,336 bytes, which at [`INGEST_BAUD`]'s 115,200 8N1 is **about
//! 63 s** on the wire (10 bits a byte, plus ~2 ms of USB turnaround per 8 KiB chunk). Raising
//! [`INGEST_BAUD`] to `Baudrate::Baud1m` takes that to about 7.5 s and needs `--baud 1000000` on the
//! host; 115,200 is the default because it is the rate this rig's VCOM is *proven* at, and a board
//! session is not the place to find out that a J-Link CDC will not do a megabaud. A host that gets
//! this wrong is not left guessing: the device sees undecodable bytes, ends the window
//! [`Advertised::Erroring`], and names its own baud on RTT.
//!
//! When the host reports no device, the [`TAG_GONE`] frame it printed says which of four unrelated
//! things happened — the window closed and the destructive run is starting, the session ended, a
//! commit refusal is holding extents, or the line was erroring. **Only when no GONE arrives at all**
//! and RTT shows the bench advertising is it the J-Link VCOM wedge, whose one fix is a physical
//! power-cycle of the DK. (A foreign `StoreId` is a fifth case that never reaches the wire: `run`
//! refuses before the ingest is offered, and only RTT says so.)
#![no_std]
#![no_main]

use core::future::Future;
use core::task::{Context, Poll, Waker};

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::uarte::{self, Baudrate, Uarte, UarteRx, UarteTx};
use embassy_time::{Duration, Instant};
use obc_crc::Crc32;
use obc_storage::flat::store::{MAX_OPEN_OBJECTS, MAX_RESERVATIONS};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectId, ObjectKind,
    PutSource, Revision, RideCheckpoint, RideRecovery, Store as _, StoreId,
};

// The critical-section impl comes from linking nrf-mpsl (the default `ble` feature set); MPSL is
// never initialised here, and its impl works from reset — the same arrangement the OBC2 benches use.
use nrf_mpsl as _;
use {defmt_rtt as _, panic_probe as _};

#[allow(dead_code)]
#[path = "../semmc.rs"]
mod semmc;

use semmc::{Semmc, SemmcError, BLOCK_BYTES};

/// sEMMC completion event: VEVIF event 20 is routed to `VPR00_IRQn` by the FLPR firmware.
#[interrupt]
unsafe fn VPR00() {
    semmc::on_vpr00_irq();
}

// The DK's VCOM, for the serial ingest below. `board.rs` owns the same nets for the app's debug
// link; this binary links none of it, so it binds its own — the same arrangement as `VPR00` above.
embassy_nrf::bind_interrupts!(struct UartIrqs {
    SERIAL20 => uarte::InterruptHandler<embassy_nrf::peripherals::SERIAL20>;
});

// ── the format constants this bench needs ───────────────────────────────────────────────────────
//
// `obc_storage::flat::layout` is `pub(crate)` — an LBA, an extent and a program page are the store's
// business and nothing above the seam may name them. A bench sits beside the store rather than above
// it, so it restates the four it needs from `FLAT_Store_Format.md` §2, §6, §7.1 and §9 rather than
// widening the seam for instrumentation. Each one is checked against the store's own behaviour by the
// measurements below: a wrong constant here shows up as an allocation the store refuses or a
// checkpoint that flushes a different number of pages than this bench predicted.

/// One extent (§6).
const EXTENT_SIZE: u64 = 1 << 20;
/// The extent area begins here, in blocks (§6).
const EXTENT_AREA: u64 = 4_096;
/// The media program page, and the granule §7.2 flushes ride payload in (§1, §2).
const PROGRAM_PAGE: usize = 16_384;
/// Tail bytes one journal slot carries (§7.1) — the ceiling a recording caller's buffer must meet.
const TAIL_CAPACITY: usize = 32_256;
/// The resident free bitmap (§9). Named here only to decompose the footprint report.
const FREE_BITMAP: usize = 8 * 1_024;

// ── the plan figures this bench is checking ─────────────────────────────────────────────────────

/// §5.6: "boot is about 100 ms".
const PLAN_BOOT_US: u64 = 100_000;
/// §5.5: "about 15–20 ms at a few hundred entries". The upper end, so a pass is unambiguous.
const PLAN_COMMIT_US: u64 = 20_000;
/// §8's initialization: "well under 1 s" (#1386).
const PLAN_INIT_US: u64 = 1_000_000;
/// The epic's resident figure: §9's 8 KiB bitmap, the 32,256-byte ride tail a recording caller
/// holds, and the store's rows — about 42 KiB together.
const PLAN_RESIDENT: usize = 42 * 1_024;

// ── the bench's own shape ───────────────────────────────────────────────────────────────────────

/// The bench's StoreId. A real initialization draws 128 CSPRNG bits (§4); a fixed value here makes a
/// store from an earlier run recognisable as this bench's rather than a live one's.
const BENCH_STORE: StoreId =
    StoreId([0xF5, 0x04, 0xF1, 0xA7, 0x5B, 0xE2, 0x11, 0x30, 0x9C, 0x64, 0xAD, 0x77, 0x02, 0xEE, 0x38, 0x41]);

/// Flip to `true` for one flash to wipe a card that carries **another** store's `StoreId`, or to
/// force phase one over this bench's own recorded ride instead of recovering it.
const FORCE_REINIT: bool = false;

/// Where the commit ladder reports its middle figure — §5.5's "a few hundred entries".
const LADDER_MID: u16 = 300;
/// Where it stops. One commit runs with the catalog holding exactly this many entries.
const LADDER_TOP: u16 = 1_024;
/// One ladder object's payload. One block, so the object costs the minimum §6 can allocate — one
/// extent — and the ladder's card cost is `LADDER_TOP` MiB rather than its payload.
const LADDER_PAYLOAD: usize = 512;

/// Samples taken at each reported catalog size. Three is enough to show whether a figure is stable;
/// the same commit moved ten per cent between two runs of the first round of this bench.
const COMMIT_SAMPLES: usize = 3;

/// The ride reserve §9 budgets: 32 MiB.
const RIDE_RESERVE: u64 = 32 * EXTENT_SIZE;
/// Payload bytes one checkpoint interval adds. A recorded ride is a few hundred bytes a second and
/// §9's cadence is 10 s, so this is one interval of a real ride rounded to something a reader can
/// multiply: nine checkpoints fill a program page, so the page flush lands mid-run rather than on a
/// boundary the bench chose.
const RIDE_GROWTH: usize = 2_048;
/// Checkpoints phase one writes. Enough to flush several payload pages and to wrap past the sixteen
/// slots, so the slot §7.3 selects is not the first one written — and deliberately not a multiple of
/// eight, so the ride is left with a partial page in its slot and phase two's ride end has to move
/// those bytes into the extents rather than finding everything already flushed.
const RIDE_CHECKPOINTS: u64 = 23;

/// What phase one's ride adds up to, from the constants it recorded with. Phase two anchors every
/// expectation here and never on what recovery reported — see the comment in [`phase_two`].
const RIDE_LEN: u64 = RIDE_CHECKPOINTS * RIDE_GROWTH as u64;
/// Payload bytes §7.2 will have flushed into the ride's own extents at that point.
const RIDE_FLUSHED: u64 = RIDE_LEN / PROGRAM_PAGE as u64 * PROGRAM_PAGE as u64;
/// And what is left in the newest journal slot: deliberately not zero.
const RIDE_TAIL_LEN: u32 = (RIDE_LEN - RIDE_FLUSHED) as u32;

/// The read-path object, if the card has room for it. `FLAT_Store_Format.md` §6.1's addressing is
/// arithmetic over 1 MiB extents, and only an object spanning thousands of them exercises it.
const BIG_TARGET: u64 = 2 * 1_024 * EXTENT_SIZE;
/// Below this the bench scales the object down and says so rather than skipping the read path.
const BIG_MINIMUM: u64 = 64 * EXTENT_SIZE;
/// One `Store::write` / `Store::read` call's span. 64 blocks per device command.
const CHUNK: usize = 32 * 1_024;
/// Random reads per pass.
const RANDOM_READS: u32 = 512;
/// One random read's length.
const RANDOM_LEN: usize = 4_096;

// ── the card ────────────────────────────────────────────────────────────────────────────────────

/// The one sEMMC host. Single-threaded and never re-entered, which is what makes the `&mut` sound.
static mut SEMMC: Semmc = Semmc::new();

/// A 4-byte-aligned byte buffer. The sEMMC firmware's DMA requires 32-bit alignment.
#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// The misaligned-span bounce. The store hands the card `[u8; 512]` locals and a 4,096-byte pad out
/// of rodata, neither of which carries an alignment attribute, so every buffer the driver would
/// refuse comes through here.
static mut BOUNCE: Aligned<4_096> = Aligned([0; 4_096]);

/// What the card was asked to do. `reads` and `writes` are calls; the `_blocks` fields are what those
/// calls covered, which is what makes an amplification ratio computable rather than inferred.
#[derive(Clone, Copy, Default)]
struct Counters {
    reads: u32,
    read_blocks: u32,
    /// Microseconds spent inside the driver on those reads.
    read_us: u64,
    writes: u32,
    write_blocks: u32,
    /// Microseconds spent inside the driver on those writes.
    write_us: u64,
    syncs: u32,
}

static mut COUNTERS: Counters =
    Counters { reads: 0, read_blocks: 0, read_us: 0, writes: 0, write_blocks: 0, write_us: 0, syncs: 0 };

/// Everything the card has been asked to do since [`arm`].
fn counters() -> Counters {
    // SAFETY: single-threaded, and no interrupt handler touches this.
    unsafe { *core::ptr::addr_of!(COUNTERS) }
}

/// Zeroes the counters. Named for what a caller does with it: arm, run one thing, read the counters.
fn arm() {
    // SAFETY: as above.
    unsafe { *core::ptr::addr_of_mut!(COUNTERS) = Counters::default() };
}

/// The [`BlockDevice`] over the sEMMC host: zero-sized, because all the state is in [`SEMMC`], which
/// is what lets the counters be read while a [`FlatStore`] owns the device by value.
#[derive(Clone, Copy)]
struct Card;

impl Card {
    /// SAFETY: the caller must not be inside another `with` — this binary never is.
    fn with<R>(f: impl FnOnce(&mut Semmc) -> R) -> R {
        // SAFETY: single-threaded, non-re-entrant, and no interrupt handler touches the host state.
        f(unsafe { &mut *core::ptr::addr_of_mut!(SEMMC) })
    }

    fn count(f: impl FnOnce(&mut Counters)) {
        // SAFETY: as above.
        f(unsafe { &mut *core::ptr::addr_of_mut!(COUNTERS) })
    }

    /// The card addresses blocks in a `u32`; the store's seam is a `u64`.
    fn lba(lba: u64) -> Result<u32, SemmcError> {
        u32::try_from(lba).map_err(|_| SemmcError::OutOfRange)
    }
}

impl BlockDevice for Card {
    type Error = SemmcError;

    fn block_count(&self) -> Result<u64, SemmcError> {
        Card::with(|sd| sd.num_blocks()).map(u64::from)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), SemmcError> {
        let start = Card::lba(lba)?;
        let blocks = (buf.len() / BLOCK_BYTES) as u32;
        // The stopwatch is around the driver call and nothing else. An interval measured above the
        // seam is the card *and* the M33, and the two have different fixes: attributing one to the
        // other is exactly the error this bench's first round made.
        let started = Instant::now();
        let outcome = Card::with(|sd| {
            if (buf.as_ptr() as usize).is_multiple_of(4) {
                return sd.read_blocks(start, buf);
            }
            // SAFETY: sole borrow; nothing else touches the bounce inside this call.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            let mut done = 0usize;
            while done < buf.len() {
                let take = (buf.len() - done).min(bounce.0.len());
                sd.read_blocks(start + (done / BLOCK_BYTES) as u32, &mut bounce.0[..take])?;
                buf[done..done + take].copy_from_slice(&bounce.0[..take]);
                done += take;
            }
            Ok(())
        });
        let elapsed = us(started);
        Card::count(|c| {
            c.reads += 1;
            c.read_blocks += blocks;
            c.read_us += elapsed;
        });
        outcome
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), SemmcError> {
        let start = Card::lba(lba)?;
        let blocks = (buf.len() / BLOCK_BYTES) as u32;
        let started = Instant::now();
        let outcome = Card::with(|sd| {
            if (buf.as_ptr() as usize).is_multiple_of(4) {
                return sd.write_blocks(start, buf);
            }
            // SAFETY: as in `read`.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            let mut done = 0usize;
            while done < buf.len() {
                let take = (buf.len() - done).min(bounce.0.len());
                bounce.0[..take].copy_from_slice(&buf[done..done + take]);
                sd.write_blocks(start + (done / BLOCK_BYTES) as u32, &bounce.0[..take])?;
                done += take;
            }
            Ok(())
        });
        let elapsed = us(started);
        Card::count(|c| {
            c.writes += 1;
            c.write_blocks += blocks;
            c.write_us += elapsed;
        });
        outcome
    }

    /// **The one thing this transport gets for free.**
    ///
    /// `Semmc::write_blocks` does not return until CMD13 says the card has left `prg` — the program
    /// cycle *is* its completion signal — so every write is already durable by the time the store's
    /// next statement runs. There is nothing left for a sync to do, and it costs nothing.
    ///
    /// This matters for reading §5.5's commit budget, which calls the three synchronizations the
    /// dominant term: on this card they are free and the commit's whole cost is its block writes. A
    /// transport with a write-back cache would move that cost back here, and the numbers this bench
    /// reports for a commit would move with it. The syncs are counted so the report can say so.
    fn sync(&self) -> Result<(), SemmcError> {
        Card::count(|c| c.syncs += 1);
        Ok(())
    }
}

// ── the payloads ────────────────────────────────────────────────────────────────────────────────

/// The repeating pattern the read-path object is made of, and the buffer every `Store::write` of it
/// comes from.
static mut PATTERN: Aligned<CHUNK> = Aligned([0; CHUNK]);
/// Where a read comes back for the byte comparison.
static mut READBACK: Aligned<CHUNK> = Aligned([0; CHUNK]);
/// The recording caller's tail buffer: §7.1's ceiling, which is the figure §9's resident budget
/// carries and not the couple of pages a real ride is ever holding.
static mut RIDE_TAIL: Aligned<TAIL_CAPACITY> = Aligned([0; TAIL_CAPACITY]);

/// The ride's payload byte at `offset`. Deterministic, so the boot after a reset can regenerate
/// exactly what the boot before it recorded and compare byte for byte.
fn ride_byte(offset: u64) -> u8 {
    (offset.wrapping_mul(7).wrapping_add(11) ^ (offset >> 11)) as u8
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        // Not optional: the sEMMC clock divisors and the firmware's wait slices are stated against
        // a 128 MHz core.
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };
    // SAFETY: arming the vector before the soft peripheral boots is what `main.rs` does too.
    unsafe {
        interrupt::VPR00.set_priority(Priority::P1);
        interrupt::VPR00.enable();
    }
    stackmeter::arm_limit();
    stackmeter::paint();
    info!("flat_store_bench: obc_storage::flat on the real card ({=str})", env!("OBC_FW_GIT"));
    info!("flat_store_bench: DESTRUCTIVE — the store owns the raw card from LBA 0, partition table included");

    let card = match Card::with(|sd| sd.start()) {
        Ok(card) => card,
        Err(error) => {
            error!("flat_store_bench: the card did not come up ({}) — nothing measured", error);
            park();
        }
    };
    info!(
        "CARD  rca=0x{=u16:04x} blocks={=u32} ({=u64} MiB) high_speed={=bool} read_clk={=u32} Hz",
        card.rca,
        card.blocks,
        (card.blocks as u64) * 512 / (1024 * 1024),
        card.high_speed,
        card.read_clk_hz
    );
    // §6's extent count, from the card's block count. The store computes the same thing from the
    // superblock; a mismatch would show up as an allocation it refuses.
    let extents = ((u64::from(card.blocks).saturating_sub(EXTENT_AREA)) / (EXTENT_SIZE / 512)).min(65_536);
    info!("CARD  §6 extent area: {=u64} extents of 1 MiB ({=u64} MiB addressable)", extents, extents);

    run(p);
    info!("STACK high-water across the whole run: {=usize} B of {=usize} B", stackmeter::used(), stackmeter::total());
    park();
}

/// The whole run, in a plain function rather than in the async `main`.
///
/// Deliberate, and the #1379 lesson restated: an async fn's locals are permanent poll-frame slots,
/// and this one places stores whose type is ten kilobytes. In an ordinary call the same locals are
/// scoped to the frame and come back at return — which is what makes the stack figure this bench
/// prints a measurement of the store's peak rather than of the executor's permanent reservation.
#[inline(never)]
fn run(p: embassy_nrf::Peripherals) {
    report_footprint();

    let boot = measure_boot("SURVEY", Some(PLAN_BOOT_US));
    let ours = boot.mode.readable() && boot.store_id == BENCH_STORE;
    if boot.mode.readable() && !ours && !FORCE_REINIT {
        error!("BOOT  this card carries another store's StoreId — REFUSING to wipe it (set FORCE_REINIT)");
        return;
    }
    // Before anything destructive: offer the ingest. A session that took it does not get the
    // measurement run afterwards — phase one would allocate the whole card out from under the object
    // it just accepted, which is a card wiped between "map ingested" and the rider seeing it.
    if ingest_offer(p, &boot) {
        info!("INGEST session over — the measurement run is deliberately SKIPPED so the ingested objects survive");
        return;
    }
    if ours && boot.recording.is_some() && !FORCE_REINIT {
        phase_two(&boot);
        return;
    }
    phase_one();
}

// ── phase one ───────────────────────────────────────────────────────────────────────────────────

fn phase_one() {
    info!("PHASE one: initialize, then measure boot, commit, ride and read on a card built from scratch");

    let Some(extents) = initialize() else { return };

    measure_boot("BOOT  empty catalog", Some(PLAN_BOOT_US));

    // How much of the card the three phases may take. The ladder is one extent per object; the ride
    // takes §9's 32 MiB reserve; the read-path object takes whatever is left, up to 2 GiB.
    let ladder_top = if u32::from(LADDER_TOP) + 64 <= extents / 2 {
        LADDER_TOP
    } else {
        let scaled = (extents / 2).saturating_sub(64).min(u32::from(u16::MAX)) as u16;
        warn!("SIZE  the card has {=u32} extents — scaling the commit ladder to {=u16} entries", extents, scaled);
        scaled
    };
    let spare = u64::from(extents.saturating_sub(u32::from(ladder_top) + 40)) * EXTENT_SIZE;
    let big = BIG_TARGET.min(spare / EXTENT_SIZE * EXTENT_SIZE);

    let ladder_free = ladder(ladder_top);
    info!("LADDER left {=u32} free extents for the ride and the read-path object", ladder_free);
    ride();
    if big < BIG_MINIMUM {
        error!("READ  only {=u64} MiB spare after the ladder — the read path needs at least 64 MiB", big / EXTENT_SIZE);
    } else {
        if big < BIG_TARGET {
            warn!(
                "READ  SCALED: the card leaves {=u64} MiB, so the read-path object is {=u64} MiB rather than 2 GiB",
                spare / EXTENT_SIZE,
                big / EXTENT_SIZE
            );
        }
        read_path(big);
    }
    info!("PHASE one done. `probe-rs reset` to run phase two: the ride recovers from the card alone.");
}

/// §8: two superblocks invalidated, gate B invalidated, sixteen slot headers invalidated, one empty
/// catalog body, its gate, then both superblocks — five synchronization points, and the mount that
/// follows is part of what `initialize` returns. Hands back the free extents the card came up with.
///
/// It is a function of its own so the store it builds goes out of scope with it: a boot figure has to
/// be a mount's own cost, and a store left standing here would sit on the stack for the whole run.
#[inline(never)]
fn initialize() -> Option<u32> {
    arm();
    let started = Instant::now();
    let store = match FlatStore::initialize(Card, BENCH_STORE) {
        Ok(store) => store,
        Err(error) => {
            error!("INIT  §8 initialization failed ({})", defmt::Debug2Format(&error));
            return None;
        }
    };
    let init_us = us(started);
    let counted = counters();
    info!(
        "INIT  §8 initialization + the mount it returns: {=u64} us ({=u32} writes / {=u32} blocks, {=u32} reads / {=u32} blocks, {=u32} syncs)",
        init_us, counted.writes, counted.write_blocks, counted.reads, counted.read_blocks, counted.syncs
    );
    verdict("INIT  first boot", init_us, PLAN_INIT_US);
    info!(
        "INIT  the store came up {} with {=u32} free extents and commit sequence {=u64}",
        defmt::Debug2Format(&store.mode()),
        store.free_extents(),
        store.sequence()
    );
    Some(store.free_extents())
}

// ── 2 and 3: mount, and the commit ladder ───────────────────────────────────────────────────────

/// What one mount found, and what it cost.
struct Boot {
    mode: Mode,
    store_id: StoreId,
    entries: u16,
    us: u64,
    counters: Counters,
    recording: Option<EntryMeta>,
    recovered: Option<RideRecovery>,
}

/// One mount, timed, with the entry the journal half of §5.6 keys on.
///
/// The store is dropped before this returns: a boot figure is what a mount costs, and holding one of
/// these open would put a second ten-kilobyte store beside whichever one the caller already has.
///
/// `plan` is a parameter and a mount that recovered a ride is reported against **no** budget,
/// because §5.6's ~100 ms is scoped to its own sentence: "on a card with no ride in progress a mount
/// reads at most 3 blocks plus the live catalog prefix". A mount that reads sixteen slot headers and
/// CRCs a 32 KiB slot is doing more than that figure covers, and the spec states no figure for it.
#[inline(never)]
fn measure_boot(label: &str, plan: Option<u64>) -> Boot {
    arm();
    let started = Instant::now();
    let store = FlatStore::mount(Card);
    let elapsed = us(started);
    let counted = counters();

    let mut recording = None;
    for entry in store.entries() {
        if entry.flags.has(EntryFlags::RECORDING) {
            recording = Some(entry);
        }
    }
    let boot = Boot {
        mode: store.mode(),
        store_id: store.store_id(),
        entries: store.entry_count(),
        us: elapsed,
        counters: counted,
        recording,
        recovered: store.recovered_ride(),
    };
    info!(
        "{=str}: {=u64} us — {} at sequence {=u64}, {=u16} entries, {=u32} free extents, {=u32} reads / {=u32} blocks",
        label,
        boot.us,
        defmt::Debug2Format(&boot.mode),
        store.sequence(),
        boot.entries,
        store.free_extents(),
        boot.counters.reads,
        boot.counters.read_blocks
    );
    if let Some(recovered) = boot.recovered {
        info!(
            "{=str}: §7.3 recovered a ride — object {=u64} revision {=u64}, checkpoint {=u64}, {=u64} B flushed + {=u32} B tail",
            label,
            recovered.id.0,
            recovered.revision.0,
            recovered.checkpoint_sequence,
            recovered.flushed,
            recovered.tail_len
        );
    }
    report_split(label, boot.us, &boot.counters);
    match (plan, boot.recovered) {
        (_, Some(_)) => info!(
            "{=str}: reported against NO budget — §5.6's ~100 ms is scoped to a card with no ride in progress, and the spec states no figure for a mount that also recovers one",
            label
        ),
        (Some(plan), None) => verdict(label, boot.us, plan),
        (None, None) => {}
    }
    boot
}

/// §5.5's commit, from an empty catalog to [`LADDER_TOP`] entries, one create per step.
///
/// Every step is `allocate` → `write` → `commit`, which is the whole publication path; only the
/// commit is on the clock, because it is the one §5.5 puts a figure on. Returns the free extents
/// left behind.
fn ladder(top: u16) -> u32 {
    let store = FlatStore::mount(Card);
    let mut total = 0u64;
    let mut worst = 0u64;
    let mut worst_at = 0u16;
    let mut publish_total = 0u64;
    for _ in 0..=top {
        let entries = store.entry_count();
        // At a reported catalog size the figure is **sampled**, not taken once: the same commit moved
        // ten per cent between two runs of this bench, so one observation is an anecdote rather than
        // a cost. The ladder's own commit below still feeds the aggregate.
        if entries == 0 || entries == LADDER_MID || entries == top {
            sample_commits(&store, entries);
            if entries > 0 {
                measure_opens(&store, entries);
            }
        }
        let started = Instant::now();
        let Some(commit) = create_once(&store) else { return store.free_extents() };
        let commit_us = commit.elapsed;
        publish_total += us(started);
        total += commit_us;
        if commit_us > worst {
            worst = commit_us;
            worst_at = entries;
        }
        if entries == LADDER_MID || entries == top {
            let label = if entries == LADDER_MID { "BOOT  300 entries" } else { "BOOT  full ladder" };
            measure_boot(label, Some(PLAN_BOOT_US));
        }
    }
    let steps = u64::from(top) + 1;
    info!(
        "LADDER {=u64} commits from an empty catalog to {=u16} entries: {=u64} us mean, {=u64} us worst (at {=u16} entries)",
        steps, top, total / steps, worst, worst_at
    );
    info!(
        "LADDER the whole publication — write + commit — averaged {=u64} us, so {=u64} objects/s sustained",
        publish_total / steps,
        1_000_000 / (publish_total / steps).max(1)
    );
    store.free_extents()
}

/// One timed commit: what it cost, what the card did, and **which catalog copy it landed on**.
struct Commit {
    elapsed: u64,
    counted: Counters,
    /// §5.5 writes the copy that is not being served and then serves it, so the copies strictly
    /// alternate from initialization — which serves copy 0 at sequence 1. A commit that produced
    /// sequence `s` therefore wrote copy `1 - (s % 2)`, and that survives any number of mounts in
    /// between because a mount serves whichever copy carries the greater sequence.
    copy: usize,
    id: ObjectId,
}

/// Which catalog copy the commit that produced `sequence` wrote.
fn copy_of(sequence: u64) -> usize {
    1 - (sequence % 2) as usize
}

/// One create: allocate, write the payload, commit. Only the commit is on the clock, because it is
/// the step §5.5 puts a figure on.
fn create_once(store: &FlatStore<Card>) -> Option<Commit> {
    let entries = store.entry_count();
    let payload = [0x5Au8; LADDER_PAYLOAD];
    let mut allocation = match store.allocate(LADDER_PAYLOAD as u64) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!("LADDER allocate at {=u16} entries refused ({})", entries, defmt::Debug2Format(&error));
            return None;
        }
    };
    if let Err(error) = store.write(&mut allocation, &payload) {
        error!("LADDER write at {=u16} entries refused ({})", entries, defmt::Debug2Format(&error));
        store.cancel(allocation);
        return None;
    }
    let meta = EntryMeta {
        id: store.next_object_id(),
        revision: Revision(1),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: LADDER_PAYLOAD as u64,
        payload_crc: obc_crc::crc32(&payload),
        name: DisplayName::new("fs4-ladder").unwrap_or_default(),
    };
    let id = meta.id;
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]);
    let elapsed = us(started);
    let counted = counters();
    let sequence = match outcome {
        Ok(sequence) => sequence,
        Err(error) => {
            error!("LADDER commit at {=u16} entries refused ({})", entries, defmt::Debug2Format(&error));
            return None;
        }
    };
    Some(Commit { elapsed, counted, copy: copy_of(sequence), id })
}

/// One removal, timed: the same whole-prefix rewrite a create pays, without a payload write.
fn remove_once(store: &FlatStore<Card>, id: ObjectId) -> Option<Commit> {
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Remove { id, revision: Revision(1) }]);
    let elapsed = us(started);
    let counted = counters();
    let sequence = match outcome {
        Ok(sequence) => sequence,
        Err(error) => {
            error!("LADDER remove of object {=u64} refused ({})", id.0, defmt::Debug2Format(&error));
            return None;
        }
    };
    Some(Commit { elapsed, counted, copy: copy_of(sequence), id })
}

/// §5.5's figure at one catalog size, sampled [`COMMIT_SAMPLES`] times **per catalog copy**.
///
/// The shape of this is the whole point, and the first version of it was confounded. §5.5 alternates
/// copies, so a create-then-remove sample sends every create to one copy and every remove to the
/// other: the 22% gap that appeared between the two was a *copy* difference wearing a mutation-kind
/// costume. Two creates in a row fix it — they land on the two copies with everything else equal —
/// and the two removals that undo them do the same, so each sample yields four figures:
///
/// | | copy A | copy B |
/// | create | first  | second |
/// | remove | third  | fourth |
///
/// Four commits per sample keeps the parity, so every sample repeats the same assignment. The entry
/// count moves by one between the paired commits, which changes no block count at any size this
/// bench reports (`1 + ceil(n/4) + 2` is flat across `n` and `n+1` at 0, 300 and 1024).
fn sample_commits(store: &FlatStore<Card>, entries: u16) {
    let mut creates = [[0u64; COMMIT_SAMPLES]; 2];
    let mut removes = [[0u64; COMMIT_SAMPLES]; 2];
    let mut last = [Counters::default(); 2];
    for index in 0..COMMIT_SAMPLES {
        let Some(first) = create_once(store) else { return };
        let Some(second) = create_once(store) else { return };
        for commit in [&first, &second] {
            creates[commit.copy][index] = commit.elapsed;
            last[commit.copy] = commit.counted;
        }
        let Some(third) = remove_once(store, first.id) else { return };
        let Some(fourth) = remove_once(store, second.id) else { return };
        for commit in [&third, &fourth] {
            removes[commit.copy][index] = commit.elapsed;
        }
    }
    // §5.5: `ceil(n/4) + 3` block writes — the body's `1 + ceil(n/4)` blocks (header included), the
    // gate invalidation and the gate itself — and three synchronizations.
    let predicted = 1 + (u32::from(entries) + 1).div_ceil(4) + 2;
    let mut cross = 0u64;
    for copy in 0..2 {
        let (mean, median, least, greatest) = spread(&creates[copy]);
        cross += mean;
        info!(
            "COMMIT at {=u16} entries, create on catalog copy {=usize} ({=usize} samples): {=u64} us mean, {=u64} median, {=u64}..{=u64} — {=u32} blocks, §5.5 predicts {=u32}",
            entries, copy, COMMIT_SAMPLES, mean, median, least, greatest, last[copy].write_blocks, predicted
        );
        report_split("COMMIT create (last sample)", creates[copy][COMMIT_SAMPLES - 1], &last[copy]);
        let (mean, median, least, greatest) = spread(&removes[copy]);
        info!(
            "COMMIT at {=u16} entries, remove on catalog copy {=usize}: {=u64} us mean, {=u64} median, {=u64}..{=u64}",
            entries, copy, mean, median, least, greatest
        );
    }
    // The figure a device actually pays: §5.5 alternates, so consecutive commits pay one copy each.
    let cross = cross / 2;
    info!(
        "COMMIT at {=u16} entries: CROSS-COPY create mean {=u64} us — this is the figure to quote, because §5.5 alternates and no caller gets to pick the cheaper copy",
        entries, cross
    );
    info!(
        "COMMIT at {=u16} entries: the {=u32} syncs are no-ops on this transport — `write_blocks` already waited out the program cycle, so none of the time above is theirs",
        entries, last[0].syncs
    );
    verdict("COMMIT", cross, PLAN_COMMIT_US);
}

/// What `open` costs — §5.3's binary search over the live prefix, then the hold row.
///
/// Twelve of them, because [`MAX_OPEN_OBJECTS`] is sized for the eleven map shards a rendered set
/// mounts plus one transfer: this is the figure a renderer pays to bring a set up. The same search is
/// `find`, which every commit's `resolve` runs once per mutation — so it is also half of why the
/// commit figures above carry the read time they do.
fn measure_opens(store: &FlatStore<Card>, entries: u16) {
    let mut ids = [ObjectId::NONE; MAX_OPEN_OBJECTS];
    let step = (entries as usize / MAX_OPEN_OBJECTS).max(1);
    let mut found = 0usize;
    for (index, meta) in store.entries().enumerate() {
        if index.is_multiple_of(step) && found < MAX_OPEN_OBJECTS {
            ids[found] = meta.id;
            found += 1;
        }
    }
    if found == 0 {
        return;
    }
    let mut handles: [Option<Handle>; MAX_OPEN_OBJECTS] = core::array::from_fn(|_| None);
    arm();
    let started = Instant::now();
    for (slot, id) in ids[..found].iter().enumerate() {
        handles[slot] = store.open(*id, None).ok();
    }
    let elapsed = us(started);
    let counted = counters();
    let opened = handles.iter().flatten().count().max(1) as u64;
    info!(
        "OPEN  {=u64} objects spread across {=u16} entries: {=u64} us total, {=u64} us each — {=u32} entry-block reads, {=u64} per open",
        opened,
        entries,
        elapsed,
        elapsed / opened,
        counted.read_blocks,
        u64::from(counted.read_blocks) / opened
    );
    report_split("OPEN ", elapsed, &counted);
    for handle in handles.into_iter().flatten() {
        store.close(handle);
    }
}

// ── 4: the ride journal ─────────────────────────────────────────────────────────────────────────

/// §7.2's write half: start a ride, then one checkpoint per [`RIDE_GROWTH`] bytes, timed.
///
/// The ride is left **recording** on purpose. It is what phase two recovers.
fn ride() {
    let store = FlatStore::mount(Card);
    let id = store.next_object_id();

    let allocation = match store.allocate(RIDE_RESERVE) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!("RIDE  the 32 MiB reserve was refused ({})", defmt::Debug2Format(&error));
            return;
        }
    };
    // §5.3: a `RECORDING` entry holds slack — it owns more extents than its payload needs — which is
    // the one thing that lets a ride grow without a commit per page.
    let meta = EntryMeta {
        id,
        revision: Revision(1),
        kind: ObjectKind::Ride,
        flags: EntryFlags::RECORDING,
        payload_len: 0,
        payload_crc: 0,
        name: DisplayName::default(),
    };
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]);
    let start_us = us(started);
    let counted = counters();
    if let Err(error) = outcome {
        error!("RIDE  the start commit was refused ({})", defmt::Debug2Format(&error));
        return;
    }
    info!(
        "RIDE  start (32 MiB reserve + one commit at {=u16} entries): {=u64} us, {=u32} writes / {=u32} blocks",
        store.entry_count() - 1,
        start_us,
        counted.writes,
        counted.write_blocks
    );

    // SAFETY: sole borrow of the tail slot; nothing else reads it.
    let tail = unsafe { &mut (*core::ptr::addr_of_mut!(RIDE_TAIL)).0 };
    let mut payload_len = 0u64;
    let mut flushed = 0u64;
    let mut digest = Crc32::new();
    let mut plain_total = 0u64;
    let mut plain = 0u64;
    let mut flush_total = 0u64;
    let mut flushes = 0u64;
    let mut worst = 0u64;
    for sequence in 1..=RIDE_CHECKPOINTS {
        for step in 0..RIDE_GROWTH {
            let offset = payload_len + step as u64;
            let byte = ride_byte(offset);
            tail[(offset - flushed) as usize] = byte;
            digest.update(&[byte]);
        }
        payload_len += RIDE_GROWTH as u64;
        let held = (payload_len - flushed) as usize;
        let checkpoint =
            RideCheckpoint { id, revision: Revision(1), tail: &tail[..held], payload_crc: digest.finalize() };
        arm();
        let started = Instant::now();
        let outcome = store.journal(checkpoint);
        let elapsed = us(started);
        let counted = counters();
        if let Err(error) = outcome {
            error!("RIDE  checkpoint {=u64} refused ({})", sequence, defmt::Debug2Format(&error));
            return;
        }
        // §7.2 flushes whole payload pages out of the front of the tail, and the caller drops exactly
        // those bytes from its own — the one bookkeeping the seam leaves to the rider.
        let pages = held / PROGRAM_PAGE;
        if pages > 0 {
            tail.copy_within(pages * PROGRAM_PAGE..held, 0);
            flushed += (pages * PROGRAM_PAGE) as u64;
            flush_total += elapsed;
            flushes += 1;
            info!(
                "RIDE  checkpoint {=u64} flushed {=usize} payload page(s): {=u64} us, {=u32} writes / {=u32} blocks",
                sequence, pages, elapsed, counted.writes, counted.write_blocks
            );
        } else {
            plain_total += elapsed;
            plain += 1;
            if sequence == 1 {
                info!(
                    "RIDE  checkpoint 1 (one 32 KiB slot, no page flush): {=u64} us, {=u32} writes / {=u32} blocks, {=u32} syncs",
                    elapsed, counted.writes, counted.write_blocks, counted.syncs
                );
            }
        }
        worst = worst.max(elapsed);
    }
    info!(
        "RIDE  {=u64} checkpoints at {=usize} B each: {=u64} us mean without a page flush ({=u64} of them), {=u64} us mean with one ({=u64}), {=u64} us worst",
        RIDE_CHECKPOINTS,
        RIDE_GROWTH,
        plain_total / plain.max(1),
        plain,
        flush_total / flushes.max(1),
        flushes,
        worst
    );
    info!(
        "RIDE  §9's cadence is one checkpoint per 10 s, so the journal costs {=u64} ppm of the ride's wall time",
        (plain_total + flush_total) / RIDE_CHECKPOINTS / 10
    );
    info!(
        "RIDE  left recording: object {=u64}, {=u64} B recorded, {=u64} B flushed, crc 0x{=u32:08x}",
        id.0,
        payload_len,
        flushed,
        digest.finalize()
    );
}

// ── 5: the read path ────────────────────────────────────────────────────────────────────────────

/// §6.1 over a multi-GiB object: write it, sweep it, then hit it at random.
fn read_path(bytes: u64) {
    let store = FlatStore::mount(Card);
    // SAFETY: sole borrows of the two payload slots.
    let pattern = unsafe { &mut (*core::ptr::addr_of_mut!(PATTERN)).0 };
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };
    for (at, byte) in pattern.iter_mut().enumerate() {
        *byte = (at * 7 + 11) as u8;
    }

    let id = store.next_object_id();
    let mut allocation = match store.allocate(bytes) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!(
                "READ  the {=u64} MiB allocation was refused ({})",
                bytes / EXTENT_SIZE,
                defmt::Debug2Format(&error)
            );
            return;
        }
    };

    // The write, with the CRC fold on its own clock: `obc-crc` is a byte-at-a-time table fold, and
    // rolling it into the write's rate would report the M33 rather than the card.
    arm();
    let mut digest = Crc32::new();
    let mut written = 0u64;
    let mut write_us = 0u64;
    let mut crc_us = 0u64;
    while written < bytes {
        let started = Instant::now();
        if let Err(error) = store.write(&mut allocation, pattern) {
            error!("READ  write at {=u64} B refused ({})", written, defmt::Debug2Format(&error));
            store.cancel(allocation);
            return;
        }
        write_us += us(started);
        let started = Instant::now();
        digest.update(pattern);
        crc_us += us(started);
        written += CHUNK as u64;
    }
    let crc = digest.finalize();
    let counted = counters();
    info!(
        "READ  wrote {=u64} MiB in {=usize} B calls: {=u64} ms, {=u64} kB/s ({=u32} device writes / {=u32} blocks)",
        bytes / EXTENT_SIZE,
        CHUNK,
        write_us / 1_000,
        rate(bytes, write_us),
        counted.writes,
        counted.write_blocks
    );
    info!(
        "READ  the CRC-32 fold over the same bytes cost {=u64} ms ({=u64} kB/s on the M33)",
        crc_us / 1_000,
        rate(bytes, crc_us)
    );

    let meta = EntryMeta {
        id,
        revision: Revision(1),
        kind: ObjectKind::MapShard,
        flags: EntryFlags::NONE,
        payload_len: bytes,
        payload_crc: crc,
        name: DisplayName::new("fs4-read-path").unwrap_or_default(),
    };
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]);
    let commit_us = us(started);
    if let Err(error) = outcome {
        error!("READ  the publishing commit was refused ({})", defmt::Debug2Format(&error));
        return;
    }
    info!("READ  published {=u64} MiB as object {=u64} in one commit: {=u64} us", bytes / EXTENT_SIZE, id.0, commit_us);

    let handle = match store.open(id, None) {
        Ok(handle) => handle,
        Err(error) => {
            error!("READ  open refused ({})", defmt::Debug2Format(&error));
            return;
        }
    };

    // The sequential sweep. Every byte is compared against the pattern and folded into a CRC, both
    // off the clock, so the rate is the store's and the verdict is still byte-exact.
    arm();
    let mut digest = Crc32::new();
    let mut offset = 0u64;
    let mut read_us = 0u64;
    let mut mismatch = None;
    while offset < bytes {
        let started = Instant::now();
        let got = match store.read(&handle, offset, readback) {
            Ok(got) => got,
            Err(error) => {
                error!("READ  sweep at {=u64} B refused ({})", offset, defmt::Debug2Format(&error));
                return;
            }
        };
        read_us += us(started);
        if got != CHUNK {
            error!("READ  sweep at {=u64} B came back short ({=usize} B)", offset, got);
            return;
        }
        if mismatch.is_none() && readback[..] != pattern[..] {
            mismatch = Some(offset);
        }
        digest.update(&readback[..got]);
        offset += got as u64;
    }
    let counted = counters();
    if let Some(at) = mismatch {
        error!("READ  the sweep read back bytes the write never put there, from {=u64} B", at);
    } else if digest.finalize() != crc {
        error!("READ  the sweep's CRC does not match the published one");
    } else {
        info!(
            "READ  the whole {=u64} MiB read back byte for byte, crc 0x{=u32:08x} confirmed",
            bytes / EXTENT_SIZE,
            crc
        );
    }
    info!(
        "READ  sequential sweep in {=usize} B calls: {=u64} ms, {=u64} kB/s ({=u64} us per call)",
        CHUNK,
        read_us / 1_000,
        rate(bytes, read_us),
        read_us / (bytes / CHUNK as u64).max(1)
    );
    amplification("READ  sequential", &counted, bytes / 512, read_us);

    random_pass(&store, &handle, bytes, 0, "aligned");
    random_pass(&store, &handle, bytes, 1, "byte-offset");
    random_pass(&store, &handle, bytes, 511, "block-straddling");
    store.close(handle);
}

/// [`RANDOM_READS`] reads of [`RANDOM_LEN`] at random offsets, `skew` bytes off a block boundary.
fn random_pass(store: &FlatStore<Card>, handle: &Handle, bytes: u64, skew: u64, label: &str) {
    // SAFETY: sole borrow; the sweep is finished with it.
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };
    let span = bytes - RANDOM_LEN as u64 - 512;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut elapsed = 0u64;
    let mut blocks = 0u64;
    arm();
    for _ in 0..RANDOM_READS {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let offset = (state % (span / 512)) * 512 + skew;
        let started = Instant::now();
        let got = store.read(handle, offset, &mut readback[..RANDOM_LEN]);
        elapsed += us(started);
        match got {
            Ok(got) if got == RANDOM_LEN => {}
            other => {
                error!("READ  {=str} read at {=u64} B failed ({})", label, offset, defmt::Debug2Format(&other));
                return;
            }
        }
        // The blocks this read's payload span actually occupies, which is what an amplification of
        // 1.00 means: the store read those and not one more.
        blocks += ((offset % 512) + RANDOM_LEN as u64).div_ceil(512);
    }
    let counted = counters();
    info!(
        "READ  {=str} random: {=u32} x {=usize} B in {=u64} ms — {=u64} us each, {=u64} reads/s, {=u64} kB/s",
        label,
        RANDOM_READS,
        RANDOM_LEN,
        elapsed / 1_000,
        elapsed / u64::from(RANDOM_READS),
        1_000_000 / (elapsed / u64::from(RANDOM_READS)).max(1),
        rate(u64::from(RANDOM_READS) * RANDOM_LEN as u64, elapsed)
    );
    amplification("READ  random", &counted, blocks, elapsed);
}

/// #1379's read-ratio check, in the shape a flat store makes it mean something.
///
/// OBC2's figure was **device reads per block**, and 1.00 was the diagnosis: the FAT layer's cache
/// could never hand the card more than one block, so a scan paid a command per 512 bytes. Here that
/// number is a batching figure and nothing more — the store issues one command per contiguous run,
/// so it is well under 1.00 by design and says only how long the runs were.
///
/// The claim §6.1 actually makes is the other ratio: **blocks read per block the payload occupies**.
/// There is no chain to walk and no indirection block to fetch, so a read that does not amplify is
/// exactly 1.00, and anything above it would be the store reading something it did not need.
fn amplification(label: &str, counted: &Counters, required: u64, elapsed: u64) {
    let ratio = u64::from(counted.read_blocks) * 100 / required.max(1);
    info!(
        "{=str}: {=u32} device blocks read for {=u64} payload blocks = {=u64}/100 — {=u32} commands ({=u64}/100 per block), {=u64} us per command",
        label,
        counted.read_blocks,
        required,
        ratio,
        counted.reads,
        u64::from(counted.reads) * 100 / required.max(1),
        elapsed / u64::from(counted.reads.max(1))
    );
    if ratio == 100 {
        info!("{=str}: amplification is exactly 1.00 — §6.1's arithmetic read no block it did not need", label);
    } else {
        error!(
            "{=str}: amplification is {=u64}/100, not 1.00 — the read path fetched blocks it did not need",
            label, ratio
        );
    }
}

// ── 6: resident cost ────────────────────────────────────────────────────────────────────────────

/// The resident total, and the build assertion that keeps it under §9's figure.
const RESIDENT: usize = core::mem::size_of::<FlatStore<Card>>() + TAIL_CAPACITY;

// **An assertion, not a measurement.** `size_of` is a compile-time fact, and the ~42 KiB plan figure
// is the sum of the same two addends — reporting one against the other as a ratio dresses an
// identity up as a result, which is what the first round of this bench did. So it is stated the
// honest way: the build fails if the store plus a recording caller's tail ever leaves §9's budget,
// and the number below is reported without a verdict. Nothing here is in `obc-storage`; the
// constants are §9's, restated at the top of this file.
const _: () = assert!(RESIDENT <= PLAN_RESIDENT, "the flat store plus §7.1's tail no longer fits §9's resident budget");
const _: () = assert!(core::mem::size_of::<FlatStore<Card>>() > FREE_BITMAP);

fn report_footprint() {
    let store = core::mem::size_of::<FlatStore<Card>>();
    let handle = core::mem::size_of::<obc_storage::flat::Handle>();
    info!(
        "RAM   [addend 1] FlatStore<Card> {=usize} B = the {=usize} B free bitmap (§6.2) + {=usize} reservation row(s) + {=usize} hold row(s) + the mounted rows",
        store, FREE_BITMAP, MAX_RESERVATIONS, MAX_OPEN_OBJECTS
    );
    info!(
        "RAM   [addend 2] the recording caller's tail buffer: {=usize} B — §7.1's ceiling, not the ~18 KiB a real ride holds",
        TAIL_CAPACITY
    );
    info!("RAM   [component] one open Handle: {=usize} B; the entry array is never resident (§5.1)", handle);
    info!("RAM   [component] this bench's own buffers, which are not the store's: {=usize} B", CHUNK * 2 + 4_096);
    info!(
        "RAM   TOTAL for one mounted store with a ride recording: {=usize} B. The build ASSERTS this stays within §9's {=usize} B — it is a compile-time identity, so it is reported without a verdict",
        RESIDENT, PLAN_RESIDENT
    );
}

// ── 7: phase two, the recovery half ─────────────────────────────────────────────────────────────

/// Everything the ride recorded before the reset must come back from the card alone, and then §7.2's
/// ride end must publish exactly those bytes.
fn phase_two(boot: &Boot) {
    info!("PHASE two: the card already carries this bench's store with a ride recording");
    let Some(entry) = boot.recording else { return };
    let Some(recovered) = boot.recovered else {
        error!("RCVR  §5.6 found a RECORDING entry but §7.3 recovered no checkpoint — a ride with no slot at all");
        return;
    };

    // **Every expectation below is anchored on the constants phase one wrote with, not on anything
    // the store just said.** Deriving the expected length from `checkpoint_sequence` — as the first
    // round of this bench did — makes the check self-fulfilling: a store that silently selected slot
    // 22 instead of 23 would hand back a shorter ride and a CRC over the shorter ride, and both would
    // "match". §7.4's loss cap is exactly the claim that would go unchecked.
    let mut digest = Crc32::new();
    for offset in 0..RIDE_LEN {
        digest.update(&[ride_byte(offset)]);
    }
    let expected_crc = digest.finalize();
    let mut ok = recovered.id == entry.id && recovered.revision == entry.revision;
    if recovered.checkpoint_sequence != RIDE_CHECKPOINTS {
        error!(
            "RCVR  §7.3 selected checkpoint {=u64}, but phase one wrote {=u64} — a checkpoint was lost or a stale slot won",
            recovered.checkpoint_sequence, RIDE_CHECKPOINTS
        );
        ok = false;
    }
    if (recovered.flushed, recovered.tail_len) != (RIDE_FLUSHED, RIDE_TAIL_LEN) {
        error!(
            "RCVR  recovery says {=u64} B flushed + {=u32} B tail; {=u64} checkpoints of {=usize} B is {=u64} + {=u32}",
            recovered.flushed, recovered.tail_len, RIDE_CHECKPOINTS, RIDE_GROWTH, RIDE_FLUSHED, RIDE_TAIL_LEN
        );
        ok = false;
    }
    if recovered.payload_len() != RIDE_LEN {
        error!("RCVR  recovery says {=u64} B of ride; phase one recorded {=u64} B", recovered.payload_len(), RIDE_LEN);
        ok = false;
    }
    if recovered.payload_crc != expected_crc {
        error!(
            "RCVR  the recovered payload CRC is 0x{=u32:08x}, not 0x{=u32:08x}",
            recovered.payload_crc, expected_crc
        );
        ok = false;
    }
    info!(
        "RCVR  §7.3 selected checkpoint {=u64} of 16 slots: {=u64} B flushed + {=u32} B tail = {=u64} B, crc 0x{=u32:08x}",
        recovered.checkpoint_sequence,
        recovered.flushed,
        recovered.tail_len,
        recovered.payload_len(),
        recovered.payload_crc
    );
    if ok {
        info!(
            "RCVR  the recovery matches the {=u64} checkpoints phase one wrote — sequence, flush point, length and CRC, all against the bench's own constants",
            RIDE_CHECKPOINTS
        );
    }

    ride_end(&entry, expected_crc);
    measure_boot("BOOT  after the ride ended", Some(PLAN_BOOT_US));
    info!("PHASE two done. `probe-rs reset` now runs phase one again — the card has no ride left to recover.");
}

/// §7.2's ride end and what it makes readable, with the store scoped to this call.
#[inline(never)]
fn ride_end(entry: &EntryMeta, expected_crc: u32) {
    let store = FlatStore::mount(Card);
    // SAFETY: sole borrows.
    let tail = unsafe { &mut (*core::ptr::addr_of_mut!(RIDE_TAIL)).0 };
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };

    // The tail the store hands back is the part of the payload that is in a journal slot rather than
    // in the ride's extents. It has to be the tail of what phase one generated.
    match store.recovered_tail(&mut tail[..RIDE_TAIL_LEN as usize]) {
        Ok(len) => {
            let bad = tail[..len]
                .iter()
                .enumerate()
                .find(|(at, byte)| **byte != ride_byte(RIDE_FLUSHED + *at as u64))
                .map(|(at, _)| at);
            match bad {
                None => info!("RCVR  the {=usize} B tail came back out of the slot byte for byte", len),
                Some(at) => error!("RCVR  the recovered tail differs from the ride at byte {=usize}", at),
            }
        }
        Err(error) => error!("RCVR  the tail would not come back ({})", defmt::Debug2Format(&error)),
    }

    // §7.2's ride end: one commit clears RECORDING, trims the reserve to the payload, and moves the
    // last partial page out of the slot and into the ride's own extents.
    let meta = EntryMeta {
        id: entry.id,
        revision: entry.revision,
        kind: ObjectKind::Ride,
        flags: EntryFlags::NONE,
        // The constants again, not the recovery's own numbers: if §7.3 had selected a stale slot,
        // this length would not match the tail that slot holds and the store would refuse the commit
        // — which is the loud failure a self-anchored expectation quietly turns into a pass.
        payload_len: RIDE_LEN,
        payload_crc: expected_crc,
        name: DisplayName::new("fs4-ride").unwrap_or_default(),
    };
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Amend }]);
    let end_us = us(started);
    let counted = counters();
    if let Err(error) = outcome {
        error!("RCVR  the ride end was refused ({})", defmt::Debug2Format(&error));
        return;
    }
    info!(
        "RCVR  §7.2 ride end (tail out of the slot, ranges trimmed, 16 slot headers zeroed): {=u64} us, {=u32} writes / {=u32} blocks",
        end_us, counted.writes, counted.write_blocks
    );

    // And now the whole ride is an ordinary object, so it reads back through the ordinary path.
    match store.open(entry.id, None) {
        Ok(handle) => {
            let mut digest = Crc32::new();
            let mut offset = 0u64;
            let mut bad = None;
            while offset < RIDE_LEN {
                let want = ((RIDE_LEN - offset) as usize).min(readback.len());
                let Ok(got) = store.read(&handle, offset, &mut readback[..want]) else {
                    error!("RCVR  the finalised ride would not read at {=u64} B", offset);
                    return;
                };
                bad = bad.or_else(|| {
                    readback[..got]
                        .iter()
                        .enumerate()
                        .find(|(at, byte)| **byte != ride_byte(offset + *at as u64))
                        .map(|(at, _)| offset + at as u64)
                });
                digest.update(&readback[..got]);
                offset += got as u64;
            }
            match bad {
                None if digest.finalize() == expected_crc => info!(
                    "RCVR  the whole {=u64} B ride read back byte for byte through the ordinary read path",
                    RIDE_LEN
                ),
                None => error!("RCVR  the finalised ride's CRC is not the one phase one recorded"),
                Some(at) => error!("RCVR  the finalised ride differs from what was recorded at byte {=u64}", at),
            }
            store.close(handle);
        }
        Err(error) => error!("RCVR  the finalised ride would not open ({})", defmt::Debug2Format(&error)),
    }

    // The read-path object phase one published, spot-checked rather than swept: the sweep is phase
    // one's measurement, and what phase two is asking is only whether it survived the reset.
    let big = store.entries().find(|entry| entry.kind == ObjectKind::MapShard);
    if let Some(meta) = big {
        spot_check(&store, &meta);
    }
}

/// A few pages of a published object, read at random and compared with the pattern that wrote it.
fn spot_check(store: &FlatStore<Card>, meta: &EntryMeta) {
    // SAFETY: sole borrows.
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };
    let Ok(handle) = store.open(meta.id, None) else {
        error!("SPOT  object {=u64} would not open after the reset", meta.id.0);
        return;
    };
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut bad = 0u32;
    for _ in 0..32 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let offset = (state % (meta.payload_len / CHUNK as u64)) * CHUNK as u64;
        let Ok(got) = store.read(&handle, offset, readback) else {
            error!("SPOT  object {=u64} would not read at {=u64} B", meta.id.0, offset);
            store.close(handle);
            return;
        };
        if readback[..got].iter().enumerate().any(|(at, byte)| *byte != (at * 7 + 11) as u8) {
            bad += 1;
        }
    }
    if bad == 0 {
        info!(
            "SPOT  object {=u64} ({=u64} MiB, published before the reset): 32 random pages read back intact",
            meta.id.0,
            meta.payload_len / EXTENT_SIZE
        );
    } else {
        error!("SPOT  object {=u64}: {=u32} of 32 random pages came back wrong", meta.id.0, bad);
    }
    store.close(handle);
}

// ── the serial map ingest ───────────────────────────────────────────────────────────────────────
//
// The wire is documented in full at the top of this file. What follows is only what the code needs
// said beside it.

/// The magic every framed message carries, in both directions.
const INGEST_MAGIC: [u8; 4] = *b"OBCI";
/// The wire version. A host speaking another one is refused, never guessed at.
const INGEST_VERSION: u8 = 1;
/// Frame tags, at byte 5 of every framed message.
const TAG_READY: u8 = b'R';
const TAG_HEADER: u8 = b'H';
const TAG_DONE: u8 = b'D';
const TAG_FAIL: u8 = b'E';
/// The device is about to stop listening. Same 14-byte shape as READY, so a host waiting for one
/// reads it with the code it already has — and finds out *why* nothing is coming instead of
/// timing out into a diagnosis it has to guess at.
const TAG_GONE: u8 = b'G';

/// Why the device stopped listening — the payload of a [`TAG_GONE`] frame.
mod gone {
    /// The advertising window closed and the **destructive measurement run is starting**. A host
    /// that reads this has seconds, not minutes, to stop the operator.
    pub const WINDOW_CLOSED: u32 = 1;
    /// The session ended because the host went quiet after its last object. Nothing is wrong.
    pub const SESSION_OVER: u32 = 2;
    /// A commit was refused and its extents are held until a remount. Reset before retrying.
    pub const RESERVATION_HELD: u32 = 3;
    /// The line was erroring. The measurements are refused; check the baud.
    pub const LINE_ERRORING: u32 = 4;
}
/// The two status bytes, ASCII ACK and NAK.
const STATUS_ACK: u8 = 0x06;
const STATUS_NAK: u8 = 0x15;

/// The fixed HEADER and RESULT sizes, named so the frame builders and the readers cannot disagree.
const HEADER_BYTES: usize = 72;
const RESULT_BYTES: usize = 42;
const READY_BYTES: usize = 14;
/// `DisplayName`'s capacity (§5.3), restated here rather than re-exported: a bench states the
/// format constants it needs beside itself, and `DisplayName::new` is still what enforces it.
const INGEST_NAME_CAP: usize = 48;

/// Payload bytes one chunk carries — the unit the host is paced in, and the size of the one buffer
/// the payload ever occupies on this device.
///
/// 8 KiB is chosen against the *ack*, not the card: at [`INGEST_BAUD`] a chunk is 711 ms of wire
/// time against ~2 ms of USB turnaround, so the pacing costs a third of a per cent, and halving the
/// chunk would double that for nothing. It is also 16 blocks, so a chunk is a whole number of device
/// write commands with no staging carry between them.
const INGEST_CHUNK: usize = 8 * 1_024;

// One chunk is one EasyDMA transfer, and the driver refuses a longer one at runtime rather than at
// build time. Here it is a build failure instead.
const _: () = assert!(INGEST_CHUNK <= embassy_nrf::EASY_DMA_SIZE, "a chunk is one EasyDMA transfer");

/// The VCOM's line rate.
///
/// 115,200 is the rate this rig is **proven** at, and the ingest is a board-session tool: a
/// transfer that takes a minute and works beats one that takes seven seconds and might not. Raising
/// this to `Baudrate::Baud1m` is a one-line change (and `--baud 1000000` on the host) once a session
/// has spare time to establish that this J-Link's CDC will carry it.
const INGEST_BAUD: Baudrate = Baudrate::Baud115200;
/// The same rate as a number, for the diagnostics. A host that disagrees with this is the single
/// most likely reason a window ends [`Advertised::Erroring`], so the message names it.
const INGEST_BAUD_HZ: u32 = 115_200;

/// How long the bench advertises before falling through to the measurement run.
const INGEST_WINDOW_MS: u64 = 10_000;
/// The advertising cadence, and the quiet period one READY requires before it may be sent.
const INGEST_READY_MS: u64 = 500;
/// A chunk's deadline. Generous: at [`INGEST_BAUD`] a chunk is 711 ms, and the only thing this
/// number is protecting against is a host that went away mid-transfer.
const INGEST_CHUNK_MS: u64 = 20_000;

/// Why the device refused. The host prints these by name; the numbers are the wire.
mod reason {
    pub const NONE: u8 = 0;
    /// Magic or version the device does not speak.
    pub const VERSION: u8 = 1;
    /// The header's own CRC did not check.
    pub const HEADER_CRC: u8 = 2;
    /// `kind` is not one of `FLAT_Store_Format.md` §3.1's.
    pub const KIND: u8 = 3;
    /// The name is longer than `DisplayName`'s 48 bytes, or is not UTF-8.
    pub const NAME: u8 = 4;
    /// A zero-length payload. The store would take it; there is no reason to want it.
    pub const EMPTY: u8 = 5;
    /// The card did not come up writable, and is not in a state initialization may repair.
    pub const NOT_WRITABLE: u8 = 6;
    /// §6 refused the reservation — the payload does not fit the free extents.
    pub const ALLOCATE: u8 = 7;
    /// A chunk would not go into the reservation.
    pub const WRITE: u8 = 8;
    /// The bytes that arrived are not the bytes the header described. Nothing was committed.
    pub const PAYLOAD_CRC: u8 = 9;
    /// §5.5 refused the publishing commit.
    pub const COMMIT: u8 = 10;
    /// The cable: a timeout or a UARTE error.
    pub const LINK: u8 = 11;
}

/// Why a read did not deliver its bytes.
///
/// The two are **not** interchangeable, and collapsing them is the bug this enum exists to prevent:
/// a timeout means the line is quiet, and an error means something is driving it that this device
/// cannot decode — a baud mismatch, a DTR glitch, a cable on its way out. Reading the second as the
/// first is how a screaming line becomes "nobody answered", and "nobody answered" is how this
/// binary starts wiping the card.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Link {
    Timeout,
    Uart,
}

/// How an advertising window ended.
enum Advertised {
    /// A host's magic arrived.
    Answered,
    /// The window expired with the line quiet. The only outcome that lets the measurements run.
    Quiet,
    /// The window ended with the UARTE reporting errors: bytes are arriving that this device cannot
    /// decode. Something is on the other end, so the measurement run — which would wipe the card —
    /// must not follow.
    Erroring,
}

/// Consecutive UARTE errors that end a window as [`Advertised::Erroring`].
///
/// One error is line noise on a cable being plugged in. Thirty-two in a row is a host transmitting
/// at a rate this device is not configured for, which is exactly what a `--baud 1000000` run against
/// the 115,200 default looks like from here.
const INGEST_UART_FAULT_LIMIT: u32 = 32;

/// The one payload buffer. A chunk is 8 KiB and lives here, in `.bss`, exactly as the read path's
/// buffers do: a map-sized temporary is what this bench's own docs were written to warn about, and
/// so is a chunk-sized one on a poll frame.
static mut INGEST_BUF: Aligned<INGEST_CHUNK> = Aligned([0; INGEST_CHUNK]);

/// Advertise on the VCOM, and run the ingest if a host answers. `true` when one did.
///
/// The UARTE is built here and dropped here, so a bench run that nobody answered leaves the
/// peripheral exactly as it found it and the measurements that follow are the measurements this
/// bench has always made.
#[inline(never)]
fn ingest_offer(p: embassy_nrf::Peripherals, boot: &Boot) -> bool {
    let mut config = uarte::Config::default();
    config.baudrate = INGEST_BAUD;
    let uart = Uarte::new(p.SERIAL20, p.P1_17, p.P1_16, UartIrqs, config);
    let (mut tx, mut rx) = uart.split();
    info!(
        "INGEST advertising on the VCOM (SERIAL20, P1_16/P1_17) for {=u64} ms — `tools/bench_ingest.py` takes it from here",
        INGEST_WINDOW_MS
    );
    match ingest_wait(&mut tx, &mut rx, INGEST_WINDOW_MS) {
        Advertised::Answered => {
            ingest_session(&mut tx, &mut rx, boot);
            true
        }
        Advertised::Quiet => {
            // The last thing this device says before it starts destroying the card. A host that
            // arrived late reads it instead of timing out and blaming the cable.
            ingest_gone(&mut tx, gone::WINDOW_CLOSED);
            info!("INGEST nobody answered — running the measurements");
            false
        }
        // The one case that is neither. Something is transmitting and this device cannot read it, so
        // there is very likely a host at the other end of the cable that believes it is sending a
        // map. Falling through here would run phase one and destroy the card it was aimed at.
        Advertised::Erroring => {
            ingest_gone(&mut tx, gone::LINE_ERRORING);
            error!(
                "INGEST the VCOM reported {=u32} consecutive errors during the window — bytes are arriving that this device cannot decode",
                INGEST_UART_FAULT_LIMIT
            );
            error!(
                "INGEST the likeliest cause is a baud mismatch: this build is at {=u32} baud, so the host needs `--baud {=u32}`",
                INGEST_BAUD_HZ, INGEST_BAUD_HZ
            );
            error!("INGEST REFUSING to run the measurements — they would destroy the card a host is aiming at");
            true
        }
    }
}

/// Advertise until a host's magic arrives, or until `window_ms` has passed.
///
/// The magic is matched **one byte at a time**, and a READY only goes out after a whole
/// [`INGEST_READY_MS`] of silence. Both are the same precaution: this cable has no flow control, so
/// the device must never be transmitting while the host is mid-burst, and it must never drop a
/// partly-matched magic on the floor because its own advertising timer came due.
///
/// A UARTE **error** is handled as its own case and not as silence. It returns from
/// [`ingest_read`] immediately rather than after [`INGEST_READY_MS`], so treating it as a quiet
/// interval would advertise at whatever rate the errors arrive — hundreds of READY frames a second,
/// straight into the burst that is causing them — and then end the window claiming nobody was there.
fn ingest_wait(tx: &mut UarteTx<'_>, rx: &mut UarteRx<'_>, window_ms: u64) -> Advertised {
    let ready = ingest_ready_frame();
    let deadline = Instant::now() + Duration::from_millis(window_ms);
    let mut matched = 0usize;
    let mut byte = [0u8; 1];
    let mut faults = 0u32;
    let mut complained = false;
    loop {
        match ingest_read(rx, &mut byte, INGEST_READY_MS) {
            Ok(()) => {
                faults = 0;
                matched = if byte[0] == INGEST_MAGIC[matched] {
                    matched + 1
                } else if byte[0] == INGEST_MAGIC[0] {
                    1
                } else {
                    0
                };
                if matched == INGEST_MAGIC.len() {
                    return Advertised::Answered;
                }
            }
            Err(Link::Timeout) => {
                // The line has been quiet for an interval, so this is the safe moment to talk — and
                // any half-matched magic is stale, because a host sends its header in one write.
                faults = 0;
                matched = 0;
                if Instant::now() >= deadline {
                    return Advertised::Quiet;
                }
                let _ = tx.blocking_write(&ready);
            }
            Err(Link::Uart) => {
                // NOT an advertising opportunity. Say so once, then either wait the line out or give
                // up on it — but never conclude from this that the cable is idle.
                matched = 0;
                faults += 1;
                if !complained {
                    complained = true;
                    warn!("INGEST the VCOM is reporting receive errors (overrun / framing / parity) — NOT treating this as a quiet line");
                }
                if faults >= INGEST_UART_FAULT_LIMIT {
                    return Advertised::Erroring;
                }
                if Instant::now() >= deadline {
                    return Advertised::Erroring;
                }
            }
        }
    }
}

/// One ingest session: bring the store up, then take objects until the host stops sending them.
///
/// The store is mounted **once** for the whole session and lives in this frame, which is why this is
/// its own out-of-line call — the same reason [`initialize`] and [`ladder`] are.
#[inline(never)]
fn ingest_session(tx: &mut UarteTx<'_>, rx: &mut UarteRx<'_>, boot: &Boot) {
    let store = if boot.mode.writable() {
        info!("INGEST the card mounted writable — taking objects into the store already on it");
        FlatStore::mount(Card)
    } else if matches!(boot.mode, Mode::Unformatted | Mode::CatalogUnreadable) {
        warn!(
            "INGEST the card came up {} — INITIALIZING it (§8), which destroys whatever is on it",
            defmt::Debug2Format(&boot.mode)
        );
        match FlatStore::initialize(Card, BENCH_STORE) {
            Ok(store) => store,
            Err(error) => {
                error!("INGEST §8 initialization failed ({})", defmt::Debug2Format(&error));
                ingest_status(tx, STATUS_NAK, reason::NOT_WRITABLE);
                return;
            }
        }
    } else {
        error!(
            "INGEST the card came up {}, which initialization does not repair — refusing",
            defmt::Debug2Format(&boot.mode)
        );
        ingest_status(tx, STATUS_NAK, reason::NOT_WRITABLE);
        return;
    };

    // The first object's magic is already consumed by `ingest_wait`; every later one re-enters the
    // same advertising loop, so a host may send several objects in one flash and a failed attempt
    // may simply be sent again.
    loop {
        if matches!(ingest_object(tx, rx, &store), Session::Stop) {
            ingest_gone(tx, gone::RESERVATION_HELD);
            return;
        }
        match ingest_wait(tx, rx, INGEST_WINDOW_MS) {
            Advertised::Answered => {}
            Advertised::Quiet => {
                ingest_gone(tx, gone::SESSION_OVER);
                info!("INGEST the host has gone quiet — the session is over");
                return;
            }
            Advertised::Erroring => {
                ingest_gone(tx, gone::LINE_ERRORING);
                error!("INGEST the VCOM started erroring between objects — ending the session rather than guessing");
                return;
            }
        }
    }
}

/// Whether the session may take another object after the one that just finished.
enum Session {
    Continue,
    Stop,
}

/// One object, from the header that follows an already-matched magic to the RESULT frame.
fn ingest_object(tx: &mut UarteTx<'_>, rx: &mut UarteRx<'_>, store: &FlatStore<Card>) -> Session {
    let mut header = [0u8; HEADER_BYTES];
    header[..4].copy_from_slice(&INGEST_MAGIC);
    if let Err(fault) = ingest_read(rx, &mut header[4..], INGEST_CHUNK_MS) {
        ingest_link_failed("the header", fault);
        ingest_status(tx, STATUS_NAK, reason::LINK);
        return Session::Continue;
    }
    if header[4] != INGEST_VERSION || header[5] != TAG_HEADER {
        error!("INGEST header version {=u8} tag {=u8} — not this wire", header[4], header[5]);
        ingest_status(tx, STATUS_NAK, reason::VERSION);
        return Session::Continue;
    }
    if u32::from_le_bytes(header[68..72].try_into().unwrap_or_default()) != obc_crc::crc32(&header[..68]) {
        error!("INGEST the header's own CRC does not check — the framing is out of step");
        ingest_status(tx, STATUS_NAK, reason::HEADER_CRC);
        return Session::Continue;
    }
    let Ok(kind) = ObjectKind::decode(u16::from(header[6])) else {
        error!("INGEST kind {=u8} is not one of §3.1's", header[6]);
        ingest_status(tx, STATUS_NAK, reason::KIND);
        return Session::Continue;
    };
    let name_len = usize::from(header[7]);
    // The bound is checked before the slice, not after: the length is a byte off the wire, and
    // `20 + name_len` past the cap would index outside the frame.
    let name = (name_len <= INGEST_NAME_CAP)
        .then(|| core::str::from_utf8(&header[20..20 + name_len]).ok())
        .flatten()
        .and_then(DisplayName::new);
    let Some(name) = name else {
        error!(
            "INGEST the name is {=usize} B, or is not UTF-8 — DisplayName takes {=usize} B of UTF-8",
            name_len, INGEST_NAME_CAP
        );
        ingest_status(tx, STATUS_NAK, reason::NAME);
        return Session::Continue;
    };
    let payload_len = u64::from_le_bytes(header[8..16].try_into().unwrap_or_default());
    let want_crc = u32::from_le_bytes(header[16..20].try_into().unwrap_or_default());
    if payload_len == 0 {
        ingest_status(tx, STATUS_NAK, reason::EMPTY);
        return Session::Continue;
    }
    info!(
        "INGEST header accepted: {} named {=str}, {=u64} B, crc 0x{=u32:08x}, {=u64} chunks of {=usize} B",
        defmt::Debug2Format(&kind),
        name.as_str().unwrap_or("?"),
        payload_len,
        want_crc,
        payload_len.div_ceil(INGEST_CHUNK as u64),
        INGEST_CHUNK
    );

    let mut allocation = match store.allocate(payload_len) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!(
                "INGEST §6 refused {=u64} B against {=u32} free extents ({})",
                payload_len,
                store.free_extents(),
                defmt::Debug2Format(&error)
            );
            ingest_status(tx, STATUS_NAK, reason::ALLOCATE);
            return Session::Continue;
        }
    };
    ingest_status(tx, STATUS_ACK, reason::NONE);

    // SAFETY: sole borrow of the chunk slot; the session is single-threaded and nothing else reads
    // it. Same discipline as PATTERN / READBACK / RIDE_TAIL above.
    let buf = unsafe { &mut (*core::ptr::addr_of_mut!(INGEST_BUF)).0 };
    let mut digest = Crc32::new();
    let mut received = 0u64;
    let started = Instant::now();
    while received < payload_len {
        let take = ((payload_len - received) as usize).min(INGEST_CHUNK);
        if let Err(fault) = ingest_read(rx, &mut buf[..take], INGEST_CHUNK_MS) {
            ingest_link_failed("a payload chunk", fault);
            store.cancel(allocation);
            ingest_status(tx, STATUS_NAK, reason::LINK);
            return Session::Continue;
        }
        if let Err(error) = store.write(&mut allocation, &buf[..take]) {
            error!("INGEST the store refused a chunk at {=u64} B ({})", received, defmt::Debug2Format(&error));
            store.cancel(allocation);
            ingest_status(tx, STATUS_NAK, reason::WRITE);
            return Session::Continue;
        }
        digest.update(&buf[..take]);
        received += take as u64;
        // The ack goes out only once the bytes are in the reservation, which is what makes it a
        // pacing signal rather than a receipt for a buffer.
        ingest_status(tx, STATUS_ACK, reason::NONE);
    }
    let elapsed = us(started);
    let got_crc = digest.finalize();
    info!(
        "INGEST {=u64} B received in {=u64} ms ({=u64} kB/s on the wire), crc 0x{=u32:08x}",
        received,
        elapsed / 1_000,
        rate(received, elapsed),
        got_crc
    );

    // Past the last chunk's ack the object closes with a RESULT rather than a STATUS — one frame,
    // whichever way it went, so the host never has to guess which of the two is coming next.
    //
    // The CRC is checked before the commit, not after: a mismatch must publish nothing, and `cancel`
    // is what hands the extents back so the next attempt can have them. This is the last point at
    // which that is still possible — the commit below takes the `Allocation` by value.
    if got_crc != want_crc {
        error!(
            "INGEST the payload CRC is 0x{=u32:08x}, not the 0x{=u32:08x} the header promised — NOT committing",
            got_crc, want_crc
        );
        store.cancel(allocation);
        ingest_result(tx, TAG_FAIL, reason::PAYLOAD_CRC, ObjectId::NONE, Revision(0), received, got_crc, 0);
        return Session::Continue;
    }

    let meta = EntryMeta {
        id: store.next_object_id(),
        revision: Revision(1),
        kind,
        flags: EntryFlags::NONE,
        payload_len,
        payload_crc: got_crc,
        name,
    };
    let id = meta.id;
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]);
    let commit_us = us(started);
    let sequence = match outcome {
        Ok(sequence) => sequence,
        // **This is the one failure that does not clean up after itself, and the session ends here.**
        //
        // `Mutation::Put` took the `Allocation` by value, and §5.5 clears the reservation row only
        // after the gate write lands — so a refused commit leaves the row occupied and its extents
        // spoken for, with no `Allocation` left anywhere to `cancel`. Only a remount frees them.
        // `MAX_RESERVATIONS` is 2, so a session that looped here would wedge on its third object
        // with a `NoSpace` that has nothing to do with the card being full. Stopping is honest:
        // the operator resets, the mount reclaims, and the next attempt starts clean.
        Err(error) => {
            error!("INGEST §5.5 refused the publishing commit ({})", defmt::Debug2Format(&error));
            error!(
                "INGEST the reservation for those {=u64} B is still held — it is freed by a remount, so `probe-rs reset` before retrying",
                payload_len
            );
            ingest_result(tx, TAG_FAIL, reason::COMMIT, ObjectId::NONE, Revision(0), received, got_crc, 0);
            return Session::Stop;
        }
    };
    info!(
        "INGEST published object {=u64} revision 1 at commit sequence {=u64} in {=u64} us",
        id.0, sequence, commit_us
    );
    ingest_result(tx, TAG_DONE, reason::NONE, id, Revision(1), payload_len, got_crc, store.entry_count());
    ingest_census(store);
    Session::Continue
}

/// The whole catalog after a commit, one line per entry. This is the acceptance evidence: the object
/// the host sent, named, sized and CRC'd, sitting in the catalog beside everything else on the card.
fn ingest_census(store: &FlatStore<Card>) {
    info!(
        "CENSUS {=u16} entries, {=u32} free extents, commit sequence {=u64}",
        store.entry_count(),
        store.free_extents(),
        store.sequence()
    );
    for entry in store.entries() {
        info!(
            "CENSUS   object {=u64} rev {=u64} {} {=u64} B crc 0x{=u32:08x} — {=str}",
            entry.id.0,
            entry.revision.0,
            defmt::Debug2Format(&entry.kind),
            entry.payload_len,
            entry.payload_crc,
            entry.name.as_str().unwrap_or("(unnamed)")
        );
    }
    if !store.entries_ok() {
        error!("CENSUS the listing did not complete — the catalog moved underneath it");
    }
}

/// The 14-byte READY.
fn ingest_ready_frame() -> [u8; READY_BYTES] {
    short_frame(TAG_READY, INGEST_CHUNK as u32)
}

/// One 14-byte frame carrying a tag and a `u32`. READY's chunk size and GONE's reason are the same
/// shape on purpose: a host blocked waiting for a READY decodes a GONE with the code it already has.
fn short_frame(tag: u8, value: u32) -> [u8; READY_BYTES] {
    let mut frame = [0u8; READY_BYTES];
    frame[..4].copy_from_slice(&INGEST_MAGIC);
    frame[4] = INGEST_VERSION;
    frame[5] = tag;
    frame[6..10].copy_from_slice(&value.to_le_bytes());
    let crc = obc_crc::crc32(&frame[..10]);
    frame[10..14].copy_from_slice(&crc.to_le_bytes());
    frame
}

/// Tell whoever is listening that this device has stopped, and why.
///
/// Sent on every path out of the ingest, because the alternative is a host that waits out its whole
/// timeout and then has to guess between four unrelated causes — one of which is "the card is being
/// wiped right now".
fn ingest_gone(tx: &mut UarteTx<'_>, why: u32) {
    let _ = tx.blocking_write(&short_frame(TAG_GONE, why));
}

/// One two-byte STATUS.
fn ingest_status(tx: &mut UarteTx<'_>, status: u8, why: u8) {
    let _ = tx.blocking_write(&[status, why]);
}

/// The 42-byte RESULT that closes an object out, successfully or not.
#[allow(clippy::too_many_arguments)]
fn ingest_result(
    tx: &mut UarteTx<'_>,
    tag: u8,
    why: u8,
    id: ObjectId,
    revision: Revision,
    payload_len: u64,
    payload_crc: u32,
    entries: u16,
) {
    let mut frame = [0u8; RESULT_BYTES];
    frame[..4].copy_from_slice(&INGEST_MAGIC);
    frame[4] = INGEST_VERSION;
    frame[5] = tag;
    frame[6] = why;
    frame[8..16].copy_from_slice(&id.0.to_le_bytes());
    frame[16..24].copy_from_slice(&revision.0.to_le_bytes());
    frame[24..32].copy_from_slice(&payload_len.to_le_bytes());
    frame[32..36].copy_from_slice(&payload_crc.to_le_bytes());
    frame[36..38].copy_from_slice(&entries.to_le_bytes());
    let crc = obc_crc::crc32(&frame[..38]);
    frame[38..42].copy_from_slice(&crc.to_le_bytes());
    let _ = tx.blocking_write(&frame);
}

/// Fill `buf` from the VCOM, or give up after `timeout_ms`.
///
/// **Deliberately not an `async fn`, and deliberately not on the executor.** This binary's whole
/// shape is the #1379 lesson — an async fn's locals are permanent poll-frame slots, and the caller
/// above holds a ten-kilobyte store — so the read is driven by polling the driver's future on *this*
/// stack against a plain deadline. The future is a local of this frame and nothing outlives the
/// call; dropping it on the timeout is what stops the DMA, which is the driver's own contract.
fn ingest_read(rx: &mut UarteRx<'_>, buf: &mut [u8], timeout_ms: u64) -> Result<(), Link> {
    if buf.is_empty() {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut cx = Context::from_waker(Waker::noop());
    let mut read = core::pin::pin!(rx.read(buf));
    loop {
        match read.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(())) => return Ok(()),
            Poll::Ready(Err(_)) => return Err(Link::Uart),
            Poll::Pending => {}
        }
        if Instant::now() >= deadline {
            return Err(Link::Timeout);
        }
    }
}

fn ingest_link_failed(what: &str, fault: Link) {
    match fault {
        Link::Timeout => {
            error!("INGEST {=str} did not arrive before the deadline — the host stopped or the VCOM has wedged", what)
        }
        Link::Uart => error!("INGEST the UARTE reported an error reading {=str} (overrun / framing)", what),
    }
}

// ── verdicts and helpers ────────────────────────────────────────────────────────────────────────

/// Where a measured interval went: the card's write half, its read half, and what was left over for
/// the M33.
///
/// Every figure this bench reports carries this line, because the halves have different causes and
/// different fixes. A commit that spends 79 blocks of writing and 156 blocks of *reading* is not one
/// figure — the reads are `merge` streaming the live prefix twice plus `find`'s binary search, and
/// dividing the total by the blocks written attributes all of it to the program cycle. That is the
/// error the first round of this bench made, and this function is the fix.
fn report_split(label: &str, elapsed: u64, counted: &Counters) {
    info!(
        "{=str}: {=u64} us WRITE {=u64} us ({=u32} calls / {=u32} blocks = {=u64} us per block) · READ {=u64} us ({=u32} / {=u32} = {=u64} us per block) · M33 {=u64} us",
        label,
        elapsed,
        counted.write_us,
        counted.writes,
        counted.write_blocks,
        counted.write_us / u64::from(counted.write_blocks.max(1)),
        counted.read_us,
        counted.reads,
        counted.read_blocks,
        counted.read_us / u64::from(counted.read_blocks.max(1)),
        elapsed.saturating_sub(counted.read_us + counted.write_us)
    );
}

/// The mean, median, least and greatest of a sample set, so a single-sample figure is never quoted
/// as if it were the cost.
///
/// The median earns its place here: this card produces occasional multi-second commits (1.2 s and
/// 3.3 s have both been seen mid-ladder, presumably its own housekeeping), and one of those inside a
/// three-sample set moves the mean by more than every effect this bench is trying to measure.
fn spread(samples: &[u64]) -> (u64, u64, u64, u64) {
    let mean = samples.iter().sum::<u64>() / samples.len() as u64;
    let mut sorted = [0u64; COMMIT_SAMPLES];
    sorted[..samples.len()].copy_from_slice(samples);
    sorted[..samples.len()].sort_unstable();
    let median = sorted[samples.len() / 2];
    (mean, median, sorted[0], sorted[samples.len() - 1])
}

/// One measured figure against its plan figure, in the form #1386 asks for: within plan, or a miss,
/// and past 2× a miss that goes back to the epic.
fn verdict(label: &str, measured: u64, plan: u64) {
    let ratio = measured * 100 / plan.max(1);
    if measured <= plan {
        info!("{=str}: WITHIN PLAN — {=u64} us against {=u64} us ({=u64}/100x)", label, measured, plan, ratio);
    } else if ratio <= 200 {
        warn!("{=str}: MISS — {=u64} us against {=u64} us ({=u64}/100x, inside 2x)", label, measured, plan, ratio);
    } else {
        error!(
            "{=str}: MISS >2x — {=u64} us against {=u64} us ({=u64}/100x) — flag on #1256",
            label, measured, plan, ratio
        );
    }
}

fn us(since: Instant) -> u64 {
    Instant::now().duration_since(since).as_micros()
}

/// Bytes per microsecond scaled to kB/s, saturating rather than dividing by zero.
fn rate(bytes: u64, elapsed_us: u64) -> u64 {
    if elapsed_us == 0 {
        return 0;
    }
    bytes * 1_000 / elapsed_us
}

/// Everything is measured; hold here so the RTT session stays attached and a `probe-rs reset`
/// starts the next cycle cleanly.
fn park() -> ! {
    info!("flat_store_bench: done — parked");
    loop {
        cortex_m::asm::wfi();
    }
}

// ── the stack meter ─────────────────────────────────────────────────────────────────────────────

/// Stack high-water: paint the free stack with a sentinel at boot, then find the lowest word that
/// is still painted. The scan must run bottom-up to the first non-painted word — a frame does not
/// write every word it covers, so a top-down scan under-reports by whole buffers.
///
/// The same measurement `main.rs` makes, restated here because this binary links none of it.
mod stackmeter {
    const PAINT: u32 = 0xC0DE_DEAD;

    extern "C" {
        static _stack_start: u32;
        static _stack_end: u32;
    }

    fn top() -> usize {
        core::ptr::addr_of!(_stack_start) as usize
    }

    fn bottom() -> usize {
        core::ptr::addr_of!(_stack_end) as usize
    }

    /// Paints everything below the current SP (less a margin) down to the stack bottom.
    pub fn paint() {
        let sp = cortex_m::register::msp::read() as usize;
        let mut at = bottom();
        let stop = sp.saturating_sub(512);
        while at < stop {
            // SAFETY: the range is inside this binary's own stack region and below the live frame.
            unsafe { (at as *mut u32).write_volatile(PAINT) };
            at += 4;
        }
    }

    /// Bytes of stack used at the deepest point reached so far.
    pub fn used() -> usize {
        let (top, bottom) = (top(), bottom());
        let mut at = bottom;
        while at < top {
            // SAFETY: reading this binary's own stack region.
            if unsafe { (at as *const u32).read_volatile() } != PAINT {
                break;
            }
            at += 4;
        }
        top - at
    }

    /// Total usable stack.
    pub fn total() -> usize {
        top() - bottom()
    }

    /// Arms the ARMv8-M `MSPLIM`, so an overflow faults at the moment of overflow instead of
    /// silently smashing whatever static tops `.bss`.
    pub fn arm_limit() {
        const HANDLER_MARGIN: usize = 512;
        // SAFETY: raising a fault on genuine overflow is strictly safer than silent corruption.
        unsafe { cortex_m::register::msplim::write((bottom() + HANDLER_MARGIN) as u32) };
    }
}
