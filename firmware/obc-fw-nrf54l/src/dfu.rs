//! The app-side DFU armer driver: the board half of `obc_dfu::armer`.
//!
//! The pure decision core lives host-tested in `obc_dfu::armer`; this module wires it to the real
//! device: [`sd::Storage`]'s stage and rollback adapters, [`RramSettingsStore`]'s boot-state page,
//! the watchdog pets between the long SD phases, the `D`-line status stream, and the final
//! `SCB::sys_reset()` into the bootloader.
//!
//! The arm sequence is normative in this order:
//!
//! 1. Scan and validate `UPDATE.BIN`: header decode, full CRC-32 pass, size gate, whole-file extent
//!    chain. Read-only, so a failure costs nothing. In the normal scan, confirm, install flow this
//!    pass already ran at [`run_scan`] and its [`StagedRef`] is carried in.
//! 2. Snapshot the rollback: the running image, read memory-mapped out of the app slot, re-wrapped
//!    as `/ROLLBACK.BIN` and extent-resolved the same way. Skipped on a first install, or when the
//!    slot no longer matches the installed record.
//! 3. Compose and write `Armed` (generation is the old page's plus 1) to the BOOT_STATE page: one
//!    CRC-framed blob in whole 16-byte RRAMC lines, with no torn intermediate.
//! 4. A brief beat to flush the status lines to the host, then `SCB::sys_reset()`.
//!
//! Power loss before step 3's write leaves nothing done, because the snapshot file is inert without
//! the record. After it, the install proceeds on the next boot exactly as if the reset had run,
//! because `Armed` is idempotent. That is why nothing else sits between 3 and 4.
//!
//! The heavy work is one sync `#[inline(never)]` call ([`arm_update`]) at the ride loop's shallow
//! drained-request depth, so the `StagedRef`s and the decoded `BootState` live in frames that pop
//! on return. Nothing large is held across an `await`. The carried scan ref parks in a ride-loop
//! local, in the loop task's own future storage, so it never deepens `arm_update`'s frame.

use core::fmt::Write;

use embassy_nrf::wdt;
use obc_dfu::armer::{self, ArmError, Rollback, ScanError};
use obc_dfu::{BootState, ImageHeader, StagedRef};

use crate::sd;
use crate::settings::RramSettingsStore;

/// Base of the app slot, from the `__app_slot_base` linker symbol, read at runtime like
/// `__settings_base` so no address is hard-coded.
fn app_slot_base() -> *const u8 {
    extern "C" {
        static __app_slot_base: u8;
    }
    core::ptr::addr_of!(__app_slot_base)
}

/// One DFU status line: always to RTT, and on debug-uart builds queued for the VCOM `D`-line
/// stream the on-glass gate watches. ASCII only, because it rides a serial console.
pub(crate) fn status(line: &str) {
    defmt::info!("dfu: {=str}", line);
    #[cfg(feature = "debug-uart")]
    obc_platform::debug_link::dfu_status(line);
}

/// Capture the running image's OBCU version for the identity strings. Called once, from `main`, as
/// soon as the settings store exists and long before the BLE and USB planes are spawned. The
/// boot-state page is the only place the device learns what it is, and to read it once at boot is
/// what keeps `firmware_revision()` non-blocking on the BLE and USB paths.
///
/// `#[inline(never)]`: the decoded [`BootState`] is a 1.7 KB temporary and must live in this frame,
/// which pops, rather than in `main`'s.
#[inline(never)]
pub(crate) fn seed_firmware_revision(settings: &mut RramSettingsStore) {
    let running = settings.read_boot_state().running_image();
    crate::link::identity::seed_installed_version(running.as_ref());
    defmt::info!("dfu: running image is {=str}", crate::link::identity::firmware_revision().as_str());
}

struct ArmReport {
    generation: u32,
    rollback: Rollback,
    staged_version: heapless::String<32>,
    staged_len: u32,
    extent_count: usize,
}

/// Why an arm failed. The boot-state page is untouched in every case.
enum ArmFailure {
    Scan(ScanError),
    Snapshot(ScanError),
    BlobStage,
    StateWrite,
}

/// The board's [`armer::ArmIo`]: the rollback snapshot over [`sd::Storage`] and the boot-state page
/// write over [`RramSettingsStore`], with a watchdog pet after the long snapshot phase.
struct BoardArmIo<'a> {
    storage: &'a mut sd::Storage,
    settings: &'a mut RramSettingsStore,
    wdt: &'a mut Option<wdt::WatchdogHandle>,
}

impl armer::ArmIo for BoardArmIo<'_> {
    fn snapshot(&mut self, installed: &ImageHeader) -> Result<Option<StagedRef>, ScanError> {
        // Gate the length before mapping the slot: the header came off a CRC-valid page, but a
        // foreign length must never build an out-of-slot slice.
        if installed.image_len == 0 || installed.image_len > obc_dfu::MAX_IMAGE_LEN {
            defmt::warn!("dfu: installed record has an implausible image_len — treating as no rollback");
            return Ok(None);
        }
        // SAFETY: the app slot is memory-mapped RRAM (XIP-readable) and `image_len` is gated to
        // the slot's capacity above; nothing writes program RRAM while the app runs.
        let image = unsafe { core::slice::from_raw_parts(app_slot_base(), installed.image_len as usize) };
        let result = self.storage.dfu_write_rollback(installed, image);
        // The snapshot is the arm's longest SD stretch, so feed the dog before the page write and
        // the reset tail.
        if let Some(h) = self.wdt.as_mut() {
            h.pet();
        }
        result
    }

    fn stage_boot_blob(&mut self) -> Result<(), obc_dfu::engine::IoError> {
        // The sEMMC image the bootloader boots the card through: the same bytes this firmware's
        // own storage bring-up copies to the FLPR carve.
        if self.settings.stage_semmc_blob(crate::semmc::firmware_image()) {
            Ok(())
        } else {
            Err(obc_dfu::engine::IoError)
        }
    }

    fn write_state(&mut self, state: &BootState) -> Result<(), obc_dfu::engine::IoError> {
        if self.settings.write_boot_state(state) {
            Ok(())
        } else {
            Err(obc_dfu::engine::IoError)
        }
    }
}

/// The whole arm as one sync, popped frame: carry or scan, read the old page, snapshot, compose,
/// write. Returns the report for the status lines; the caller owns the beat and the reset.
///
/// `cached` is the [`StagedRef`] the confirm's preceding scan already validated, so the arm drops
/// straight to the snapshot with no second full read and CRC of `UPDATE.BIN`. It is absent only for
/// an install that arrives with no preceding scan. A stale carried ref is safe: the bootloader
/// re-reads and re-CRCs the raw extents before it erases, so a mismatch costs at worst a
/// `StageRejected` next boot. This is not a re-validation point.
#[inline(never)]
fn arm_update(
    storage: &mut sd::Storage,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
    cached: Option<StagedRef>,
) -> Result<ArmReport, ArmFailure> {
    let staged = match cached {
        // The confirm's scan already read and CRC'd the whole image; carry that verdict.
        Some(staged) => staged,
        // Fallback: an install with no preceding scan. Read and CRC the stage now.
        None => {
            let staged = storage.dfu_scan_update().map_err(ArmFailure::Scan)?;
            // The CRC pass over a 900 KB stage takes seconds; pet before the snapshot.
            if let Some(h) = wdt.as_mut() {
                h.pet();
            }
            staged
        }
    };
    let mut staged_version: heapless::String<32> = heapless::String::new();
    let _ = staged_version.push_str(staged.header.fw_version_str());
    let (staged_len, extent_count) = (staged.len, staged.extent_count());

    // Read and decode the old page first, because the generation bump is old plus 1. Then hand the
    // pure sequencer the IO: it snapshots before it writes.
    let current = settings.read_boot_state();
    let mut io = BoardArmIo { storage, settings, wdt };
    let ticket = armer::arm(&mut io, &current, staged).map_err(|e| match e {
        ArmError::Snapshot(s) => ArmFailure::Snapshot(s),
        ArmError::BlobStage => ArmFailure::BlobStage,
        ArmError::StateWrite => ArmFailure::StateWrite,
    })?;
    Ok(ArmReport { generation: ticket.generation, rollback: ticket.rollback, staged_version, staged_len, extent_count })
}

/// Format and push one status line, truncating at the stream's own 96-byte cap.
macro_rules! statusf {
    ($($arg:tt)*) => {{
        let mut s: heapless::String<96> = heapless::String::new();
        let _ = write!(s, $($arg)*);
        status(&s);
    }};
}

/// Run a drained install request end to end: status lines per phase, the sync [`arm_update`] under
/// the caller's storage and settings access, then, on success, a brief beat so the `D`-lines flush
/// to the host, and `SCB::sys_reset()` into the bootloader.
///
/// Returns only on failure, with the state page untouched and the device still riding. The typed
/// [`DfuInstallError`](obc_app::DfuInstallError) goes to the pass's fact stage, so the spinner is
/// replaced by an error card instead of hanging. On success the call diverges into the reset.
pub(crate) async fn run_install(
    storage: &mut sd::Storage,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
    cached: Option<StagedRef>,
) -> Option<obc_app::DfuInstallError> {
    // The record shows which path armed: the normal confirm carries the scan's ref, and the
    // fallback re-reads here.
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
            // The armer's breadcrumb for the next boot's outcome reconcile. Best-effort: a torn
            // write costs the verdict card its precision, never the install.
            let marker = obc_app::dfu::ArmMarker { generation: report.generation, staged: report.staged_version };
            settings.write_arm_marker(&marker);
            // The beat: nothing else may run between here and the reset except this flush.
            embassy_time::Timer::after_millis(400).await;
            cortex_m::peripheral::SCB::sys_reset();
        }
        // Each failure keeps its `D`-line breadcrumb and returns the app-facing bucket, so the
        // caller can swap the spinner for the error card.
        Err(ArmFailure::Scan(e)) => {
            report_scan_error("scan", e);
            Some(obc_app::DfuInstallError::Scan(map_scan_error(e)))
        }
        Err(ArmFailure::Snapshot(e)) => {
            report_scan_error("rollback snapshot", e);
            Some(obc_app::DfuInstallError::SnapshotFailed)
        }
        // The blob stage and the page write are both RRAM writes with the same user story, so they
        // share one app bucket; the `D`-line breadcrumb keeps them apart for diagnostics.
        Err(ArmFailure::BlobStage) => {
            status("install failed: sEMMC blob stage write failed -- nothing armed");
            Some(obc_app::DfuInstallError::StateWriteFailed)
        }
        Err(ArmFailure::StateWrite) => {
            status("install failed: boot-state page write failed -- nothing armed");
            Some(obc_app::DfuInstallError::StateWriteFailed)
        }
    }
}

/// The scan-only phase: the UI's read-only "Checking card" step. Validates `UPDATE.BIN` exactly as
/// the arm's first step does, but touches nothing, and reads the boot-state page for the pre-arm
/// no-rollback fact. Returns the report the confirm screen shows, or a mapped
/// [`DfuScanError`](obc_app::DfuScanError) for the error card.
///
/// The [`StagedRef`] comes back beside the report, so the caller can park it and hand it to
/// [`run_install`]; the confirm then arms with no second full read and CRC pass over the 900 KB
/// `UPDATE.BIN`.
pub(crate) fn run_scan(
    storage: &mut sd::Storage,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
) -> Result<(obc_app::DfuScanReport, StagedRef), obc_app::DfuScanError> {
    let staged = storage.dfu_scan_update().map_err(map_scan_error)?;
    // The full CRC pass over a 900 KB stage takes seconds; feed the dog before returning.
    if let Some(h) = wdt.as_mut() {
        h.pet();
    }
    // The no-rollback fact is knowable before the arm from the boot-state page: `Idle` with no
    // installed record, and defensively any non-`Idle` page, arms with no rollback, so an
    // unconfirmed trial is accepted rather than rolled back. The running-mismatch case needs the
    // slot CRC, which is too heavy before the confirm, so it is not surfaced.
    let installed = match settings.read_boot_state() {
        BootState::Idle { installed, .. } => installed,
        _ => None,
    };
    let mut staged_version: heapless::String<32> = heapless::String::new();
    let _ = staged_version.push_str(staged.header.fw_version_str());
    // The installed side of the confirm screen, and its same-version equality check, must speak
    // the same dialect as the staged side: the OBCU version string, with the build's git hash as
    // the dev-device fallback. That is the rule `link::identity` publishes over DIS and USB, so the
    // assembler is shared and cannot drift. The record read here is the live page, not identity's
    // boot snapshot, because a confirm screen must show what the page says now.
    let installed_version = obc_app::dfu::clamp(crate::link::identity::revision_from(installed.as_ref()).as_str());
    Ok((
        obc_app::DfuScanReport {
            installed: installed_version,
            staged: staged_version,
            first_install: installed.is_none(),
        },
        staged,
    ))
}

/// Fold `obc_dfu`'s finer [`ScanError`] variants into the user-facing
/// [`DfuScanError`](obc_app::DfuScanError) buckets the app's error card shows.
fn map_scan_error(e: ScanError) -> obc_app::DfuScanError {
    use obc_app::DfuScanError as U;
    match e {
        ScanError::Missing => U::NotFound,
        ScanError::Io => U::Unreadable,
        ScanError::BadHeader | ScanError::BadCrc | ScanError::Truncated => U::Damaged,
        ScanError::Oversize => U::TooLarge,
        ScanError::TooFragmented { .. } => U::TooFragmented,
        // Intact but not ours: an unsigned container, or a signature that does not verify.
        ScanError::Unsigned | ScanError::BadSignature => U::Untrusted,
    }
}

/// One typed error, phrased for the harness.
fn report_scan_error(phase: &str, e: ScanError) {
    match e {
        ScanError::TooFragmented { extents } => {
            statusf!("install failed ({phase}): {} [{extents} extents]", e.describe());
        }
        _ => statusf!("install failed ({phase}): {}", e.describe()),
    }
}

/// The trial confirm, called once by the ride loop at the health anchor (first frame presented and
/// SD mounted). A `Trial` record becomes `Idle { installed }` and returns the confirmed header;
/// anything else is a silent no-op. The hardware watchdog already turns a wedged boot into the
/// reset that triggers the rollback, so there is deliberately no second timer here.
pub(crate) fn confirm_trial(settings: &mut RramSettingsStore) -> Option<ImageHeader> {
    let current = settings.read_boot_state();
    let (next, installed) = armer::confirm_trial(&current)?;
    if settings.write_boot_state(&next) {
        defmt::info!("dfu: trial confirmed — running {=str} is now the installed image", installed.fw_version_str());
        // The arm's verdict is delivered, so retire its breadcrumb.
        settings.clear_arm_marker();
        Some(installed)
    } else {
        // The trial record stands; an unconfirmed trial rolls back next boot.
        defmt::error!("dfu: trial-confirm page write failed — next boot will roll back");
        None
    }
}

/// The boot-outcome reconcile: called once per boot, before the ride loop runs, to turn the
/// boot-state page and the armer's breadcrumb into the one-time post-update verdict the UI shows.
///
/// The decision itself is the pure, host-tested [`obc_dfu::verdict`], which reads the boot state's
/// recorded [`LastOutcome`](obc_dfu::LastOutcome) and never version strings. This function is the
/// IO and card mapping around it. A `TrialInProgress` verdict means this is the trial boot and the
/// health-anchor confirm owns the verdict. `NotStarted` means an `Armed` record survived into the
/// app, so the stray arm is downgraded to `Idle` and cannot fire by surprise later.
pub(crate) fn reconcile_boot_outcome(
    facts: &mut obc_app::device_core::ExternalFacts,
    settings: &mut RramSettingsStore,
) {
    use obc_app::device_core::UpdateResult;
    // There is one boot per boot, so the one-shot slot is free and this cannot be refused. A
    // rejection would mean a second verdict was minted, which is a producer bug worth naming.
    let mut report = |result: UpdateResult| {
        if facts.note_update_result(result).is_err() {
            defmt::error!("dfu: a second boot verdict was produced — the first is the one the rider is owed");
            debug_assert!(false, "one boot, one update verdict");
        }
    };
    let marker = settings.read_arm_marker();
    let state = settings.read_boot_state();
    match obc_dfu::verdict(&state, marker.as_ref().map(|m| m.generation)) {
        obc_dfu::Verdict::TrialInProgress | obc_dfu::Verdict::None => {}
        obc_dfu::Verdict::Confirmed => {
            settings.clear_arm_marker();
            // Confirmed is only returned with a marker present, so `staged` is set.
            let staged = marker.as_ref().map(|m| m.staged.as_str()).unwrap_or("");
            defmt::info!("dfu: staged {=str} accepted after an unconfirmed trial", staged);
            report(UpdateResult::Confirmed(obc_app::dfu::clamp(staged)));
        }
        obc_dfu::Verdict::Reverted => {
            settings.clear_arm_marker();
            let staged = marker.as_ref().map(|m| m.staged.as_str());
            // RTT is the only forensics channel on glass, so name the staged version when the
            // marker carries one.
            match staged {
                Some(v) => defmt::warn!("dfu: staged {=str} is not the running image — rejected or rolled back", v),
                None => defmt::warn!("dfu: staged update is not the running image — rejected or rolled back"),
            }
            report(UpdateResult::Failed {
                why: obc_app::DfuFailure::Reverted,
                staged: staged.map(obc_app::dfu::clamp),
            });
        }
        obc_dfu::Verdict::NotStarted => {
            defmt::warn!("dfu: Armed record survived into the app — bootloader never ran the install");
            // Downgrade the stray arm and name the staged version from the `Armed` record; the
            // marker may be absent here.
            let (installed, staged) = match &state {
                BootState::Armed { update, rollback, .. } => (rollback.map(|r| r.header), Some(update.header)),
                _ => (None, None),
            };
            settings.write_boot_state(&BootState::Idle { installed, last_outcome: None });
            settings.clear_arm_marker();
            report(UpdateResult::Failed {
                why: obc_app::DfuFailure::NotStarted,
                staged: staged.as_ref().map(|h| obc_app::dfu::clamp(h.fw_version_str())),
            });
        }
    }
}
