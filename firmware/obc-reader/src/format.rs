//! OBCM on-wire flag/sentinel bit constants — the single definition, referenced by **both** the
//! reader ([`crate::reader`]) and the packer (`obc-pack`) so a layout change is one edit rather
//! than two hand-synced literals. A one-bit drift between the two sides still parses but decodes
//! wrong (a polygon read as a line, a branch walked as a leaf) — the hardest corruption to trace.
//!
//! Byte *offsets* and record *lengths* live next to the code that walks them
//! ([`crate::HEADER_LEN`], [`crate::reader::LOD_ENTRY_LEN`]). See `OBCM_Spec.md` for the layout tour.

/// Per-feature `flags` byte — geometry deltas are 16-bit signed (else 8-bit signed).
pub const FEATURE_FLAG_16BIT: u8 = 0x01;
/// Per-feature `flags` byte — the feature is a polygon (else a polyline).
pub const FEATURE_FLAG_POLYGON: u8 = 0x02;
/// Per-feature `flags` byte — the polygon carries interior rings (holes).
pub const FEATURE_FLAG_HOLES: u8 = 0x04;

/// Style-record `flags` byte — the low two bits hold `priority - 1`
/// (stored `0..=3` ⇒ render priority `1..=4`).
pub const STYLE_PRIORITY_MASK: u8 = 0x03;

/// Quadtree index node — the high bit set marks a **branch**; the low 31 bits are then the
/// first-child node index. Clear ⇒ a leaf, whose low bits are a geometry-chunk id (or
/// [`EMPTY_LEAF`]). Mask a leaf's chunk id with `!BRANCH_BIT` / extract a branch's child
/// index the same way.
pub const BRANCH_BIT: u32 = 0x8000_0000;
/// Quadtree index node — a leaf with no geometry chunk (numerically `!BRANCH_BIT`).
pub const EMPTY_LEAF: u32 = 0x7FFF_FFFF;
