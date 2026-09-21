//! Store identity metadata — the remaining FAT route-id floor and the id-era epoch nonce.
//!
//! These codecs protect the object store's identity invariants — ids never reuse, and an id-era
//! reset is phone-detectable. They live together because the mint rule couples them.
//!
//! Also home to the shared [`crc16`] used by the settings, arm-marker and identity codecs.

/// CRC-16/CCITT-FALSE (poly `0x1021`, init `0xFFFF`) over `data` — table-free, and enough to reject
/// a blank or half-written blob.
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

// The FAT route reader names routes by durable `u16` ids (`RT{id}.OBR`). The phone persists those
// ids, so an id must never be reused, even after the file it named is deleted and a reboot re-scans
// the card. `scan_max + 1` alone re-issues a deleted id, so a CRC-checked 16-byte RRAM line holds
// the next fresh route id and allocation is `max(scan_max + 1, stored_next)`.
//
// The codec lives here rather than in the board crate, because the board crate is target-only and
// the torn-line semantics must be host-testable.

/// The id high-water line's fixed length: one RRAM write line. Layout: `magic(4) · version(1) ·
/// pad(1) · next_route_id u16 LE · reserved(4) · crc16 LE · pad(2)` — CRC-16 over bytes `[0..12]`.
pub const ID_MARKS_LEN: usize = 16;
/// The id-marks line's tag. Anything else there decodes to "no floor", and allocation falls back
/// to `scan_max + 1`.
const ID_MARKS_MAGIC: [u8; 4] = *b"OBCI";
/// Id-marks layout version. An old version reads as no floor.
const ID_MARKS_VERSION: u8 = 1;
/// CRC-covered prefix of the id-marks line.
const ID_MARKS_PAYLOAD: usize = 12;

/// The durable FAT route-id floor. Flat-store rides use the catalog's u64 `next_object` cursor and
/// have no RRAM/FAT filename floor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdMarks {
    /// One past the highest route object id ever assigned (`RT{id}.OBR` uploads).
    pub next_route_id: u16,
}

impl IdMarks {
    /// Allocate the next fresh route id: `max(scan_next, stored floor)`, bumping the floor past it.
    /// `scan_next` is one past the highest id the card scan saw. Persist `self` afterwards.
    pub fn alloc_route(&mut self, scan_next: u16) -> u16 {
        let id = self.next_route_id.max(scan_next);
        self.next_route_id = id.saturating_add(1);
        id
    }
}

/// Pack the id high-water marks into their fixed 16-byte RRAM line. Inverse of
/// [`decode_id_marks`].
pub fn encode_id_marks(m: &IdMarks) -> [u8; ID_MARKS_LEN] {
    let mut b = [0u8; ID_MARKS_LEN];
    b[0..4].copy_from_slice(&ID_MARKS_MAGIC);
    b[4] = ID_MARKS_VERSION;
    b[6..8].copy_from_slice(&m.next_route_id.to_le_bytes());
    let crc = crc16(&b[0..ID_MARKS_PAYLOAD]);
    b[ID_MARKS_PAYLOAD..ID_MARKS_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
    b
}

/// Decode an id high-water line, or `None` for anything but a clean read of this format. `None`
/// means no floor, so the caller falls back to `scan_max + 1`.
pub fn decode_id_marks(bytes: &[u8]) -> Option<IdMarks> {
    if bytes.len() < ID_MARKS_LEN {
        return None;
    }
    let b = &bytes[..ID_MARKS_LEN];
    if b[0..4] != ID_MARKS_MAGIC || b[4] != ID_MARKS_VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[ID_MARKS_PAYLOAD], b[ID_MARKS_PAYLOAD + 1]]);
    if crc != crc16(&b[0..ID_MARKS_PAYLOAD]) {
        return None;
    }
    if b[8..12].iter().any(|byte| *byte != 0) {
        return None;
    }
    Some(IdMarks { next_route_id: u16::from_le_bytes([b[6], b[7]]) })
}

// A per-id-era `u32` nonce that lets the phone detect an id-era reset. Any event that loses the
// durable id floor above while the app keeps its library — a reflash, a factory reset, a torn
// id-marks write — reopens already-issued object ids, so freshly-minted ids alias months-old
// phone-side state. The nonce is drawn from the TRNG and persisted in a card-resident file
// (`EPOCH.OBE` in the card root), so a card swap transplants the store's identity and a card
// written by another device presents its own epoch. The app scopes all id-keyed state by (device
// serial, store epoch), so an era change makes the old era's keys stop matching by construction.
//
// The mint decision ([`store_epoch_mint`]) is a pure function, so the rule is host-testable without
// the board crate. A torn, absent or foreign file reads as `None`, exactly the id-marks
// convention.

/// The store-epoch file's fixed length: 12 bytes, `magic(4) · version(1) · pad(1) · epoch u32 LE ·
/// crc16 LE` — CRC-16 over bytes `[0..10]`. A card sidecar, not an RRAM line, so no write-line
/// padding.
pub const STORE_EPOCH_LEN: usize = 12;
/// The store-epoch file's tag. Anything else there decodes to `None` — "no epoch", which the mint
/// rule treats as clause 1.
const STORE_EPOCH_MAGIC: [u8; 4] = *b"OBCE";
/// Store-epoch layout version. An old version reads as no epoch.
const STORE_EPOCH_VERSION: u8 = 1;
/// CRC-covered prefix of the store-epoch file: `magic(4) · version(1) · pad(1) · epoch u32 LE`.
const STORE_EPOCH_PAYLOAD: usize = 10;

/// Pack the store-epoch nonce into its fixed 12-byte card file. Inverse of [`decode_store_epoch`].
pub fn encode_store_epoch(epoch: u32) -> [u8; STORE_EPOCH_LEN] {
    let mut b = [0u8; STORE_EPOCH_LEN];
    b[0..4].copy_from_slice(&STORE_EPOCH_MAGIC);
    b[4] = STORE_EPOCH_VERSION;
    b[6..10].copy_from_slice(&epoch.to_le_bytes());
    let crc = crc16(&b[0..STORE_EPOCH_PAYLOAD]);
    b[STORE_EPOCH_PAYLOAD..STORE_EPOCH_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
    b
}

/// Decode a store-epoch file, or `None` for anything but a clean read of this format. `None` means
/// no epoch, and the mint rule draws a fresh one.
pub fn decode_store_epoch(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < STORE_EPOCH_LEN {
        return None;
    }
    let b = &bytes[..STORE_EPOCH_LEN];
    if b[0..4] != STORE_EPOCH_MAGIC || b[4] != STORE_EPOCH_VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[STORE_EPOCH_PAYLOAD], b[STORE_EPOCH_PAYLOAD + 1]]);
    if crc != crc16(&b[0..STORE_EPOCH_PAYLOAD]) {
        return None;
    }
    Some(u32::from_le_bytes([b[6], b[7], b[8], b[9]]))
}

/// The boot-time store-epoch mint decision, a pure function so the rule is host-testable. Given the
/// decoded card epoch, the RRAM id-marks line (each `None` when absent, torn or foreign) and one
/// freshly-drawn TRNG word `fresh`, it returns:
///
/// - `None` — keep the card's epoch and write nothing. A card swap to another store with a valid
///   epoch lands here, which is the transplant.
/// - `Some((new_epoch, marks))` — persist `new_epoch` to the card epoch file and (re)write the RRAM
///   id-marks line to `marks` in the same boot pass.
///
/// A mint fires when the card epoch is absent (clause 1), or when the id-marks line decodes to "no
/// floor" (clause 2): a floor lost under an intact card epoch would be undetectable aliasing, so a
/// lost floor is a new era.
///
/// The marks (re)write establishes the invariant "a valid epoch implies a valid id-marks line at
/// mint". Without it a fresh device, which has no id-marks line by design, would re-mint on every
/// boot through clause 2.
pub fn store_epoch_mint(epoch: Option<u32>, marks: Option<IdMarks>, fresh: u32) -> Option<(u32, IdMarks)> {
    if epoch.is_some() && marks.is_some() {
        return None; // steady state: valid card epoch + valid floors → nothing to write this boot
    }
    Some((fresh, marks.unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_marks_codec_round_trips_and_rejects_torn_lines() {
        let m = IdMarks { next_route_id: 7 };
        assert_eq!(decode_id_marks(&encode_id_marks(&m)), Some(m));
        assert_eq!(decode_id_marks(&encode_id_marks(&IdMarks::default())), Some(IdMarks::default()));

        assert_eq!(decode_id_marks(&[0u8; ID_MARKS_LEN]), None, "a blank (all-zero) line is no floor");
        assert_eq!(decode_id_marks(&[0xFF; ID_MARKS_LEN]), None, "an erased (all-ones) line is no floor");
        assert_eq!(decode_id_marks(&encode_id_marks(&m)[..ID_MARKS_LEN - 1]), None, "a short slice is rejected");
        let mut torn = encode_id_marks(&m);
        torn[7] ^= 0xFF; // a payload byte flipped without fixing the CRC is the torn-write shape
        assert_eq!(decode_id_marks(&torn), None, "a CRC mismatch (torn write) is no floor");
        let mut old = encode_id_marks(&m);
        old[4] = ID_MARKS_VERSION + 1;
        let crc = crc16(&old[0..ID_MARKS_PAYLOAD]);
        old[ID_MARKS_PAYLOAD..ID_MARKS_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_id_marks(&old), None, "a foreign layout version is no floor");
    }

    #[test]
    fn store_epoch_codec_round_trips_and_rejects_torn_lines() {
        assert_eq!(encode_store_epoch(0).len(), STORE_EPOCH_LEN, "the file is 12 bytes, no RRAM padding");
        assert_eq!(decode_store_epoch(&encode_store_epoch(0xDEAD_BEEF)), Some(0xDEAD_BEEF));
        assert_eq!(decode_store_epoch(&encode_store_epoch(0)), Some(0), "a zero nonce is a legal value");

        assert_eq!(decode_store_epoch(&[0u8; STORE_EPOCH_LEN]), None, "a blank (all-zero) file is no epoch");
        assert_eq!(decode_store_epoch(&[0xFF; STORE_EPOCH_LEN]), None, "an erased (all-ones) file is no epoch");
        assert_eq!(decode_store_epoch(&[]), None, "an absent (empty) file is no epoch");
        assert_eq!(
            decode_store_epoch(&encode_store_epoch(0xDEAD_BEEF)[..STORE_EPOCH_LEN - 1]),
            None,
            "a short slice is rejected"
        );
        let mut torn = encode_store_epoch(0xDEAD_BEEF);
        torn[7] ^= 0xFF; // an epoch byte flipped without fixing the CRC is the torn-write shape
        assert_eq!(decode_store_epoch(&torn), None, "a CRC mismatch (torn write) is no epoch");
        let mut old = encode_store_epoch(0xDEAD_BEEF);
        old[4] = STORE_EPOCH_VERSION + 1;
        let crc = crc16(&old[0..STORE_EPOCH_PAYLOAD]);
        old[STORE_EPOCH_PAYLOAD..STORE_EPOCH_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_store_epoch(&old), None, "a foreign layout version is no epoch");
    }

    /// `FRESH` is the TRNG word the board draws. The pure function never draws it, so the test is
    /// deterministic.
    #[test]
    fn store_epoch_mint_rule() {
        const FRESH: u32 = 0x1234_5678;
        let floor = IdMarks { next_route_id: 9 };

        // Steady state: a valid card epoch + valid floors → keep the card's epoch, write nothing.
        assert_eq!(store_epoch_mint(Some(0xABCD), Some(floor), FRESH), None);

        // Clause 1 only: mint a fresh epoch but keep the existing floors verbatim. A torn or absent
        // epoch file must never cost the durable id floor.
        assert_eq!(store_epoch_mint(None, Some(floor), FRESH), Some((FRESH, floor)));

        // Clause 2: a lost floor is a new era, so mint although the card's epoch was valid and
        // reseed the floor to the default, which the store re-derives from the card scan.
        assert_eq!(store_epoch_mint(Some(0xABCD), None, FRESH), Some((FRESH, IdMarks::default())));

        // Fresh device (no card epoch + no floor): mint + seed default floors.
        assert_eq!(store_epoch_mint(None, None, FRESH), Some((FRESH, IdMarks::default())));
    }

    /// The epoch rides the card, so swapping cards transplants the store identity with no mint. A
    /// card swap is never an era event by itself; clause 2 is only about a lost floor.
    #[test]
    fn store_epoch_card_swap_transplants_the_era() {
        const FRESH: u32 = 0xDEAD_0001; // never consumed: every step below is a "keep"
        let floor = IdMarks { next_route_id: 3 };
        let e_a = 0xAAAA_1111u32; // card A's epoch
        let e_b = 0xBBBB_2222u32; // card B's epoch

        // Card A mounted, steady state → no mint, the served epoch is card A's.
        assert_eq!(store_epoch_mint(Some(e_a), Some(floor), FRESH), None, "card A steady: no mint");

        // Swap to card B, which has its own valid epoch: no mint, and the served epoch is now e_b,
        // a different store identity on the wire.
        assert_eq!(store_epoch_mint(Some(e_b), Some(floor), FRESH), None, "card B adopted verbatim — transplant");

        // Swap back to card A → no mint again, e_a served. The original era is restored intact.
        assert_eq!(store_epoch_mint(Some(e_a), Some(floor), FRESH), None, "swap-back restores card A's era");
    }

    /// After a mint the caller persists both records, and a re-decode of what it wrote leaves both
    /// valid, so the next boot cannot mistake the fresh state for a torn one.
    #[test]
    fn store_epoch_mint_writes_a_valid_marks_line() {
        const FRESH: u32 = 0x0BAD_F00D;
        // A clause-2 mint: blank id-marks, intact card epoch.
        let (new_epoch, new_marks) = store_epoch_mint(Some(0x55), None, FRESH).expect("clause 2 mints");
        // Persist-then-reload both records exactly as the board does.
        assert_eq!(decode_store_epoch(&encode_store_epoch(new_epoch)), Some(FRESH), "epoch file valid post-mint");
        assert_eq!(decode_id_marks(&encode_id_marks(&new_marks)), Some(new_marks), "id-marks line valid post-mint");
    }

    /// A device that never saves a ride or uploads a route mints once, and every later boot keeps
    /// that same epoch.
    #[test]
    fn store_epoch_fresh_device_stability() {
        const FRESH: u32 = 0xFEED_BEEF;
        // Boot 1: no card epoch + no floor → mint.
        let (epoch, marks) = store_epoch_mint(None, None, FRESH).expect("first boot mints");
        // The board writes both records; model them as the card file and RRAM line it persisted.
        let epoch_line = encode_store_epoch(epoch);
        let marks_line = encode_id_marks(&marks);

        // Boots 2..N with no route allocations: both read back valid, so the decision is "keep".
        // A different TRNG word each boot is irrelevant, because the function never reaches it.
        for boot_fresh in [0x1111_1111u32, 0x2222_2222, 0x3333_3333] {
            let e = decode_store_epoch(&epoch_line);
            let m = decode_id_marks(&marks_line);
            assert_eq!(store_epoch_mint(e, m, boot_fresh), None, "a settled fresh device never re-mints");
        }
        assert_eq!(decode_store_epoch(&epoch_line), Some(epoch), "and the epoch is stable across boots");
    }

    /// Rides are absent on purpose: their full-width ids come from the flat catalog cursor.
    #[test]
    fn id_allocation_never_reuses_after_delete() {
        let mut card: heapless::Vec<u16, 8> = heapless::Vec::new(); // live RT{id} files
        let mut marks = IdMarks::default(); // fresh device: no floor
        let scan_next = |card: &[u16]| card.iter().max().map_or(0, |m| m + 1);

        // Three routes saved: 0, 1, 2 — the same as `scan_max + 1` while nothing deletes.
        for want in 0..3u16 {
            let id = marks.alloc_route(scan_next(&card));
            assert_eq!(id, want);
            let _ = card.push(id);
        }

        // Delete the highest id, which `scan_max + 1` alone would re-issue.
        card.retain(|&id| id != 2);
        // A reboot: the floor survives in RRAM and the scan is rebuilt from the card.
        let mut rebooted = decode_id_marks(&encode_id_marks(&marks)).expect("persisted floor survives");
        let id = rebooted.alloc_route(scan_next(&card));
        assert_eq!(id, 3, "the deleted id 2 is never reused");
        let _ = card.push(id);

        // A torn floor line falls back to `scan_max + 1`, so ids can collide with tombstones
        // again.
        let mut torn = encode_id_marks(&rebooted);
        torn[7] ^= 0x55;
        let mut no_floor = decode_id_marks(&torn).unwrap_or_default();
        assert_eq!(no_floor.alloc_route(scan_next(&card)), 4, "torn line → scan-max+1");
    }
}
