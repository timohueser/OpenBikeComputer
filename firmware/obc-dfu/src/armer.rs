//! The app-side **armer**'s decision core (S4, #619) — pure and host-tested, like [`engine`].
//!
//! The board crate (`obc-fw-nrf54l`) owns the concrete FAT/SPI/RRAMC plumbing; everything that can
//! be *wrong* — the staging-scan validation matrix, the arm sequencing (snapshot **before** the
//! boot-state page write), the generation bump, the first-install no-rollback path, and the trial
//! confirm — lives here behind two small IO traits so the whole matrix runs on the host with mocks
//! (`tests/armer.rs`). The mirror of the [`engine`](crate::engine)/`obc-boot` split.
//!
//! Per `OBCU_Spec.md` §2.3 (normative, pinned in S3): the extent chain a [`StageIo`] resolves
//! covers the **whole staged file** — the 64-byte OBCU header is part of the chain — while the
//! [`StagedRef`]'s `len`/`crc32` stay **raw-image** values. [`scan`] validates exactly that shape;
//! the bootloader's install engine consumes it with the same skip arithmetic.
//!
//! [`engine`]: crate::engine

use crate::crc32::Crc32;
use crate::engine::IoError;
use crate::image::{ImageHeader, HEADER_LEN, MAX_IMAGE_LEN};
use crate::state::{BootState, Extent, StagedRef, MAX_EXTENTS};

/// Why the staging scan rejected `UPDATE.BIN`. Surfaced **verbatim** by S5's UI (and, until then,
/// by the `dfu-install` debug-link command as text via [`describe`](ScanError::describe)) — so the
/// variants are user-actionable, not internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanError {
    /// No `UPDATE.BIN` in the card root.
    Missing,
    /// The file is shorter than its own header claims (`64 + image_len`) — a torn copy.
    Truncated,
    /// The 64-byte OBCU header didn't decode: bad magic, wrong header version, or a failed
    /// header CRC — not an update image (or a torn header).
    BadHeader,
    /// The full CRC-32 pass over the image body didn't match the header — a corrupt copy.
    BadCrc,
    /// `image_len` exceeds [`MAX_IMAGE_LEN`] (the app slot) — the image can never be flashed.
    Oversize,
    /// The file resolves to more than [`MAX_EXTENTS`] block runs. Carries the true count; the
    /// fix is deleting + re-copying the file (fresh FAT allocation is contiguous).
    TooFragmented {
        /// The file's true extent count.
        extents: u32,
    },
    /// An SD read (or the FAT-chain walk) failed — possibly transient.
    Io,
}

impl ScanError {
    /// A short, stable, user-facing phrase per variant — what the debug link streams today and
    /// S5's error card shows tomorrow. (`TooFragmented` drops the count here; callers that can
    /// format append it.)
    pub fn describe(&self) -> &'static str {
        match self {
            ScanError::Missing => "no UPDATE.BIN in the card root",
            ScanError::Truncated => "UPDATE.BIN is shorter than its header claims (torn copy?)",
            ScanError::BadHeader => "UPDATE.BIN is not a valid update image (bad header)",
            ScanError::BadCrc => "UPDATE.BIN failed its CRC check (corrupt copy?)",
            ScanError::Oversize => "update image is too large for this device",
            ScanError::TooFragmented { .. } => "UPDATE.BIN is too fragmented — delete it and copy it again",
            ScanError::Io => "SD read failed — try again",
        }
    }
}

/// Why a [`StageIo::stage_extents`] resolve failed — folded into [`ScanError`] by [`scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtentsError {
    /// The chain has more runs than the caller's table (or the FAT walker's own cap). Carries
    /// the true count for the log/UI.
    TooFragmented {
        /// The file's true extent count.
        extents: u32,
    },
    /// A raw block read failed, or the volume geometry couldn't be safely described.
    Io,
}

/// The staged file, as the scan reads it — implemented over FatFs + the raw card on the board,
/// and over an in-memory fake in the host tests.
pub trait StageIo {
    /// `UPDATE.BIN`'s byte length, or `None` when the file is absent from the card root.
    fn stage_len(&mut self) -> Option<u32>;

    /// Read staged-file bytes at `offset` (the scan never reads past
    /// [`stage_len`](StageIo::stage_len)).
    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), IoError>;

    /// Resolve the staged file — the **whole file**, header included (spec §2.3) — to absolute
    /// 512-byte block runs, written into `out` in file order. Returns the run count.
    fn stage_extents(&mut self, out: &mut [Extent; MAX_EXTENTS]) -> Result<usize, ExtentsError>;
}

/// The staging scan + validation (issue #619 §1): find the file, decode its header, gate the
/// size, run the **full CRC-32 pass** over the image body, and resolve the whole-file extent
/// chain — returning the [`StagedRef`] the arm records, or the first [`ScanError`] hit. Nothing
/// is written anywhere; a failed scan costs nothing.
///
/// `chunk` is the caller's CRC staging buffer (any non-empty size; the board passes a small
/// stack buffer matching `sd.rs`'s 512-byte transfer idiom — no new resident statics).
pub fn scan(io: &mut impl StageIo, chunk: &mut [u8]) -> Result<StagedRef, ScanError> {
    debug_assert!(!chunk.is_empty(), "scan needs a non-empty CRC staging buffer");
    let file_len = io.stage_len().ok_or(ScanError::Missing)?;
    if (file_len as usize) < HEADER_LEN {
        return Err(ScanError::Truncated);
    }

    // The 64-byte OBCU header, decoded by the shared codec: valid CRC ⇒ `Some`, anything else
    // (bad magic / version / header CRC) is a typed reject before any bulk read.
    let mut hdr = [0u8; HEADER_LEN];
    io.read_stage(0, &mut hdr).map_err(|_| ScanError::Io)?;
    let header = ImageHeader::decode(&hdr).ok_or(ScanError::BadHeader)?;

    if header.image_len == 0 || header.image_len > MAX_IMAGE_LEN {
        return Err(ScanError::Oversize);
    }
    if (file_len as u64) < HEADER_LEN as u64 + header.image_len as u64 {
        return Err(ScanError::Truncated);
    }

    // Full CRC-32 pass over the image body through the byte source — the armer-side half of
    // "verify before erase" (the bootloader re-verifies over the raw extents).
    let mut crc = Crc32::new();
    let mut done = 0u32;
    while done < header.image_len {
        let n = chunk.len().min((header.image_len - done) as usize);
        io.read_stage(HEADER_LEN as u32 + done, &mut chunk[..n]).map_err(|_| ScanError::Io)?;
        crc.update(&chunk[..n]);
        done += n as u32;
    }
    if crc.finalize() != header.image_crc32 {
        return Err(ScanError::BadCrc);
    }

    // The whole-file extent chain (spec §2.3). The count gate is double-walled: the resolver
    // reports an over-long chain itself, and `StagedRef::new` re-rejects anything past
    // MAX_EXTENTS (it can't fail on len/crc — they come from the same header).
    let mut extents = [Extent::default(); MAX_EXTENTS];
    let count = match io.stage_extents(&mut extents) {
        Ok(n) if n <= MAX_EXTENTS => n,
        Ok(n) => return Err(ScanError::TooFragmented { extents: n as u32 }),
        Err(ExtentsError::TooFragmented { extents }) => return Err(ScanError::TooFragmented { extents }),
        Err(ExtentsError::Io) => return Err(ScanError::Io),
    };
    StagedRef::new(header, header.image_len, header.image_crc32, &extents[..count])
        .ok_or(ScanError::TooFragmented { extents: count as u32 })
}

/// What the arm recorded as its rollback — carried in the [`ArmTicket`] so the caller (today the
/// debug link, from S5 the UI) can warn on the no-rollback paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rollback {
    /// The running image was snapshotted to `ROLLBACK.BIN`; an unconfirmed trial restores it.
    Snapshot,
    /// First-ever install (dev-flashed device, `installed: None`): no snapshot exists, so an
    /// unconfirmed trial is accepted rather than rolled back (spec §2.4).
    FirstInstall,
    /// The boot-state page named an installed image, but the app slot no longer holds those
    /// bytes (an SWD reflash since the last install) — snapshotting would record a rollback the
    /// bootloader could never verify, so the arm proceeds without one, like a first install.
    RunningMismatch,
}

/// A successful arm: what was written to the boot-state page (minus the bulky extents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmTicket {
    /// The `Armed` record's generation — the old page's generation + 1.
    pub generation: u32,
    /// Which rollback story the record carries.
    pub rollback: Rollback,
}

/// Why an arm failed. Scan errors never reach here — [`arm`] takes an already-validated
/// [`StagedRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmError {
    /// The rollback snapshot couldn't be written/resolved. The arm is **aborted** (never
    /// silently armed without the rollback the state implied) — the boot-state page is untouched.
    Snapshot(ScanError),
    /// The boot-state page write failed. Nothing was armed (a torn page decodes to `Idle`).
    StateWrite,
}

/// The board-side effects [`arm`] sequences — a rollback snapshot and the page write.
pub trait ArmIo {
    /// Snapshot the running image (RRAM, `installed.image_len` bytes at the app slot base) to
    /// `/ROLLBACK.BIN` as a full OBCU container and extent-resolve it (whole-file chain, spec
    /// §2.3). `Ok(None)` = the slot's bytes no longer CRC-match `installed` (an SWD reflash) —
    /// arm without a rollback rather than record one the bootloader would reject.
    fn snapshot(&mut self, installed: &ImageHeader) -> Result<Option<StagedRef>, ScanError>;

    /// Persist `state` to the BOOT_STATE page (encode + 16-byte-line RRAM writes).
    fn write_state(&mut self, state: &BootState) -> Result<(), IoError>;
}

/// The arm sequence (issue #619 §3), order **normative**: snapshot the rollback *first*, then
/// compose and write the `Armed` page. A power cut before the page write = nothing happened (the
/// snapshot file is inert without the record pointing at it); after = the install proceeds. No
/// torn intermediate exists — the page is one CRC-framed blob.
///
/// `current` is the decoded boot-state page (read **before** composing — the generation bump is
/// `current.generation() + 1`). Only `Idle { installed: Some(_) }` yields a snapshot; a fresh
/// device (`installed: None`) — and, defensively, a non-`Idle` page that should never be live
/// mid-run — arms with `rollback: None` (the first-install story, spec §2.4).
pub fn arm(io: &mut impl ArmIo, current: &BootState, update: StagedRef) -> Result<ArmTicket, ArmError> {
    let installed = match current {
        BootState::Idle { installed } => *installed,
        // Armed/Trial can't be live mid-run (the bootloader consumes Armed; bring-up confirms
        // Trial) — stay total: treat like a fresh device rather than guess at a rollback.
        _ => None,
    };
    let (rollback, kind) = match installed {
        Some(h) => match io.snapshot(&h).map_err(ArmError::Snapshot)? {
            Some(snap) => (Some(snap), Rollback::Snapshot),
            None => (None, Rollback::RunningMismatch),
        },
        None => (None, Rollback::FirstInstall),
    };
    let generation = current.generation().wrapping_add(1);
    io.write_state(&BootState::Armed { generation, update, rollback }).map_err(|_| ArmError::StateWrite)?;
    Ok(ArmTicket { generation, rollback: kind })
}

/// The trial confirm (issue #619 §4): a healthy app — first frame presented, SD mounted — turns
/// `Trial { installed, .. }` into `Idle { installed: Some(installed) }`. Anything else (the
/// steady-state `Idle`, or a stale `Armed` that should be impossible mid-run) confirms nothing.
/// Returns the state to write plus the just-confirmed image's header (for the S5 toast).
pub fn confirm_trial(current: &BootState) -> Option<(BootState, ImageHeader)> {
    match current {
        BootState::Trial { installed, .. } => Some((BootState::Idle { installed: Some(*installed) }, *installed)),
        _ => None,
    }
}
