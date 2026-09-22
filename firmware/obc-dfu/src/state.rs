//! The boot-state RRAM handoff page and the bootloader's decision logic (`OBCU_Spec.md`).
//!
//! One CRC-framed blob in a dedicated 4 KB RRAM page, the only channel from the app (the armer) to
//! the bootloader (the installer). Anything that does not decode cleanly is [`BootState::Idle`], so
//! a torn, blank or corrupt page means "no pending update, jump to the app", never a garbage
//! install. RRAMC writes 16-byte lines, so the encoded length is always a multiple of 16.
//!
//! The `generation` counter labels an arm so a recorded [`LastOutcome`] can be tied back to it (see
//! [`verdict`]). Nothing compares generations to reject a page.

use crate::crc32::crc32;
use crate::image::{ImageHeader, HEADER_LEN};

/// A reader can pass the whole page to [`BootState::decode`].
pub const PAGE_LEN: usize = 4096;

/// The watchdog period shared across the DFU boot chain: 24 s of 32768 Hz LFCLK ticks.
///
/// The app resets into the bootloader with its watchdog live (a started watchdog cannot be
/// stopped), so the bootloader must adopt and pet that same one. `Watchdog::try_new` re-adopts a
/// running watchdog only when the whole hardware config matches. Keep the two construction sites,
/// `obc-fw-nrf54l/src/main.rs` and `obc-boot/src/wdt.rs`, identical to this value, with
/// pause-under-debug-halt, run-through-sleep, and one pet handle (RREN bit 0).
pub const WDT_TIMEOUT_TICKS: u32 = 24 * 32768;

const MAGIC: [u8; 4] = *b"OBCB";

/// The layout this crate writes. Bump it on any byte-layout change; an unknown version decodes to
/// `Idle`, because a format skew must never read as a live install request.
///
/// Readers accept [`FORMAT_VERSION_V1`] too: the bootloader is flashed by probe and is not updated
/// by DFU, so a fielded bootloader keeps writing v1 pages after the app updates. `Armed`/`Trial`
/// payloads are identical in v1 and v2; a v1 `Idle` body ends after the installed option and
/// decodes with `last_outcome: None`.
const FORMAT_VERSION: u16 = 2;

/// Accepted by [`BootState::decode`], never written; see [`FORMAT_VERSION`].
const FORMAT_VERSION_V1: u16 = 1;

// Fixed header offsets (little-endian throughout).
const OFF_MAGIC: usize = 0; //  4 : magic
const OFF_VERSION: usize = 4; //  2 : format_version
const OFF_TAG: usize = 6; //  1 : state tag (0 Idle, 1 Armed, 2 Trial)
                          //  7 : 1 : reserved (0)
const OFF_BLOB_LEN: usize = 8; //  4 : blob_len (whole 16-aligned encoded length incl. CRC)
const OFF_GENERATION: usize = 12; //  4 : generation
const HDR_LEN: usize = 16; // payload starts here
const CRC_LEN: usize = 4; // trailing whole-blob CRC-32

const TAG_IDLE: u8 = 0;
const TAG_ARMED: u8 = 1;
const TAG_TRIAL: u8 = 2;

const OUTCOME_INSTALLED: u8 = 0;
const OUTCOME_ROLLED_BACK: u8 = 1;
const OUTCOME_STAGE_REJECTED: u8 = 2;
const OUTCOME_ARM_ABANDONED: u8 = 3;

/// Maximum extents in a single [`StagedRef`]. A staged object holds at most eight extent ranges on
/// the card, and each range is one contiguous block run, so eight is the whole of what an arm can
/// ever resolve. The armer errors out past this instead of truncating the extent chain.
pub const MAX_EXTENTS: usize = 8;

/// A contiguous run of absolute 512-byte SD blocks. The extents are resolved before the arm, so the
/// bootloader reads the staged image with no catalog and no store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extent {
    pub start_block: u32,
    /// Length of the run in 512-byte blocks.
    pub blocks: u32,
}

const EXTENT_WIRE: usize = 8;
const fn staged_wire_len(n: usize) -> usize {
    HEADER_LEN + 4 /* len */ + 4 /* crc32 */ + 2 /* extent_count */ + n * EXTENT_WIRE
}

/// A staged image on the SD card, resolved to raw block extents. Fixed capacity and `Copy`, so it
/// nests inside [`BootState`] and [`BootDecision`] without borrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedRef {
    pub header: ImageHeader,
    /// Raw image length in bytes. It repeats `header.image_len` so the installer does not decode the
    /// header to know how far to read.
    pub len: u32,
    /// CRC-32 over the whole raw image. The installer checks it before it erases the app slot.
    pub crc32: u32,
    extents: [Extent; MAX_EXTENTS],
    extent_count: u16,
}

impl StagedRef {
    /// Returns `None` past [`MAX_EXTENTS`], or if `len`/`crc32` disagree with the header: a record
    /// whose redundant fields diverge was never built from one coherent image. Unused capacity is
    /// zero-filled, so equality depends only on the live extents.
    pub fn new(header: ImageHeader, len: u32, crc32: u32, extents: &[Extent]) -> Option<StagedRef> {
        if extents.len() > MAX_EXTENTS || len != header.image_len || crc32 != header.image_crc32 {
            return None;
        }
        let mut store = [Extent::default(); MAX_EXTENTS];
        store[..extents.len()].copy_from_slice(extents);
        Some(StagedRef { header, len, crc32, extents: store, extent_count: extents.len() as u16 })
    }

    /// The live extents, in image order.
    pub fn extents(&self) -> &[Extent] {
        &self.extents[..self.extent_count as usize]
    }

    pub fn extent_count(&self) -> usize {
        self.extent_count as usize
    }

    fn write(&self, out: &mut [u8], off: usize) -> usize {
        let mut c = off;
        out[c..c + HEADER_LEN].copy_from_slice(&self.header.encode());
        c += HEADER_LEN;
        out[c..c + 4].copy_from_slice(&self.len.to_le_bytes());
        c += 4;
        out[c..c + 4].copy_from_slice(&self.crc32.to_le_bytes());
        c += 4;
        out[c..c + 2].copy_from_slice(&self.extent_count.to_le_bytes());
        c += 2;
        for e in self.extents() {
            out[c..c + 4].copy_from_slice(&e.start_block.to_le_bytes());
            out[c + 4..c + 8].copy_from_slice(&e.blocks.to_le_bytes());
            c += EXTENT_WIRE;
        }
        c
    }

    /// `None` on any bounds or consistency failure, which decodes the whole page to `Idle`.
    fn read(body: &[u8], off: usize) -> Option<(StagedRef, usize)> {
        let mut c = off;
        let header_bytes: &[u8; HEADER_LEN] = body.get(c..c + HEADER_LEN)?.try_into().ok()?;
        let header = ImageHeader::decode(header_bytes)?;
        c += HEADER_LEN;
        let len = u32::from_le_bytes(body.get(c..c + 4)?.try_into().ok()?);
        c += 4;
        let crc = u32::from_le_bytes(body.get(c..c + 4)?.try_into().ok()?);
        c += 4;
        let count = u16::from_le_bytes(body.get(c..c + 2)?.try_into().ok()?) as usize;
        c += 2;
        if count > MAX_EXTENTS {
            return None;
        }
        // The redundant fields must agree with the header, or the installer has two truths to pick
        // from.
        if len != header.image_len || crc != header.image_crc32 {
            return None;
        }
        let mut extents = [Extent::default(); MAX_EXTENTS];
        for e in extents.iter_mut().take(count) {
            let start_block = u32::from_le_bytes(body.get(c..c + 4)?.try_into().ok()?);
            let blocks = u32::from_le_bytes(body.get(c + 4..c + 8)?.try_into().ok()?);
            *e = Extent { start_block, blocks };
            c += EXTENT_WIRE;
        }
        Some((StagedRef { header, len, crc32: crc, extents, extent_count: count as u16 }, c))
    }
}

/// What terminally happened to the last arm, written into the `Idle` that ends it so the next
/// boot's [`verdict`] reads a fact. Version strings cannot give the answer: a same-version re-stage
/// leaves the same `installed` header whether it was installed, rolled back or rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    /// The staged image is now the running image: a trial accepted with no rollback, or a rollback
    /// that could not restore its snapshot and therefore kept the flashed image.
    Installed,
    /// An unconfirmed trial was rolled back to its snapshot.
    RolledBack,
    /// The staged image failed verification before the app slot was erased; the old app is intact.
    StageRejected,
    /// The bootloader gave up on an `Armed` card it could not read and booted the intact app.
    /// Nothing in this crate writes it yet.
    ArmAbandoned,
}

impl OutcomeKind {
    fn tag(self) -> u8 {
        match self {
            OutcomeKind::Installed => OUTCOME_INSTALLED,
            OutcomeKind::RolledBack => OUTCOME_ROLLED_BACK,
            OutcomeKind::StageRejected => OUTCOME_STAGE_REJECTED,
            OutcomeKind::ArmAbandoned => OUTCOME_ARM_ABANDONED,
        }
    }

    fn from_tag(tag: u8) -> Option<OutcomeKind> {
        match tag {
            OUTCOME_INSTALLED => Some(OutcomeKind::Installed),
            OUTCOME_ROLLED_BACK => Some(OutcomeKind::RolledBack),
            OUTCOME_STAGE_REJECTED => Some(OutcomeKind::StageRejected),
            OUTCOME_ARM_ABANDONED => Some(OutcomeKind::ArmAbandoned),
            _ => None,
        }
    }
}

/// Carried in [`BootState::Idle`]. The `generation` ties the outcome to the boot's arm marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastOutcome {
    pub kind: OutcomeKind,
    pub generation: u32,
}

/// The handoff state the app writes and the bootloader reads.
///
/// The variants differ in size because [`StagedRef`] inlines its extent array; the bootloader has
/// no `alloc`, and the enum lives on the stack only for the length of a boot decision.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootState {
    /// Normal boot: no pending update. `installed` is the running image, or `None` on a device that
    /// never installed an update. `last_outcome` is `None` unless this `Idle` ended an arm.
    Idle { installed: Option<ImageHeader>, last_outcome: Option<LastOutcome> },
    /// An update is staged and armed. `rollback` is the snapshot of the outgoing image, absent on a
    /// first install.
    Armed { generation: u32, update: StagedRef, rollback: Option<StagedRef> },
    /// A freshly-installed image is on its single trial boot. The app confirms health by writing
    /// `Idle`; if it does not, the next boot rolls back to `rollback`, or accepts when there is none.
    Trial { generation: u32, installed: ImageHeader, rollback: Option<StagedRef> },
}

/// The largest encoded blob: `Armed` with two full [`StagedRef`]s, padded to the 16-byte RRAM line.
pub const MAX_ENCODED_LEN: usize = {
    let max_staged = staged_wire_len(MAX_EXTENTS);
    let armed_payload = max_staged + 1 /* has_rollback */ + max_staged;
    let unpadded = HDR_LEN + armed_payload + CRC_LEN;
    unpadded.div_ceil(16) * 16
};

const _: () = assert!(MAX_ENCODED_LEN.is_multiple_of(16), "encoded blob must be 16-byte-line aligned for RRAMC");
const _: () = assert!(MAX_ENCODED_LEN <= PAGE_LEN, "encoded blob must fit the boot-state page");

pub struct EncodedPage {
    buf: [u8; MAX_ENCODED_LEN],
    len: usize,
}

impl EncodedPage {
    /// The bytes to write to RRAM; the length is a multiple of 16.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Always false; the method exists for the `len`-without-`is_empty` lint.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

fn put_opt_header(out: &mut [u8], off: usize, h: &Option<ImageHeader>) -> usize {
    match h {
        Some(h) => {
            out[off] = 1;
            out[off + 1..off + 1 + HEADER_LEN].copy_from_slice(&h.encode());
            off + 1 + HEADER_LEN
        }
        None => {
            out[off] = 0;
            off + 1
        }
    }
}

fn put_opt_staged(out: &mut [u8], off: usize, s: &Option<StagedRef>) -> usize {
    match s {
        Some(s) => {
            out[off] = 1;
            s.write(out, off + 1)
        }
        None => {
            out[off] = 0;
            off + 1
        }
    }
}

fn put_opt_outcome(out: &mut [u8], off: usize, o: &Option<LastOutcome>) -> usize {
    match o {
        Some(o) => {
            out[off] = 1;
            out[off + 1] = o.kind.tag();
            out[off + 2..off + 6].copy_from_slice(&o.generation.to_le_bytes());
            off + 6
        }
        None => {
            out[off] = 0;
            off + 1
        }
    }
}

fn get_opt_outcome(body: &[u8], off: usize) -> Option<(Option<LastOutcome>, usize)> {
    match body.get(off)? {
        0 => Some((None, off + 1)),
        1 => {
            let kind = OutcomeKind::from_tag(*body.get(off + 1)?)?;
            let generation = u32::from_le_bytes(body.get(off + 2..off + 6)?.try_into().ok()?);
            Some((Some(LastOutcome { kind, generation }), off + 6))
        }
        _ => None,
    }
}

fn get_opt_header(body: &[u8], off: usize) -> Option<(Option<ImageHeader>, usize)> {
    match body.get(off)? {
        0 => Some((None, off + 1)),
        1 => {
            let hb: &[u8; HEADER_LEN] = body.get(off + 1..off + 1 + HEADER_LEN)?.try_into().ok()?;
            Some((Some(ImageHeader::decode(hb)?), off + 1 + HEADER_LEN))
        }
        _ => None,
    }
}

fn get_opt_staged(body: &[u8], off: usize) -> Option<(Option<StagedRef>, usize)> {
    match body.get(off)? {
        0 => Some((None, off + 1)),
        1 => {
            let (s, c) = StagedRef::read(body, off + 1)?;
            Some((Some(s), c))
        }
        _ => None,
    }
}

impl BootState {
    /// The `generation` of an `Armed` or `Trial` page, else `0`. It labels the in-flight arm so a
    /// recorded [`LastOutcome`] can be tied back to it; it is not monotonic and guards nothing.
    pub fn generation(&self) -> u32 {
        match self {
            BootState::Idle { .. } => 0,
            BootState::Armed { generation, .. } | BootState::Trial { generation, .. } => *generation,
        }
    }

    /// The OBCU header of the image that is running now, when the page records one.
    ///
    /// Each state names the running image in a different place. An `Armed` page reports the
    /// `rollback` snapshot, because the bootloader never consumed the arm and the old image still
    /// runs. It is never the staged image: an arm is a request, not a fact about what runs.
    pub fn running_image(&self) -> Option<ImageHeader> {
        match self {
            BootState::Idle { installed, .. } => *installed,
            BootState::Trial { installed, .. } => Some(*installed),
            BootState::Armed { rollback, .. } => rollback.map(|r| r.header),
        }
    }

    /// Never fails: every [`BootState`] fits [`MAX_ENCODED_LEN`] by construction.
    pub fn encode(&self) -> EncodedPage {
        let mut b = [0u8; MAX_ENCODED_LEN];
        b[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC);
        b[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        let (tag, generation, end) = match self {
            BootState::Idle { installed, last_outcome } => {
                let c = put_opt_header(&mut b, HDR_LEN, installed);
                (TAG_IDLE, 0, put_opt_outcome(&mut b, c, last_outcome))
            }
            BootState::Armed { generation, update, rollback } => {
                let c = update.write(&mut b, HDR_LEN);
                (TAG_ARMED, *generation, put_opt_staged(&mut b, c, rollback))
            }
            BootState::Trial { generation, installed, rollback } => {
                b[HDR_LEN..HDR_LEN + HEADER_LEN].copy_from_slice(&installed.encode());
                (TAG_TRIAL, *generation, put_opt_staged(&mut b, HDR_LEN + HEADER_LEN, rollback))
            }
        };
        b[OFF_TAG] = tag;
        // OFF_TAG + 1 reserved (0)
        b[OFF_GENERATION..OFF_GENERATION + 4].copy_from_slice(&generation.to_le_bytes());
        // Pad up to a whole 16-byte line; the padding is inside the CRC-covered span.
        let blob_len = (end + CRC_LEN).div_ceil(16) * 16;
        b[OFF_BLOB_LEN..OFF_BLOB_LEN + 4].copy_from_slice(&(blob_len as u32).to_le_bytes());
        let crc_off = blob_len - CRC_LEN;
        let crc = crc32(&b[..crc_off]);
        b[crc_off..crc_off + CRC_LEN].copy_from_slice(&crc.to_le_bytes());
        EncodedPage { buf: b, len: blob_len }
    }

    /// Anything that is not a clean read of a known format decodes to an empty `Idle`: bad magic,
    /// an unknown version, a bad `blob_len`, a failed CRC, or an inconsistent payload. The
    /// bootloader always gets a sane state and, at worst, jumps straight to the app.
    pub fn decode(bytes: &[u8]) -> BootState {
        Self::decode_inner(bytes).unwrap_or(BootState::Idle { installed: None, last_outcome: None })
    }

    fn decode_inner(bytes: &[u8]) -> Option<BootState> {
        if bytes.len() < HDR_LEN + CRC_LEN {
            return None;
        }
        if bytes[OFF_MAGIC..OFF_MAGIC + 4] != MAGIC {
            return None;
        }
        let version = u16::from_le_bytes([bytes[OFF_VERSION], bytes[OFF_VERSION + 1]]);
        if version != FORMAT_VERSION && version != FORMAT_VERSION_V1 {
            return None;
        }
        let blob_len = u32::from_le_bytes([
            bytes[OFF_BLOB_LEN],
            bytes[OFF_BLOB_LEN + 1],
            bytes[OFF_BLOB_LEN + 2],
            bytes[OFF_BLOB_LEN + 3],
        ]) as usize;
        // A corrupt length must not read past the buffer or CRC a bogus span.
        if blob_len < HDR_LEN + CRC_LEN || blob_len > bytes.len() || !blob_len.is_multiple_of(16) {
            return None;
        }
        let crc_off = blob_len - CRC_LEN;
        let stored = u32::from_le_bytes([bytes[crc_off], bytes[crc_off + 1], bytes[crc_off + 2], bytes[crc_off + 3]]);
        if stored != crc32(&bytes[..crc_off]) {
            return None;
        }
        let generation = u32::from_le_bytes([
            bytes[OFF_GENERATION],
            bytes[OFF_GENERATION + 1],
            bytes[OFF_GENERATION + 2],
            bytes[OFF_GENERATION + 3],
        ]);
        // Parse only within the CRC-covered span.
        let body = &bytes[..crc_off];
        match bytes[OFF_TAG] {
            TAG_IDLE => {
                let (installed, c) = get_opt_header(body, HDR_LEN)?;
                // A v1 Idle body ends after the installed option. Gate on the version: the zero
                // padding that follows must not parse as an outcome field.
                let last_outcome = if version == FORMAT_VERSION_V1 {
                    None
                } else {
                    let (last_outcome, _) = get_opt_outcome(body, c)?;
                    last_outcome
                };
                Some(BootState::Idle { installed, last_outcome })
            }
            TAG_ARMED => {
                let (update, c) = StagedRef::read(body, HDR_LEN)?;
                let (rollback, _) = get_opt_staged(body, c)?;
                Some(BootState::Armed { generation, update, rollback })
            }
            TAG_TRIAL => {
                let hb: &[u8; HEADER_LEN] = body.get(HDR_LEN..HDR_LEN + HEADER_LEN)?.try_into().ok()?;
                let installed = ImageHeader::decode(hb)?;
                let (rollback, _) = get_opt_staged(body, HDR_LEN + HEADER_LEN)?;
                Some(BootState::Trial { generation, installed, rollback })
            }
            _ => None,
        }
    }
}

/// What the bootloader must do this boot. Each variant carries everything
/// [`engine::run`](crate::engine::run) needs to execute it, so the engine never re-reads the source
/// [`BootState`] to recover a `generation` or a `rollback`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootDecision {
    Jump,
    /// Flash the staged `update`, then write `Trial` and jump into it. Idempotent: a power loss
    /// during the flash re-enters here on the next boot.
    Install {
        update: StagedRef,
        generation: u32,
        rollback: Option<StagedRef>,
    },
    /// The trial boot was not confirmed and a `snapshot` exists: flash the snapshot back.
    Rollback {
        snapshot: StagedRef,
        installed: ImageHeader,
        generation: u32,
    },
    /// The trial boot was not confirmed and there is no snapshot (a first install): accept the
    /// running image and clear the state to `Idle`.
    AcceptAndClear {
        installed: ImageHeader,
        generation: u32,
    },
}

/// The bootloader's decision function. A healthy app confirms by writing `Idle` mid-run, so a
/// `Trial` page that reaches the bootloader means the trial failed and must roll back. The install
/// path must therefore jump into the new image, never reset: a reset would re-enter the bootloader
/// with the fresh `Trial` and roll the image back before it ever ran.
pub fn decide(state: &BootState) -> BootDecision {
    match state {
        BootState::Idle { .. } => BootDecision::Jump,
        BootState::Armed { generation, update, rollback } => {
            BootDecision::Install { update: *update, generation: *generation, rollback: *rollback }
        }
        BootState::Trial { generation, installed, rollback } => match rollback {
            Some(r) => BootDecision::Rollback { snapshot: *r, installed: *installed, generation: *generation },
            None => BootDecision::AcceptAndClear { installed: *installed, generation: *generation },
        },
    }
}

/// The one-time post-update verdict, from the boot-state page and the app's arm marker. It reads
/// the recorded [`LastOutcome`], never a version string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// This boot is the trial boot; the app's confirm owns the outcome and the board does nothing.
    TrialInProgress,
    /// No arm was pending.
    None,
    /// The staged image is now the running image.
    Confirmed,
    /// The staged image is not running: rejected, rolled back, or abandoned.
    Reverted,
    /// An `Armed` record survived into the running app, so the bootloader never consumed it. The
    /// board downgrades the stray arm to `Idle`.
    NotStarted,
}

/// Turns the boot-state page and the arm marker's `generation` (`None` = no arm was pending) into
/// the one-time [`Verdict`]. An `Idle` with a marker but no outcome of that generation reads as
/// [`Reverted`](Verdict::Reverted): we cannot prove the staged image is running.
pub fn verdict(state: &BootState, marker_generation: Option<u32>) -> Verdict {
    match state {
        BootState::Trial { .. } => Verdict::TrialInProgress,
        BootState::Armed { .. } => Verdict::NotStarted,
        BootState::Idle { last_outcome, .. } => match marker_generation {
            None => Verdict::None,
            Some(gen) => match last_outcome {
                Some(o) if o.generation == gen => match o.kind {
                    OutcomeKind::Installed => Verdict::Confirmed,
                    OutcomeKind::RolledBack | OutcomeKind::StageRejected | OutcomeKind::ArmAbandoned => {
                        Verdict::Reverted
                    }
                },
                _ => Verdict::Reverted,
            },
        },
    }
}
