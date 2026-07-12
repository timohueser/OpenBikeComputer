//! The app-side DFU **armer driver** (epic #615 S4, #619) — the board half of `obc_dfu::armer`.
//!
//! The pure decision core (scan matrix, snapshot-before-page-write sequencing, generation bump,
//! trial confirm) lives host-tested in `obc_dfu::armer`; this module wires it to the real device:
//! [`sd::Storage`]'s stage/rollback adapters, [`RramSettingsStore`]'s boot-state page, the
//! watchdog pets between the long SD phases, the `D`-line status stream for the debug-link
//! harness, and the final `SCB::sys_reset()` into the bootloader.
//!
//! ## The arm sequence (order normative — issue #619 §3)
//!
//! 1. **Scan + validate** `UPDATE.BIN` (header decode, full CRC-32 pass, size gate, whole-file
//!    extent chain — `OBCU_Spec.md` §2.3). Read-only; any failure costs nothing. In the normal
//!    Scan→confirm→Install flow this pass already ran at the confirm's [`run_scan`], and its
//!    [`StagedRef`] is carried into the arm (DR6, #734) — this step re-scans only when an Install
//!    arrives with no carried ref (the `dfu-install` debug path).
//! 2. **Snapshot the rollback**: the running image, read memory-mapped out of the app slot
//!    (`__app_slot_base`, RRAM is XIP-readable), re-wrapped as `/ROLLBACK.BIN` and
//!    extent-resolved the same way. Skipped on a first install (`installed: None`) or when the
//!    slot no longer matches the installed record (SWD reflash) — the arm then carries
//!    `rollback: None` and the trial-accept path applies.
//! 3. **Compose + write `Armed`** (generation = old page's + 1) to the BOOT_STATE page — one
//!    CRC-framed blob, whole 16-byte RRAMC lines, no torn intermediate.
//! 4. A **brief beat** (flush the status lines to the host), then `SCB::sys_reset()`.
//!
//! Power loss: before step 3's write, nothing happened (the snapshot file is inert without the
//! record); after it, the install proceeds on the next boot exactly as if the reset had run —
//! `Armed` is idempotent (epic invariant 2). That's why nothing else sits between 3 and 4.
//!
//! ## Stack discipline
//!
//! The heavy work is one sync `#[inline(never)]` call ([`arm_update`]) at the ride loop's
//! shallow drained-request depth: the ~850 B `StagedRef`s, the ~1.7 KB decoded `BootState`, and
//! `sd.rs`'s transient ~2 KB `ExtentTable` all live in frames that pop on return — nothing new
//! is resident, and nothing large is held across an `.await` (the loop only awaits the beat,
//! holding a few words). CRC/copy staging stays on `sd.rs`'s existing 512-byte-chunk idiom.
//!
//! The carried scan ref (DR6, #734) parks in an `Option<StagedRef>` **ride-loop local**, not on
//! this sync call stack: it lives in the loop task's future storage (a static task arena), so it
//! never deepens `arm_update`'s frame. The arm still holds exactly one `StagedRef` at a time — the
//! parameter, copied in place of the old locally-scanned one — so the hot-stack footprint is
//! unchanged from the re-scan version; only the task future grows by the parked ~850 B.

use core::fmt::Write;

use embassy_nrf::wdt;
use obc_dfu::armer::{self, ArmError, Rollback, ScanError};
use obc_dfu::{BootState, ImageHeader, StagedRef};

use crate::sd;
use crate::settings::RramSettingsStore;

/// Base of the app slot — the `__app_slot_base` linker symbol (`ORIGIN(FLASH)`, provided by
/// build.rs's memory.x), read at runtime like `__settings_base` so no address is hard-coded.
fn app_slot_base() -> *const u8 {
    extern "C" {
        static __app_slot_base: u8;
    }
    core::ptr::addr_of!(__app_slot_base)
}

/// One DFU status line: always to RTT, and (debug-uart builds) queued for the VCOM `D`-line
/// stream the on-glass gate watches. ASCII only — it rides a serial console.
pub(crate) fn status(line: &str) {
    defmt::info!("dfu: {=str}", line);
    #[cfg(feature = "debug-uart")]
    obc_platform::debug_link::dfu_status(line);
}

/// What a successful arm wrote — the drain's status-line material.
struct ArmReport {
    generation: u32,
    rollback: Rollback,
    staged_version: heapless::String<32>,
    staged_len: u32,
    extent_count: usize,
}

/// Why an arm failed (the boot-state page is untouched in every case — see `obc_dfu::armer`).
enum ArmFailure {
    Scan(ScanError),
    Snapshot(ScanError),
    StateWrite,
}

/// The board's [`armer::ArmIo`]: the rollback snapshot over [`sd::Storage`] + the boot-state
/// page write over [`RramSettingsStore`], with a watchdog pet after the long snapshot phase.
struct BoardArmIo<'a> {
    storage: &'a mut sd::Storage,
    settings: &'a mut RramSettingsStore,
    wdt: &'a mut Option<wdt::WatchdogHandle>,
}

impl armer::ArmIo for BoardArmIo<'_> {
    fn snapshot(&mut self, installed: &ImageHeader) -> Result<Option<StagedRef>, ScanError> {
        // Gate the length before mapping the slot: the header came off a CRC-valid page, but a
        // foreign/garbage length must never build an out-of-slot slice.
        if installed.image_len == 0 || installed.image_len > obc_dfu::MAX_IMAGE_LEN {
            defmt::warn!("dfu: installed record has an implausible image_len — treating as no rollback");
            return Ok(None);
        }
        // SAFETY: the app slot is memory-mapped RRAM (XIP-readable) and `image_len` is gated to
        // the slot's capacity above; nothing writes program RRAM while the app runs.
        let image = unsafe { core::slice::from_raw_parts(app_slot_base(), installed.image_len as usize) };
        let result = self.storage.dfu_write_rollback(installed, image);
        // The snapshot is the arm's longest SD stretch (an image-sized write) — feed the dog
        // before the page write + reset tail.
        if let Some(h) = self.wdt.as_mut() {
            h.pet();
        }
        result
    }

    fn write_state(&mut self, state: &BootState) -> Result<(), obc_dfu::engine::IoError> {
        if self.settings.write_boot_state(state) {
            Ok(())
        } else {
            Err(obc_dfu::engine::IoError)
        }
    }
}

/// The whole arm as **one sync, popped frame** (see the module's stack note): (carry-or-scan) →
/// read the old page → snapshot → compose → write. Returns the report for the status lines; the
/// caller owns the beat + reset.
///
/// `cached` is the [`StagedRef`] the confirm's preceding scan already validated (DR6, #734) —
/// present in the normal Scan→confirm→Install flow, so the arm drops straight to the snapshot with
/// no second full read + CRC of `UPDATE.BIN`. It's absent only for an Install that arrives without
/// a preceding Scan (the `dfu-install` debug path, or a hypothetical UI that skips the confirm);
/// the re-scan fallback keeps the action total. A stale carried ref is safe: the bootloader's
/// verify-before-erase re-reads and re-CRCs the raw extents post-reboot regardless, so a mismatch
/// costs at worst a `StageRejected` next boot — this is not a TOCTOU re-validation point.
#[inline(never)]
fn arm_update(
    storage: &mut sd::Storage,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
    cached: Option<StagedRef>,
) -> Result<ArmReport, ArmFailure> {
    let staged = match cached {
        // The confirm's scan already read + CRC'd the whole image — carry that verdict.
        Some(staged) => staged,
        // Fallback: an Install with no preceding Scan. Read + CRC the stage now.
        None => {
            let staged = storage.dfu_scan_update().map_err(ArmFailure::Scan)?;
            // The CRC pass over a ~900 KB stage takes seconds — pet between it and the snapshot.
            if let Some(h) = wdt.as_mut() {
                h.pet();
            }
            staged
        }
    };
    let mut staged_version: heapless::String<32> = heapless::String::new();
    let _ = staged_version.push_str(staged.header.fw_version_str());
    let (staged_len, extent_count) = (staged.len, staged.extent_count());

    // Read + decode the old page FIRST (the generation bump is old + 1), then hand the pure
    // sequencer the IO — it snapshots before it writes, host-asserted in obc-dfu's tests.
    let current = settings.read_boot_state();
    let mut io = BoardArmIo { storage, settings, wdt };
    let ticket = armer::arm(&mut io, &current, staged).map_err(|e| match e {
        ArmError::Snapshot(s) => ArmFailure::Snapshot(s),
        ArmError::StateWrite => ArmFailure::StateWrite,
    })?;
    Ok(ArmReport { generation: ticket.generation, rollback: ticket.rollback, staged_version, staged_len, extent_count })
}

/// Format-and-push one status line (96-byte cap, truncating — matching the stream's own cap).
macro_rules! statusf {
    ($($arg:tt)*) => {{
        let mut s: heapless::String<96> = heapless::String::new();
        let _ = write!(s, $($arg)*);
        status(&s);
    }};
}

/// Run a drained install request end to end (issue #619 §6): status lines per phase, the sync
/// [`arm_update`] under the caller's storage/settings access, then — on success — a brief beat
/// so the `D`-lines flush to the host, and `SCB::sys_reset()` straight into the bootloader.
/// Returns only on failure (the state page is then untouched; the device keeps riding).
pub(crate) async fn run_install(
    storage: &mut sd::Storage,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
    cached: Option<StagedRef>,
) {
    // The RTT/`D`-line record shows which path armed: the normal confirm carries the scan's ref
    // (one CRC pass, done back at the Scan), the fallback re-reads here.
    if cached.is_some() {
        statusf!("arming from the scan's validated image (running {})", env!("OBC_FW_GIT"));
    } else {
        statusf!("scanning UPDATE.BIN (running {})", env!("OBC_FW_GIT"));
    }
    match arm_update(storage, settings, wdt, cached) {
        Ok(report) => {
            statusf!("scan ok: {} ({} B, {} extent(s))", report.staged_version, report.staged_len, report.extent_count);
            match report.rollback {
                Rollback::Snapshot => status("rollback snapshot written to ROLLBACK.BIN"),
                Rollback::FirstInstall => status("no rollback: first install -- an unconfirmed trial will be accepted"),
                Rollback::RunningMismatch => {
                    status("no rollback: running image differs from installed record (SWD reflash?)")
                }
            }
            statusf!("armed gen={} -- rebooting into the bootloader", report.generation);
            // The armer's breadcrumb for the next boot's outcome reconcile (best-effort — a
            // failed or torn write only costs the verdict card its precision, never the install:
            // a power cut anywhere past the page write is exactly the armed-install path).
            let marker = obc_app::settings::ArmMarker { generation: report.generation, staged: report.staged_version };
            settings.write_arm_marker(&marker);
            // The beat: nothing else may run between here and the reset except this flush
            // (issue #619 §3).
            embassy_time::Timer::after_millis(400).await;
            cortex_m::peripheral::SCB::sys_reset();
        }
        Err(ArmFailure::Scan(e)) => report_scan_error("scan", e),
        Err(ArmFailure::Snapshot(e)) => report_scan_error("rollback snapshot", e),
        Err(ArmFailure::StateWrite) => status("install failed: boot-state page write failed -- nothing armed"),
    }
}

/// The **scan-only** phase (epic #615 S5, #620) — the UI's read-only "Checking card..." step,
/// posted as [`DfuAction::Scan`](obc_app::DfuAction) by the System settings screen. Validates
/// `UPDATE.BIN` exactly as the arm's first step does (header, full CRC-32, extents) but touches
/// nothing, and reads the boot-state page for the pre-arm no-rollback fact, returning the
/// app-native [`DfuScanReport`](obc_app::DfuScanReport) the confirm screen shows — or a mapped
/// [`DfuScanError`](obc_app::DfuScanError) for the error card. The board answers the app through
/// [`App::notify_dfu_scan_result`](obc_app::App::notify_dfu_scan_result); a failed scan, like the
/// arm's, costs nothing.
///
/// Returns the [`StagedRef`] alongside the report so the caller can park it next to its pending-DFU
/// state and hand it straight to the confirm's [`run_install`] — the confirm then arms without a
/// second full read + CRC pass over the ~900 KB `UPDATE.BIN` (DR6, #734). The ref is the *only*
/// thing the arm needs from the scan; the report is what the app renders.
pub(crate) fn run_scan(
    storage: &mut sd::Storage,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
) -> Result<(obc_app::DfuScanReport, StagedRef), obc_app::DfuScanError> {
    let staged = storage.dfu_scan_update().map_err(map_scan_error)?;
    // The full CRC pass over a ~900 KB stage takes seconds — feed the dog before returning.
    if let Some(h) = wdt.as_mut() {
        h.pet();
    }
    // The no-rollback fact is knowable pre-arm from the boot-state page: `Idle { installed: None }`
    // (a dev-flashed device, spec §2.4) — and, defensively, any non-`Idle` page — arms without a
    // rollback, so an unconfirmed trial is accepted rather than rolled back. (The running-mismatch
    // no-rollback case needs the slot CRC, too heavy pre-confirm, so it isn't surfaced — see the PR.)
    let installed = match settings.read_boot_state() {
        BootState::Idle { installed, .. } => installed,
        _ => None,
    };
    let mut staged_version: heapless::String<32> = heapless::String::new();
    let _ = staged_version.push_str(staged.header.fw_version_str());
    // The installed side of the confirm screen — and its same-version equality check — must speak
    // the same dialect as the staged side: the OBCU version string. After any confirmed install the
    // boot-state page's `installed` header holds exactly the string the running image was wrapped
    // with (a release tag like `v0.4.0`), so prefer it; `OBC_FW_GIT` — a bare `rev-parse` hash that
    // can never equal a wrapped tag — is only the fallback for dev-flashed devices with no install
    // history (where the README recipe wraps with a describe/hash string anyway).
    let mut installed_version: heapless::String<32> = heapless::String::new();
    let _ = installed_version.push_str(match installed.as_ref().map(|h| h.fw_version_str()) {
        Some(v) if !v.is_empty() => v,
        _ => env!("OBC_FW_GIT"),
    });
    Ok((
        obc_app::DfuScanReport {
            installed: installed_version,
            staged: staged_version,
            first_install: installed.is_none(),
        },
        staged,
    ))
}

/// Fold `obc_dfu`'s finer [`ScanError`] variants into the five user-facing
/// [`DfuScanError`](obc_app::DfuScanError) buckets the app's error card shows (issue #620 §2).
fn map_scan_error(e: ScanError) -> obc_app::DfuScanError {
    use obc_app::DfuScanError as U;
    match e {
        ScanError::Missing => U::NotFound,
        ScanError::Io => U::Unreadable,
        ScanError::BadHeader | ScanError::BadCrc | ScanError::Truncated => U::Damaged,
        ScanError::Oversize => U::TooLarge,
        ScanError::TooFragmented { .. } => U::TooFragmented,
    }
}

/// One typed error, phrased for the harness (S5 reuses `ScanError::describe` verbatim).
fn report_scan_error(phase: &str, e: ScanError) {
    match e {
        ScanError::TooFragmented { extents } => {
            statusf!("install failed ({phase}): {} [{extents} extents]", e.describe());
        }
        _ => statusf!("install failed ({phase}): {}", e.describe()),
    }
}

/// The trial confirm (issue #619 §4), called once by the ride loop at the health anchor (first
/// frame presented + SD mounted): `Trial { installed, .. }` ⇒ write `Idle { installed }` and
/// return the confirmed header (the S5 toast's version); anything else is a silent no-op. The
/// hardware watchdog (#349) already converts a wedged boot into the reset that triggers S3's
/// rollback — there is deliberately no second timer here.
pub(crate) fn confirm_trial(settings: &mut RramSettingsStore) -> Option<ImageHeader> {
    let current = settings.read_boot_state();
    let (next, installed) = armer::confirm_trial(&current)?;
    if settings.write_boot_state(&next) {
        defmt::info!("dfu: trial confirmed — running {=str} is now the installed image", installed.fw_version_str());
        // The arm's verdict is delivered (the success toast) — retire its breadcrumb.
        settings.clear_arm_marker();
        Some(installed)
    } else {
        // The trial record stands; an unconfirmed trial rolls back next boot — safe, loud.
        defmt::error!("dfu: trial-confirm page write failed — next boot will roll back");
        None
    }
}

/// The **boot-outcome reconcile**: called once per boot, before the ride loop runs, to turn the
/// boot-state page + the armer's breadcrumb ([`ArmMarker`](obc_app::settings::ArmMarker)) into the
/// one-time post-update verdict the UI shows.
///
/// The decision itself is the pure, host-tested [`obc_dfu::verdict`] — it reads the boot state's
/// recorded [`LastOutcome`](obc_dfu::LastOutcome), **never** version strings (killing the
/// same-version misreport, DR2 #730). This function is the IO + card mapping around it:
///
/// - [`TrialInProgress`](obc_dfu::Verdict::TrialInProgress) — this IS the trial boot; the
///   health-anchor confirm owns the verdict (and the marker). Nothing to do.
/// - [`Verdict::None`](obc_dfu::Verdict::None) — a plain boot; nothing happened, nothing shows.
/// - [`Confirmed`](obc_dfu::Verdict::Confirmed) — the staged image is now running (accepted after an
///   unconfirmed first-install trial). Clear the marker, show the success toast.
/// - [`Reverted`](obc_dfu::Verdict::Reverted) — the staged image is not running (rejected before the
///   erase, or its trial rolled back). Clear the marker, show the failure card.
/// - [`NotStarted`](obc_dfu::Verdict::NotStarted) — an `Armed` record survived into the app: the
///   bootloader never consumed it (stale or missing). Downgrade the stray arm to `Idle` so it can't
///   fire by surprise later (the rollback snapshot's header is carried into `installed`, mirroring
///   the engine's reject path), clear the marker, and show the not-started card.
pub(crate) fn reconcile_boot_outcome(app: &mut obc_app::App, settings: &mut RramSettingsStore) {
    let marker = settings.read_arm_marker();
    let state = settings.read_boot_state();
    match obc_dfu::verdict(&state, marker.as_ref().map(|m| m.generation)) {
        obc_dfu::Verdict::TrialInProgress | obc_dfu::Verdict::None => {}
        obc_dfu::Verdict::Confirmed => {
            settings.clear_arm_marker();
            // Confirmed is only returned with a marker present (see `verdict`), so `staged` is set.
            let staged = marker.as_ref().map(|m| m.staged.as_str()).unwrap_or("");
            defmt::info!("dfu: staged {=str} accepted after an unconfirmed trial", staged);
            app.notify_update_confirmed(staged);
        }
        obc_dfu::Verdict::Reverted => {
            settings.clear_arm_marker();
            let staged = marker.as_ref().map(|m| m.staged.as_str());
            // RTT is the only forensics channel on glass — name the staged version when the marker
            // carries one (Reverted is only returned with a marker present, but stay total).
            match staged {
                Some(v) => defmt::warn!("dfu: staged {=str} is not the running image — rejected or rolled back", v),
                None => defmt::warn!("dfu: staged update is not the running image — rejected or rolled back"),
            }
            app.notify_update_failed(obc_app::DfuFailure::Reverted, staged);
        }
        obc_dfu::Verdict::NotStarted => {
            defmt::warn!("dfu: Armed record survived into the app — bootloader never ran the install");
            // Downgrade the stray arm + name the staged version from the Armed record (the marker
            // may be absent here — `verdict` returns NotStarted for `Armed` regardless of a marker).
            let (installed, staged) = match &state {
                BootState::Armed { update, rollback, .. } => (rollback.map(|r| r.header), Some(update.header)),
                _ => (None, None),
            };
            settings.write_boot_state(&BootState::Idle { installed, last_outcome: None });
            settings.clear_arm_marker();
            app.notify_update_failed(obc_app::DfuFailure::NotStarted, staged.as_ref().map(|h| h.fw_version_str()));
        }
    }
}
