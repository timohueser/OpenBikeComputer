//! The bootloader's install engine — verify → flash → readback → state transition (S3, #618).
//!
//! ALL install **sequencing** lives here as a pure, host-testable driver: the ordering of the
//! passes, the retry counts, the header-skip arithmetic, and which state gets written on which
//! failure. That ordering is the safety property of the whole DFU epic (#615), so it gets unit
//! tests with mock IO (`tests/engine.rs`), not trust. `obc-boot` stays a dumb driver that wires
//! real SPI block reads and RRAMC line writes into the [`InstallIo`] trait and maps the returned
//! [`Outcome`] to an LED pattern + jump/reset/halt.
//!
//! ## What the extents cover (pinned here so S4 and the bootloader can never disagree)
//!
//! The armer resolves the **whole `UPDATE.BIN` file** to block extents, so the extent chain's
//! byte stream is `64-byte OBCU header ‖ raw image` (`OBCU_Spec.md` §1). The [`StagedRef`]'s
//! `len`/`crc32` are the **raw-image** values (they must match the embedded header's own fields —
//! the codec enforces it). Both passes therefore skip the first [`HEADER_LEN`] bytes of the
//! chain: the verify CRC covers the raw image **only**, and the flash pass writes the raw image
//! (never the container header) to the app slot.
//!
//! ## Failure semantics (each is a host test)
//!
//! - **Verify mismatch** (bad CRC, foreign/diverging embedded header, chain too short, image
//!   over the slot): deterministic bad stage. The app slot was never touched, so the arm is
//!   cleared to `Idle` and the outcome is [`Outcome::StageRejected`] — a bad stage must never
//!   cost the running firmware (epic invariant 1).
//! - **SD read error** (any pass): could be a transient card wobble, so the arm is **not**
//!   cleared — [`Outcome::SdError`], state untouched, the caller backs off and retries (the card
//!   is life-support; recovery is reinsert + power cycle).
//! - **Readback mismatch / RRAM write error**: the flash pass is retried up to [`FLASH_RETRIES`]
//!   more times, then [`Outcome::FlashError`] — the caller halts (LED SOS) with the state still
//!   `Armed`, so the next power cycle retries from scratch (epic invariant 2).
//! - **Power loss anywhere**: nothing here writes the state page until the readback has passed,
//!   so a torn install re-enters as `Armed` and simply reruns. A torn *state-page* write itself
//!   decodes to `Idle` (the §2 CRC frame) — by then the slot already holds the verified image,
//!   so the device still boots it (only the trial/rollback bookkeeping is lost).

use crate::crc32::Crc32;
use crate::image::{ImageHeader, HEADER_LEN, MAX_IMAGE_LEN};
use crate::state::{decide, BootDecision, BootState, Extent, LastOutcome, OutcomeKind, StagedRef};

/// SD block size — extents are runs of these (`OBCU_Spec.md` §2.3).
pub const SD_BLOCK_LEN: usize = 512;

/// RRAMC write granularity: one 128-bit line. Every [`InstallIo::write_lines`] call is a whole
/// number of these, at a line-aligned address, by construction.
pub const RRAM_LINE_LEN: usize = 16;

/// How many times the flash pass is **retried** after a failed readback (or a failed RRAM
/// write) before the engine gives up with [`Outcome::FlashError`] — i.e. `1 + FLASH_RETRIES`
/// flash passes total.
pub const FLASH_RETRIES: u32 = 3;

/// The byte the image tail is padded with up to a whole RRAM line (matches the erased-RRAM
/// convention `Rramc::erase` emulates).
pub const PAD_BYTE: u8 = 0xFF;

/// The app slot the engine flashes into: base address (the app's link origin, `0x8000`) and
/// capacity in bytes. Passed in by the bootloader so the engine owns the "padded image must fit
/// the slot" check (host-tested) without hard-coding the board's memory map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// First byte of the slot (16-byte-line aligned).
    pub base: u32,
    /// Slot capacity, bytes.
    pub len: u32,
}

/// An IO operation failed. Deliberately carries nothing: the engine maps each failure by *which
/// call* failed (an SD read ⇒ [`Outcome::SdError`], a flash write/readback ⇒ retry then
/// [`Outcome::FlashError`]), so the driver-side error detail stays in the driver's own log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IoError;

/// Which pass the engine is in — for the driver's LED heartbeat and throughput measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Streaming the staged extents, CRC-checking before anything is erased (slow LED blink).
    Verify,
    /// Re-streaming the extents and writing the app slot (fast LED heartbeat).
    Flash,
    /// CRC over the freshly-written slot (fast LED heartbeat, like Flash).
    Readback,
}

/// The IO the engine drives — real SPI/RRAMC in `obc-boot`, a scriptable mock in the host tests.
/// Every method is infallible-or-[`IoError`]; the engine owns what each failure *means*.
pub trait InstallIo {
    /// Read `buf.len() / 512` whole 512-byte blocks, starting at **absolute** SD block
    /// `start_block`, into `buf` (`buf.len()` is always a non-zero multiple of [`SD_BLOCK_LEN`]).
    fn read_blocks(&mut self, start_block: u32, buf: &mut [u8]) -> Result<(), IoError>;

    /// Write `data` (16-byte-line aligned address, `data.len()` a non-zero multiple of
    /// [`RRAM_LINE_LEN`]) to absolute RRAM address `addr`.
    fn write_lines(&mut self, addr: u32, data: &[u8]) -> Result<(), IoError>;

    /// Read back `buf.len()` bytes from absolute RRAM address `addr` — on the device a plain
    /// memory-mapped slice copy (RRAM is XIP-readable), in the mock a read of its flash model
    /// (so the readback pass observably checks what [`write_lines`](Self::write_lines) wrote).
    fn read_flash(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), IoError>;

    /// Persist a boot state to the BOOT_STATE page (encode + 16-byte-line writes).
    fn write_state(&mut self, state: &BootState) -> Result<(), IoError>;

    /// Progress hook, called at the start of each pass and after every chunk: LED heartbeat +
    /// (rtt builds) wall-time/throughput measurement. Default: no-op.
    fn progress(&mut self, _phase: Phase, _done: u32, _total: u32) {}
}

/// What the bootloader must do after [`run`] returns. Every arm is terminal for this boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Boot the app in the slot: nothing was pending (`Idle`), or an unconfirmed first-install
    /// trial was accepted and cleared (`AcceptAndClear` — no snapshot exists to roll back to).
    Jump,
    /// The staged image failed verification **before anything was erased** — the arm was cleared
    /// to `Idle` and the old app is intact. Caller: error LED code, then jump.
    StageRejected,
    /// The image was flashed, readback-verified, and the follow-up state (`Trial` after an
    /// install, `Idle` after a rollback) written. Caller: **jump straight to the app slot** —
    /// never reset. A reset would re-enter the bootloader with the just-written `Trial`, which
    /// [`decide`] reads as an *unconfirmed* trial and rolls straight back; the trial boot must
    /// be the very next thing that runs.
    Installed,
    /// An SD read failed mid-pass. The state page was **not** touched (a transient card error
    /// must never clear a valid arm) — caller: LED code, back off, bring the card up again, and
    /// re-run; state-wise this boot never happened.
    SdError,
    /// The readback never matched (or an RRAM write failed) after `1 +` [`FLASH_RETRIES`] flash
    /// passes — or the post-flash state write itself failed. The state page still holds the
    /// `Armed`/`Trial` record, so the next power cycle retries from scratch. Caller: LED SOS,
    /// halt.
    FlashError,
}

/// Why a stream fill couldn't complete.
enum StreamError {
    /// The extent chain ran out before the requested bytes — a malformed stage (deterministic).
    Exhausted,
    /// An SD read failed — possibly transient.
    Io,
}

/// Byte-stream reader over an extent chain: hides the 512-byte block granularity so the passes
/// deal in plain byte runs (the 64-byte header skip and the tail chunk never align to blocks).
/// Whole-block spans are read straight into the caller's buffer (letting the SD driver use
/// multi-block reads); stragglers go through a one-block scratch.
struct ExtentStream<'a> {
    extents: &'a [Extent],
    /// Current extent index.
    idx: usize,
    /// Blocks already consumed from the current extent.
    blocks_done: u32,
    /// One-block scratch for partial-block reads.
    scratch: [u8; SD_BLOCK_LEN],
    /// Next unread byte in `scratch`.
    scratch_pos: usize,
    /// Valid bytes in `scratch` (0 = empty).
    scratch_len: usize,
}

impl<'a> ExtentStream<'a> {
    fn new(extents: &'a [Extent]) -> Self {
        ExtentStream { extents, idx: 0, blocks_done: 0, scratch: [0; SD_BLOCK_LEN], scratch_pos: 0, scratch_len: 0 }
    }

    /// The current extent's next absolute block, or `None` when the chain is exhausted (also
    /// treats a block-index overflow — garbage extents — as exhaustion; never panics).
    fn next_block(&mut self) -> Option<(u32, u32)> {
        loop {
            let e = self.extents.get(self.idx)?;
            let left = e.blocks.saturating_sub(self.blocks_done);
            if left == 0 {
                self.idx += 1;
                self.blocks_done = 0;
                continue;
            }
            let start = e.start_block.checked_add(self.blocks_done)?;
            return Some((start, left));
        }
    }

    /// Fill `out` completely with the next bytes of the chain's byte stream.
    fn fill(&mut self, io: &mut impl InstallIo, out: &mut [u8]) -> Result<(), StreamError> {
        let mut w = 0;
        while w < out.len() {
            // Drain the scratch block first.
            if self.scratch_pos < self.scratch_len {
                let n = (self.scratch_len - self.scratch_pos).min(out.len() - w);
                out[w..w + n].copy_from_slice(&self.scratch[self.scratch_pos..self.scratch_pos + n]);
                self.scratch_pos += n;
                w += n;
                continue;
            }
            let (start, left) = self.next_block().ok_or(StreamError::Exhausted)?;
            let whole = ((out.len() - w) / SD_BLOCK_LEN).min(left as usize);
            if whole > 0 {
                // Whole blocks go straight into the caller's buffer (multi-block read).
                io.read_blocks(start, &mut out[w..w + whole * SD_BLOCK_LEN]).map_err(|_| StreamError::Io)?;
                self.blocks_done += whole as u32;
                w += whole * SD_BLOCK_LEN;
            } else {
                // Less than a block wanted — stage one block in the scratch.
                io.read_blocks(start, &mut self.scratch).map_err(|_| StreamError::Io)?;
                self.blocks_done += 1;
                self.scratch_pos = 0;
                self.scratch_len = SD_BLOCK_LEN;
            }
        }
        Ok(())
    }
}

enum VerifyError {
    /// Deterministic bad stage — reject it (clear the arm; the old app was never touched).
    Mismatch,
    /// SD read failed — possibly transient; do NOT clear the arm.
    Io,
}

/// The verify pass: stream the whole extent chain, check the embedded OBCU header against the
/// staged record, and CRC the raw image (header skipped — see the module doc) **before anything
/// is erased**. Also owns the size gates: a zero/oversized image or one whose line-padded length
/// exceeds the slot is a mismatch (never a partial flash later).
fn verify(io: &mut impl InstallIo, staged: &StagedRef, slot: &Slot, buf: &mut [u8]) -> Result<(), VerifyError> {
    let len = staged.len;
    // `StagedRef` decode already pins len == header.image_len and crc32 == header.image_crc32;
    // these gates are about the slot, not internal consistency.
    if len == 0 || len > MAX_IMAGE_LEN || padded_len(len) > slot.len {
        return Err(VerifyError::Mismatch);
    }
    let mut stream = ExtentStream::new(staged.extents());
    // The chain starts with the staged file's own 64-byte OBCU header: it must decode and match
    // the header the armer recorded, or the blocks on card are not the image this arm described.
    let mut hdr = [0u8; HEADER_LEN];
    match stream.fill(io, &mut hdr) {
        Ok(()) => {}
        Err(StreamError::Exhausted) => return Err(VerifyError::Mismatch),
        Err(StreamError::Io) => return Err(VerifyError::Io),
    }
    if ImageHeader::decode(&hdr) != Some(staged.header) {
        return Err(VerifyError::Mismatch);
    }
    io.progress(Phase::Verify, 0, len);
    let mut crc = Crc32::new();
    let mut done = 0usize;
    while done < len as usize {
        let n = buf.len().min(len as usize - done);
        match stream.fill(io, &mut buf[..n]) {
            Ok(()) => {}
            Err(StreamError::Exhausted) => return Err(VerifyError::Mismatch),
            Err(StreamError::Io) => return Err(VerifyError::Io),
        }
        crc.update(&buf[..n]);
        done += n;
        io.progress(Phase::Verify, done as u32, len);
    }
    if crc.finalize() != staged.crc32 {
        return Err(VerifyError::Mismatch);
    }
    Ok(())
}

enum PassError {
    /// An SD read failed (transient — abort the engine run with state untouched).
    Sd,
    /// An RRAM write or readback failed (retry the flash pass, then give up).
    Flash,
}

/// One flash pass: re-stream the extents, skip the 64-byte container header, and write the raw
/// image to the slot in buffer-sized, 16-byte-line-aligned chunks, tail padded with [`PAD_BYTE`].
fn flash_pass(io: &mut impl InstallIo, staged: &StagedRef, slot: &Slot, buf: &mut [u8]) -> Result<(), PassError> {
    let len = staged.len as usize;
    let mut stream = ExtentStream::new(staged.extents());
    // Skip the container header (already validated by the verify pass). An exhausted chain here
    // means the card changed under us mid-install — report it as an SD problem, not a bad stage.
    let mut hdr = [0u8; HEADER_LEN];
    stream.fill(io, &mut hdr).map_err(|_| PassError::Sd)?;
    io.progress(Phase::Flash, 0, len as u32);
    let mut done = 0usize;
    while done < len {
        let n = buf.len().min(len - done);
        stream.fill(io, &mut buf[..n]).map_err(|_| PassError::Sd)?;
        // Pad the tail chunk up to a whole RRAM line (verify pinned padded_len ≤ slot.len, so
        // the pad never writes past the slot). Intermediate chunks are already line-multiples.
        let padded = n.div_ceil(RRAM_LINE_LEN) * RRAM_LINE_LEN;
        buf[n..padded].fill(PAD_BYTE);
        io.write_lines(slot.base + done as u32, &buf[..padded]).map_err(|_| PassError::Flash)?;
        done += n;
        io.progress(Phase::Flash, done as u32, len as u32);
    }
    Ok(())
}

/// The readback pass: CRC-32 over the just-written slot bytes (`slot.base .. slot.base + len`,
/// pad excluded — the CRC is defined over the raw image). `Ok(true)` = it matches the stage.
fn readback(io: &mut impl InstallIo, staged: &StagedRef, slot: &Slot, buf: &mut [u8]) -> Result<bool, PassError> {
    let len = staged.len as usize;
    io.progress(Phase::Readback, 0, len as u32);
    let mut crc = Crc32::new();
    let mut done = 0usize;
    while done < len {
        let n = buf.len().min(len - done);
        io.read_flash(slot.base + done as u32, &mut buf[..n]).map_err(|_| PassError::Flash)?;
        crc.update(&buf[..n]);
        done += n;
        io.progress(Phase::Readback, done as u32, len as u32);
    }
    Ok(crc.finalize() == staged.crc32)
}

/// Flash + readback with the retry policy: up to `1 +` [`FLASH_RETRIES`] flash passes, each
/// followed by a full readback. An SD read error aborts immediately (transient — the whole run
/// is retried by the caller); a write/readback failure consumes a retry.
fn flash_verified(io: &mut impl InstallIo, staged: &StagedRef, slot: &Slot, buf: &mut [u8]) -> Result<(), PassError> {
    let mut attempts_left = 1 + FLASH_RETRIES;
    loop {
        attempts_left -= 1;
        let failed = match flash_pass(io, staged, slot, buf) {
            Err(PassError::Sd) => return Err(PassError::Sd),
            Err(PassError::Flash) => true,
            Ok(()) => match readback(io, staged, slot, buf) {
                Err(PassError::Sd) => return Err(PassError::Sd),
                Err(PassError::Flash) => true,
                Ok(matched) => !matched,
            },
        };
        if !failed {
            return Ok(());
        }
        if attempts_left == 0 {
            return Err(PassError::Flash);
        }
    }
}

/// Line-padded image length: what the flash pass actually writes.
fn padded_len(len: u32) -> u32 {
    len.div_ceil(RRAM_LINE_LEN as u32) * RRAM_LINE_LEN as u32
}

/// Run the whole boot-time install engine for a decoded [`BootState`]: decide, then execute the
/// decision's verify → flash → readback → state-transition sequence over `io`, using `buf` as
/// the SD↔RRAM staging buffer (a non-zero multiple of [`SD_BLOCK_LEN`]; the bootloader passes
/// 4 KB). Returns what the bootloader must do next — see [`Outcome`]. Never panics on any state
/// content (the bootloader's standing rule); the only `debug_assert` is the caller's buffer
/// contract, which is a compile-time constant in `obc-boot` and pinned by the host tests.
pub fn run(state: &BootState, slot: &Slot, io: &mut impl InstallIo, buf: &mut [u8]) -> Outcome {
    debug_assert!(!buf.is_empty() && buf.len().is_multiple_of(SD_BLOCK_LEN), "buffer must be whole SD blocks");
    // Belt-and-braces for release: clamp to whole blocks; an unusable buffer halts (caller bug,
    // state left Armed) rather than corrupting arithmetic below.
    let whole = buf.len() - buf.len() % SD_BLOCK_LEN;
    if whole == 0 {
        return Outcome::FlashError;
    }
    let buf = &mut buf[..whole];

    match decide(state) {
        BootDecision::Jump => Outcome::Jump,

        // Unconfirmed trial with no snapshot (first install): accept the running image and
        // clear to Idle. A failed clear changes nothing observable — next boot re-accepts.
        BootDecision::AcceptAndClear => {
            let (installed, generation) = match state {
                BootState::Trial { installed, generation, .. } => (Some(*installed), *generation),
                _ => (None, 0), // unreachable via decide(); stay total rather than panic
            };
            // The running (trial) image is accepted as permanent — record it as Installed.
            let last_outcome = Some(LastOutcome { kind: OutcomeKind::Installed, generation });
            let _ = io.write_state(&BootState::Idle { installed, last_outcome });
            Outcome::Jump
        }

        BootDecision::Install(update) => {
            let (generation, rollback) = match state {
                BootState::Armed { generation, rollback, .. } => (*generation, *rollback),
                _ => (0, None), // unreachable via decide(); stay total rather than panic
            };
            match verify(io, &update, slot, buf) {
                // Bad stage, old app never touched: clear the arm (carrying forward the
                // outgoing image's header from the rollback snapshot, if the armer took one)
                // and boot the old app. If even the clear fails, still jump — the old app is
                // intact and the next boot repeats this same safe path.
                Err(VerifyError::Mismatch) => {
                    let last_outcome = Some(LastOutcome { kind: OutcomeKind::StageRejected, generation });
                    let _ = io.write_state(&BootState::Idle { installed: rollback.map(|r| r.header), last_outcome });
                    Outcome::StageRejected
                }
                Err(VerifyError::Io) => Outcome::SdError,
                Ok(()) => match flash_verified(io, &update, slot, buf) {
                    Err(PassError::Sd) => Outcome::SdError,
                    Err(PassError::Flash) => Outcome::FlashError,
                    // The slot now provably holds the image — record the single trial boot.
                    // A failed Trial write leaves Armed ⇒ halt; next power cycle re-installs.
                    Ok(()) => {
                        match io.write_state(&BootState::Trial { generation, installed: update.header, rollback }) {
                            Ok(()) => Outcome::Installed,
                            Err(_) => Outcome::FlashError,
                        }
                    }
                },
            }
        }

        // Unconfirmed trial with a snapshot: same engine, source = the rollback extents.
        BootDecision::Rollback(snapshot) => {
            let (installed, generation) = match state {
                BootState::Trial { installed, generation, .. } => (Some(*installed), *generation),
                _ => (None, 0), // unreachable via decide(); stay total rather than panic
            };
            match verify(io, &snapshot, slot, buf) {
                // The snapshot on card is bad and the trial image is what's in the slot — the
                // only bootable thing we have. Accept it (clear to Idle) rather than brick:
                // a rollback to garbage would cost the running firmware, which invariant 1
                // forbids in both directions. The trial image stuck, so record it as Installed.
                Err(VerifyError::Mismatch) => {
                    let last_outcome = Some(LastOutcome { kind: OutcomeKind::Installed, generation });
                    let _ = io.write_state(&BootState::Idle { installed, last_outcome });
                    Outcome::StageRejected
                }
                Err(VerifyError::Io) => Outcome::SdError,
                Ok(()) => match flash_verified(io, &snapshot, slot, buf) {
                    Err(PassError::Sd) => Outcome::SdError,
                    Err(PassError::Flash) => Outcome::FlashError,
                    // Rollback complete: straight to Idle (no trial for the known-good image).
                    Ok(()) => {
                        let last_outcome = Some(LastOutcome { kind: OutcomeKind::RolledBack, generation });
                        match io.write_state(&BootState::Idle { installed: Some(snapshot.header), last_outcome }) {
                            Ok(()) => Outcome::Installed,
                            Err(_) => Outcome::FlashError,
                        }
                    }
                },
            }
        }
    }
}
