//! The boot-state RRAM handoff page + the bootloader's decision logic (see `OBCU_Spec.md` §2).
//!
//! One CRC-framed blob written to a dedicated 4 KB RRAM page ([`PAGE_LEN`]), the sole channel between
//! the app (the *armer*, §S4) and the bootloader (the *installer*, §S3). It is torn-write-safe by the
//! settings-codec recipe: **anything that doesn't cleanly decode is [`BootState::Idle`]** — so a
//! half-written page, a blank page, or a bit-flip the CRC catches all mean "no pending update, jump to
//! the app", never a garbage install. A `generation` counter is bumped on every arm and carried through
//! the `Armed`/`Trial` records and into the app's arm marker; it is the key that ties a recorded
//! [`LastOutcome`] back to the arm that produced it (see [`verdict`]). It is a diagnostic breadcrumb, not
//! a replay guard: the single-page overwrite-in-place design has no live stale-replay vector, so nothing
//! compares generations to *reject* a page — a torn or stale page is caught by the CRC frame and decodes
//! to `Idle` regardless of its generation.
//!
//! RRAMC writes 16-byte lines, so the encoded blob length is always a multiple of 16 (guaranteed by
//! construction and pinned by a compile-time assert, mirroring `obc-app`'s settings codec) — the armer
//! writes whole lines with no read-modify-write.

use crate::crc32::crc32;
use crate::image::{ImageHeader, HEADER_LEN};

/// Physical size of the boot-state RRAM page. The encoded blob always fits inside it with room to
/// spare; the reader may pass the whole 4 KB read to [`BootState::decode`].
pub const PAGE_LEN: usize = 4096;

/// The hardware watchdog period shared across the DFU boot chain: 24 s of 32768 Hz LFCLK ticks.
///
/// Not a byte format, but a handoff contract all the same (DR1, #729). The app arms an update and
/// warm-resets into the bootloader with its WDT **live** (a started dog can never be stopped), so
/// the bootloader must adopt and pet that exact dog through the install; and before jumping into a
/// trial boot the bootloader starts the same dog itself, so a trial image that wedges before the
/// app's own WDT setup still resets back into a rollback. embassy-nrf's `Watchdog::try_new` only
/// re-adopts a running watchdog when the *whole* hardware config matches, so both sides must
/// construct it identically: this timeout, pause-under-debug-halt, run-through-sleep, and **one**
/// pet handle (RREN = bit 0). The embassy `Config` type itself can't live here (`obc-dfu` is
/// core-only, no HAL edge); the two construction sites are `obc-fw-nrf54l/src/main.rs` (the app,
/// #349) and `obc-boot/src/wdt.rs` — keep them field-for-field in sync with this value.
pub const WDT_TIMEOUT_TICKS: u32 = 24 * 32768;

/// Blob magic — `b"OBCB"` (OpenBikeComputer Boot). A blank/torn page won't match it ⇒ `Idle`.
const MAGIC: [u8; 4] = *b"OBCB";

/// The boot-state layout this crate **writes**. Bump on any byte-layout change; an unknown version
/// decodes to `Idle` (a format skew must never be read as a live install request).
///
/// **v2** (DR2 #730): `Idle` gained a trailing `last_outcome` record so the engine's terminal writes
/// carry *what happened* (accepted / rolled back / rejected / abandoned + the arm's generation)
/// instead of leaving the boot-outcome verdict to version-string inference.
///
/// Readers accept **v1 too** ([`FORMAT_VERSION_V1`]): the bootloader is flashed once by probe and is
/// NOT updated by DFU, so a fielded bootloader keeps writing v1 pages after the app updates. A hard
/// cutover would make the new app reject the old bootloader's freshly-written v1 `Trial` page, never
/// confirm the trial, and self-revert the very update that carries this bump. `Armed`/`Trial`
/// payloads are byte-identical across v1/v2; a v1 `Idle` body simply ends after the installed option
/// and decodes with `last_outcome: None` — gated on the version field explicitly, never by trusting
/// the zero padding to happen to parse as "no outcome".
const FORMAT_VERSION: u16 = 2;

/// The pre-DR2 layout, still **read-accepted** by [`BootState::decode`] (see [`FORMAT_VERSION`] for
/// the skew story). Writers always emit [`FORMAT_VERSION`].
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

// `LastOutcome::kind` wire tags (see [`OutcomeKind`]).
const OUTCOME_INSTALLED: u8 = 0;
const OUTCOME_ROLLED_BACK: u8 = 1;
const OUTCOME_STAGE_REJECTED: u8 = 2;
const OUTCOME_ARM_ABANDONED: u8 = 3;

/// Maximum extents in a single [`StagedRef`]. A ~900 KB image over 16 KB FAT clusters resolves to
/// ≈56 extents; 96 leaves headroom for a moderately fragmented card. The armer errors out past this
/// (suggesting a re-copy to defragment, §S4) rather than truncating the extent chain.
pub const MAX_EXTENTS: usize = 96;

/// A contiguous run of absolute 512-byte SD blocks — the pre-resolved location of part of a staged
/// image, so the bootloader reads raw SPI blocks with **no FAT** (epic #615 architecture step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extent {
    /// First absolute 512-byte block of the run.
    pub start_block: u32,
    /// Number of 512-byte blocks in the run.
    pub blocks: u32,
}

/// Bytes an [`Extent`] occupies on the wire.
const EXTENT_WIRE: usize = 8;
/// Wire size of a [`StagedRef`] with `n` extents.
const fn staged_wire_len(n: usize) -> usize {
    HEADER_LEN + 4 /* len */ + 4 /* crc32 */ + 2 /* extent_count */ + n * EXTENT_WIRE
}

/// A staged image on the SD card, resolved to raw block extents: its OBCU [`header`](StagedRef::header),
/// total [`len`](StagedRef::len) + whole-image [`crc32`](StagedRef::crc32) (what the installer verifies
/// before erasing), and the bounded extent chain that locates the bytes. `Copy` (a fixed-capacity POD)
/// so it nests inside [`BootState`] and [`BootDecision`] without borrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedRef {
    /// The staged image's OBCU header (its own length/CRC/version, self-validating).
    pub header: ImageHeader,
    /// Total raw image length, bytes (matches `header.image_len`; stored so the installer needn't
    /// re-decode the header to know how far to read).
    pub len: u32,
    /// CRC-32/IEEE over the whole raw image — the installer's verify-before-erase check.
    pub crc32: u32,
    /// Fixed-capacity extent store; only the first [`extent_count`](StagedRef::extent_count) are live.
    extents: [Extent; MAX_EXTENTS],
    extent_count: u16,
}

impl StagedRef {
    /// Build a [`StagedRef`] from a slice of extents, or `None` if it exceeds [`MAX_EXTENTS`] (the
    /// too-fragmented case the armer turns into a user-facing "re-copy the file" error, §S4) or if
    /// `len`/`crc32` disagree with the embedded header's own `image_len`/`image_crc32` — the fields
    /// are deliberately redundant (the installer reads them without re-decoding the header), so a
    /// record where they diverge was never built from one coherent image. Unused capacity is
    /// zero-filled so equality is defined solely by the live prefix.
    pub fn new(header: ImageHeader, len: u32, crc32: u32, extents: &[Extent]) -> Option<StagedRef> {
        if extents.len() > MAX_EXTENTS || len != header.image_len || crc32 != header.image_crc32 {
            return None;
        }
        let mut store = [Extent::default(); MAX_EXTENTS];
        store[..extents.len()].copy_from_slice(extents);
        Some(StagedRef { header, len, crc32, extents: store, extent_count: extents.len() as u16 })
    }

    /// The live extent chain (absolute 512-byte SD blocks, in image order).
    pub fn extents(&self) -> &[Extent] {
        &self.extents[..self.extent_count as usize]
    }

    /// Number of live extents.
    pub fn extent_count(&self) -> usize {
        self.extent_count as usize
    }

    /// Serialize into `out` at `off`, returning the offset past the last byte written. Caller has
    /// pre-checked `out` is large enough (the encode buffer is sized to the max).
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

    /// Parse a [`StagedRef`] at `off` within `body`, returning it and the offset past it, or `None`
    /// on any bounds/consistency failure (which bubbles up to an `Idle` decode).
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
        // The redundant fields MUST agree with the embedded header (spec §2.3) — a diverging record
        // would leave the installer silently picking one of two truths, so it decodes to Idle instead.
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

/// What terminally happened to the last arm, recorded by the engine (and, for the confirm, the
/// armer) in the `Idle` it writes so the next boot's [`verdict`] reads a **fact** instead of
/// inferring one from version strings (DR2 #730). The two failure endings — a rollback and a
/// pre-erase reject — otherwise both land in an `Idle` whose `installed` header can equal the
/// staged version (a same-version re-stage), which is exactly the misreport this record kills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    /// The staged image is now the running image: a first-install trial accepted without a rollback
    /// ([`BootDecision::AcceptAndClear`]), or a rollback that couldn't restore its (bad) snapshot and
    /// therefore kept the freshly-flashed image.
    Installed,
    /// An unconfirmed trial was rolled back to its snapshot — the staged image is **not** running.
    RolledBack,
    /// The staged image failed verification before the app slot was erased — the old app is intact
    /// and the staged image never ran.
    StageRejected,
    /// The bootloader gave up on an `Armed` card it couldn't read and booted the intact app instead.
    /// **Reserved for DR3 (#731)**, which becomes its writer; nothing in this crate writes it yet, but
    /// [`verdict`] handles it so DR3 is a pure addition.
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

/// The recorded outcome of the last arm: [`what happened`](OutcomeKind) plus the `generation` of the
/// arm it belongs to (so [`verdict`] can tie it to the boot's arm marker — generation's first real
/// reader). Carried in [`BootState::Idle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastOutcome {
    /// What terminally happened to the arm.
    pub kind: OutcomeKind,
    /// The `generation` of the arm this outcome belongs to (the `Armed`/`Trial` record's generation).
    pub generation: u32,
}

/// The handoff state the bootloader reads and the app writes. See the module doc for the torn-write
/// contract; every variant round-trips through [`encode`](BootState::encode)/[`decode`](BootState::decode).
///
/// The variants differ in size because [`StagedRef`] inlines a [`MAX_EXTENTS`] extent array (there is
/// no `alloc` in the bootloader budget, so `Box` is out — the fixed-capacity store *is* the design).
/// The enum lives on the stack for the length of a boot decision and is never held in bulk, so the
/// size spread is fine; the `large_enum_variant` lint doesn't apply to a no-alloc target.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootState {
    /// Normal boot: no pending update. Carries the header of the image currently installed (for the
    /// UI's "current version" and to seed a rollback snapshot), or `None` on a fresh device, plus the
    /// [`last_outcome`](LastOutcome) of the arm that produced this `Idle` (the terminal write records
    /// what happened for the next boot's [`verdict`]; `None` for a plain steady-state `Idle`, a
    /// fresh device, or a v1 page migrated up).
    Idle { installed: Option<ImageHeader>, last_outcome: Option<LastOutcome> },
    /// An update is staged and armed: `install` it. `generation` bumps on every arm; `rollback` is the
    /// snapshot of the outgoing image (absent on the very first install).
    Armed { generation: u32, update: StagedRef, rollback: Option<StagedRef> },
    /// A freshly-installed image is on its **single trial boot**: the app confirms health by writing
    /// `Idle`, otherwise the next boot rolls back. `installed` is what's running; `rollback` is the
    /// snapshot to restore (absent ⇒ accept, first-install case).
    Trial { generation: u32, installed: ImageHeader, rollback: Option<StagedRef> },
}

/// The largest possible encoded blob: `Armed` with two full [`MAX_EXTENTS`] [`StagedRef`]s, padded to
/// the 16-byte RRAM line. Sizes the encode buffer and is asserted to fit the page below.
pub const MAX_ENCODED_LEN: usize = {
    let max_staged = staged_wire_len(MAX_EXTENTS);
    let armed_payload = max_staged + 1 /* has_rollback */ + max_staged;
    let unpadded = HDR_LEN + armed_payload + CRC_LEN;
    unpadded.div_ceil(16) * 16
};

// Compile-time guards (the settings-codec pattern): the encoded blob is a whole number of 16-byte
// RRAM lines and always fits the physical page.
const _: () = assert!(MAX_ENCODED_LEN.is_multiple_of(16), "encoded blob must be 16-byte-line aligned for RRAMC");
const _: () = assert!(MAX_ENCODED_LEN <= PAGE_LEN, "encoded blob must fit the boot-state page");

/// A written boot-state blob: a fixed-capacity buffer plus the live length. [`as_bytes`](EncodedPage::as_bytes)
/// yields exactly the bytes to write — already a whole number of 16-byte lines.
pub struct EncodedPage {
    buf: [u8; MAX_ENCODED_LEN],
    len: usize,
}

impl EncodedPage {
    /// The encoded bytes to write to RRAM (length is a multiple of 16).
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Encoded length, bytes (a multiple of 16).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the encoding is empty — never true (a blob always has the 16-byte header + CRC); the
    /// method exists only to satisfy the `len`-without-`is_empty` lint.
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
    /// The `generation` counter for the variants that carry one (`Armed`/`Trial`), else `0` (`Idle`).
    /// A diagnostic breadcrumb: it labels the in-flight arm so a recorded [`LastOutcome`] can be tied
    /// back to it (see [`verdict`]). Nothing compares it to *reject* a page — `Idle` pins it to `0`
    /// ([`encode`](BootState::encode)), so it is not monotonic across a cycle and carries no replay
    /// guarantee; corruption is caught by the CRC frame, not this field.
    pub fn generation(&self) -> u32 {
        match self {
            BootState::Idle { .. } => 0,
            BootState::Armed { generation, .. } | BootState::Trial { generation, .. } => *generation,
        }
    }

    /// The OBCU header of the image **that is running right now**, when the page records one.
    ///
    /// Every state names the running image in its own place, and getting the mapping wrong would
    /// misreport the device's version to a host (#996 / epic #773), so it lives here beside the
    /// codec rather than in each reader:
    ///
    /// - `Idle` — the `installed` record, i.e. what the last confirmed install left behind. `None`
    ///   on a device that has never installed an update (probe-flashed), which is the whole reason
    ///   this returns an `Option`.
    /// - `Trial` — `installed` *is* what's running: the bootloader wrote this page on its way into
    ///   the freshly-installed image's single trial boot, before the app confirms it.
    /// - `Armed` — an arm that the bootloader never consumed, so the **old** image is still
    ///   running, and its header is the one the armer snapshotted into `rollback` (absent on a
    ///   first install, exactly as the arm recorded it). The same mapping the app's stray-arm
    ///   downgrade uses when it rewrites this page as `Idle`.
    ///
    /// Never the *staged* image: `Armed { update }` is a request, not a fact about the running
    /// image, and reporting it would claim a version the device may never boot.
    pub fn running_image(&self) -> Option<ImageHeader> {
        match self {
            BootState::Idle { installed, .. } => *installed,
            BootState::Trial { installed, .. } => Some(*installed),
            BootState::Armed { rollback, .. } => rollback.map(|r| r.header),
        }
    }

    /// Encode into the CRC-framed, 16-byte-line-aligned blob (see the module doc + `OBCU_Spec.md` §2).
    /// Never fails: every [`BootState`] fits [`MAX_ENCODED_LEN`] by construction.
    pub fn encode(&self) -> EncodedPage {
        let mut b = [0u8; MAX_ENCODED_LEN];
        b[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC);
        b[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        // One match over `self` yields the tag, generation, and the payload end offset together — the
        // payload writers fill `b[HDR_LEN..]`, disjoint from the tag/generation header fields written
        // below, so the write order doesn't matter and `blob_len` only needs `end`.
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
        // Pad (with the CRC) up to a whole 16-byte line; the padding is inside the CRC-covered span.
        let blob_len = (end + CRC_LEN).div_ceil(16) * 16;
        b[OFF_BLOB_LEN..OFF_BLOB_LEN + 4].copy_from_slice(&(blob_len as u32).to_le_bytes());
        let crc_off = blob_len - CRC_LEN;
        let crc = crc32(&b[..crc_off]);
        b[crc_off..crc_off + CRC_LEN].copy_from_slice(&crc.to_le_bytes());
        EncodedPage { buf: b, len: blob_len }
    }

    /// Decode a boot-state page. **Anything that isn't a clean read of a known format decodes to
    /// `Idle { installed: None, last_outcome: None }`** — a blank/torn page, bad magic, an unknown
    /// version (v1 and v2 are both accepted — see [`FORMAT_VERSION`]), a bad `blob_len`, a failed
    /// whole-blob CRC, or an inconsistent payload. This is the torn-write safety net (invariant 4):
    /// the bootloader always gets a sane state and, worst case, jumps straight to the app.
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
        // A corrupt length must never let us read past the buffer or CRC a bogus span.
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
        // Parse only within the CRC-covered span (trailing pad is ignored by the getters).
        let body = &bytes[..crc_off];
        match bytes[OFF_TAG] {
            TAG_IDLE => {
                let (installed, c) = get_opt_header(body, HDR_LEN)?;
                // A v1 Idle body ends right after the installed option — there is no outcome field
                // to read, and the zero padding that follows must NOT be parsed as one (gate on the
                // version explicitly; padding bytes are format-noise, not fields).
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

/// What the bootloader should do this boot (see `OBCU_Spec.md` §2.4). Derived purely from the decoded
/// [`BootState`] by [`decide`] so the bootloader `main` stays a dumb driver and the whole matrix is
/// host-tested here.
///
/// Sized by its inlined [`StagedRef`] like [`BootState`] — same no-alloc rationale (see that type).
///
/// Each variant carries **everything [`engine::run`](crate::engine::run) needs to execute it**, so
/// the engine never re-matches the source [`BootState`] to recover a `generation` or `rollback` —
/// which used to require dead "unreachable via decide()" fallbacks that would have silently
/// substituted `generation = 0` / `rollback = None` into a written state record if `decide`'s
/// mapping ever drifted (DR7, #735).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootDecision {
    /// Nothing pending — jump to the app (`Idle`).
    Jump,
    /// Flash the staged `update`, then write `Trial` and **jump into it** — the one trial boot
    /// (`Armed`). Idempotent: a power loss mid-flash re-enters here next boot (invariant 2). Carries
    /// the arm's `generation` (recorded in the terminal outcome) and the `rollback` snapshot that
    /// rides into the `Trial` record (and forward into the reject `Idle` on a bad stage).
    Install { update: StagedRef, generation: u32, rollback: Option<StagedRef> },
    /// The trial boot went unconfirmed and a `snapshot` exists — flash it back (`Trial` with a
    /// rollback). Carries the trial's `installed` header (carried into the `Idle` on a bad snapshot)
    /// and the arm's `generation` (recorded in the terminal outcome).
    Rollback { snapshot: StagedRef, installed: ImageHeader, generation: u32 },
    /// The trial boot went unconfirmed with **no** snapshot (the first-install case) — accept the
    /// running image and clear the state to `Idle` (`Trial` without a rollback). Carries the running
    /// image's `installed` header and the arm's `generation`, both recorded in the accepted `Idle`.
    AcceptAndClear { installed: ImageHeader, generation: u32 },
}

/// The bootloader's decision function, as pure logic. The bootloader entry reads the page, calls this,
/// and executes the returned action; every branch is unit-tested here so `main` carries no logic.
///
/// - `Idle`  ⇒ [`Jump`](BootDecision::Jump).
/// - `Armed` ⇒ [`Install`](BootDecision::Install) the staged update.
/// - `Trial` ⇒ [`Rollback`](BootDecision::Rollback) if a snapshot exists, else
///   [`AcceptAndClear`](BootDecision::AcceptAndClear).
///
/// (The confirm path — a healthy app writing `Idle{installed}` mid-run — never reaches the bootloader
/// as a `Trial`, which is exactly why an *unconfirmed* `Trial` means "roll back". Load-bearing
/// corollary: the install path must **jump, never reset**, after writing `Trial` — a reset would
/// re-enter the bootloader with the fresh `Trial` and roll the image back before it ever ran.)
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

/// The one-time post-update verdict, derived from the boot-state page (which carries the recorded
/// [`LastOutcome`]) and the app's arm marker — the pure sibling of [`decide`]/[`confirm_trial`],
/// host-tested here so the board's `reconcile_boot_outcome` shrinks to IO + a card mapping (DR2 #730).
///
/// It **never reads version strings**: the outcome is a recorded fact, so a same-version re-stage
/// that was rejected or rolled back reads as [`Reverted`](Verdict::Reverted), not the false
/// [`Confirmed`](Verdict::Confirmed) the old version-equality check produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// This boot **is** the trial boot (the page is still `Trial`); the app's health-anchor confirm
    /// owns the outcome. The board does nothing here.
    TrialInProgress,
    /// No arm was pending — a plain boot (no marker). Nothing to show.
    None,
    /// The staged image is now the running image. The board clears the marker and shows the success
    /// toast.
    Confirmed,
    /// The staged image is **not** running — rejected before the erase, rolled back, or (DR3) an
    /// abandoned arm. The board clears the marker and shows the failure card.
    Reverted,
    /// An `Armed` record survived into the running app — the bootloader never consumed it (stale or
    /// missing bootloader). The board downgrades the stray arm to `Idle`, clears the marker, and
    /// shows the not-started card.
    NotStarted,
}

/// Turn the decoded boot-state page + the arm marker's `generation` (`None` = no marker, i.e. no arm
/// was pending) into the one-time [`Verdict`]. Pure; the whole matrix is host-tested.
///
/// - **`Trial`** ⇒ [`TrialInProgress`](Verdict::TrialInProgress) — the confirm owns it.
/// - **`Armed`** ⇒ [`NotStarted`](Verdict::NotStarted) — the bootloader never ran (marker-independent,
///   matching the pre-DR2 board behaviour).
/// - **`Idle`** with **no marker** ⇒ [`None`](Verdict::None); the recorded outcome is just history.
/// - **`Idle`** with a marker whose `generation` matches the recorded [`LastOutcome`] ⇒ the outcome
///   decides: [`Installed`](OutcomeKind::Installed) ⇒ [`Confirmed`](Verdict::Confirmed), every other
///   outcome ⇒ [`Reverted`](Verdict::Reverted).
/// - **`Idle`** with a marker but **no / a stale-generation** outcome (a v1→v2 migrated page, a torn
///   engine write, or an outcome left by an earlier arm) ⇒ [`Reverted`](Verdict::Reverted): the
///   conservative call, since we cannot prove the staged image is running. This is the one-time
///   imprecision a v1 page costs (see `OBCU_Spec.md` §2 + the DR2 PR).
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
