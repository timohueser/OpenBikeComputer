//! The OBC2 contract capacities (`OBC2_Storage_Format.md` §2) and the format geometry every
//! record codec addresses itself against (§1.1, §3, §5, §6, §7, §10, §12).
//!
//! §2 is "the sole authority" for the capacities: the wire contract's `ResourceLimits` block
//! (`Device_Object_Protocol_v3.md` §5.1) restates them and is corrected against this table, which
//! is why the tests below pin both the storage values and the mirror's encoding of them. Every
//! value here is a compile-time constant: none of them is inferred from available RAM, and raising
//! one is a resource review — changing a fixed array or file size is an OBC2 format-version change.

// -------------------------------------------------------------------------------------------
// §2 contract capacities
// -------------------------------------------------------------------------------------------

/// Logical catalog heads, all kinds.
pub const MAX_CATALOG_HEADS: usize = 256;
/// Route heads.
pub const MAX_ROUTE_HEADS: usize = 64;
/// Trip heads.
pub const MAX_TRIP_HEADS: usize = 16;
/// Ride heads.
pub const MAX_RIDE_HEADS: usize = 128;
/// Weather heads.
pub const MAX_WEATHER_HEADS: usize = 1;
/// Volume-manifest heads.
pub const MAX_VOLUME_MANIFEST_HEADS: usize = 8;
/// Update-package heads.
pub const MAX_UPDATE_PACKAGE_HEADS: usize = 8;
/// Normal active claimed operations.
pub const MAX_NORMAL_ACTIVE_OPERATIONS: usize = 8;
/// The one reserved maintenance/cancellation/recovery claim.
pub const RESERVED_ACTIVE_OPERATIONS: usize = 1;
/// Rows in the active-operation region: eight normal plus the one reserved.
pub const MAX_ACTIVE_OPERATIONS: usize = MAX_NORMAL_ACTIVE_OPERATIONS + RESERVED_ACTIVE_OPERATIONS;
/// Active resumable upload/work records.
pub const MAX_ACTIVE_WORK: usize = 4;
/// Attached heavy stream sessions, system-wide.
pub const MAX_HEAVY_STREAM_SESSIONS: usize = 1;
/// Active draft parents.
pub const MAX_DRAFT_PARENTS: usize = 1;
/// Sealed or streaming draft parts of the one active parent.
pub const MAX_DRAFT_PARTS: usize = 32;
/// Children referenced by one manifest.
pub const MAX_MANIFEST_CHILDREN: usize = 32;
/// Simultaneously mounted map data files on the current board.
pub const MAX_MOUNTED_MAP_FILES: usize = 11;
/// Live download leases.
pub const MAX_LEASES: usize = 4;
/// Retained previous generations.
pub const MAX_RETAINED_PREVIOUS: usize = 8;
/// Active or recoverable ride journals.
pub const MAX_ACTIVE_RIDES: usize = 1;
/// Terminal operation results retained in the ring.
pub const MAX_TERMINAL_RESULTS: usize = 64;
/// Journal slots.
pub const JOURNAL_SLOTS: usize = 256;
/// Valid slots in one epoch that trigger compaction: a 193rd record is refused until it runs.
pub const JOURNAL_COMPACTION_TRIGGER: usize = 192;
/// Inactive-work retention horizon, in later terminal commits.
pub const WORK_EXPIRY_HORIZON: u64 = 256;
/// Maximum length of one embedded FAT generation — also the adapter's `MAX_FILE_SIZE` (§13.1).
pub const MAX_GENERATION_LEN: u64 = 0xFFFF_FFFF;

/// Repository-state rows the checkpoint region holds. This is a region capacity rather than a §2
/// product limit: `ObjectKind` is a `u16` registry and the checkpoint reserves 16 rows for it.
pub const MAX_REPOSITORY_STATES: usize = 16;

// -------------------------------------------------------------------------------------------
// §1.1 media geometry
// -------------------------------------------------------------------------------------------

/// The media program page, a format constant. A cut during programming may corrupt any sector
/// inside the page being programmed and no byte outside it.
pub const PROGRAM_PAGE: usize = 16_384;
/// Every gated slot begins at a multiple of this stride inside its file, which under the §1.1
/// volume-geometry preconditions makes each slot exactly one physical program page.
pub const SLOT_STRIDE: usize = PROGRAM_PAGE;
/// One sector.
pub const SECTOR: usize = 512;
/// Every gate is exactly one sector.
pub const GATE_LEN: usize = SECTOR;

// -------------------------------------------------------------------------------------------
// §5, §6, §7, §10, §12 file and slot geometry
// -------------------------------------------------------------------------------------------

/// `CAT0.CHK`/`CAT1.CHK`: four slot strides.
pub const CHECKPOINT_FILE_LEN: usize = 65_536;
/// Checkpoint body bytes, gate excluded.
pub const CHECKPOINT_BODY_LEN: usize = 65_024;
/// The checkpoint body CRC, at the end of the body.
pub const CHECKPOINT_BODY_CRC_OFFSET: usize = 65_020;
/// The checkpoint gate, in the file.
pub const CHECKPOINT_GATE_OFFSET: usize = 65_024;

/// `COMMIT.JNL`: 256 slots of one stride.
pub const JOURNAL_FILE_LEN: usize = 4_194_304;
/// A journal body, at its slot base.
pub const JOURNAL_BODY_LEN: usize = 1_536;
/// The journal body CRC, at the end of the body.
pub const JOURNAL_BODY_CRC_OFFSET: usize = 1_532;
/// A journal gate, relative to its slot base.
pub const JOURNAL_GATE_OFFSET: usize = 1_536;
/// The fixed mutation length a journal body declares and carries.
pub const MUTATION_LEN: usize = 1_272;

/// A `WORK` file: two alternating slots.
pub const WORK_FILE_LEN: usize = 32_768;
/// `WORK` slots.
pub const WORK_SLOTS: usize = 2;
/// `RIDE.ACT`: 16 circular slots.
pub const RIDE_FILE_LEN: usize = 262_144;
/// `RIDE.ACT` slots.
pub const RIDE_SLOTS: usize = 16;
/// `ARM0.HND`/`ARM1.HND` and `INIT.REC`: one stride each.
pub const SLOT_FILE_LEN: usize = SLOT_STRIDE;
/// The bodies of the WORK, RIDE, ARM and INIT records are one sector each.
pub const SMALL_BODY_LEN: usize = SECTOR;
/// Their body CRC sits at the end of that sector.
pub const SMALL_BODY_CRC_OFFSET: usize = 508;
/// Their gate follows their body, at slot base `+ 512`.
pub const SMALL_GATE_OFFSET: usize = 512;

/// `HandoffRef`, the fixed reference both the ARM body and the checkpoint projection carry.
pub const HANDOFF_REF_LEN: usize = 240;

/// Bytes of fixed-size metadata files zero-filled at initialization (§13.1): the journal, both
/// checkpoints, both ARM files, `RIDE.ACT` and `INIT.REC`.
pub const INITIALIZATION_ZERO_FILL: usize =
    JOURNAL_FILE_LEN + 2 * CHECKPOINT_FILE_LEN + 2 * SLOT_FILE_LEN + RIDE_FILE_LEN + SLOT_FILE_LEN;

#[cfg(test)]
mod tests {
    use super::*;

    /// §2's table, value by value. The point of the test is that the table and the code cannot
    /// drift apart silently: a capacity is a contract constant, so changing one has to change a
    /// line here too.
    #[test]
    fn section_2_capacities() {
        assert_eq!(MAX_CATALOG_HEADS, 256);
        assert_eq!(MAX_ROUTE_HEADS, 64);
        assert_eq!(MAX_TRIP_HEADS, 16);
        assert_eq!(MAX_RIDE_HEADS, 128);
        assert_eq!(MAX_WEATHER_HEADS, 1);
        assert_eq!(MAX_VOLUME_MANIFEST_HEADS, 8);
        assert_eq!(MAX_UPDATE_PACKAGE_HEADS, 8);
        assert_eq!(MAX_NORMAL_ACTIVE_OPERATIONS, 8);
        assert_eq!(RESERVED_ACTIVE_OPERATIONS, 1);
        assert_eq!(MAX_ACTIVE_OPERATIONS, 9);
        assert_eq!(MAX_ACTIVE_WORK, 4);
        assert_eq!(MAX_HEAVY_STREAM_SESSIONS, 1);
        assert_eq!(MAX_DRAFT_PARENTS, 1);
        assert_eq!(MAX_DRAFT_PARTS, 32);
        assert_eq!(MAX_MANIFEST_CHILDREN, 32);
        assert_eq!(MAX_MOUNTED_MAP_FILES, 11);
        assert_eq!(MAX_LEASES, 4);
        assert_eq!(MAX_RETAINED_PREVIOUS, 8);
        assert_eq!(MAX_ACTIVE_RIDES, 1);
        assert_eq!(MAX_TERMINAL_RESULTS, 64);
        assert_eq!(JOURNAL_SLOTS, 256);
        assert_eq!(JOURNAL_COMPACTION_TRIGGER, 192);
        assert_eq!(WORK_EXPIRY_HORIZON, 256);
        assert_eq!(MAX_GENERATION_LEN, 0xFFFF_FFFF);
    }

    /// The recovery-suffix budget of §6.3: 32 draft-part transitions, nine active-row terminals,
    /// four update-reconciliation records, two ride-publication records and eight lease-clearing
    /// retention records is 55, nine below the 64-slot headroom above the compaction trigger.
    #[test]
    fn recovery_suffix_fits_the_headroom() {
        let suffix = MAX_DRAFT_PARTS + MAX_ACTIVE_OPERATIONS + 4 + 2 + MAX_RETAINED_PREVIOUS;
        assert_eq!(suffix, 55);
        let headroom = JOURNAL_SLOTS - JOURNAL_COMPACTION_TRIGGER;
        assert_eq!(headroom, 64);
        assert_eq!(suffix + 9, headroom);
    }

    /// §2: the eight retained-previous entries exceed the seven a legitimate workload holds at
    /// once — four live leases, two update-rollback entries, and the one weather domain-retention
    /// entry — so admission never has to refuse a mutation for want of one.
    #[test]
    fn retention_table_has_one_entry_of_margin() {
        assert_eq!(MAX_LEASES + 2 + MAX_WEATHER_HEADS, 7);
        assert_eq!(MAX_RETAINED_PREVIOUS, 7 + 1);
    }

    /// §1.1 and §3 geometry: every gated slot is one program page, and every fixed file is a whole
    /// number of strides.
    #[test]
    fn geometry_is_page_aligned() {
        assert_eq!(PROGRAM_PAGE, 16_384);
        for len in [CHECKPOINT_FILE_LEN, JOURNAL_FILE_LEN, WORK_FILE_LEN, RIDE_FILE_LEN, SLOT_FILE_LEN] {
            assert_eq!(len % SLOT_STRIDE, 0, "{len} is not a whole number of strides");
        }
        assert_eq!(JOURNAL_FILE_LEN / SLOT_STRIDE, JOURNAL_SLOTS);
        assert_eq!(WORK_FILE_LEN / SLOT_STRIDE, WORK_SLOTS);
        assert_eq!(RIDE_FILE_LEN / SLOT_STRIDE, RIDE_SLOTS);
        assert_eq!(CHECKPOINT_FILE_LEN, CHECKPOINT_BODY_LEN + GATE_LEN);
        assert_eq!(CHECKPOINT_BODY_CRC_OFFSET + 4, CHECKPOINT_BODY_LEN);
        assert_eq!(JOURNAL_BODY_CRC_OFFSET + 4, JOURNAL_BODY_LEN);
        assert_eq!(SMALL_BODY_CRC_OFFSET + 4, SMALL_BODY_LEN);
        assert_eq!(SMALL_GATE_OFFSET, SMALL_BODY_LEN);
        assert_eq!(JOURNAL_GATE_OFFSET, JOURNAL_BODY_LEN);
    }

    /// §13.1's zero-fill figure, which the initialization measurement is quoted against.
    #[test]
    fn initialization_zero_fill_matches_the_adapter_contract() {
        assert_eq!(INITIALIZATION_ZERO_FILL, 4_636_672);
    }

    /// `Device_Object_Protocol_v3.md` §5.1's 56-byte mirror, encoded from these constants. The
    /// wire block is not authoritative — this proves the mirror still restates §2 exactly.
    #[test]
    fn wire_resource_limits_mirror_restates_the_same_values() {
        let mut block = [0u8; 56];
        block[0] = 1;
        block[1] = 56;
        block[4..6].copy_from_slice(&(MAX_CATALOG_HEADS as u16).to_le_bytes());
        block[6] = MAX_NORMAL_ACTIVE_OPERATIONS as u8;
        block[7] = MAX_ACTIVE_WORK as u8;
        block[8] = MAX_DRAFT_PARENTS as u8;
        block[9] = MAX_DRAFT_PARTS as u8;
        block[10] = MAX_MANIFEST_CHILDREN as u8;
        block[11] = MAX_MOUNTED_MAP_FILES as u8;
        block[12] = MAX_LEASES as u8;
        block[13] = MAX_RETAINED_PREVIOUS as u8;
        block[14..16].copy_from_slice(&(MAX_TERMINAL_RESULTS as u16).to_le_bytes());
        block[16..18].copy_from_slice(&(WORK_EXPIRY_HORIZON as u16).to_le_bytes());
        block[20..28].copy_from_slice(&MAX_GENERATION_LEN.to_le_bytes());
        block[36..38].copy_from_slice(&(MAX_ROUTE_HEADS as u16).to_le_bytes());
        block[38..40].copy_from_slice(&(MAX_TRIP_HEADS as u16).to_le_bytes());
        block[40..42].copy_from_slice(&(MAX_RIDE_HEADS as u16).to_le_bytes());
        block[42..44].copy_from_slice(&(MAX_WEATHER_HEADS as u16).to_le_bytes());
        block[44..46].copy_from_slice(&(MAX_VOLUME_MANIFEST_HEADS as u16).to_le_bytes());
        block[46..48].copy_from_slice(&(MAX_UPDATE_PACKAGE_HEADS as u16).to_le_bytes());
        block[48] = MAX_HEAVY_STREAM_SESSIONS as u8;
        block[49] = RESERVED_ACTIVE_OPERATIONS as u8;
        block[50] = MAX_ACTIVE_RIDES as u8;

        // The exact bytes §5.1's table spells out, with a zero reservation snapshot at 28..36.
        let expected: [u8; 56] = [
            1, 56, 0, 0, 0, 1, 8, 4, 1, 32, 32, 11, 4, 8, 64, 0, 0, 1, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 64, 0, 16, 0, 128, 0, 1, 0, 8, 0, 8, 0, 1, 1, 1, 0, 0, 0, 0, 0,
        ];
        assert_eq!(block, expected);
    }
}
