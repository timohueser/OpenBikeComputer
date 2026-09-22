//! The bootloader's install engine: verify, flash, readback, then the state transition.
//!
//! All install sequencing lives here as a pure driver over [`InstallIo`], so `obc-boot` only wires
//! real SPI block reads and RRAMC line writes into it and maps the returned [`Outcome`] to an LED
//! pattern and a jump, reset or halt.
//!
//! The armer resolves the whole staged object, so the extent chain reads as `64-byte OBCU
//! header ‖ raw image` (`OBCU_Spec.md`). Both passes skip the first [`HEADER_LEN`] bytes: the
//! verify CRC covers the raw image only, and the flash pass writes the raw image to the app slot.
//!
//! Nothing writes the state page before the readback passes, so a torn install re-enters as
//! `Armed` and reruns.

use crate::crc32::Crc32;
use crate::image::{ImageHeader, HEADER_LEN, MAX_IMAGE_LEN};
use crate::state::{decide, BootDecision, BootState, Extent, LastOutcome, OutcomeKind, StagedRef};

pub const SD_BLOCK_LEN: usize = 512;

/// RRAMC write granularity. Every [`InstallIo::write_lines`] call is a whole number of these, at a
/// line-aligned address.
pub const RRAM_LINE_LEN: usize = 16;

/// Retries of the flash pass after a failed readback or RRAM write: `1 + FLASH_RETRIES` passes in
/// total, then [`Outcome::FlashError`].
pub const FLASH_RETRIES: u32 = 3;

/// The byte that pads the image tail up to a whole RRAM line; it matches erased RRAM.
pub const PAD_BYTE: u8 = 0xFF;

/// The bootloader passes the slot in, so the engine checks that the padded image fits without
/// knowing the board's memory map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// First byte of the slot; 16-byte-line aligned.
    pub base: u32,
    pub len: u32,
}

/// An IO operation failed. The engine maps a failure by which call failed, so the driver keeps the
/// error detail in its own log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IoError;

/// Which pass the engine is in; the driver uses it for the LED heartbeat and throughput.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Verify,
    Flash,
    Readback,
}

/// The IO the engine drives. The engine owns what each failure means.
pub trait InstallIo {
    /// Reads whole blocks from absolute SD block `start_block`. `buf.len()` is always a non-zero
    /// multiple of [`SD_BLOCK_LEN`].
    fn read_blocks(&mut self, start_block: u32, buf: &mut [u8]) -> Result<(), IoError>;

    /// Writes to absolute RRAM address `addr`, which is line-aligned; `data.len()` is a non-zero
    /// multiple of [`RRAM_LINE_LEN`].
    fn write_lines(&mut self, addr: u32, data: &[u8]) -> Result<(), IoError>;

    /// Reads back from absolute RRAM address `addr`. The read must observe what
    /// [`write_lines`](Self::write_lines) wrote.
    fn read_flash(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), IoError>;

    fn write_state(&mut self, state: &BootState) -> Result<(), IoError>;

    /// Called at the start of each pass and after every chunk.
    fn progress(&mut self, _phase: Phase, _done: u32, _total: u32) {}
}

/// What the bootloader must do after [`run`] returns; every variant is terminal for this boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Boot the app in the slot: nothing was pending, or an unconfirmed first-install trial was
    /// accepted and cleared.
    Jump,
    /// The staged image failed verification before anything was erased. The arm is cleared and the
    /// old app is intact, so the caller shows an error code and jumps.
    StageRejected,
    /// The caller gave up on an `Armed` card it could not read, before the flash pass began, and
    /// the arm is cleared. The old app is intact, so the caller shows an error code and jumps.
    /// Only [`abandon_arm`] returns it.
    ArmAbandoned,
    /// The image was flashed, readback-verified, and the follow-up state written. The caller must
    /// jump straight to the app slot, never reset: a reset would re-enter the bootloader with the
    /// just-written `Trial` and roll it back before the trial image ever ran.
    Installed,
    /// An SD read failed mid-pass. The state page was not touched, because a transient card error
    /// must never clear a valid arm. The caller backs off, brings the card up again, and re-runs.
    ///
    /// `pre_erase` is true only when the failure was in an `Install`'s verify pass, before any slot
    /// byte could have been written. Only then may the caller give up and call [`abandon_arm`].
    /// When it is false the slot may be half-written, or the run was a `Rollback` whose trial image
    /// is the only bootable thing, and abandoning would brick the device.
    SdError { pre_erase: bool },
    /// The readback never matched, an RRAM write failed after all the retries, or the follow-up
    /// state write failed. The state page still holds the `Armed`/`Trial` record, so the next power
    /// cycle retries from the start. The caller halts.
    FlashError,
}

enum StreamError {
    /// The extent chain ran out before the requested bytes: a malformed stage.
    Exhausted,
    /// An SD read failed; possibly transient.
    Io,
}

/// Byte-stream reader over an extent chain. It hides the 512-byte block granularity, because the
/// header skip and the tail chunk do not align to blocks. Whole-block spans go straight into the
/// caller's buffer; shorter reads go through a one-block scratch.
struct ExtentStream<'a> {
    extents: &'a [Extent],
    idx: usize,
    blocks_done: u32,
    scratch: [u8; SD_BLOCK_LEN],
    scratch_pos: usize,
    scratch_len: usize,
}

impl<'a> ExtentStream<'a> {
    fn new(extents: &'a [Extent]) -> Self {
        ExtentStream { extents, idx: 0, blocks_done: 0, scratch: [0; SD_BLOCK_LEN], scratch_pos: 0, scratch_len: 0 }
    }

    /// The next absolute block, or `None` when the chain is exhausted. A block-index overflow from
    /// garbage extents also reads as exhaustion, so this never panics.
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

    /// Fills `out` completely from the chain's byte stream.
    fn fill(&mut self, io: &mut impl InstallIo, out: &mut [u8]) -> Result<(), StreamError> {
        let mut w = 0;
        while w < out.len() {
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
                io.read_blocks(start, &mut out[w..w + whole * SD_BLOCK_LEN]).map_err(|_| StreamError::Io)?;
                self.blocks_done += whole as u32;
                w += whole * SD_BLOCK_LEN;
            } else {
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
    /// A bad stage: reject it and clear the arm; the old app was never touched.
    Mismatch,
    /// An SD read failed. It can be transient, so the arm must not be cleared.
    Io,
}

/// Streams the whole chain, checks the embedded header against the staged record, and CRCs the raw
/// image, all before anything is erased. A zero or oversized image, or one whose padded length
/// exceeds the slot, is a mismatch here rather than a partial flash later.
fn verify(io: &mut impl InstallIo, staged: &StagedRef, slot: &Slot, buf: &mut [u8]) -> Result<(), VerifyError> {
    let len = staged.len;
    // The codec already pins len and crc32 against the header; these gates are about the slot.
    if len == 0 || len > MAX_IMAGE_LEN || padded_len(len) > slot.len {
        return Err(VerifyError::Mismatch);
    }
    let mut stream = ExtentStream::new(staged.extents());
    // The chain starts with the file's own OBCU header. It must match the header the armer
    // recorded, or the blocks on card are not the image this arm described.
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
    /// An SD read failed; abort the run with the state untouched.
    Sd,
    /// An RRAM write or readback failed; retry the flash pass, then give up.
    Flash,
}

/// Re-streams the extents, skips the container header, and writes the raw image to the slot in
/// line-aligned chunks, with the tail padded with [`PAD_BYTE`].
fn flash_pass(io: &mut impl InstallIo, staged: &StagedRef, slot: &Slot, buf: &mut [u8]) -> Result<(), PassError> {
    let len = staged.len as usize;
    let mut stream = ExtentStream::new(staged.extents());
    // Skip the container header. An exhausted chain here means the card changed mid-install, so it
    // is an SD problem, not a bad stage.
    let mut hdr = [0u8; HEADER_LEN];
    stream.fill(io, &mut hdr).map_err(|_| PassError::Sd)?;
    io.progress(Phase::Flash, 0, len as u32);
    let mut done = 0usize;
    while done < len {
        let n = buf.len().min(len - done);
        stream.fill(io, &mut buf[..n]).map_err(|_| PassError::Sd)?;
        // Pad the tail chunk up to a whole RRAM line. Verify pinned padded_len <= slot.len, so the
        // pad never writes past the slot.
        let padded = n.div_ceil(RRAM_LINE_LEN) * RRAM_LINE_LEN;
        // An off-contract buffer fails the pass instead of panicking.
        let chunk = buf.get_mut(..padded).ok_or(PassError::Flash)?;
        chunk[n..].fill(PAD_BYTE);
        io.write_lines(slot.base + done as u32, chunk).map_err(|_| PassError::Flash)?;
        done += n;
        io.progress(Phase::Flash, done as u32, len as u32);
    }
    Ok(())
}

/// CRCs the just-written slot bytes, pad excluded, because the CRC is defined over the raw image.
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

/// Up to `1 +` [`FLASH_RETRIES`] flash passes, each followed by a full readback. An SD read error
/// aborts immediately; a write or readback failure uses one retry.
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

fn padded_len(len: u32) -> u32 {
    len.div_ceil(RRAM_LINE_LEN as u32) * RRAM_LINE_LEN as u32
}

// An image at `MAX_IMAGE_LEN` fills the slot, so its padded length must not spill past the end.
const _: () = assert!(crate::layout::APP_SLOT_LEN.is_multiple_of(RRAM_LINE_LEN as u32));

/// The shared pipeline behind an `Install` and a `Rollback`, so the two safety paths cannot
/// diverge. `mismatch_state` is written when `verify` rejects the stage, `success_state` after a
/// verified flash, and `verify_io_pre_erase` says whether a verify-pass SD error leaves the arm
/// abandonable: true only for an `Install`, whose old app is still intact.
fn install_pipeline(
    io: &mut impl InstallIo,
    source: &StagedRef,
    slot: &Slot,
    buf: &mut [u8],
    verify_io_pre_erase: bool,
    mismatch_state: &BootState,
    success_state: &BootState,
) -> Outcome {
    match verify(io, source, slot, buf) {
        // Bad stage, old app never touched: clear the arm and boot the old app. If even the clear
        // fails, still jump; the next boot repeats this same safe path.
        Err(VerifyError::Mismatch) => {
            let _ = io.write_state(mismatch_state);
            Outcome::StageRejected
        }
        Err(VerifyError::Io) => Outcome::SdError { pre_erase: verify_io_pre_erase },
        Ok(()) => match flash_verified(io, source, slot, buf) {
            // The flash pass has begun, so the slot may be half-written: never abandonable.
            Err(PassError::Sd) => Outcome::SdError { pre_erase: false },
            Err(PassError::Flash) => Outcome::FlashError,
            // The slot now holds the image. A failed state write leaves the Armed or Trial record,
            // so the next power cycle retries from the start.
            Ok(()) => match io.write_state(success_state) {
                Ok(()) => Outcome::Installed,
                Err(_) => Outcome::FlashError,
            },
        },
    }
}

/// Decides from the [`BootState`], then runs that decision over `io`. `buf` is the staging buffer
/// and must be a non-zero multiple of [`SD_BLOCK_LEN`]. It never panics on any state content.
pub fn run(state: &BootState, slot: &Slot, io: &mut impl InstallIo, buf: &mut [u8]) -> Outcome {
    debug_assert!(!buf.is_empty() && buf.len().is_multiple_of(SD_BLOCK_LEN), "buffer must be whole SD blocks");
    // Clamp to whole blocks; an unusable buffer halts instead of corrupting the arithmetic below.
    let whole = buf.len() - buf.len() % SD_BLOCK_LEN;
    if whole == 0 {
        return Outcome::FlashError;
    }
    let buf = &mut buf[..whole];

    match decide(state) {
        BootDecision::Jump => Outcome::Jump,

        // A failed clear changes nothing: the next boot accepts the image again.
        BootDecision::AcceptAndClear { installed, generation } => {
            let last_outcome = Some(LastOutcome { kind: OutcomeKind::Installed, generation });
            let _ = io.write_state(&BootState::Idle { installed: Some(installed), last_outcome });
            Outcome::Jump
        }

        BootDecision::Install { update, generation, rollback } => {
            // Bad stage: clear the arm, carry the outgoing image's header forward from the
            // snapshot, and boot the old app. A bad stage must never cost the running firmware.
            let mismatch = BootState::Idle {
                installed: rollback.map(|r| r.header),
                last_outcome: Some(LastOutcome { kind: OutcomeKind::StageRejected, generation }),
            };
            // The snapshot rides into the Trial record: an unconfirmed Trial rolls back next boot.
            let success = BootState::Trial { generation, installed: update.header, rollback };
            // A verify-pass SD error is abandonable here: the old app is still intact.
            install_pipeline(io, &update, slot, buf, true, &mismatch, &success)
        }

        BootDecision::Rollback { snapshot, installed, generation } => {
            // The snapshot on card is bad, and the trial image in the slot is the only bootable
            // thing. Accept it instead of flashing garbage over it.
            let mismatch = BootState::Idle {
                installed: Some(installed),
                last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation }),
            };
            // The restored image is known good, so it gets no trial boot.
            let success = BootState::Idle {
                installed: Some(snapshot.header),
                last_outcome: Some(LastOutcome { kind: OutcomeKind::RolledBack, generation }),
            };
            // A Rollback is never abandonable: the trial image in the slot is the only bootable
            // thing, so even a verify-pass card error must keep retrying.
            install_pipeline(io, &snapshot, slot, buf, false, &mismatch, &success)
        }
    }
}

/// Abandons an `Armed` arm the bootloader could not bring the card up for: clears the state to
/// `Idle` and records [`OutcomeKind::ArmAbandoned`] against the arm's `generation`.
///
/// The caller must call this only after a [`run`] that returned [`Outcome::SdError`] with
/// `pre_erase: true`. Once the flash pass has begun the slot may be half-written, and clearing the
/// arm then would strand a bricked slot. The driver owns the retry count that decides when to give
/// up. A non-`Armed` state writes nothing and returns [`Outcome::Jump`].
pub fn abandon_arm(state: &BootState, io: &mut impl InstallIo) -> Outcome {
    match state {
        BootState::Armed { generation, rollback, .. } => {
            let last_outcome = Some(LastOutcome { kind: OutcomeKind::ArmAbandoned, generation: *generation });
            // The app slot is never touched. If even this clear fails, the caller still boots the
            // intact old app.
            let installed = rollback.as_ref().map(|r| r.header);
            let _ = io.write_state(&BootState::Idle { installed, last_outcome });
            Outcome::ArmAbandoned
        }
        _ => Outcome::Jump,
    }
}
