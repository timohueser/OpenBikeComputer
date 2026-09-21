//! The app-side view of the firmware-update flow.
//!
//! The scan and arm machinery runs board-side; the app posts the
//! [`DfuAction`](crate::activity::DfuAction) one-shots and receives the answer through the pass's
//! fact stage. The types here are that answer, kept host-agnostic so no `obc-dfu` dependency
//! reaches `obc-app`: the board maps its own errors into a [`DfuScanError`] and fills a
//! [`DfuScanReport`].
//!
//! Version strings are `git describe` identifiers, not UI copy, so they are never translated: they
//! ride as fixed inline buffers and the confirm screen prints them verbatim.

/// A firmware version string, the OBCU container's 32-byte field. Sized to match
/// `obc_dfu::image::FW_VERSION_LEN` without depending on that crate.
pub type Version = heapless::String<32>;

/// Copy `s` into a [`Version`], truncating to the buffer's cap on a char boundary. `pub` so a host
/// reporting this boot's update verdict from its own `&str` versions applies the same bound.
pub fn clamp(s: &str) -> Version {
    let mut v = Version::new();
    let mut end = s.len().min(v.capacity());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let _ = v.push_str(&s[..end]);
    v
}

/// A successful staging scan's result, so the confirm screen can show installed against staged and
/// warn on the no-undo and same-version cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DfuScanReport {
    /// The running firmware's version: what an install replaces.
    pub installed: Version,
    /// The staged image's version, read from the validated package header.
    pub staged: Version,
    /// This install would arm with no rollback snapshot, which is knowable before arming from the
    /// boot-state page. An unconfirmed trial is then accepted rather than rolled back, so the
    /// confirm screen notes there is no automatic undo.
    pub first_install: bool,
}

impl DfuScanReport {
    /// Build a report from `&str` versions, each truncated to [`Version`]'s cap on a char boundary,
    /// so no caller reaches for `heapless` directly.
    pub fn new(installed: &str, staged: &str, first_install: bool) -> DfuScanReport {
        DfuScanReport { installed: clamp(installed), staged: clamp(staged), first_install }
    }

    /// Whether the staged image is the same version already running, as a byte-for-byte string
    /// match. Only equality is flagged: a true downgrade is not cleanly determinable from
    /// `git describe` strings.
    pub fn same_version(&self) -> bool {
        self.installed == self.staged
    }
}

/// Why an armed update is not the running firmware: the boot-time reconcile's verdict, shown once
/// by the "UPDATE FAILED" card. The board derives it from the boot-state page and the arm marker it
/// left before the install reboot; the app only carries the fact to the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuFailure {
    /// The `Armed` record survived into a running app, so the bootloader never consumed it. The
    /// board clears the leftover arm so it cannot fire by surprise on a later reboot.
    NotStarted,
    /// The bootloader consumed the arm but the staged image is not what is running: it was rejected
    /// before the erase, or its trial boot went unconfirmed and was rolled back.
    Reverted,
}

/// Why the install drain refused or failed to arm an update: the failure twin of [`DfuScanError`],
/// so a live spinner is replaced by the error card instead of hanging forever. Every non-reboot
/// outcome of the board's install drain maps to one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuInstallError {
    /// Refused: a ride is recording, and arming ends in a reboot that would lose the live ride.
    Recording,
    /// Refused: the store cannot be written, so the rollback reserve has nowhere to go.
    NoCard,
    /// The arm re-scanned the staged package and it failed validation, folded into the same
    /// [`DfuScanError`] buckets the scan card shows.
    Scan(DfuScanError),
    /// Writing the rollback reserve failed, before anything was armed.
    SnapshotFailed,
    /// An RRAM write on the arm path failed. Either way nothing was armed and the device keeps
    /// running the old image, so all such failures share one bucket; the board's breadcrumb tells
    /// them apart.
    StateWriteFailed,
}

/// Why the staging scan rejected the staged package, phrased for the app's error card. The board
/// folds its own finer variants into these six user-facing buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuScanError {
    /// No update package is staged.
    NotFound,
    /// A card read failed, possibly transiently.
    Unreadable,
    /// The package is not a valid update image: bad header, a failed CRC, or a torn upload.
    Damaged,
    /// The image is larger than the app slot can hold.
    TooLarge,
    /// The package resolves to too many block runs to install. The fix is to upload it again.
    TooFragmented,
    /// The package is intact but not trusted: no signature this firmware verifies, or one that does
    /// not check out. Its own bucket rather than [`Damaged`](Self::Damaged), because "corrupt" and
    /// "not ours" are different problems with different fixes.
    Untrusted,
}

// The armer's breadcrumb, written right after the `Armed` boot-state write and just before the
// reboot into the bootloader. At the next boot the board reads it back and, together with the
// boot-state page, derives the one-time verdict card. Cleared wherever a verdict is delivered. A
// torn, blank or foreign slot decodes to `None`, which means no arm happened.

/// The arm marker's fixed slot length: 3 whole 16-byte RRAM lines.
pub const ARM_MARKER_LEN: usize = 48;
/// The arm-marker tag; anything else there decodes to "no arm happened".
const ARM_MARKER_MAGIC: [u8; 4] = *b"OBCA";
/// Arm-marker layout version. Bump it on any field change; an old version reads as no marker.
const ARM_MARKER_VERSION: u8 = 1;
/// CRC-covered prefix: `magic(4) · version(1) · vlen(1) · pad(2) · generation u32 LE · version
/// string bytes(32)`.
const ARM_MARKER_PAYLOAD: usize = 44;

/// What the armer records before rebooting into the bootloader: the arm's generation and the
/// staged image's version string, which is the popup's "which update" fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmMarker {
    /// The `Armed` record's generation, the ticket the armer reported.
    pub generation: u32,
    /// The staged image's version string, verbatim from its OBCU header.
    pub staged: heapless::String<32>,
}

/// Pack an arm marker into its fixed [`ARM_MARKER_LEN`]-byte slot.
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

/// Decode an arm-marker slot, or `None` for anything but a clean read of this format: a blank slot,
/// a torn write, a short slice, an older layout, or a version string that is not UTF-8. `None`
/// means no arm happened, and the boot-outcome reconcile treats the boot as plain.
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

    /// The arm-marker slot round-trips, and every torn, blank or foreign shape decodes to `None`.
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
        torn[15] ^= 0xFF; // flip a version-string byte without fixing the CRC
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

// The DFU domain protocol. `DfuState` owns the update's user-visible lifecycle: when a scan may
// run, whether an install is admissible, and what terminal state the panel holds through the
// bootloader. The executor scans a package or arms an install, both bounded and both failable, and
// neither of them a policy decision.

use crate::device_core::{DfuTag, OperationToken};

/// What the rider (or the remote-DFU door) asks of the update domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuIntent {
    /// Validate the staged package and report what it holds. Read-only and free to refuse.
    ScanRequested,
    /// Arm the update and reboot into the bootloader. Admissible only while
    /// [`DfuCapabilities::install`](crate::device_core::DfuCapabilities) holds.
    InstallRequested,
}

/// One bounded physical update operation, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuEffect {
    /// Validate the staged package: header, CRC, extents, signature.
    Scan { token: OperationToken<DfuTag> },
    /// Snapshot the running image, write the arm record, and reset. On success this never returns.
    ArmInstall { token: OperationToken<DfuTag> },
}

impl DfuEffect {
    pub fn token(&self) -> OperationToken<DfuTag> {
        match self {
            DfuEffect::Scan { token } | DfuEffect::ArmInstall { token } => *token,
        }
    }
}

/// The result of one [`DfuEffect`].
///
/// [`InstallBegan`](DfuOutcome::InstallBegan) is the terminal answer of a successful arm: the board
/// reboots immediately after it, so the panel swaps to the installing card and holds it through the
/// bootloader. Whether the update took is not an outcome at all; it is next boot's
/// [`UpdateResult`](crate::device_core::UpdateResult) external fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DfuOutcome {
    /// The staged package validated; `report` is what the confirm screen shows.
    ScanFinished { token: OperationToken<DfuTag>, report: DfuScanReport },
    /// The staged package was rejected.
    ScanFailed { token: OperationToken<DfuTag>, error: DfuScanError },
    /// The arm succeeded and the reset is imminent.
    InstallBegan { token: OperationToken<DfuTag> },
    /// The arm refused or failed without rebooting; the device keeps running the old image.
    InstallFailed { token: OperationToken<DfuTag>, error: DfuInstallError },
    /// The executor abandoned the operation without completing it.
    Cancelled { token: OperationToken<DfuTag> },
}

impl DfuOutcome {
    pub fn token(&self) -> OperationToken<DfuTag> {
        match self {
            DfuOutcome::ScanFinished { token, .. }
            | DfuOutcome::ScanFailed { token, .. }
            | DfuOutcome::InstallBegan { token }
            | DfuOutcome::InstallFailed { token, .. }
            | DfuOutcome::Cancelled { token } => *token,
        }
    }
}

// Layout tripwires. `DfuOutcome` is the largest message in the DeviceCore protocol and is meant to
// be: [`DfuScanReport`] carries the two fixed 32-byte version strings the confirm screen prints
// verbatim. That is also why the outcome enum is `Clone` rather than `Copy`.
const _: () = assert!(core::mem::size_of::<DfuIntent>() <= 1, "two fieldless requests");
const _: () = assert!(core::mem::size_of::<DfuEffect>() <= 8, "a bare token");
const _: () = assert!(core::mem::size_of::<DfuOutcome>() <= 96, "the two fixed version strings dominate");

use crate::activity::DfuAction;

/// The update domain's own state: the single phase slot and the operation token.
///
/// One phase at a time, most-recent-wins: a rider's later post replaces an undelivered earlier one
/// rather than queueing behind it, because a scan the rider walked away from must not run after the
/// install they asked for instead. The remote BLE door is the one caller that must not replace;
/// it reads [`request_pending`](DfuState::request_pending) and defers, because a phone must never
/// displace what the rider is doing on the device.
#[derive(Debug, Default)]
pub struct DfuState {
    /// The rider's or the remote door's request, until an executor takes it.
    request: Option<DfuAction>,
    ops: crate::device_core::TokenSource<DfuTag>,
}

impl DfuState {
    /// The boot state: nothing staged, nothing running.
    pub(crate) const fn new() -> Self {
        DfuState { request: None, ops: crate::device_core::TokenSource::new() }
    }

    /// Admit one update request. Most-recent-wins, and superseding invalidates the older token so a
    /// scan answer cannot land on the install that replaced it.
    pub(crate) fn admit_intent(&mut self, intent: DfuIntent) {
        self.ops.invalidate();
        self.request = Some(match intent {
            DfuIntent::ScanRequested => DfuAction::Scan,
            DfuIntent::InstallRequested => DfuAction::Install,
        });
    }

    /// The next bounded update operation, or `None` when nothing is owed.
    pub(crate) fn next_effect(&mut self) -> Option<DfuEffect> {
        Some(match self.request.take()? {
            DfuAction::Scan => DfuEffect::Scan { token: self.ops.issue() },
            DfuAction::Install => DfuEffect::ArmInstall { token: self.ops.issue() },
        })
    }

    /// Whether `outcome` still answers the phase the domain is waiting for.
    pub(crate) fn accepts(&self, outcome: &DfuOutcome) -> bool {
        self.ops.is_current(outcome.token())
    }

    /// A terminal answer landed: the phase is over, so a repeat of it is no longer current.
    pub(crate) fn note_answer(&mut self) {
        self.ops.invalidate();
    }

    /// Whether a request is posted but undelivered: the remote door's deferral gate.
    pub(crate) fn request_pending(&self) -> bool {
        self.request.is_some()
    }

    /// Assert the boot state, field by field.
    #[cfg(test)]
    pub(crate) fn assert_boot_state(&self) {
        let DfuState { request, ops } = self;
        assert!(request.is_none(), "no update phase posted");
        assert_eq!(format!("{ops:?}"), "TokenSource(0)", "no update operation has been issued");
    }
}

// Layout tripwire: one phase and a token.
const _: () = assert!(core::mem::size_of::<DfuState>() <= 8, "one phase slot and a generation");

#[cfg(test)]
mod dfu_state_tests {
    use super::*;

    fn phase(state: &mut DfuState) -> Option<DfuAction> {
        state.next_effect().map(|effect| match effect {
            DfuEffect::Scan { .. } => DfuAction::Scan,
            DfuEffect::ArmInstall { .. } => DfuAction::Install,
        })
    }

    /// One phase at a time, most-recent-wins: a later post replaces an undelivered earlier one, and
    /// superseding invalidates the older token so a scan's answer cannot land on the install that
    /// replaced it.
    #[test]
    fn the_phase_slot_is_most_recent_wins() {
        let mut state = DfuState::new();
        state.admit_intent(DfuIntent::ScanRequested);
        state.admit_intent(DfuIntent::InstallRequested);
        assert_eq!(phase(&mut state), Some(DfuAction::Install), "the rider's latest is what runs");
        assert_eq!(phase(&mut state), None, "…exactly once");
    }

    /// A scan answer that belongs to a superseded phase changes nothing, so the rider is not shown
    /// a scan report for an update they already asked to install.
    #[test]
    fn an_answer_to_a_superseded_phase_is_refused() {
        let mut state = DfuState::new();
        state.admit_intent(DfuIntent::ScanRequested);
        let scan = state.next_effect().expect("the scan goes out");
        state.admit_intent(DfuIntent::InstallRequested);

        let report = DfuScanReport::new("v1", "v2", false);
        assert!(!state.accepts(&DfuOutcome::ScanFinished { token: scan.token(), report }));
    }

    /// Both terminal failures answer the phase they belong to, so the panel's card is the one the
    /// rider was waiting on.
    #[test]
    fn a_scan_failure_and_an_install_failure_both_answer_their_own_phase() {
        for (intent, failure) in [(DfuIntent::ScanRequested, 0), (DfuIntent::InstallRequested, 1)] {
            let mut state = DfuState::new();
            state.admit_intent(intent);
            let effect = state.next_effect().expect("the phase goes out");
            let token = effect.token();
            let outcome = if failure == 0 {
                DfuOutcome::ScanFailed { token, error: DfuScanError::NotFound }
            } else {
                DfuOutcome::InstallFailed { token, error: DfuInstallError::NoCard }
            };
            assert!(state.accepts(&outcome), "the failure answers the phase that was asked for");
            state.note_answer();
            assert!(!state.accepts(&outcome), "and a repeat of it is no longer current");
        }
    }
}
