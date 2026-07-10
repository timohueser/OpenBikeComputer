//! The boot-state RRAM handoff page + the bootloader's decision logic (see `OBCU_Spec.md` §2).
//!
//! One CRC-framed blob written to a dedicated 4 KB RRAM page ([`PAGE_LEN`]), the sole channel between
//! the app (the *armer*, §S4) and the bootloader (the *installer*, §S3). It is torn-write-safe by the
//! settings-codec recipe: **anything that doesn't cleanly decode is [`BootState::Idle`]** — so a
//! half-written page, a blank page, or a bit-flip the CRC catches all mean "no pending update, jump to
//! the app", never a garbage install. A `generation` counter (bumped on every arm) lets the installer
//! reject a stale replay (epic #615 safety invariant 4).
//!
//! RRAMC writes 16-byte lines, so the encoded blob length is always a multiple of 16 (guaranteed by
//! construction and pinned by a compile-time assert, mirroring `obc-app`'s settings codec) — the armer
//! writes whole lines with no read-modify-write.

use crate::crc32::crc32;
use crate::image::{ImageHeader, HEADER_LEN};

/// Physical size of the boot-state RRAM page. The encoded blob always fits inside it with room to
/// spare; the reader may pass the whole 4 KB read to [`BootState::decode`].
pub const PAGE_LEN: usize = 4096;

/// Blob magic — `b"OBCB"` (OpenBikeComputer Boot). A blank/torn page won't match it ⇒ `Idle`.
const MAGIC: [u8; 4] = *b"OBCB";

/// The only boot-state layout this crate reads/writes. Bump on any byte-layout change; a mismatched
/// version decodes to `Idle` (a format skew must never be read as a live install request).
const FORMAT_VERSION: u16 = 1;

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
    /// UI's "current version" and to seed a rollback snapshot), or `None` on a fresh device.
    Idle { installed: Option<ImageHeader> },
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
    /// The installer compares it against the last one it acted on to reject a stale-page replay.
    pub fn generation(&self) -> u32 {
        match self {
            BootState::Idle { .. } => 0,
            BootState::Armed { generation, .. } | BootState::Trial { generation, .. } => *generation,
        }
    }

    /// Encode into the CRC-framed, 16-byte-line-aligned blob (see the module doc + `OBCU_Spec.md` §2).
    /// Never fails: every [`BootState`] fits [`MAX_ENCODED_LEN`] by construction.
    pub fn encode(&self) -> EncodedPage {
        let mut b = [0u8; MAX_ENCODED_LEN];
        b[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC);
        b[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        let (tag, generation) = match self {
            BootState::Idle { .. } => (TAG_IDLE, 0),
            BootState::Armed { generation, .. } => (TAG_ARMED, *generation),
            BootState::Trial { generation, .. } => (TAG_TRIAL, *generation),
        };
        b[OFF_TAG] = tag;
        // OFF_TAG + 1 reserved (0)
        b[OFF_GENERATION..OFF_GENERATION + 4].copy_from_slice(&generation.to_le_bytes());
        let end = match self {
            BootState::Idle { installed } => put_opt_header(&mut b, HDR_LEN, installed),
            BootState::Armed { update, rollback, .. } => {
                let c = update.write(&mut b, HDR_LEN);
                put_opt_staged(&mut b, c, rollback)
            }
            BootState::Trial { installed, rollback, .. } => {
                b[HDR_LEN..HDR_LEN + HEADER_LEN].copy_from_slice(&installed.encode());
                put_opt_staged(&mut b, HDR_LEN + HEADER_LEN, rollback)
            }
        };
        // Pad (with the CRC) up to a whole 16-byte line; the padding is inside the CRC-covered span.
        let blob_len = (end + CRC_LEN).div_ceil(16) * 16;
        b[OFF_BLOB_LEN..OFF_BLOB_LEN + 4].copy_from_slice(&(blob_len as u32).to_le_bytes());
        let crc_off = blob_len - CRC_LEN;
        let crc = crc32(&b[..crc_off]);
        b[crc_off..crc_off + CRC_LEN].copy_from_slice(&crc.to_le_bytes());
        EncodedPage { buf: b, len: blob_len }
    }

    /// Decode a boot-state page. **Anything that isn't a clean read of this format decodes to
    /// `Idle { installed: None }`** — a blank/torn page, bad magic/version, a bad `blob_len`, a failed
    /// whole-blob CRC, or an inconsistent payload. This is the torn-write safety net (invariant 4):
    /// the bootloader always gets a sane state and, worst case, jumps straight to the app.
    pub fn decode(bytes: &[u8]) -> BootState {
        Self::decode_inner(bytes).unwrap_or(BootState::Idle { installed: None })
    }

    fn decode_inner(bytes: &[u8]) -> Option<BootState> {
        if bytes.len() < HDR_LEN + CRC_LEN {
            return None;
        }
        if bytes[OFF_MAGIC..OFF_MAGIC + 4] != MAGIC {
            return None;
        }
        if u16::from_le_bytes([bytes[OFF_VERSION], bytes[OFF_VERSION + 1]]) != FORMAT_VERSION {
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
                let (installed, _) = get_opt_header(body, HDR_LEN)?;
                Some(BootState::Idle { installed })
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
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootDecision {
    /// Nothing pending — jump to the app (`Idle`).
    Jump,
    /// Flash the staged image, then write `Trial` and **jump into it** — the one trial boot
    /// (`Armed`). Idempotent: a power loss mid-flash re-enters here next boot (invariant 2).
    Install(StagedRef),
    /// The trial boot went unconfirmed and a snapshot exists — flash it back (`Trial` with a rollback).
    Rollback(StagedRef),
    /// The trial boot went unconfirmed with **no** snapshot (the first-install case) — accept the
    /// running image and clear the state to `Idle` (`Trial` without a rollback).
    AcceptAndClear,
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
        BootState::Armed { update, .. } => BootDecision::Install(*update),
        BootState::Trial { rollback, .. } => match rollback {
            Some(r) => BootDecision::Rollback(*r),
            None => BootDecision::AcceptAndClear,
        },
    }
}
