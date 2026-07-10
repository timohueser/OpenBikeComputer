//! The app-side view of the SD-sideload firmware-update flow (epic #615 S5, #620).
//!
//! The scan/arm machinery runs **board-side** (`obc_dfu::armer` + `obc-fw-nrf54l`'s `dfu.rs`); the
//! app only posts the [`DfuAction`](crate::activity::DfuAction) one-shots and receives the scan's
//! answer through [`App::notify_dfu_scan_result`](crate::App::notify_dfu_scan_result). These two
//! types are that answer, kept **host-agnostic** (no `obc-dfu` dependency reaches `obc-app`): the
//! board maps `obc_dfu::ScanError` into a [`DfuScanError`] and fills a [`DfuScanReport`] with the
//! version strings it read off the card + the running image.
//!
//! Version strings are never translated — they are `git describe` identifiers, not UI copy — so
//! they ride as fixed inline buffers and the confirm screen prints them verbatim.

/// A firmware version string (`git describe`: `vMAJOR.MINOR.PATCH-N-gHASH`), the OBCU container's
/// 32-byte field. Sized to match `obc_dfu::image::FW_VERSION_LEN` without depending on that crate.
pub type Version = heapless::String<32>;

/// Copy `s` into a [`Version`], truncating to the buffer's cap on a char boundary.
pub(crate) fn clamp(s: &str) -> Version {
    let mut v = Version::new();
    let mut end = s.len().min(v.capacity());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let _ = v.push_str(&s[..end]);
    v
}

/// A successful staging scan's result, handed to
/// [`App::notify_dfu_scan_result`](crate::App::notify_dfu_scan_result) so the confirm screen can
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

/// Why the staging scan rejected `UPDATE.BIN`, phrased for the app's error card (issue #620 §2).
/// The board folds `obc_dfu::ScanError`'s finer variants into these five user-facing buckets.
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
}
