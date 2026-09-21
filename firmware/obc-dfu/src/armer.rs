//! The app-side armer: the staging scan, the arm sequence, and the trial confirm, as pure logic
//! over two IO traits. The board crate owns the FAT, SPI and RRAMC plumbing.
//!
//! The armer is the trust boundary: [`scan`] verifies the container's Ed25519 signature against a
//! caller-supplied key before an arm is possible, and rejects unsigned containers. The bootloader
//! does not verify, because it is flashed once and can never be updated, so the trust root lives in
//! the half that ships with every image (see [`crate::sig`]).
//!
//! The extent chain a [`StageIo`] resolves covers the whole staged file, header included, while the
//! [`StagedRef`]'s `len`/`crc32` stay raw-image values (`OBCU_Spec.md`). The install engine
//! consumes it with the same skip arithmetic.

use crate::crc32::Crc32;
use crate::engine::IoError;
use crate::image::{ImageHeader, HEADER_LEN, MAX_IMAGE_LEN};
use crate::sig::{PublicKey, Verifier, SIG_LEN, SIG_SCHEME_ED25519};
use crate::state::{BootState, Extent, LastOutcome, OutcomeKind, StagedRef, MAX_EXTENTS};

/// Why the staging scan rejected `UPDATE.BIN`. The UI shows these verbatim through
/// [`describe`](ScanError::describe), so the variants are user-actionable, not internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanError {
    /// No `UPDATE.BIN` in the card root.
    Missing,
    /// The file is shorter than its own header claims: a torn copy.
    Truncated,
    /// The OBCU header did not decode: bad magic, wrong version, or a failed header CRC.
    BadHeader,
    /// The CRC-32 over the image body did not match the header: a corrupt copy.
    BadCrc,
    /// The container carries no signature this firmware can verify: an unsigned image, or a future
    /// scheme. Accepting these would make the signature bypassable by re-wrapping the payload.
    Unsigned,
    /// The signature does not verify against the key this firmware trusts.
    BadSignature,
    /// `image_len` exceeds [`MAX_IMAGE_LEN`], so the image can never be flashed.
    Oversize,
    /// The file resolves to more than [`MAX_EXTENTS`] block runs. The fix is to delete the file and
    /// copy it again, because a fresh FAT allocation is contiguous.
    TooFragmented { extents: u32 },
    /// An SD read or the FAT-chain walk failed; possibly transient.
    Io,
}

impl ScanError {
    /// A short user-facing phrase. `TooFragmented` drops the count; callers that can format it
    /// append it.
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

/// Why a [`StageIo::stage_extents`] resolve failed; [`scan`] folds it into a [`ScanError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtentsError {
    /// The chain has more runs than the caller's table, or than the FAT walker's own cap.
    TooFragmented { extents: u32 },
    /// A raw block read failed, or the volume geometry could not be described safely.
    Io,
}

pub trait StageIo {
    /// `UPDATE.BIN`'s byte length, or `None` when the file is absent from the card root.
    fn stage_len(&mut self) -> Option<u32>;

    /// The scan never reads past [`stage_len`](StageIo::stage_len).
    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), IoError>;

    /// Resolves the whole file, header included, to absolute 512-byte block runs in file order.
    /// Returns the run count.
    fn stage_extents(&mut self, out: &mut [Extent; MAX_EXTENTS]) -> Result<usize, ExtentsError>;
}

/// Finds `UPDATE.BIN`, decodes its header, gates the size and the signature scheme, CRCs and
/// verifies the body in one pass, then resolves the whole-file extent chain into the [`StagedRef`]
/// the arm records. Nothing is written, so a failed scan costs nothing.
///
/// `chunk` is the caller's staging buffer, of any non-empty size. The image is read once: every
/// byte feeds the CRC and the signature hash on the way past.
///
/// `key` is a plain parameter, never swapped behind a `cfg` or a feature, so the tests exercise the
/// path that ships. The board passes [`RELEASE_PUBKEY`](crate::sig::RELEASE_PUBKEY).
pub fn scan(io: &mut impl StageIo, chunk: &mut [u8], key: &PublicKey) -> Result<StagedRef, ScanError> {
    debug_assert!(!chunk.is_empty(), "scan needs a non-empty staging buffer");
    let file_len = io.stage_len().ok_or(ScanError::Missing)?;
    if (file_len as usize) < HEADER_LEN {
        return Err(ScanError::Truncated);
    }

    let mut hdr = [0u8; HEADER_LEN];
    io.read_stage(0, &mut hdr).map_err(|_| ScanError::Io)?;
    let header = ImageHeader::decode(&hdr).ok_or(ScanError::BadHeader)?;

    if header.image_len == 0 || header.image_len > MAX_IMAGE_LEN {
        return Err(ScanError::Oversize);
    }
    // The signature gate, before any bulk read: only the scheme this device verifies, at the
    // length it must have. An unsigned container and a future scheme both land here.
    if header.sig_scheme != SIG_SCHEME_ED25519 || header.sig_len as usize != SIG_LEN {
        return Err(ScanError::Unsigned);
    }
    // `container_len` counts the signature trailer, so a file that stops before it is truncated.
    if (file_len as u64) < header.container_len() {
        return Err(ScanError::Truncated);
    }

    // Read the trailer before the streaming pass, so a malformed signature or key costs nothing.
    let mut signature = [0u8; SIG_LEN];
    io.read_stage(header.sig_offset() as u32, &mut signature).map_err(|_| ScanError::Io)?;
    let mut verifier = Verifier::new(key, &header, &signature).map_err(|_| ScanError::BadSignature)?;

    let mut crc = Crc32::new();
    let mut done = 0u32;
    while done < header.image_len {
        let n = chunk.len().min((header.image_len - done) as usize);
        io.read_stage(HEADER_LEN as u32 + done, &mut chunk[..n]).map_err(|_| ScanError::Io)?;
        crc.update(&chunk[..n]);
        verifier.absorb(&chunk[..n]);
        done += n as u32;
    }
    // Corruption first: it is the likelier failure and the more actionable message.
    if crc.finalize() != header.image_crc32 {
        return Err(ScanError::BadCrc);
    }
    verifier.finish().map_err(|_| ScanError::BadSignature)?;

    let mut extents = [Extent::default(); MAX_EXTENTS];
    let count = match io.stage_extents(&mut extents) {
        Ok(n) => n,
        Err(ExtentsError::TooFragmented { extents }) => return Err(ScanError::TooFragmented { extents }),
        Err(ExtentsError::Io) => return Err(ScanError::Io),
    };
    StagedRef::new(header, header.image_len, header.image_crc32, &extents[..count])
        .ok_or(ScanError::TooFragmented { extents: count as u32 })
}

/// What the arm recorded as its rollback, so the caller can warn on the paths without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rollback {
    /// The running image was snapshotted to `ROLLBACK.BIN`; an unconfirmed trial restores it.
    Snapshot,
    /// First install: no snapshot exists, so an unconfirmed trial is accepted, not rolled back.
    FirstInstall,
    /// The boot-state page named an installed image, but the app slot no longer holds those bytes,
    /// after an SWD reflash. A snapshot would record a rollback the bootloader could never verify,
    /// so the arm proceeds without one.
    RunningMismatch,
}

/// A successful arm: what was written to the boot-state page, without the bulky extents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmTicket {
    pub generation: u32,
    pub rollback: Rollback,
}

/// Why an arm failed. Scan errors never reach here: [`arm`] takes a validated [`StagedRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmError {
    /// The rollback snapshot could not be written or resolved. The arm aborts with the boot-state
    /// page untouched, rather than arm without the rollback the state implied.
    Snapshot(ScanError),
    /// The storage-blob stage could not be written or verified. The arm aborts: an `Armed` page
    /// whose carve the bootloader cannot validate would only be abandoned on the next boot.
    BlobStage,
    /// The boot-state page write failed. Nothing was armed; a torn page decodes to `Idle`.
    StateWrite,
}

/// The board-side effects [`arm`] sequences.
pub trait ArmIo {
    /// Writes the running image to `/ROLLBACK.BIN` as an OBCU container and resolves its extents.
    /// `Ok(None)` means the slot's bytes no longer CRC-match `installed`, so the arm proceeds
    /// without a rollback rather than record one the bootloader would reject.
    ///
    /// The snapshot is an unsigned container: the device cannot re-create the original signature
    /// from slot bytes, and nothing verifies one, because the bootloader's rollback path checks the
    /// snapshot by CRC.
    fn snapshot(&mut self, installed: &ImageHeader) -> Result<Option<StagedRef>, ScanError>;

    /// Stages the sEMMC soft-peripheral image the bootloader boots the card through into the
    /// `SEMMC_STAGE` RRAM carve: blob body first, the CRC-framed header line last, then a readback.
    /// It is idempotent, and inert without an `Armed` record that points at it.
    fn stage_boot_blob(&mut self) -> Result<(), IoError>;

    fn write_state(&mut self, state: &BootState) -> Result<(), IoError>;
}

/// The arm sequence. The order is normative: snapshot the rollback first, then stage the storage
/// blob, then write the `Armed` page. A power cut before the page write means nothing happened,
/// because the snapshot file and the staged blob are both inert without the record that points at
/// them. The order also means a valid `Armed` page implies a valid blob carve.
///
/// Only `Idle { installed: Some(_) }` makes a snapshot. A fresh device, and defensively a non-`Idle`
/// page, arms with `rollback: None`.
pub fn arm(io: &mut impl ArmIo, current: &BootState, update: StagedRef) -> Result<ArmTicket, ArmError> {
    let installed = match current {
        BootState::Idle { installed, .. } => *installed,
        // Armed and Trial cannot be live mid-run, so treat them like a fresh device rather than
        // guess at a rollback.
        _ => None,
    };
    let (rollback, kind) = match installed {
        Some(h) => match io.snapshot(&h).map_err(ArmError::Snapshot)? {
            Some(snap) => (Some(snap), Rollback::Snapshot),
            None => (None, Rollback::RunningMismatch),
        },
        None => (None, Rollback::FirstInstall),
    };
    // The blob stage is before the commit point, so a failed stage aborts with the page untouched.
    io.stage_boot_blob().map_err(|_| ArmError::BlobStage)?;
    let generation = current.generation().wrapping_add(1);
    io.write_state(&BootState::Armed { generation, update, rollback }).map_err(|_| ArmError::StateWrite)?;
    Ok(ArmTicket { generation, rollback: kind })
}

/// The trial confirm: a healthy app turns a `Trial` page into an `Idle` that records `Installed`.
/// Any other state confirms nothing. Returns the state to write and the confirmed image's header.
pub fn confirm_trial(current: &BootState) -> Option<(BootState, ImageHeader)> {
    match current {
        BootState::Trial { installed, generation, .. } => {
            let last_outcome = Some(LastOutcome { kind: OutcomeKind::Installed, generation: *generation });
            Some((BootState::Idle { installed: Some(*installed), last_outcome }, *installed))
        }
        _ => None,
    }
}
