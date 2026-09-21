//! The app-side DFU armer driver: the board half of `obc_dfu::armer`.
//!
//! The pure decision core lives host-tested in `obc_dfu::armer`; this module wires it to the real
//! device: the flat store's staged update package and rollback reserve, [`RramSettingsStore`]'s
//! boot-state page, the watchdog pets between the long card phases, the `D`-line status stream, and
//! the final `SCB::sys_reset()` into the bootloader.
//!
//! The arm sequence is normative in this order:
//!
//! 1. Scan and validate the staged package: header decode, full CRC-32 pass, signature, size gate,
//!    whole-object extent chain. Read-only, so a failure costs nothing. In the normal scan, confirm,
//!    install flow this pass already ran at [`run_scan`] and its [`StagedRef`] is carried in.
//! 2. Write the rollback: the running image, read memory-mapped out of the app slot, re-wrapped as
//!    an unsigned OBCU container into a fresh rollback reserve, and extent-resolved the same way.
//!    Skipped on a first install, or when the slot no longer matches the installed record.
//! 3. Compose and write `Armed` (generation is the old page's plus 1) to the BOOT_STATE page: one
//!    CRC-framed blob in whole 16-byte RRAMC lines, with no torn intermediate.
//! 4. A brief beat to flush the status lines to the host, then `SCB::sys_reset()`.
//!
//! Power loss before step 3's write leaves nothing done, because the reserve and the staged blob are
//! both inert without the record. After it, the install proceeds on the next boot exactly as if the
//! reset had run, because `Armed` is idempotent. That is why nothing else sits between 3 and 4.
//!
//! The heavy work runs at the ride loop's shallow drained-request depth in `#[inline(never)]` calls,
//! so the `StagedRef`s and the decoded `BootState` live in frames that pop on return. The carried
//! scan ref parks in a ride-loop local, in the loop task's own future storage.

use core::fmt::Write;

use embassy_nrf::wdt;
use obc_dfu::armer::{self, ArmError, ExtentsError, Rollback, ScanError, StageIo};
use obc_dfu::{BootState, Extent, ImageHeader, StagedRef, HEADER_LEN, MAX_EXTENTS};
use obc_formats::io::ByteSource;
use obc_storage::flat::source::StoreSource;
use obc_storage::flat::{
    BlockRun, DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision,
    Store as _, MAX_RANGES,
};

use crate::flat_store::{FlatCard, Outcome, Reply, Request, Writer};
use crate::settings::RramSettingsStore;

/// The board's flat store.
type Flat = FlatStore<FlatCard>;

/// An object holds at most [`MAX_RANGES`] extent ranges, and one range is one contiguous block run,
/// so the boot handoff's extent table can never be the thing that refuses an arm.
const _: () = assert!(MAX_RANGES <= MAX_EXTENTS);

/// The arm's own reply slot. One call is in flight at a time: the arm runs in the ride loop's store
/// tail, which serves one install request per pass and diverges into the reset on success.
static ARM_REPLY: Reply = Reply::new();

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

/// Why the two RRAM writes failed. The boot-state page is untouched in both cases.
enum ArmFailure {
    BlobStage,
    StateWrite,
}

/// The board's [`armer::ArmIo`]: the two RRAM writes the arm sequences, with a watchdog pet after
/// the long snapshot phase that precedes them.
struct BoardArmIo<'a> {
    settings: &'a mut RramSettingsStore,
}

impl armer::ArmIo for BoardArmIo<'_> {
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

/// The staged update package: the catalog's update-package head, or [`ScanError::Missing`] when a
/// client has not uploaded one. The greatest `ObjectId` wins, because a client that uploads a second
/// package rather than replacing the first means the newer one.
///
/// A listing that stopped early is a media failure, never an absent package: answering "nothing is
/// staged" out of a failed read would send the rider to upload a package that is already there.
fn staged_package(store: &Flat) -> Result<EntryMeta, ScanError> {
    let found = store
        .entries()
        .filter(|entry| entry.kind == ObjectKind::UpdatePackage && entry.flags == EntryFlags::NONE)
        .last();
    if !store.entries_ok() {
        return Err(ScanError::Io);
    }
    found.ok_or(ScanError::Missing)
}

/// Resolve one committed object's extents into the bootloader's absolute block runs.
///
/// The boot handoff addresses a block in a `u32`, which is the card driver's own wall, so a card
/// past 2 TiB refuses here rather than truncating an address.
fn block_extents(
    store: &Flat,
    id: ObjectId,
    revision: Revision,
    out: &mut [Extent; MAX_EXTENTS],
) -> Result<usize, ExtentsError> {
    let mut runs = [BlockRun::default(); MAX_RANGES];
    let count = store.block_runs(id, revision, &mut runs).map_err(|_| ExtentsError::Io)?;
    for (slot, run) in out.iter_mut().zip(&runs[..count]) {
        let start_block = u32::try_from(run.start_block).map_err(|_| ExtentsError::Io)?;
        let blocks = u32::try_from(run.blocks).map_err(|_| ExtentsError::Io)?;
        *slot = Extent { start_block, blocks };
    }
    Ok(count)
}

/// The armer's [`StageIo`] over the open update package: byte reads through the store's own random
/// access, and the whole-object extent resolve off the catalog entry.
struct FlatStage<'a> {
    store: &'a Flat,
    source: &'a StoreSource<'a, FlatCard>,
    id: ObjectId,
    revision: Revision,
}

impl StageIo for FlatStage<'_> {
    fn stage_len(&mut self) -> Option<u32> {
        u32::try_from(self.source.len()).ok()
    }

    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), obc_dfu::engine::IoError> {
        self.source.read_at(offset.into(), buf).map_err(|_| obc_dfu::engine::IoError)
    }

    fn stage_extents(&mut self, out: &mut [Extent; MAX_EXTENTS]) -> Result<usize, ExtentsError> {
        block_extents(self.store, self.id, self.revision, out)
    }
}

/// Read and validate the staged package end to end: OBCU header, size gate, one streaming pass for
/// the CRC-32 and the Ed25519 verification, then the extent chain the arm records.
///
/// The trusted key is [`obc_dfu::RELEASE_PUBKEY`], and this is the only place the firmware names it.
/// `obc_dfu::armer::scan` takes the key as a parameter, so tests inject their own.
///
/// `#[inline(never)]`: the staging buffer and the returned `StagedRef` belong in a frame that pops.
#[inline(never)]
fn scan_package(store: &Flat) -> Result<StagedRef, ScanError> {
    let meta = staged_package(store)?;
    let scanned = store.with_source(meta.id, Some(meta.revision), |source| {
        let mut stage = FlatStage { store, source, id: meta.id, revision: meta.revision };
        // The CRC and signature staging buffer is a stack chunk: no new resident statics, and the
        // frame pops with the scan, verifier state included.
        let mut chunk = [0u8; 512];
        armer::scan(&mut stage, &mut chunk, &obc_dfu::RELEASE_PUBKEY)
    });
    scanned.map_err(|_| ScanError::Io)?
}

/// Write the running image into a fresh rollback reserve and resolve the reserve's extents.
///
/// The reserve is one `RESERVED` entry of kind 8: the store owns the extents and never serves their
/// bytes, and the bootloader restores the image from them when a trial boot goes unconfirmed. The
/// bytes land before the commit, so a cut anywhere before it leaves an anonymous allocation the next
/// mount frees. The commit that publishes the new reserve removes the previous one in the same
/// batch, so an arm never leaves two.
///
/// `Ok(None)` means the slot's bytes no longer CRC-match the installed header, so a reserve would
/// record a rollback the bootloader must reject. None is taken. Errors abort the arm.
///
/// The snapshot is an unsigned container: the device cannot re-create the release signature from
/// slot bytes, and nothing verifies this one, because the bootloader checks it by CRC.
async fn write_rollback(
    store: &'static Flat,
    writer: &Writer,
    installed: &ImageHeader,
    image: &'static [u8],
) -> Result<Option<StagedRef>, ScanError> {
    debug_assert_eq!(image.len() as u32, installed.image_len);
    let crc = obc_dfu::crc32(image);
    if crc != installed.image_crc32 {
        defmt::warn!("dfu: running image doesn't match the installed record (SWD reflash?) — no rollback");
        return Ok(None);
    }
    let header = installed.unsigned();
    let bytes = (HEADER_LEN + image.len()) as u64;

    let Ok(Outcome::Allocated(allocation)) = writer.call(Request::Allocate { bytes }, &ARM_REPLY).await else {
        defmt::warn!("dfu: no room for a {=u64} B rollback reserve — arm aborted", bytes);
        return Err(ScanError::Io);
    };
    let request = Request::WriteRollback { allocation, header: header.encode(), image };
    let Ok(Outcome::Wrote(allocation)) = writer.call(request, &ARM_REPLY).await else {
        let _ = writer.call(Request::Cancel { allocation }, &ARM_REPLY).await;
        defmt::warn!("dfu: rollback reserve write failed — arm aborted");
        return Err(ScanError::Io);
    };

    let mut batch: heapless::Vec<Mutation, { obc_storage::flat::store::MAX_BATCH }> = heapless::Vec::new();
    for stale in store.entries().filter(|entry| entry.kind == ObjectKind::RollbackReserve) {
        if batch.push(Mutation::Remove { id: stale.id, revision: stale.revision }).is_err() {
            break;
        }
    }
    if !store.entries_ok() {
        let _ = writer.call(Request::Cancel { allocation }, &ARM_REPLY).await;
        return Err(ScanError::Io);
    }
    let meta = EntryMeta {
        added_at_utc: 0,
        id: store.next_object_id(),
        revision: Revision(1),
        kind: ObjectKind::RollbackReserve,
        flags: EntryFlags::RESERVED,
        payload_len: 0,
        payload_crc: 0,
        name: DisplayName::default(),
    };
    if batch.push(Mutation::Put { meta, source: PutSource::Fresh(allocation) }).is_err() {
        let _ = writer.call(Request::Cancel { allocation }, &ARM_REPLY).await;
        defmt::warn!("dfu: too many stale rollback reserves to swap in one commit — arm aborted");
        return Err(ScanError::Io);
    }
    if !matches!(writer.call(Request::Commit { batch }, &ARM_REPLY).await, Ok(Outcome::Committed(_))) {
        let _ = writer.call(Request::Cancel { allocation }, &ARM_REPLY).await;
        defmt::warn!("dfu: rollback reserve commit failed — arm aborted");
        return Err(ScanError::Io);
    }

    let mut extents = [Extent::default(); MAX_EXTENTS];
    let count = block_extents(store, meta.id, meta.revision, &mut extents).map_err(|e| match e {
        ExtentsError::TooFragmented { extents } => ScanError::TooFragmented { extents },
        ExtentsError::Io => ScanError::Io,
    })?;
    defmt::info!("dfu: rollback reserve written ({=u32} B raw image, {=usize} extent(s))", installed.image_len, count);
    StagedRef::new(header, installed.image_len, crc, &extents[..count])
        .map(Some)
        .ok_or(ScanError::TooFragmented { extents: count as u32 })
}

/// The running image as the reserve records it, or `None` when this arm gets no rollback: a first
/// install, or defensively a page that is not `Idle`.
fn rollback_source(current: &BootState) -> Option<ImageHeader> {
    match current {
        BootState::Idle { installed, .. } => *installed,
        // Armed and Trial cannot be live mid-run, so treat them like a fresh device rather than
        // guess at a rollback.
        _ => None,
    }
}

/// The running image's bytes, read memory-mapped out of the app slot, or `None` when the installed
/// record's length is implausible.
fn running_image(installed: &ImageHeader) -> Option<&'static [u8]> {
    // Gate the length before mapping the slot: the header came off a CRC-valid page, but a foreign
    // length must never build an out-of-slot slice.
    if installed.image_len == 0 || installed.image_len > obc_dfu::MAX_IMAGE_LEN {
        defmt::warn!("dfu: installed record has an implausible image_len — treating as no rollback");
        return None;
    }
    // SAFETY: the app slot is memory-mapped RRAM (XIP-readable) and `image_len` is gated to the
    // slot's capacity above; nothing writes program RRAM while the app runs.
    Some(unsafe { core::slice::from_raw_parts(app_slot_base(), installed.image_len as usize) })
}

/// The two RRAM writes that end the arm: the sEMMC blob stage and the `Armed` page.
///
/// `#[inline(never)]`: the decoded `BootState` and the composed page are ~300 B temporaries that
/// must live in a frame that pops rather than in the ride loop's poll frame.
#[inline(never)]
fn commit_arm(
    settings: &mut RramSettingsStore,
    current: &BootState,
    staged: StagedRef,
    rollback: Option<StagedRef>,
) -> Result<ArmReport, ArmFailure> {
    let mut staged_version: heapless::String<32> = heapless::String::new();
    let _ = staged_version.push_str(staged.header.fw_version_str());
    let (staged_len, extent_count) = (staged.len, staged.extent_count());
    let mut io = BoardArmIo { settings };
    let ticket = armer::arm(&mut io, current, staged, rollback).map_err(|e| match e {
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

/// Run a drained install request end to end: status lines per phase, the rollback reserve, the two
/// RRAM writes, then, on success, a brief beat so the `D`-lines flush to the host, and
/// `SCB::sys_reset()` into the bootloader.
///
/// Returns only on failure, with the state page untouched and the device still riding. The typed
/// [`DfuInstallError`](obc_app::DfuInstallError) goes to the pass's fact stage, so the spinner is
/// replaced by an error card instead of hanging. On success the call diverges into the reset.
///
/// `cached` is the [`StagedRef`] the confirm's preceding scan already validated, so the arm drops
/// straight to the rollback with no second full read and CRC of the package. It is absent only for
/// an install that arrives with no preceding scan. A stale carried ref is safe: the bootloader
/// re-reads and re-CRCs the raw extents before it erases, so a mismatch costs at worst a
/// `StageRejected` next boot. This is not a re-validation point.
pub(crate) async fn run_install(
    store: &'static Flat,
    writer: &Writer,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
    cached: Option<StagedRef>,
) -> Option<obc_app::DfuInstallError> {
    // The record shows which path armed: the normal confirm carries the scan's ref, and the
    // fallback re-reads here.
    if cached.is_some() {
        statusf!("arming from the scan's validated image (running {})", env!("OBC_FW_GIT"));
    } else {
        statusf!("scanning the staged update package (running {})", env!("OBC_FW_GIT"));
    }
    let staged = match cached {
        // The confirm's scan already read and CRC'd the whole image; carry that verdict.
        Some(staged) => staged,
        // Fallback: an install with no preceding scan. Read and CRC the package now.
        None => match scan_package(store) {
            Ok(staged) => {
                // The CRC pass over a 900 KB package takes seconds; pet before the reserve.
                pet(wdt);
                staged
            }
            Err(e) => {
                report_scan_error("scan", e);
                return Some(obc_app::DfuInstallError::Scan(map_scan_error(e)));
            }
        },
    };

    let current = settings.read_boot_state();
    let rollback = match rollback_source(&current).and_then(|installed| Some((installed, running_image(&installed)?))) {
        Some((installed, image)) => match write_rollback(store, writer, &installed, image).await {
            Ok(rollback) => {
                // The reserve is the arm's longest card stretch, so feed the dog before the page
                // writes and the reset tail.
                pet(wdt);
                rollback
            }
            Err(e) => {
                report_scan_error("rollback reserve", e);
                return Some(obc_app::DfuInstallError::SnapshotFailed);
            }
        },
        None => None,
    };

    match commit_arm(settings, &current, staged, rollback) {
        Ok(report) => {
            statusf!("scan ok: {} ({} B, {} extent(s))", report.staged_version, report.staged_len, report.extent_count);
            match report.rollback {
                Rollback::Snapshot => status("rollback written to the reserve"),
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

/// Feed the hardware watchdog between the arm's long phases.
fn pet(wdt: &mut Option<wdt::WatchdogHandle>) {
    if let Some(h) = wdt.as_mut() {
        h.pet();
    }
}

/// The scan-only phase: the UI's read-only "Checking card" step. Validates the staged package
/// exactly as the arm's first step does, but touches nothing, and reads the boot-state page for the
/// pre-arm no-rollback fact. Returns the report the confirm screen shows, or a mapped
/// [`DfuScanError`](obc_app::DfuScanError) for the error card.
///
/// The [`StagedRef`] comes back beside the report, so the caller can park it and hand it to
/// [`run_install`]; the confirm then arms with no second full read and CRC pass over the package.
pub(crate) fn run_scan(
    store: &Flat,
    settings: &mut RramSettingsStore,
    wdt: &mut Option<wdt::WatchdogHandle>,
) -> Result<(obc_app::DfuScanReport, StagedRef), obc_app::DfuScanError> {
    let staged = scan_package(store).map_err(map_scan_error)?;
    // The full CRC pass over a 900 KB package takes seconds; feed the dog before returning.
    pet(wdt);
    // The no-rollback fact is knowable before the arm from the boot-state page: `Idle` with no
    // installed record, and defensively any non-`Idle` page, arms with no rollback, so an
    // unconfirmed trial is accepted rather than rolled back. The running-mismatch case needs the
    // slot CRC, which is too heavy before the confirm, so it is not surfaced.
    let installed = rollback_source(&settings.read_boot_state());
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
