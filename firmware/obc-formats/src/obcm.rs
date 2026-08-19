//! OBCM map-format constants from `OBCM_Spec.md`.

use crate::io::{rd_u16, validate_prefix, DecodeError};

pub const MAGIC: [u8; 4] = *b"OBCM";
pub const VERSION: u8 = 14;
/// The v14 header (§1): the v13 40-byte layout plus `Offset Scale` and the `Terrain Offset` /
/// `Terrain Length` pair. 49 is not a whole number of units at any scale above `0`, which is why
/// the style table begins at the first unit boundary at or after it rather than at byte 49 (§1.2).
pub const HEADER_LEN: usize = 49;
/// Header offset of the v14 `Offset Scale` byte (§1.1).
pub const HEADER_OFFSET_SCALE_OFF: usize = 40;
/// Header offset of the v14 `Terrain Offset` field (§1.3).
pub const HEADER_TERRAIN_OFFSET_OFF: usize = 41;
/// Header offset of the v14 `Terrain Length` field (§1.3), counted in **units** like the offset
/// beside it.
pub const HEADER_TERRAIN_LENGTH_OFF: usize = 45;
pub const LOD_ENTRY_LEN: usize = 18;
pub const STYLE_RECORD_LEN: usize = 8;

/// Width of the **compact** feature header (§5): `style, flags, pt_count u8, anchor u16 ×2`.
/// The common case — a feature of ≤ 255 vertices whose leaf-relative anchor fits `0..=65535`.
pub const FEATURE_HEADER_COMPACT_LEN: usize = 7;
/// Width of the **wide** feature header (§5, `FEATURE_FLAG_WIDE` set): `style, flags,
/// pt_count u16, anchor i32 ×2` — the escape for a big feature or a leaf spanning more than
/// 65 535 µdeg. Both layouts put `flags` at byte 1, so a reader knows the width before it needs it.
pub const FEATURE_HEADER_WIDE_LEN: usize = 12;

pub const FEATURE_FLAG_16BIT: u8 = 0x01;
pub const FEATURE_FLAG_POLYGON: u8 = 0x02;
pub const FEATURE_FLAG_HOLES: u8 = 0x04;
pub const FEATURE_FLAG_WIDE: u8 = 0x08;
pub const STYLE_PRIORITY_MASK: u8 = 0x03;
pub const STYLE_DASHED_BIT: u8 = 0x04;
pub const STYLE_HAS_COLOR2_BIT: u8 = 0x08;
/// Style-record flag bit 4 (§2, #1095): the style's `weight` is the on-screen stroke width in
/// device pixels, used **verbatim** — the renderer's zoom→width ramp is bypassed for it.
pub const STYLE_FIXED_WIDTH_BIT: u8 = 0x10;
/// Style-record flag bit 5 (§2, #1095): the style belongs to the suppressible **terrain layer**.
/// Written by the packer; the consumer is the device Settings toggle (#1096).
pub const STYLE_TERRAIN_LAYER_BIT: u8 = 0x20;
/// Style-record flag bits 6-7 (§2): still reserved, written `0`. Unlike a *feature*'s flags
/// (§5.2, [`FEATURE_FLAG_WIDE`] & friends), a reader MUST **ignore** style bits it does not
/// define rather than reject the record — that is what lets a bit be defined in place.
pub const STYLE_RESERVED_MASK: u8 = 0xC0;

pub const BRANCH_BIT: u32 = 0x8000_0000;
pub const EMPTY_LEAF: u32 = 0x7FFF_FFFF;
pub const CHUNK_END: u8 = 0xFF;
/// The one fill byte (§1.2). Every gap a scaled offset rounds past — between sections, before a
/// region's chunks, between offset-table-addressed chunks, and §8's 512-byte alignment run — is
/// written `0xFF`, because that is already this format's "nothing here" byte in every chunked
/// section, so filler that leaked into a decode path meets a stop rather than a plausible record.
/// A reserved **field** is still written `0`: a field is content that means nothing yet, and a gap
/// is not content at all.
pub const FILLER: u8 = 0xFF;

// --- §1.1 offset scaling ------------------------------------------------------

/// Largest legal `Offset Scale` (§1.1). `9` is the largest scale at which `512 % U == 0`, and 512
/// is both the card block and this format's own fixed chunk stride — so while `U` divides 512,
/// every §7/§8 chunk start falls on a unit boundary the region start already established.
pub const OFFSET_SCALE_MAX: u8 = 9;
/// The scale every producer in this tree writes (§1.1): `U = 16`, a 64 GiB addressable interior.
/// Also a byte-determinism pin — two bakes of one input agree on this byte or they do not agree.
pub const OFFSET_SCALE_DEFAULT: u8 = 4;

/// One file's offset **unit**, carried as the base-2 logarithm the header stores (§1.1).
///
/// Recording it as a logarithm is what makes "a power of two" a property of the encoding rather
/// than a rule someone has to check: no value of this type names a unit that is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OffsetScale {
    log2: u8,
}

impl OffsetScale {
    /// The default every producer writes ([`OFFSET_SCALE_DEFAULT`]).
    pub const DEFAULT: OffsetScale = OffsetScale { log2: OFFSET_SCALE_DEFAULT };

    /// Accept a header's scale byte. `0..=9` only (§1.1); anything else is
    /// [`DecodeError::Layout`] — deliberately **not** [`DecodeError::Version`], because a scale a
    /// reader cannot resolve is an unreadable file, not an old one, and telling a rider the map is
    /// from a future firmware when the byte is simply corrupt is the wrong answer.
    #[inline]
    pub const fn new(log2: u8) -> Result<OffsetScale, DecodeError> {
        if log2 > OFFSET_SCALE_MAX {
            return Err(DecodeError::Layout);
        }
        Ok(OffsetScale { log2 })
    }

    /// The stored logarithm — the byte at [`HEADER_OFFSET_SCALE_OFF`].
    #[inline]
    pub const fn log2(self) -> u8 {
        self.log2
    }

    /// `U`, the unit in bytes. `1..=512`, so it always divides the 512-byte §7/§8 chunk stride.
    #[inline]
    pub const fn unit(self) -> u64 {
        1u64 << self.log2
    }

    /// Round `bytes` up to the next unit boundary — the `align_up(x, U)` of §3, §7.1 and §8.1.
    /// `None` on `u64` overflow, which no real layout reaches and a corrupt directory can.
    #[inline]
    pub const fn align_up(self, bytes: u64) -> Option<u64> {
        let unit = self.unit();
        match bytes.checked_add(unit - 1) {
            Some(sum) => Some(sum & !(unit - 1)),
            None => None,
        }
    }

    /// Interpret a stored `uint32` field as an offset in this file's units.
    #[inline]
    pub const fn offset(self, units: u32) -> ScaledOffset {
        ScaledOffset { units, scale: self }
    }

    /// The offset naming byte `bytes`, or `None` when `bytes` is not on a unit boundary or does not
    /// fit `uint32` units. A **writer**'s check: a scaled offset cannot name a byte that is not a
    /// multiple of `U`, so a layout that tries is a bug in the layout, not a rounding request.
    #[inline]
    pub const fn scaled(self, bytes: u64) -> Option<ScaledOffset> {
        if bytes & (self.unit() - 1) != 0 {
            return None;
        }
        let units = bytes >> self.log2;
        if units > u32::MAX as u64 {
            return None;
        }
        Some(ScaledOffset { units: units as u32, scale: self })
    }

    /// Whether this scale covers a file of `total` bytes — the one rule §1.1 binds a producer to:
    /// `2^32 × U` must be **at least** the file's total length, the largest legal file being
    /// exactly that many bytes.
    #[inline]
    pub const fn covers(self, total: u64) -> bool {
        total <= (1u64 << 32) * self.unit()
    }
}

/// A `uint32` offset field **together with the file it came from's unit** (§1.1).
///
/// The scale travels inside the value on purpose. An assembler holds many cell files and one
/// output open at once, and an offset read out of one of them resolved against another's `U` lands
/// *inside* the wrong file rather than outside it — the read succeeds and returns the wrong
/// section. Pairing the two at the point of decode makes that combination unspellable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaledOffset {
    units: u32,
    scale: OffsetScale,
}

impl ScaledOffset {
    /// The stored `uint32`, for a writer emitting the field back out.
    #[inline]
    pub const fn units(self) -> u32 {
        self.units
    }

    /// The unit this offset counts.
    #[inline]
    pub const fn scale(self) -> OffsetScale {
        self.scale
    }

    /// The byte offset, **always widened before the multiply** (§1.1). `u32(field) * U` is the one
    /// way to get this wrong and it is wrong silently — the product wraps and lands inside the file
    /// — so this returns `u64` and there is no narrower spelling of it in the tree.
    #[inline]
    pub const fn bytes(self) -> u64 {
        self.units as u64 * self.scale.unit()
    }

    /// `Terrain Offset == 0` is the one offset that legitimately means absence (§1.3).
    #[inline]
    pub const fn is_zero(self) -> bool {
        self.units == 0
    }
}

// --- §1.2 unit-boundary writing -----------------------------------------------

/// One run of [`FILLER`], long enough for any single §1.2 gap a producer emits.
///
/// The longest is §8.1's alignment run, which lands the fixed 512-byte nav chunks on a sector; a
/// section boundary's own gap is at most `U − 1 ≤ 511`. So one 512-byte run covers both, and no
/// producer ever allocates a gap.
pub const FILLER_RUN: [u8; 512] = [FILLER; 512];

/// A file position that only moves by writing, and that knows where the next §1.2 unit boundary is.
///
/// Every structure a header or directory offset reaches begins on one (§1.2), which used to be a
/// discipline: each boundary was an `align_up` of a hand-carried cursor, a `resize`/`write` of that
/// many [`FILLER`] bytes, and a `scaled()` of the result — three steps that had to agree, at every
/// boundary, in every writer. [`begin_section`](Self::begin_section) is those three steps as one
/// call, and the cursor it advances is the same one the bytes went through, so the position a field
/// names and the position the bytes landed at cannot drift apart.
///
/// **The sink is a closure, so a writer that discards its bytes still moves the cursor.** That is
/// how a producer *projects* a layout with the code that *emits* it, rather than with a second copy
/// of the arithmetic: hand [`new`](Self::new) a sink that ignores what it is given and read
/// [`at`](Self::at) at the end. A projection that disagrees with the write is then not a bug that
/// tests catch — it is a program that does not exist.
///
/// The `scale` travels with the cursor for the reason it travels inside a [`ScaledOffset`]: an
/// assembler holds several files' offsets at once, and a boundary in one file's unit is not a
/// boundary in another's.
pub struct UnitWriter<'a, E> {
    at: u64,
    scale: OffsetScale,
    sink: &'a mut dyn FnMut(&[u8]) -> Result<(), E>,
}

impl<'a, E> UnitWriter<'a, E> {
    /// A cursor at byte `at` of the file `sink` receives, counting `scale`'s units.
    ///
    /// `at` need not be a boundary: a section writer starts its cursor at the absolute byte its
    /// caller placed it, and §1.2's whole point is that the boundary is found rather than assumed.
    #[inline]
    pub fn new(scale: OffsetScale, at: u64, sink: &'a mut dyn FnMut(&[u8]) -> Result<(), E>) -> UnitWriter<'a, E> {
        UnitWriter { at, scale, sink }
    }

    /// The byte the next write lands on.
    #[inline]
    pub const fn at(&self) -> u64 {
        self.at
    }

    /// The unit every offset this writer's file stores counts (§1.1).
    #[inline]
    pub const fn scale(&self) -> OffsetScale {
        self.scale
    }

    /// Append `bytes`, advancing the cursor by their length.
    #[inline]
    pub fn put(&mut self, bytes: &[u8]) -> Result<(), E> {
        (self.sink)(bytes)?;
        self.at += bytes.len() as u64;
        Ok(())
    }

    /// Append `len` bytes of §1.2 [`FILLER`]. Used where a gap's length is computed rather than
    /// found — §8.1's alignment run, whose size answers to a sector as well as to a unit.
    pub fn pad(&mut self, len: u64) -> Result<(), E> {
        let mut left = len;
        while left > 0 {
            let run = if left > FILLER_RUN.len() as u64 { FILLER_RUN.len() } else { left as usize };
            self.put(&FILLER_RUN[..run])?;
            left -= run as u64;
        }
        Ok(())
    }

    /// Move the cursor past `len` bytes **without writing them**.
    ///
    /// **This is a projection's tool and a writer must never reach for it.** A layout walked over a
    /// discarding sink still has to account for bodies it is not holding — a nav section's chunk run
    /// is gigabytes that live on a scratch seam, and padding past them one [`FILLER_RUN`] at a time
    /// to learn where the next boundary falls would be millions of no-op calls to answer a question
    /// about arithmetic. This is that arithmetic.
    ///
    /// The hazard is exactly what it looks like — a writer that skips instead of putting emits a
    /// file with a hole where the cursor says there are bytes — so a producer using this type is
    /// expected to check the bytes its sink actually received against the position it ended at. In
    /// this tree the map writer does, and it is a refusal rather than an assertion.
    #[inline]
    pub fn advance(&mut self, len: u64) {
        self.at += len;
    }

    /// Start a structure a scaled offset names: pad to the next unit boundary with [`FILLER`] and
    /// return that boundary's **byte** offset.
    ///
    /// The return value is a byte offset rather than a [`ScaledOffset`] because a producer past the
    /// interior its scale addresses (§1.1) is a refusal each writer words for its own reader — the
    /// packer panics on a layout bug, the browser engine returns a capacity error. Pairing this with
    /// that crate's own `scaled()` keeps the policy where it belongs and still makes the boundary
    /// impossible to skip: the value handed to it is the aligned cursor, never the raw one.
    pub fn begin_section(&mut self) -> Result<u64, E> {
        let boundary = self.scale.align_up(self.at).expect("a layout cursor never approaches u64::MAX");
        self.pad(boundary - self.at)?;
        Ok(boundary)
    }
}

// --- §8.4 edge addressing -----------------------------------------------------

/// Bits of an `Edge Id` naming the record's ordinal within its chunk (§8.4). The remaining 27 name
/// the 512-byte chunk, giving the pool a `2^27 × 512 = 2^36` byte reach — exactly the interior a
/// scale-4 file addresses.
pub const NAV_EDGE_ORDINAL_BITS: u32 = 5;
/// Mask of the ordinal half of an `Edge Id`.
pub const NAV_EDGE_ORDINAL_MASK: u32 = (1 << NAV_EDGE_ORDINAL_BITS) - 1;
/// Largest `Edge Chunk Count` a directory may declare (§8.1/§8.4): past it, no `Edge Id` could name
/// the chunks, so the tail would be bytes the directory claims and no id reaches.
pub const NAV_EDGE_MAX_CHUNKS: u64 = 1 << (32 - NAV_EDGE_ORDINAL_BITS);
/// **A chunk holds at most 31 records** (§8.4) — a producer MUST NOT write a 32nd, so `ordinal` is
/// never more than `30`. 31 and not 32 is what makes [`NAV_EDGE_ID_NONE`] impossible
/// *unconditionally*: `0xFFFFFFFF` is ordinal `31` of chunk `2^27 − 1`, and a 32-record cap would
/// permit that the moment a record shrank to 16 bytes.
pub const NAV_EDGE_MAX_RECORDS_PER_CHUNK: usize = 31;
/// The §8.7 snap sentinel, and an impossible `Edge Id` under the cap above.
pub const NAV_EDGE_ID_NONE: u32 = 0xFFFF_FFFF;
/// `Pt Count == 0xFFFF` is the §8.4 end-of-chunk sentinel: `Pt Count` is at least `2` in every real
/// record, and a `0xFF`-filled gap already spells it at `p + 4` for free.
pub const NAV_EDGE_PT_COUNT_SENTINEL: u16 = 0xFFFF;
/// The smallest record this format can express: `15 + 4 × (2 − 1)`. It is what makes the 26-record
/// chunk — and therefore the 5-bit ordinal — a property of the format rather than an observation
/// about real maps.
pub const NAV_EDGE_MIN_LEN: usize = NAV_EDGE_FIXED_LEN + 4;

/// Pack a `(chunk_index, ordinal)` pair into the §8.4 wire `Edge Id`. `None` when either half is
/// out of its field — the producer-side twin of [`nav_edge_id_chunk`]/[`nav_edge_id_ordinal`].
#[inline]
pub const fn nav_edge_id(chunk_index: u32, ordinal: u32) -> Option<u32> {
    if chunk_index >= (NAV_EDGE_MAX_CHUNKS as u32) || ordinal > NAV_EDGE_ORDINAL_MASK {
        return None;
    }
    Some((chunk_index << NAV_EDGE_ORDINAL_BITS) | ordinal)
}

/// The 27-bit chunk half of an `Edge Id` (§8.4).
#[inline]
pub const fn nav_edge_id_chunk(id: u32) -> u32 {
    id >> NAV_EDGE_ORDINAL_BITS
}

/// The 5-bit ordinal half of an `Edge Id` (§8.4) — the record's **position** within its chunk,
/// counting from `0`, not a byte offset into it.
#[inline]
pub const fn nav_edge_id_ordinal(id: u32) -> u32 {
    id & NAV_EDGE_ORDINAL_MASK
}

/// One step of the §8.4 resolve walk: the length of the record at byte position `p` of a 512-byte
/// edge chunk, or `None` to **refuse**.
///
/// Transcribed from the spec block verbatim, and two of its lines are load-bearing:
///
/// - **`NAV_CHUNK_SIZE - p` appears nowhere**, in any width. Written `512 - p < 19` the first guard
///   is a bug in every unsigned language: once `p` passes `512` — which a corrupt `Pt Count` does in
///   a single step — the subtraction wraps to a huge value, the guard passes, and the read at
///   `p + 4` lands outside the chunk. Every bound below is written **additively** on `p`.
/// - **`len` is evaluated in 32 bits**, not in `Pt Count`'s own `u16`. The largest `n` the guards
///   let through is `0xFFFE`, giving `15 + 4 × 65 533 = 262 147`, which a transcriber computing in
///   the operand's width wraps to `3`; then `p + 3 > 512` is false, the last check passes, and the
///   walk advances three bytes into the middle of a record instead of refusing.
#[inline]
pub fn nav_edge_step(chunk: &[u8], p: usize) -> Option<usize> {
    if chunk.len() < NAV_CHUNK_SIZE {
        return None;
    }
    if p.checked_add(NAV_EDGE_MIN_LEN)? > NAV_CHUNK_SIZE {
        return None;
    }
    let n = rd_u16(chunk, p + 4);
    if n == NAV_EDGE_PT_COUNT_SENTINEL {
        return None;
    }
    if n < 2 {
        return None;
    }
    // 32-bit evaluation, per the note above; `n - 1` cannot underflow behind the `n < 2` refusal.
    let len = NAV_EDGE_FIXED_LEN as u32 + 4 * (n as u32 - 1);
    let len = len as usize;
    if p.checked_add(len)? > NAV_CHUNK_SIZE {
        return None;
    }
    Some(len)
}

/// Resolve `ordinal` to the record's byte range within its 512-byte chunk (§8.4), walking from the
/// chunk's first byte and taking each record's length from its own `Pt Count`.
///
/// Every record the walk touches — the intermediate ones and the target alike — gets the same four
/// checks, so [`nav_edge_step`] is written once and applied `ordinal + 1` times. A refused id is a
/// malformed map, not an absent edge.
#[inline]
pub fn nav_edge_record_range(chunk: &[u8], ordinal: u32) -> Option<(usize, usize)> {
    let mut p = 0usize;
    for _ in 0..ordinal {
        p += nav_edge_step(chunk, p)?;
    }
    let len = nav_edge_step(chunk, p)?;
    Some((p, p + len))
}

pub const POI_CATEGORY_COUNT: u8 = 6;
pub const POI_RECORD_LEN: usize = 36;
pub const POI_NAME_LEN: usize = 24;
pub const POI_HOURS_REF_NONE: u16 = 0xFFFF;
pub const POI_HOURS_BLOB_LEN: usize = 29;
pub const POI_HOURS_DAYS: usize = 7;
pub const POI_HOURS_SLOTS_PER_DAY: usize = 2;
pub const POI_HOURS_FLAG_SEASONAL: u8 = 0x01;
pub const POI_HOURS_FLAG_TRUNCATED: u8 = 0x02;
pub const POI_CHUNK_SIZE: usize = 512;
pub const POI_CAT_ENTRY_LEN: usize = 13;
pub const POI_DIR_POOL_FIELDS_LEN: usize = 6;

/// Width of the v13 §8.1 navigation directory. The v12 28-byte graph/profile prefix is followed by
/// the sparse exact-edge snap index's `(index offset, node count, chunk count)` triple.
pub const NAV_DIR_LEN: usize = 40;
pub const NAV_CHUNK_SIZE: usize = 512;
pub const NAV_NODE_FIXED_LEN: usize = 13;
/// Width of one §8.3 adjacency entry. **17 in v12** (#1073): `id u32, dlat i16, dlon i16,
/// edge_id u32, cost_m u16, way_kind u8, ascent_m u16`.
pub const NAV_NEIGHBOR_LEN: usize = 17;
/// Offset of the v12 `Ascent M` field inside a §8.3 neighbor entry — the integrated climb of
/// riding the edge **from this record's node toward the neighbor**, so the two sides of an edge
/// carry different values (§8.3's one exception to "both sides agree").
pub const NAV_NEIGHBOR_ASCENT_OFF: usize = 15;
pub const NAV_EDGE_FIXED_LEN: usize = 15;
/// Width of one §8.6 profile record. **56 in v12** (#1073): the 52-byte v9 record plus
/// [`NAV_PROFILE_CLIMB_WEIGHT_OFF`] and three reserved bytes written `0`.
pub const NAV_PROFILE_LEN: usize = 56;
pub const NAV_PROFILE_NAME_LEN: usize = 12;
/// Offset of the v12 `Climb Weight` byte inside a §8.6 profile record: flat-metres-equivalent per
/// metre of ascent, `0` = climb-blind.
pub const NAV_PROFILE_CLIMB_WEIGHT_OFF: usize = 52;
/// Length of the reserved tail after `Climb Weight`; written `0`, ignored on read.
pub const NAV_PROFILE_RESERVED_LEN: usize = 3;
pub const NAV_MAX_PROFILES: usize = 8;
pub const NAV_MAX_DEGREE: usize = 24;
/// One §8.7 snap-anchor record: absolute `lat i32`, absolute `lon i32`, and §8.4 `edge_id u32`.
pub const NAV_SNAP_RECORD_LEN: usize = 12;
/// Edges no longer than this need no interior anchor: every on-edge position is already within
/// half this distance of a graph endpoint. Together with [`NAV_SNAP_ANCHOR_GAP_M`], this gives the
/// 250 m node-or-anchor lookup a 100 m road-proximity guarantee.
pub const NAV_SNAP_EDGE_MIN_M: u32 = 300;
/// Maximum along-polyline gap between graph endpoints / interior snap anchors.
pub const NAV_SNAP_ANCHOR_GAP_M: u32 = 300;

/// Padding inserted immediately before a populated §8.2 node (or §8.7 snap) index so that the
/// fixed 512-byte chunks following it begin on a physical sector boundary **and** the index itself
/// starts on a unit boundary — the two alignments §8.1 says do not fight. `unpadded` is the file
/// offset the index would take with no padding at all; the return is always `0..512`.
///
/// The reader computes the first chunk as `align_up(index_offset × U + node_count × 4, U)`, so the
/// index has to end anywhere in the `U` bytes *below* the sector target rather than exactly on it.
/// That slack is what makes both alignments satisfiable for every node count: reserve
/// `align_up(index_len, U)` bytes, round the target up to the sector, and hand the index the
/// difference. 512 is a multiple of `U` at every legal scale (§1.1 caps it at 512), so a start
/// derived this way is a multiple of `U` by construction.
///
/// Keeping the arithmetic here makes the standalone packer and the streaming assembler agree
/// exactly, which is what their byte-determinism pins compare.
#[inline]
pub const fn nav_index_padding(scale: OffsetScale, unpadded: u64, index_len: u64) -> Option<usize> {
    let sector = NAV_CHUNK_SIZE as u64;
    let need = match scale.align_up(index_len) {
        Some(need) => need,
        None => return None,
    };
    let sum = match unpadded.checked_add(need) {
        Some(sum) => sum,
        None => return None,
    };
    let target = match sum.checked_add(sector - 1) {
        Some(raised) => raised & !(sector - 1),
        None => return None,
    };
    Some((target - need - unpadded) as usize)
}

/// The browsable POI categories from OBCM spec §7.4. Discriminants are stable wire ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PoiCategory {
    Water = 1,
    Campsite = 2,
    Accommodation = 3,
    Resupply = 4,
    Pharmacy = 5,
    BikeShop = 6,
}

impl PoiCategory {
    pub const ALL: [PoiCategory; POI_CATEGORY_COUNT as usize] = [
        PoiCategory::Water,
        PoiCategory::Campsite,
        PoiCategory::Accommodation,
        PoiCategory::Resupply,
        PoiCategory::Pharmacy,
        PoiCategory::BikeShop,
    ];

    #[inline]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[inline]
    pub const fn from_id(id: u8) -> Option<PoiCategory> {
        Some(match id {
            1 => PoiCategory::Water,
            2 => PoiCategory::Campsite,
            3 => PoiCategory::Accommodation,
            4 => PoiCategory::Resupply,
            5 => PoiCategory::Pharmacy,
            6 => PoiCategory::BikeShop,
            _ => return None,
        })
    }

    /// Stable device-facing category label. Distinct from a subtype fallback label.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            PoiCategory::Water => "Water",
            PoiCategory::Campsite => "Campsite",
            PoiCategory::Accommodation => "Lodging",
            PoiCategory::Resupply => "Resupply",
            PoiCategory::Pharmacy => "Pharmacy",
            PoiCategory::BikeShop => "Bike shop",
        }
    }
}

/// One append-only OBCM spec §7.4 subtype row.
#[derive(Debug, Clone, Copy)]
pub struct PoiSubtype {
    pub category: PoiCategory,
    pub label: &'static str,
}

const fn subtype(category: PoiCategory, label: &'static str) -> PoiSubtype {
    PoiSubtype { category, label }
}

/// Canonical subtype table, indexed by `subtype_id - 1`.
pub const POI_SUBTYPES: [PoiSubtype; 18] = [
    subtype(PoiCategory::Water, "Drinking water"),
    subtype(PoiCategory::Water, "Spring"),
    subtype(PoiCategory::Water, "Water tap"),
    subtype(PoiCategory::Water, "Water point"),
    subtype(PoiCategory::Campsite, "Campsite"),
    subtype(PoiCategory::Campsite, "Caravan site"),
    subtype(PoiCategory::Accommodation, "Hotel"),
    subtype(PoiCategory::Accommodation, "Hostel"),
    subtype(PoiCategory::Accommodation, "Guest house"),
    subtype(PoiCategory::Accommodation, "Motel"),
    subtype(PoiCategory::Accommodation, "Wilderness hut"),
    subtype(PoiCategory::Accommodation, "Alpine hut"),
    subtype(PoiCategory::Resupply, "Supermarket"),
    subtype(PoiCategory::Resupply, "Convenience"),
    subtype(PoiCategory::Resupply, "Bakery"),
    subtype(PoiCategory::Resupply, "Marketplace"),
    subtype(PoiCategory::Pharmacy, "Pharmacy"),
    subtype(PoiCategory::BikeShop, "Bike shop"),
];

#[inline]
pub fn poi_subtype_row(subtype_id: u8) -> Option<&'static PoiSubtype> {
    if subtype_id == 0 {
        return None;
    }
    POI_SUBTYPES.get(subtype_id as usize - 1)
}

#[inline]
pub fn poi_category_of(subtype_id: u8) -> Option<PoiCategory> {
    poi_subtype_row(subtype_id).map(|row| row.category)
}

#[inline]
pub fn poi_label_of(subtype_id: u8) -> Option<&'static str> {
    poi_subtype_row(subtype_id).map(|row| row.label)
}

pub fn validate_header_prefix(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_prefix(bytes, &MAGIC, VERSION, VERSION).map(|_| ())
}

const _: () = assert!(EMPTY_LEAF == !BRANCH_BIT);
const _: () = assert!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN <= NAV_CHUNK_SIZE);
const _: () = assert!(POI_HOURS_BLOB_LEN == 1 + POI_HOURS_DAYS * POI_HOURS_SLOTS_PER_DAY * 2);
// §1.1: `9` is the largest scale at which `U` still divides the fixed 512-byte chunk stride, which
// is what keeps §7's and §8's chunk runs free of internal filler.
const _: () = assert!(NAV_CHUNK_SIZE.is_multiple_of(1usize << OFFSET_SCALE_MAX));
const _: () = assert!(POI_CHUNK_SIZE.is_multiple_of(1usize << OFFSET_SCALE_MAX));
// §8.4: the real per-chunk maximum sits below the 31-record cap, so the cap gives up nothing today
// and keeps the encoding sound if a future record ever shrinks.
const _: () = assert!(NAV_CHUNK_SIZE / NAV_EDGE_MIN_LEN <= NAV_EDGE_MAX_RECORDS_PER_CHUNK);
// …and the cap is what makes `0xFFFFFFFF` an impossible id whatever the chunk index.
const _: () = assert!((NAV_EDGE_MAX_RECORDS_PER_CHUNK as u32 - 1) < NAV_EDGE_ORDINAL_MASK);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_prefix_uses_the_authoritative_version_and_length() {
        let mut fixture = [0u8; HEADER_LEN];
        fixture[..4].copy_from_slice(&MAGIC);
        fixture[4] = VERSION;
        // v14 §1.2: the header is 49 bytes, so the style table begins at the first unit boundary
        // at or after it — `64` at the default `U = 16`, giving `Style Offset = 4`.
        let style = OffsetScale::DEFAULT.scaled(OffsetScale::DEFAULT.align_up(HEADER_LEN as u64).unwrap()).unwrap();
        fixture[21..25].copy_from_slice(&style.units().to_le_bytes());
        fixture[HEADER_OFFSET_SCALE_OFF] = OFFSET_SCALE_DEFAULT;
        validate_header_prefix(&fixture).unwrap();
        assert_eq!(fixture[4], 0x0E, "the version byte is the hard cut, and it cuts in both directions");
        assert_eq!(style.units(), 4);
        assert_eq!(style.bytes(), 64);
    }

    #[test]
    fn the_version_byte_refuses_older_and_newer_alike() {
        let mut fixture = [0u8; HEADER_LEN];
        fixture[..4].copy_from_slice(&MAGIC);
        for version in [VERSION - 1, VERSION + 1] {
            fixture[4] = version;
            assert_eq!(validate_header_prefix(&fixture), Err(DecodeError::Version));
        }
    }

    #[test]
    fn a_scaled_offset_resolves_only_against_its_own_files_unit() {
        let coarse = OffsetScale::new(9).unwrap();
        let fine = OffsetScale::new(0).unwrap();
        assert_eq!(coarse.unit(), 512);
        assert_eq!(fine.unit(), 1);
        // The same stored `uint32` names two different bytes in two files, which is the whole
        // reason the scale rides inside the value.
        assert_eq!(coarse.offset(3).bytes(), 1536);
        assert_eq!(fine.offset(3).bytes(), 3);
        assert_eq!(OffsetScale::DEFAULT.offset(3).bytes(), 48);
        // At scale 0 the arithmetic is v13's exactly — the point of writing it this way.
        assert_eq!(fine.offset(u32::MAX).bytes(), u32::MAX as u64);
        // …and the widening is real: `u32(field) * U` would wrap to 0xFFFFFFF0 here.
        assert_eq!(OffsetScale::DEFAULT.offset(u32::MAX).bytes(), 0xF_FFFF_FFF0);
    }

    #[test]
    fn offset_scale_admits_zero_through_nine_and_nothing_else() {
        for log2 in 0..=OFFSET_SCALE_MAX {
            assert_eq!(OffsetScale::new(log2).unwrap().log2(), log2);
        }
        for log2 in [OFFSET_SCALE_MAX + 1, 16, 255] {
            // Layout, not Version: an unresolvable scale is an unreadable file, not an old one.
            assert_eq!(OffsetScale::new(log2), Err(DecodeError::Layout));
        }
        // The producer rule: `2^32 × U` must be at least the file's total length — "at least",
        // so the largest legal file is exactly that many bytes.
        assert!(OffsetScale::DEFAULT.covers(1 << 36));
        assert!(!OffsetScale::DEFAULT.covers((1 << 36) + 1));
    }

    #[test]
    fn align_up_and_scaled_are_the_two_halves_of_one_boundary_rule() {
        let scale = OffsetScale::DEFAULT;
        assert_eq!(scale.align_up(0), Some(0));
        assert_eq!(scale.align_up(49), Some(64));
        assert_eq!(scale.align_up(64), Some(64));
        assert_eq!(scale.align_up(65), Some(80));
        // A writer may not name a byte that is not a multiple of `U`.
        assert!(scale.scaled(49).is_none());
        assert_eq!(scale.scaled(64).map(ScaledOffset::units), Some(4));
        // …nor one past the `uint32` field's reach.
        assert!(scale.scaled(1 << 36).is_none());
        assert_eq!(scale.scaled((1u64 << 36) - 16).map(ScaledOffset::units), Some(u32::MAX));
    }

    /// The three steps a §1.2 boundary used to be — round the cursor up, write that many `0xFF`, and
    /// scale the *rounded* value — are one call, and this is the proof that it is all three.
    ///
    /// The pin that matters is the last of the three: the offset a field stores names the byte the
    /// bytes actually landed on, because there is no longer a spelling in which it could name the
    /// unrounded one.
    #[test]
    fn begin_section_pads_the_gap_and_names_the_boundary_it_reached() {
        let mut out: std::vec::Vec<u8> = std::vec::Vec::new();
        let boundary = {
            let mut sink = |bytes: &[u8]| -> Result<(), core::convert::Infallible> {
                out.extend_from_slice(bytes);
                Ok(())
            };
            let mut w = UnitWriter::new(OffsetScale::DEFAULT, 0, &mut sink);
            w.put(&[0u8; HEADER_LEN]).unwrap();
            assert_eq!(w.at(), 49);
            let boundary = w.begin_section().unwrap();
            assert_eq!(w.at(), boundary, "the cursor is the boundary it just reached");
            w.put(b"style").unwrap();
            // …and a cursor already on a boundary writes no filler at all.
            assert_eq!(w.at(), 69);
            let next = w.begin_section().unwrap();
            assert_eq!(next, 80);
            assert_eq!(w.begin_section().unwrap(), 80, "a second call at a boundary is a no-op");
            boundary
        };
        assert_eq!(boundary, 64, "§1.2: the style table is the first unit boundary past the 49-byte header");
        assert_eq!(OffsetScale::DEFAULT.scaled(boundary).map(ScaledOffset::units), Some(4), "…which is `4` in units");
        assert_eq!(&out[49..64], &[FILLER; 15], "the gap is the format's one fill byte");
        assert_eq!(&out[69..80], &[FILLER; 11]);
        assert_eq!(out.len(), 80);
    }

    /// A writer over a sink that keeps nothing still moves its cursor, which is how a producer
    /// *projects* a layout with the code that *emits* it.
    #[test]
    fn a_discarding_sink_still_advances_the_cursor() {
        let mut discard = |_: &[u8]| -> Result<(), core::convert::Infallible> { Ok(()) };
        let mut w = UnitWriter::new(OffsetScale::DEFAULT, 0, &mut discard);
        w.put(&[0u8; HEADER_LEN]).unwrap();
        w.begin_section().unwrap();
        w.pad(3).unwrap();
        assert_eq!(w.at(), 67, "49 rounded to 64, then three bytes of a computed run");
        // §8.1's alignment run is a whole sector, which one `FILLER_RUN` covers without allocating.
        w.pad(512).unwrap();
        assert_eq!(w.at(), 579);
    }

    /// [`UnitWriter::pad`] must be able to exceed one [`FILLER_RUN`]. No §1.2 gap does today — the
    /// longest is a sector — so the loop that makes it possible is otherwise unreached, and an
    /// off-by-one in it would sit unseen until the day a caller needed a longer run.
    #[test]
    fn pad_spans_more_than_one_filler_run() {
        let mut out: std::vec::Vec<u8> = std::vec::Vec::new();
        {
            let mut sink = |bytes: &[u8]| -> Result<(), core::convert::Infallible> {
                out.extend_from_slice(bytes);
                Ok(())
            };
            let mut w = UnitWriter::new(OffsetScale::DEFAULT, 0, &mut sink);
            w.pad(1_500).unwrap();
            assert_eq!(w.at(), 1_500);
            w.pad(0).unwrap();
            assert_eq!(w.at(), 1_500, "an empty run writes nothing and moves nothing");
        }
        assert_eq!(out.len(), 1_500, "1500 = 512 + 512 + 476, and the last run is the remainder");
        assert!(out.iter().all(|&b| b == FILLER));
    }

    /// [`UnitWriter::advance`] is the one method that moves the cursor **without** writing, and the
    /// one a writer must never reach for. Two properties, both stated here because the type cannot
    /// enforce either: it moves exactly `n` and hands the sink nothing, and a walk that advances
    /// past its bodies lands on the same byte as the walk that writes them — which is the whole
    /// reason a projection may use it.
    #[test]
    fn advance_moves_the_cursor_by_exactly_its_length_and_writes_nothing() {
        // One script, run twice: once putting real bodies, once advancing past them. The boundaries
        // between them are found the same way both times.
        let bodies: [&[u8]; 3] = [&[1u8; 49], &[2u8; 7], &[3u8; 600]];
        let walk = |w: &mut UnitWriter<'_, core::convert::Infallible>, write: bool| {
            for body in bodies {
                if write {
                    w.put(body).unwrap();
                } else {
                    w.advance(body.len() as u64);
                }
                w.begin_section().unwrap();
            }
            w.at()
        };

        let mut written: std::vec::Vec<u8> = std::vec::Vec::new();
        let emitted_at = {
            let mut sink = |bytes: &[u8]| -> Result<(), core::convert::Infallible> {
                written.extend_from_slice(bytes);
                Ok(())
            };
            walk(&mut UnitWriter::new(OffsetScale::DEFAULT, 96, &mut sink), true)
        };

        let mut skipped: std::vec::Vec<u8> = std::vec::Vec::new();
        let projected_at = {
            let mut sink = |bytes: &[u8]| -> Result<(), core::convert::Infallible> {
                skipped.extend_from_slice(bytes);
                Ok(())
            };
            walk(&mut UnitWriter::new(OffsetScale::DEFAULT, 96, &mut sink), false)
        };

        assert_eq!(emitted_at, projected_at, "the projection lands on the byte the write ends at");
        assert_eq!(written.len(), (emitted_at - 96) as usize, "the write delivered every byte it moved over");
        // …and the projection delivered only the filler, which is the hole a writer would leave.
        assert_eq!(skipped.len(), written.len() - bodies.iter().map(|b| b.len()).sum::<usize>());
        assert!(skipped.iter().all(|&b| b == FILLER));

        // The bare property, without a script around it.
        let mut discard = |_: &[u8]| -> Result<(), core::convert::Infallible> { panic!("advance writes nothing") };
        let mut w = UnitWriter::new(OffsetScale::DEFAULT, 7, &mut discard);
        w.advance(0);
        assert_eq!(w.at(), 7);
        w.advance(3_000_000_000);
        assert_eq!(w.at(), 3_000_000_007, "and it is `u64` arithmetic, not a loop over a buffer");
    }

    /// The filler run must cover the longest single gap the format can ask for: `U − 1` at the
    /// largest legal scale, which is also §8.1's 512-byte alignment run.
    #[test]
    fn one_filler_run_covers_every_gap_the_format_can_ask_for() {
        const _: () = assert!(FILLER_RUN.len() as u64 == 1 << OFFSET_SCALE_MAX);
        const _: () = assert!(FILLER_RUN.len() == NAV_CHUNK_SIZE);
        assert!(FILLER_RUN.iter().all(|&b| b == FILLER));
    }

    #[test]
    fn nav_index_padding_lands_the_chunks_on_a_sector_and_the_index_on_a_unit() {
        let scale = OffsetScale::DEFAULT;
        // §8.5's worked example: a one-node index whose chunk must begin at S+512.
        let pad = nav_index_padding(scale, 104, 4).unwrap();
        assert_eq!(104 + pad, 496, "the index takes S+496, twelve bytes of filler carry it to S+512");
        // The two properties, checked for every index length rather than at the example.
        for unpadded in [0u64, 40, 104, 500, 1021] {
            for index_len in [4u64, 16, 20, 508, 512, 4096] {
                let pad = nav_index_padding(scale, unpadded, index_len).unwrap();
                assert!(pad < NAV_CHUNK_SIZE, "the run is one sector at most");
                let start = unpadded + pad as u64;
                assert_eq!(start % scale.unit(), 0, "the index must start on a unit boundary");
                let chunks = scale.align_up(start + index_len).unwrap();
                assert_eq!(chunks % NAV_CHUNK_SIZE as u64, 0, "the first chunk must start on a sector");
            }
        }
        // At `U = 1` this degrades to v13's arithmetic: no rounding slack, the index abuts the
        // sector exactly.
        let fine = OffsetScale::new(0).unwrap();
        assert_eq!(nav_index_padding(fine, 104, 4), Some(404));
    }

    #[test]
    fn an_edge_id_is_a_chunk_and_an_ordinal() {
        assert_eq!(nav_edge_id(0, 0), Some(0), "the first record of the first chunk is still id 0");
        assert_eq!(nav_edge_id(1, 0), Some(32));
        assert_eq!(nav_edge_id(0, 1), Some(1));
        for id in [0u32, 1, 32, 0x1234_5678] {
            assert_eq!(nav_edge_id(nav_edge_id_chunk(id), nav_edge_id_ordinal(id)), Some(id));
        }
        // Both halves have a reach, and the sentinel sits outside what a producer may write.
        assert_eq!(nav_edge_id(NAV_EDGE_MAX_CHUNKS as u32, 0), None);
        assert_eq!(nav_edge_id(0, NAV_EDGE_ORDINAL_MASK + 1), None);
        assert_eq!(nav_edge_id_ordinal(NAV_EDGE_ID_NONE), NAV_EDGE_ORDINAL_MASK);
        assert_eq!(NAV_EDGE_MAX_CHUNKS * NAV_CHUNK_SIZE as u64, 1 << 36, "the pool reaches the interior");
    }

    /// The §8.4 walk, including the two transcription traps the spec spells out because this block
    /// is the one a reader copies verbatim.
    #[test]
    fn the_edge_resolve_walk_applies_its_four_refusals_every_step() {
        /// A chunk holding `counts.len()` records of the given point counts, `0xFF`-filled after.
        fn chunk_of(counts: &[u16]) -> std::vec::Vec<u8> {
            let mut out = std::vec![FILLER; NAV_CHUNK_SIZE];
            let mut p = 0usize;
            for &n in counts {
                let len = NAV_EDGE_FIXED_LEN + 4 * (n as usize - 1);
                out[p..p + len].fill(0);
                out[p + 4..p + 6].copy_from_slice(&n.to_le_bytes());
                p += len;
            }
            out
        }

        let chunk = chunk_of(&[2, 5, 2]);
        assert_eq!(nav_edge_record_range(&chunk, 0), Some((0, 19)));
        assert_eq!(nav_edge_record_range(&chunk, 1), Some((19, 50)));
        assert_eq!(nav_edge_record_range(&chunk, 2), Some((50, 69)));
        // The walk MUST NOT pass the chunk's last record: the filler behind it spells the sentinel.
        assert_eq!(nav_edge_record_range(&chunk, 3), None);
        assert_eq!(nav_edge_record_range(&chunk, NAV_EDGE_ORDINAL_MASK), None);

        // A record claiming bytes past its chunk is refused rather than truncated.
        let mut overrun = std::vec![FILLER; NAV_CHUNK_SIZE];
        overrun[..NAV_EDGE_MIN_LEN].fill(0);
        overrun[4..6].copy_from_slice(&200u16.to_le_bytes()); // 15 + 4*199 = 811 > 512
        assert_eq!(nav_edge_step(&overrun, 0), None);

        // `Pt Count` below 2 is impossible, and is also what stops `4 * (n - 1)` underflowing.
        for n in [0u16, 1] {
            let mut bad = std::vec![FILLER; NAV_CHUNK_SIZE];
            bad[..NAV_EDGE_MIN_LEN].fill(0);
            bad[4..6].copy_from_slice(&n.to_le_bytes());
            assert_eq!(nav_edge_step(&bad, 0), None);
        }

        // The `512 - p` trap: a position past the chunk must refuse, not wrap into a huge span.
        assert_eq!(nav_edge_step(&chunk, NAV_CHUNK_SIZE), None);
        assert_eq!(nav_edge_step(&chunk, NAV_CHUNK_SIZE - NAV_EDGE_MIN_LEN + 1), None);
        assert_eq!(nav_edge_step(&chunk, usize::MAX), None);
        // The last position a record can start at is admitted, so the guard is not off by one.
        let mut edge_case = std::vec![FILLER; NAV_CHUNK_SIZE];
        let last = NAV_CHUNK_SIZE - NAV_EDGE_MIN_LEN;
        edge_case[last..].fill(0);
        edge_case[last + 4..last + 6].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(nav_edge_step(&edge_case, last), Some(NAV_EDGE_MIN_LEN));

        // The 19-bit `len` trap: `0xFFFE` computed in `u16` wraps to 3 and the walk would advance
        // into the middle of a record. Evaluated in 32 bits it is 262 147 and refuses.
        let mut wide = std::vec![FILLER; NAV_CHUNK_SIZE];
        wide[..NAV_EDGE_MIN_LEN].fill(0);
        wide[4..6].copy_from_slice(&0xFFFEu16.to_le_bytes());
        assert_eq!(nav_edge_step(&wide, 0), None);
        assert_eq!(15u32 + 4 * (0xFFFEu32 - 1), 262_147);
        assert_eq!((15u16).wrapping_add(4u16.wrapping_mul(0xFFFEu16 - 1)), 3, "the wrapped value the trap yields");

        // A short buffer is not a chunk: the walk needs the full 512 bytes to bound itself.
        assert_eq!(nav_edge_step(&chunk[..NAV_CHUNK_SIZE - 1], 0), None);
    }

    /// The refusals above are all taken at **ordinal 0**, where the refused record *is* the target.
    /// That leaves the doc comment's actual claim — "applied `ordinal + 1` times", so an
    /// intermediate record is bounds-checked exactly as the target is — resting on nothing a test
    /// disagrees with.
    ///
    /// It is a live mutation, not a hypothetical: drop the `?` in
    /// [`nav_edge_record_range`]'s loop for `unwrap_or(NAV_EDGE_MIN_LEN)`, or hoist the walk to
    /// start at the target's own byte, and every assertion in the test above still passes. What
    /// fails is only this: a chunk whose **record 0 is malformed** but which has a perfectly valid
    /// record behind it must not resolve that later record. A walk that steps over a record it
    /// could not parse is guessing where the next one starts, and on a §8.4 chunk it is guessing
    /// inside attacker-shaped bytes.
    #[test]
    fn a_malformed_intermediate_record_refuses_the_ordinals_behind_it() {
        // Record 0 is `Pt Count = 0` — refused by the `n < 2` guard. Byte 19 then holds a record
        // that is valid in isolation, exactly where a 2-point record 0 would have ended.
        let mut chunk = std::vec![FILLER; NAV_CHUNK_SIZE];
        chunk[..NAV_EDGE_MIN_LEN].fill(0);
        chunk[4..6].copy_from_slice(&0u16.to_le_bytes());
        chunk[NAV_EDGE_MIN_LEN..NAV_EDGE_MIN_LEN + NAV_EDGE_MIN_LEN].fill(0);
        chunk[NAV_EDGE_MIN_LEN + 4..NAV_EDGE_MIN_LEN + 6].copy_from_slice(&2u16.to_le_bytes());

        // In isolation the record at 19 is well-formed — so the walk is the only thing that can
        // refuse it, and this assertion is what makes the test non-vacuous.
        assert_eq!(nav_edge_step(&chunk, NAV_EDGE_MIN_LEN), Some(NAV_EDGE_MIN_LEN));

        // The target itself is refused, as before …
        assert_eq!(nav_edge_record_range(&chunk, 0), None);
        // … and so is every ordinal *behind* the malformed record, which is the new claim.
        assert_eq!(nav_edge_record_range(&chunk, 1), None);
        assert_eq!(nav_edge_record_range(&chunk, 2), None);
        assert_eq!(nav_edge_record_range(&chunk, NAV_EDGE_ORDINAL_MASK), None);
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(STYLE_RECORD_LEN, 1 + 1 + 2 + 1 + 1 + 2);
        assert_eq!(FEATURE_HEADER_COMPACT_LEN, 1 + 1 + 1 + 2 + 2);
        assert_eq!(FEATURE_HEADER_WIDE_LEN, 1 + 1 + 2 + 4 + 4);
        assert_eq!(POI_RECORD_LEN, 4 + 4 + 1 + 1 + POI_NAME_LEN + 2);
        // v12 §8.6: the v9 record (name + two multiplier tables) plus climb weight + reserved.
        assert_eq!(NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN + 32 + 8 + 1 + NAV_PROFILE_RESERVED_LEN);
        assert_eq!(NAV_PROFILE_CLIMB_WEIGHT_OFF, NAV_PROFILE_NAME_LEN + 32 + 8);
        // v12 §8.3: the v9 entry plus the directional `Ascent M`.
        assert_eq!(NAV_NEIGHBOR_LEN, 4 + 2 + 2 + 4 + 2 + 1 + 2);
        assert_eq!(NAV_NEIGHBOR_ASCENT_OFF, NAV_NEIGHBOR_LEN - 2);
        // The §8.3 degree-cap derivation: 13 + 17 × 24 = 421 ≤ 512, so the pinned nav chunk still
        // holds a cap-degree record whole.
        assert_eq!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN, 421);
    }

    #[test]
    fn poi_id_tables_pin_the_append_only_contract() {
        assert_eq!(POI_SUBTYPES.len(), 18);
        assert_eq!(PoiCategory::ALL.map(PoiCategory::id), [1, 2, 3, 4, 5, 6]);
        for (index, row) in POI_SUBTYPES.iter().enumerate() {
            let subtype_id = (index + 1) as u8;
            assert_eq!(poi_subtype_row(subtype_id).map(|value| value.label), Some(row.label));
            assert_eq!(PoiCategory::from_id(row.category.id()), Some(row.category));
            assert!(row.label.len() <= 14);
            assert!(row.label.is_ascii() && row.label.bytes().all(|byte| (0x20..=0x7E).contains(&byte)));
        }
        assert!(poi_subtype_row(0).is_none());
        assert!(poi_subtype_row(CHUNK_END).is_none());
        assert!(poi_subtype_row(19).is_none());
        assert_eq!(PoiCategory::from_id(0), None);
        assert_eq!(PoiCategory::from_id(7), None);
    }
}
