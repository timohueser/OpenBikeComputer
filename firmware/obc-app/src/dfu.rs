//! The app-side view of the SD-sideload firmware-update flow (epic #615 S5, #620).
//!
//! The scan/arm machinery runs **board-side** (`obc_dfu::armer` + `obc-fw-nrf54l`'s `dfu.rs`); the
//! app only posts the [`DfuAction`](crate::activity::DfuAction) one-shots and receives the scan's
//! answer through [`App::apply_event`](crate::App::apply_event). These two
//! types are that answer, kept **host-agnostic** (no `obc-dfu` dependency reaches `obc-app`): the
//! board maps `obc_dfu::ScanError` into a [`DfuScanError`] and fills a [`DfuScanReport`] with the
//! version strings it read off the card + the running image.
//!
//! Version strings are never translated — they are `git describe` identifiers, not UI copy — so
//! they ride as fixed inline buffers and the confirm screen prints them verbatim.

/// A firmware version string (`git describe`: `vMAJOR.MINOR.PATCH-N-gHASH`), the OBCU container's
/// 32-byte field. Sized to match `obc_dfu::image::FW_VERSION_LEN` without depending on that crate.
pub type Version = heapless::String<32>;

/// Copy `s` into a [`Version`], truncating to the buffer's cap on a char boundary. `pub` so a
/// fully-typed host constructing [`HostEvent::UpdateConfirmed`](crate::HostEvent::UpdateConfirmed) /
/// [`UpdateFailed`](crate::HostEvent::UpdateFailed) from its `&str` boot-outcome versions applies
/// the same bound.
pub fn clamp(s: &str) -> Version {
    let mut v = Version::new();
    let mut end = s.len().min(v.capacity());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let _ = v.push_str(&s[..end]);
    v
}

/// A successful staging scan's result, handed to
/// [`App::apply_event`](crate::App::apply_event) so the confirm screen can
/// show *installed → update* and warn on the no-undo / same-version cases (issue #620 §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DfuScanReport {
    /// The **running** firmware's version (the board's `OBC_FW_GIT`) — what an install replaces.
    pub installed: Version,
    /// The **staged** image's version, read from the validated `UPDATE.BIN` header.
    pub staged: Version,
    /// This install would arm with **no rollback snapshot** — knowable before arming from the
    /// boot-state page reading `Idle { installed: None }` (a dev-flashed device, spec §2.4). An
    /// unconfirmed trial is then *accepted* rather than rolled back, so the confirm screen notes
    /// there is no automatic undo. (The rarer running-mismatch no-rollback case needs the slot
    /// CRC, too heavy to read pre-confirm, so it is not surfaced here — see the PR.)
    pub first_install: bool,
}

impl DfuScanReport {
    /// Build a report from `&str` versions (each truncated to [`Version`]'s cap on a char
    /// boundary) — the board fills this from `OBC_FW_GIT` + the scanned header, and the sim from a
    /// synthetic scan, without either reaching for `heapless` directly.
    pub fn new(installed: &str, staged: &str, first_install: bool) -> DfuScanReport {
        DfuScanReport { installed: clamp(installed), staged: clamp(staged), first_install }
    }

    /// Whether the staged image is the **same version** already running — a plain byte-for-byte
    /// string match (issue #620 §2: "string compare is fine for equality"). The confirm screen
    /// warns on it; a true downgrade ("predates") isn't cleanly determinable from `git describe`
    /// strings, so only equality is flagged.
    pub fn same_version(&self) -> bool {
        self.installed == self.staged
    }
}

/// Why an **armed update is not the running firmware** — the boot-time reconcile's verdict, shown
/// once by the "UPDATE FAILED" card ([`DfuFailedScreen`](crate::screen::DfuFailedScreen)). The
/// board derives it from the boot-state page + the arm marker it left before the install reboot
/// (its `dfu::reconcile_boot_outcome`); the app only carries the fact to the card, like
/// [`DfuScanError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuFailure {
    /// The `Armed` record survived into a running app — the bootloader never consumed it (stale
    /// or missing bootloader). The board clears the leftover arm so it can't fire by surprise on
    /// some later reboot.
    NotStarted,
    /// The bootloader consumed the arm but the staged image is not what's running: it was
    /// rejected before the erase (old app intact) or its trial boot went unconfirmed and was
    /// rolled back.
    Reverted,
}

/// Why the **install drain refused or failed to arm** an update (issue #755) — the failure twin of
/// [`DfuScanError`], carried to the app by
/// [`App::apply_event`](crate::App::apply_event) so a live
/// [`DfuProgress`](crate::screen::DfuProgressScreen) spinner is replaced by the error card instead
/// of hanging forever. Every non-reboot outcome of the board's `DfuAction::Install` drain maps to
/// one of these. Kept **host-agnostic** like [`DfuScanError`]: the board maps its refusal guards and
/// `obc_dfu` arm errors into these buckets (the re-scan bucket reuses [`DfuScanError`]'s fold).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuInstallError {
    /// Refused: a ride is recording. Arming ends in a reboot that would lose the live ride, so the
    /// drain declines (the board's `is_tracking` guard).
    Recording,
    /// Refused: no SD card is mounted, so there is nothing to install from.
    NoCard,
    /// The arm re-scanned `UPDATE.BIN` (the confirm carried no validated ref) and it failed
    /// validation — folded into the same [`DfuScanError`] buckets the scan card shows.
    Scan(DfuScanError),
    /// Writing the rollback snapshot (`ROLLBACK.BIN`) to the card failed — an SD IO error before
    /// anything was armed.
    SnapshotFailed,
    /// An RRAM write on the arm path failed — the boot-state page, or (#1158) the sEMMC blob
    /// stage the bootloader needs to read the card. Either way nothing was armed; the device
    /// keeps running the old image. One bucket because the user story is identical ("could not
    /// prepare the update, nothing changed"); the board's `D`-line breadcrumb tells them apart.
    StateWriteFailed,
}

/// Why the staging scan rejected `UPDATE.BIN`, phrased for the app's error card (issue #620 §2).
/// The board folds `obc_dfu::ScanError`'s finer variants into these six user-facing buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuScanError {
    /// No `UPDATE.BIN` in the card root (`obc_dfu::ScanError::Missing`).
    NotFound,
    /// An SD read failed — possibly transient (`obc_dfu::ScanError::Io`).
    Unreadable,
    /// The file isn't a valid update image: bad magic/header, a failed CRC, or a torn/short copy
    /// (`obc_dfu::ScanError::{BadHeader, BadCrc, Truncated}`).
    Damaged,
    /// The image is larger than the app slot can hold (`obc_dfu::ScanError::Oversize`).
    TooLarge,
    /// The file resolves to too many block runs to install — the fix is deleting and re-copying it
    /// (`obc_dfu::ScanError::TooFragmented`).
    TooFragmented,
    /// The file is intact but **not trusted**: it carries no signature this firmware verifies, or a
    /// signature that doesn't check out against the release key
    /// (`obc_dfu::ScanError::{Unsigned, BadSignature}`, OBCU v2 / #997). Deliberately its own bucket
    /// rather than folded into [`Damaged`](Self::Damaged) — "this file is corrupt" and "this file is
    /// not ours" are different problems with different fixes, and telling a rider to re-copy a
    /// perfectly intact forged image would be a lie.
    Untrusted,
}

// ==================== DFU arm marker (boot-outcome popup) ====================
//
// The armer's breadcrumb: written to its settings-page line right after the `Armed` boot-state
// write, just before the reboot into the bootloader. At the next boot the board's
// `dfu::reconcile_boot_outcome` reads it back and — together with the boot-state page — derives
// the one-time verdict card: `Trial` = the confirm path owns it, `Armed` = the bootloader never
// ran the install, `Idle` + this marker = the staged version either accepted (it IS the installed
// header, first-install case) or failed (rejected / rolled back). Cleared wherever a verdict is
// delivered. Torn/blank/foreign decodes to `None` — "no arm happened", a plain boot.

/// The arm marker's fixed slot length: 3 whole 16-byte RRAM lines.
pub const ARM_MARKER_LEN: usize = 48;
/// The arm-marker tag; anything else there decodes to "no arm happened".
const ARM_MARKER_MAGIC: [u8; 4] = *b"OBCA";
/// Arm-marker layout version — bump on any field change (an old version reads as no marker).
const ARM_MARKER_VERSION: u8 = 1;
/// CRC-covered prefix: `magic(4) · version(1) · vlen(1) · pad(2) · generation u32 LE · version
/// string bytes(32)`.
const ARM_MARKER_PAYLOAD: usize = 44;

/// What the armer records before rebooting into the bootloader: the arm's generation and the
/// staged image's OBCU version string (the popup's "which update" fact).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmMarker {
    /// The `Armed` record's generation (the ticket the armer reported).
    pub generation: u32,
    /// The staged image's version string, verbatim from its OBCU header (≤ 32 bytes).
    pub staged: heapless::String<32>,
}

/// Pack an arm marker into its fixed [`ARM_MARKER_LEN`]-byte slot. Inverse of
/// [`decode_arm_marker`].
pub fn encode_arm_marker(m: &ArmMarker) -> [u8; ARM_MARKER_LEN] {
    let mut b = [0u8; ARM_MARKER_LEN];
    b[0..4].copy_from_slice(&ARM_MARKER_MAGIC);
    b[4] = ARM_MARKER_VERSION;
    let v = m.staged.as_bytes();
    let vlen = v.len().min(32);
    b[5] = vlen as u8;
    b[8..12].copy_from_slice(&m.generation.to_le_bytes());
    b[12..12 + vlen].copy_from_slice(&v[..vlen]);
    let crc = crate::store_meta::crc16(&b[0..ARM_MARKER_PAYLOAD]);
    b[ARM_MARKER_PAYLOAD..ARM_MARKER_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
    b
}

/// Decode an arm-marker slot, or `None` for anything but a clean read of this format — a blank
/// slot, a torn write, a short slice, an older layout, or a version string that isn't UTF-8.
/// `None` means **no arm happened**: the boot-outcome reconcile treats the boot as plain.
pub fn decode_arm_marker(bytes: &[u8]) -> Option<ArmMarker> {
    if bytes.len() < ARM_MARKER_LEN {
        return None;
    }
    let b = &bytes[..ARM_MARKER_LEN];
    if b[0..4] != ARM_MARKER_MAGIC || b[4] != ARM_MARKER_VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[ARM_MARKER_PAYLOAD], b[ARM_MARKER_PAYLOAD + 1]]);
    if crc != crate::store_meta::crc16(&b[0..ARM_MARKER_PAYLOAD]) {
        return None;
    }
    let vlen = b[5] as usize;
    if vlen > 32 {
        return None;
    }
    let mut staged: heapless::String<32> = heapless::String::new();
    staged.push_str(core::str::from_utf8(&b[12..12 + vlen]).ok()?).ok()?;
    Some(ArmMarker { generation: u32::from_le_bytes([b[8], b[9], b[10], b[11]]), staged })
}

#[cfg(test)]
mod arm_marker_tests {
    use super::*;

    /// The 48-byte arm-marker slot round-trips (generation + verbatim version string), and every
    /// torn/blank/foreign shape decodes to `None` — "no arm happened", a plain boot.
    #[test]
    fn arm_marker_codec_round_trips_and_rejects_torn_slots() {
        let m = ArmMarker { generation: 3, staged: heapless::String::try_from("v0.4.0-12-gabc1234").unwrap() };
        assert_eq!(decode_arm_marker(&encode_arm_marker(&m)), Some(m.clone()));
        let empty = ArmMarker { generation: 1, staged: heapless::String::new() };
        assert_eq!(decode_arm_marker(&encode_arm_marker(&empty)), Some(empty), "an empty version string is legal");

        assert_eq!(decode_arm_marker(&[0u8; ARM_MARKER_LEN]), None, "a blank (all-zero) slot is no marker");
        assert_eq!(decode_arm_marker(&[0xFF; ARM_MARKER_LEN]), None, "an erased (all-ones) slot is no marker");
        assert_eq!(decode_arm_marker(&encode_arm_marker(&m)[..ARM_MARKER_LEN - 1]), None, "a short slice is rejected");
        let mut torn = encode_arm_marker(&m);
        torn[15] ^= 0xFF; // flip a version-string byte without fixing the CRC — the torn-write shape
        assert_eq!(decode_arm_marker(&torn), None, "a CRC mismatch (torn write) is no marker");
        let mut old = encode_arm_marker(&m);
        old[4] = ARM_MARKER_VERSION + 1;
        let crc = crate::store_meta::crc16(&old[0..ARM_MARKER_PAYLOAD]);
        old[ARM_MARKER_PAYLOAD..ARM_MARKER_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_arm_marker(&old), None, "a foreign layout version is no marker");
        let mut bad_utf8 = encode_arm_marker(&m);
        bad_utf8[12] = 0xFF; // a non-UTF-8 version byte
        let crc = crate::store_meta::crc16(&bad_utf8[0..ARM_MARKER_PAYLOAD]);
        bad_utf8[ARM_MARKER_PAYLOAD..ARM_MARKER_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_arm_marker(&bad_utf8), None, "a non-UTF-8 version string is no marker");
    }
}
