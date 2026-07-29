//! The app-side **armer**'s decision core (S4, #619) — pure and host-tested, like [`engine`].
//!
//! The board crate (`obc-fw-nrf54l`) owns the concrete FAT/SPI/RRAMC plumbing; everything that can
//! be *wrong* — the staging-scan validation matrix, the arm sequencing (snapshot **before** the
//! boot-state page write), the generation bump, the first-install no-rollback path, and the trial
//! confirm — lives here behind two small IO traits so the whole matrix runs on the host with mocks
//! (`tests/armer.rs`). The mirror of the [`engine`](crate::engine)/`obc-boot` split.
//!
//! Since #997 the armer is also the **trust boundary**: [`scan`] verifies the container's Ed25519
//! signature (`OBCU_Spec.md` §1.3) against a caller-supplied key before an arm is even possible, and
//! rejects unsigned/v1 containers outright. The 32 KB bootloader deliberately does not verify — it is
//! flashed once and can never be updated, so the trust root lives in the half that ships with every
//! image (see [`crate::sig`]).
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
use crate::sig::{PublicKey, Verifier, SIG_LEN, SIG_SCHEME_ED25519};
use crate::state::{BootState, Extent, LastOutcome, OutcomeKind, StagedRef, MAX_EXTENTS};

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
    /// The container carries **no signature this firmware can verify** (`OBCU_Spec.md` §1.3): either
    /// a plain v1/unsigned image, or a `sig_scheme` from some future scheme. Rejected, not merely
    /// warned about — accepting unsigned containers would make the signature bypassable by simply
    /// re-wrapping a payload the v1 way.
    Unsigned,
    /// The container is signed, the CRC passed, and the **Ed25519 signature did not verify** against
    /// the key this firmware trusts: a forged or tampered image, a re-labelled version/length, or an
    /// image signed by a key that isn't ours.
    BadSignature,
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
            ScanError::Unsigned => "UPDATE.BIN is not signed — this device only installs signed updates",
            ScanError::BadSignature => "UPDATE.BIN's signature is not valid for this device",
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

/// The staging scan + validation (issue #619 §1, extended by #997): find the file, decode its
/// header, gate the size, gate the **signature scheme**, run the **full CRC-32 pass** over the image
/// body *and the Ed25519 verification in the same pass*, then resolve the whole-file extent chain —
/// returning the [`StagedRef`] the arm records, or the first [`ScanError`] hit. Nothing is written
/// anywhere; a failed scan costs nothing.
///
/// `chunk` is the caller's staging buffer (any non-empty size; the board passes a small stack buffer
/// matching `sd.rs`'s 512-byte transfer idiom — no new resident statics). The image is read
/// **once**: every byte is fed to the CRC and to the signature hash on the way past, so adding
/// verification cost no extra card traffic.
///
/// `key` is the **verify-before-arm seam** (#997): the board passes
/// [`RELEASE_PUBKEY`](crate::sig::RELEASE_PUBKEY), the host tests pass a test key. It is a plain
/// parameter on purpose — the trusted key is never swapped behind a `cfg`/feature, so the code path
/// the tests exercise is exactly the one that ships.
///
/// **Policy** (`OBCU_Spec.md` §1.4): an unsigned (v1) container is *rejected*, not merely flagged.
/// CRC-32 remains the corruption check it always was — it runs first, so a corrupt copy still reads
/// as "damaged", not "untrusted".
pub fn scan(io: &mut impl StageIo, chunk: &mut [u8], key: &PublicKey) -> Result<StagedRef, ScanError> {
    debug_assert!(!chunk.is_empty(), "scan needs a non-empty staging buffer");
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
    // The signature gate, before any bulk read: only the scheme we verify, at the length it must
    // be. A v1/unsigned container and a future scheme both land here — this device cannot vouch
    // for either, and "install it anyway" is precisely the bypass v2 exists to close.
    if header.sig_scheme != SIG_SCHEME_ED25519 || header.sig_len as usize != SIG_LEN {
        return Err(ScanError::Unsigned);
    }
    // `container_len` counts the signature trailer, so a file that stops before it is Truncated.
    if (file_len as u64) < header.container_len() {
        return Err(ScanError::Truncated);
    }

    // The trailer, read before the streaming pass so a malformed signature or key costs nothing.
    let mut signature = [0u8; SIG_LEN];
    io.read_stage(header.sig_offset() as u32, &mut signature).map_err(|_| ScanError::Io)?;
    let mut verifier = Verifier::new(key, &header, &signature).map_err(|_| ScanError::BadSignature)?;

    // One pass over the image body: the CRC-32 (the armer-side half of "verify before erase" —
    // the bootloader re-CRCs over the raw extents) and the signature hash together.
    let mut crc = Crc32::new();
    let mut done = 0u32;
    while done < header.image_len {
        let n = chunk.len().min((header.image_len - done) as usize);
        io.read_stage(HEADER_LEN as u32 + done, &mut chunk[..n]).map_err(|_| ScanError::Io)?;
        crc.update(&chunk[..n]);
        verifier.absorb(&chunk[..n]);
        done += n as u32;
    }
    // Corruption first (it is the likelier failure and the more actionable message), then trust.
    if crc.finalize() != header.image_crc32 {
        return Err(ScanError::BadCrc);
    }
    verifier.finish().map_err(|_| ScanError::BadSignature)?;

    // The whole-file extent chain (spec §2.3). The too-fragmented count gate has two real walls: the
    // fixed-capacity `[Extent; MAX_EXTENTS]` buffer physically caps what `stage_extents` can write
    // (its contract returns the run count, so a correct impl cannot report `Ok(n > MAX_EXTENTS)`),
    // and it reports an over-long chain itself via `ExtentsError::TooFragmented`. `StagedRef::new`'s
    // own `> MAX_EXTENTS` reject then stands as belt-and-braces (it can't fail on len/crc — they come
    // from the same header).
    let mut extents = [Extent::default(); MAX_EXTENTS];
    let count = match io.stage_extents(&mut extents) {
        Ok(n) => n,
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
    ///
    /// The snapshot is written as an **unsigned** container ([`ImageHeader::unsigned`]): the device
    /// cannot re-create the original signature from slot bytes alone, and nothing verifies one —
    /// the snapshot never passes through [`scan`], and the bootloader's rollback path checks it by
    /// CRC. Marking it signed would be a lie in a file `obc-mkimage inspect` reads.
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
        BootState::Idle { installed, .. } => *installed,
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
/// `Trial { installed, generation, .. }` into `Idle { installed: Some(installed), last_outcome:
/// Installed(generation) }`. Anything else (the steady-state `Idle`, or a stale `Armed` that should
/// be impossible mid-run) confirms nothing. Returns the state to write plus the just-confirmed
/// image's header (for the S5 toast).
///
/// The confirm path surfaces its own success toast and clears the arm marker in the same beat, so
/// this `Idle`'s recorded outcome is normally only read as history — but recording `Installed` keeps
/// the page honest if a later boot re-reads it with the marker still present (a torn marker clear).
pub fn confirm_trial(current: &BootState) -> Option<(BootState, ImageHeader)> {
    match current {
        BootState::Trial { installed, generation, .. } => {
            let last_outcome = Some(LastOutcome { kind: OutcomeKind::Installed, generation: *generation });
            Some((BootState::Idle { installed: Some(*installed), last_outcome }, *installed))
        }
        _ => None,
    }
}
