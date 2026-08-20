//! Store identity metadata — the remaining FAT route-id floor and the id-era epoch nonce.
//!
//! These codecs protect the *object store's identity* invariants (ids never reuse; an id-era reset
//! is phone-detectable), not the rider's settings. They live together because the mint rule couples
//! them.
//!
//! Also home to the shared [`crc16`] the persistent sidecar/line codecs guard themselves with
//! (settings blob, arm marker, synced-ride and route-CRC sidecars included).

/// CRC-16/CCITT-FALSE (poly `0x1021`, init `0xFFFF`) over `data` — small, table-free, and
/// plenty to reject a blank/half-written blob. Guards the codec on both stores.
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

// ==================== durable object-id high-water marks (#450) ====================
//
// The remaining FAT route reader names routes by durable `u16` ids (`RT{id}.OBR`). The phone
// persists those ids, so an id must
// **never be reused** — even after the file it named is deleted and a reboot re-scans the card.
// `scan-max + 1` alone re-issues a deleted id; these high-water marks are the durable floor:
// one CRC-checked 16-byte RRAM line holding the next fresh route id, bumped on every
// assignment. Allocation = `max(scan_max + 1, stored_next)`.
//
// The codec lives here — beside the settings blob codec, the established precedent — because the
// board crate is target-only: encode/decode/torn-line semantics must be host-testable.

/// The id high-water line's fixed length: one RRAM write line (16 bytes), like the bond and
/// boot-counter lines. Layout: `magic(4) · version(1) · pad(1) · next_route_id u16 LE ·
/// reserved(4) · crc16 LE · pad(2)` — CRC-16 over bytes `[0..12]`.
pub const ID_MARKS_LEN: usize = 16;
/// The id-marks line's tag; anything else there (blank page, torn write, older layout) decodes to
/// "no floor" and allocation falls back to scan-max + 1 (exactly today's behaviour).
const ID_MARKS_MAGIC: [u8; 4] = *b"OBCI";
/// Id-marks layout version — bump on any field change (an old version reads as no floor).
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
    /// Allocate the next fresh **route** id: `max(scan_next, stored floor)`, bumping the floor past
    /// it — call with `scan_next` = one past the highest id the card scan saw, then persist `self`.
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

/// Decode an id high-water line, or `None` for anything but a clean read of this format — a blank
/// page, a torn write, a short slice, or an older layout. `None` means **no floor**: the caller
/// falls back to scan-max + 1, so a fresh device behaves exactly as before the marks existed.
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

// ==================== store-epoch nonce (protocol v2, #632/#767; card-resident #776) ====================
//
// A per-id-era `u32` nonce that lets the phone detect an **id-era reset**: any event that loses the
// durable id floor (the id-marks line above) while the app keeps its library — a full-chip reflash,
// a factory reset / RMA / recovery, or a torn id-marks write — reopens already-issued object ids, so
// freshly-minted ids silently *alias* months-old phone-side state (the 2026-07-12 ride-sync
// incident). The nonce is minted from the TRNG and persisted in a small **card-resident file**
// (`EPOCH.OBE` in the card root), so the SD card is the sole home of the id-era name (#776): a card
// swap **transplants** the store's identity (swap back restores the old era, a card upgrade-by-copy
// carries the era along), and a card written by a *different* device presents *its* epoch — its own
// scope — closing the residual foreign-card hole the RRAM-line design left open. It is served over
// the pre-pairing `protocolVersion` read (V2, #766); the app scopes all id-keyed state by
// (device serial, store epoch), so an era change makes the old era's keys stop matching by
// construction — no migration code.
//
// The mint decision ([`store_epoch_mint`]) is a pure function so the subtle rule is host-tested
// without the board crate; the board glue reads the card epoch file + the RRAM id-marks line, draws
// one TRNG word, and writes back (epoch → card, id-marks → RRAM). Torn/absent/foreign file → `None`,
// exactly the id-marks (and other sidecar) conventions. The file carries no RRAM line-size padding;
// like `ROUTES.CRC`, it is a card record rather than the retired RRAM line.

/// The store-epoch file's fixed length: 12 bytes, `magic(4) · version(1) · pad(1) · epoch u32 LE ·
/// crc16 LE` — CRC-16 over bytes `[0..10]`. A card sidecar, not an RRAM line, so no 16-byte write-line
/// padding (unlike the retired id-era RRAM line this replaced).
pub const STORE_EPOCH_LEN: usize = 12;
/// The store-epoch file's tag; anything else there (absent, torn write, older layout) decodes
/// to `None` — "no epoch", which the mint rule treats as clause 1 (mint a fresh nonce).
const STORE_EPOCH_MAGIC: [u8; 4] = *b"OBCE";
/// Store-epoch layout version — bump on any field change (an old version reads as no epoch).
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

/// Decode a store-epoch file, or `None` for anything but a clean read of this format — an absent
/// file (the board returns `None` before calling this), a torn write, a short slice, or an older
/// layout. `None` means **no epoch**: the mint rule draws a fresh one (clause 1).
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

/// The boot-time store-epoch mint decision (protocol v2, #632 item 5; card-resident #776) — a pure
/// function so the subtle rule is host-testable. Given the decoded **card epoch** (from `EPOCH.OBE`)
/// and the RRAM id-marks line (each `None` when absent/torn/foreign) plus one freshly-drawn TRNG word
/// `fresh`, returns:
///
/// - `None` — **keep** the card's epoch: this boot writes nothing (the common steady-state path,
///   including a *card swap* to another store with a valid epoch — that epoch is adopted verbatim,
///   the transplant semantics #776 exists for).
/// - `Some((new_epoch, marks))` — **mint**: persist `new_epoch` to the **card** epoch file **and**
///   (re)write the RRAM id-marks line to `marks` in the same boot pass.
///
/// Mint fires when the card epoch is absent (**clause 1**: absent/torn/foreign file) **or** the
/// id-marks line decodes to "no floor" (**clause 2**: a torn id-marks write — floors lost under an
/// intact card epoch would be *undetectable* aliasing, so a lost floor **is** a new era, and it
/// reopens the deleted-id band on the very card whose epoch was intact).
///
/// The marks (re)write is what makes clause 2 unambiguous: an already-valid id-marks line is kept
/// verbatim (its durable floors survive a clause-1-only mint), while an absent one is (re)seeded to
/// [`IdMarks::default`] — "no floor", which the store's `max(scan_max + 1, floor)` allocation
/// re-derives from the card scan at the first allocation (today's fallback; the board mints before
/// the scan runs). This establishes the invariant *a valid epoch implies a valid id-marks line at
/// mint*: without it a fresh device (no ride/upload → no id-marks line **by design**) would re-mint
/// on every boot via clause 2; with it, "valid epoch + no floor" is unambiguous torn-line evidence
/// — exactly what clause 2 exists to catch.
///
/// Note the function is agnostic to *where* the epoch is stored — the #776 move is entirely in the
/// board glue (it now reads/writes the card file, not an RRAM line); the decision logic is
/// unchanged, which is why the mint matrix + stability tests carry straight over.
pub fn store_epoch_mint(epoch: Option<u32>, marks: Option<IdMarks>, fresh: u32) -> Option<(u32, IdMarks)> {
    if epoch.is_some() && marks.is_some() {
        return None; // steady state: valid card epoch + valid floors → nothing to write this boot
    }
    Some((fresh, marks.unwrap_or_default()))
}

// ==================== the selected map (issue #927) ====================
//
// Which `.obcm` in the card root the renderer streams from. Before #927 there was no choice to
// record — the loader took *the first* map the directory scan yielded — but once the device can
// receive a map there are several on the card and "first by directory order" stops being an answer
// a rider could predict, let alone change.
//
// It is a **card** file (`MAP.SEL` in the root, beside `EPOCH.OBE`) and not an RRAM setting for the
// same reason the store epoch is: the thing it names lives on the card, so the choice must travel
// with it. Swap in a card built for another trip and it brings its own selection; put the first card
// back and the old one returns. An RRAM line would instead point at a filename that may not exist on
// the card now in the slot.
//
// The payload is the map's **8.3 filename**, not its object id, because side-loaded maps (a plain
// `something.obcm` the rider dragged on from a laptop) carry no device-assigned id — the filename is
// the one name every map on the card has. A selection naming a file that is no longer there decodes
// fine and the loader simply falls back, which is also what a torn/absent file does: the codec's
// failure direction is "no preference", never "no map".

/// The selected-map file's fixed length: 24 bytes, `magic(4) · version(1) · len(1) · name[12]
/// (ASCII, NUL-padded) · pad(4) · crc16 LE · pad(2)` — CRC-16 over bytes `[0..20]`. Fixed-length
/// like the other tiny card sidecars, so a rewrite is one truncating write with no torn-tail case.
pub const SELECTED_MAP_LEN: usize = 24;
/// The selected-map file's tag; anything else there (absent, torn, older layout) decodes to `None`
/// — "no preference", and the loader falls back to the first valid map on the card.
///
/// It shares those four bytes with the ride sidecar's `SYNCED_MAGIC` — different files, no shared
/// parser, and a grep for `b"OBCS"` finds both. It shared them with the OBCA volume-set manifest as
/// well until OBCM v14 retired the set, which is the one of the three that was a *format*.
const SELECTED_MAP_MAGIC: [u8; 4] = *b"OBCS";
/// Selected-map layout version — bump on any field change (an old version reads as no preference).
const SELECTED_MAP_VERSION: u8 = 1;
/// CRC-covered prefix of the selected-map file.
const SELECTED_MAP_PAYLOAD: usize = 20;
/// The widest 8.3 name the file can carry: `12345678.EXT`.
pub const SELECTED_MAP_NAME_MAX: usize = 12;

/// Pack a selected map's 8.3 filename into its fixed 24-byte card file. Inverse of
/// [`decode_selected_map`]. A name longer than [`SELECTED_MAP_NAME_MAX`] or carrying a non-ASCII
/// byte is refused (`None`) rather than truncated — a truncated name would select a *different*
/// file, and silently pointing the renderer somewhere else is worse than keeping the old choice.
pub fn encode_selected_map(name: &str) -> Option<[u8; SELECTED_MAP_LEN]> {
    let raw = name.as_bytes();
    if raw.is_empty() || raw.len() > SELECTED_MAP_NAME_MAX || !raw.iter().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let mut b = [0u8; SELECTED_MAP_LEN];
    b[0..4].copy_from_slice(&SELECTED_MAP_MAGIC);
    b[4] = SELECTED_MAP_VERSION;
    b[5] = raw.len() as u8;
    b[6..6 + raw.len()].copy_from_slice(raw);
    let crc = crc16(&b[0..SELECTED_MAP_PAYLOAD]);
    b[SELECTED_MAP_PAYLOAD..SELECTED_MAP_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
    Some(b)
}

/// Decode a selected-map file into the 8.3 filename it names, or `None` for anything but a clean
/// read — absent, torn, short, an older layout, or a length/charset the encoder would never have
/// written. `None` is **no preference**, so every failure lands on "load the first valid map",
/// which is exactly the pre-#927 behaviour.
pub fn decode_selected_map(bytes: &[u8]) -> Option<&str> {
    if bytes.len() < SELECTED_MAP_LEN {
        return None;
    }
    let b = &bytes[..SELECTED_MAP_LEN];
    if b[0..4] != SELECTED_MAP_MAGIC || b[4] != SELECTED_MAP_VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[SELECTED_MAP_PAYLOAD], b[SELECTED_MAP_PAYLOAD + 1]]);
    if crc != crc16(&b[0..SELECTED_MAP_PAYLOAD]) {
        return None;
    }
    let len = b[5] as usize;
    if len == 0 || len > SELECTED_MAP_NAME_MAX {
        return None;
    }
    let name = &b[6..6 + len];
    if !name.iter().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    core::str::from_utf8(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 16-byte id-marks line round-trips, and every torn/blank/foreign shape decodes to
    /// `None` — "no floor", the fall-back-to-scan-max behaviour.
    #[test]
    fn id_marks_codec_round_trips_and_rejects_torn_lines() {
        let m = IdMarks { next_route_id: 7 };
        assert_eq!(decode_id_marks(&encode_id_marks(&m)), Some(m));
        assert_eq!(decode_id_marks(&encode_id_marks(&IdMarks::default())), Some(IdMarks::default()));

        assert_eq!(decode_id_marks(&[0u8; ID_MARKS_LEN]), None, "a blank (all-zero) line is no floor");
        assert_eq!(decode_id_marks(&[0xFF; ID_MARKS_LEN]), None, "an erased (all-ones) line is no floor");
        assert_eq!(decode_id_marks(&encode_id_marks(&m)[..ID_MARKS_LEN - 1]), None, "a short slice is rejected");
        let mut torn = encode_id_marks(&m);
        torn[7] ^= 0xFF; // flip a payload byte without fixing the CRC — the torn-write shape
        assert_eq!(decode_id_marks(&torn), None, "a CRC mismatch (torn write) is no floor");
        let mut old = encode_id_marks(&m);
        old[4] = ID_MARKS_VERSION + 1;
        let crc = crc16(&old[0..ID_MARKS_PAYLOAD]);
        old[ID_MARKS_PAYLOAD..ID_MARKS_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_id_marks(&old), None, "a foreign layout version is no floor");
    }

    /// The 12-byte store-epoch card file round-trips, and every torn/absent/foreign shape decodes to
    /// `None` — "no epoch", which the mint rule reads as clause 1.
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
        torn[7] ^= 0xFF; // flip an epoch byte without fixing the CRC — the torn-write shape
        assert_eq!(decode_store_epoch(&torn), None, "a CRC mismatch (torn write) is no epoch");
        let mut old = encode_store_epoch(0xDEAD_BEEF);
        old[4] = STORE_EPOCH_VERSION + 1;
        let crc = crc16(&old[0..STORE_EPOCH_PAYLOAD]);
        old[STORE_EPOCH_PAYLOAD..STORE_EPOCH_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_store_epoch(&old), None, "a foreign layout version is no epoch");
    }

    /// The 24-byte selected-map card file round-trips every name the device can write, and every
    /// torn/absent/foreign/malformed shape decodes to `None` — "no preference", which the loader
    /// reads as "take the first valid map", i.e. exactly the pre-#927 behaviour.
    #[test]
    fn selected_map_codec_round_trips_and_falls_back_on_anything_else() {
        for name in ["MP1.OBM", "MP65535.OBM", "12345678.OBM", "A.O", "FREIBU~1.OBC"] {
            let encoded = encode_selected_map(name).expect("a legal 8.3 name encodes");
            assert_eq!(encoded.len(), SELECTED_MAP_LEN, "the file is a fixed 24 bytes");
            assert_eq!(decode_selected_map(&encoded), Some(name), "{name} round-trips");
        }

        // Refused rather than truncated: a truncated name would select a *different* file.
        assert_eq!(encode_selected_map(""), None, "an empty name is not a selection");
        assert_eq!(encode_selected_map("123456789.OBM"), None, "13 chars is past the 8.3 ceiling");
        assert_eq!(encode_selected_map("MP1 .OBM"), None, "a space is not a graphic 8.3 byte");
        assert_eq!(encode_selected_map("MÄP.OBM"), None, "a non-ASCII name is refused");

        let good = encode_selected_map("MP7.OBM").unwrap();
        assert_eq!(decode_selected_map(&[0u8; SELECTED_MAP_LEN]), None, "a blank file is no preference");
        assert_eq!(decode_selected_map(&[0xFF; SELECTED_MAP_LEN]), None, "an erased file is no preference");
        assert_eq!(decode_selected_map(&[]), None, "an absent (empty) file is no preference");
        assert_eq!(decode_selected_map(&good[..SELECTED_MAP_LEN - 1]), None, "a short slice is rejected");
        let mut torn = good;
        torn[7] ^= 0xFF; // flip a name byte without fixing the CRC — the torn-write shape
        assert_eq!(decode_selected_map(&torn), None, "a CRC mismatch (torn write) is no preference");
        let mut old = good;
        old[4] = SELECTED_MAP_VERSION + 1;
        let crc = crc16(&old[0..SELECTED_MAP_PAYLOAD]);
        old[SELECTED_MAP_PAYLOAD..SELECTED_MAP_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_selected_map(&old), None, "a foreign layout version is no preference");

        // A CRC-valid file whose declared length is impossible must not escape as a bogus name.
        let mut lying = good;
        lying[5] = (SELECTED_MAP_NAME_MAX + 1) as u8;
        let crc = crc16(&lying[0..SELECTED_MAP_PAYLOAD]);
        lying[SELECTED_MAP_PAYLOAD..SELECTED_MAP_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_selected_map(&lying), None, "an over-long declared length is rejected");
        let mut zero = good;
        zero[5] = 0;
        let crc = crc16(&zero[0..SELECTED_MAP_PAYLOAD]);
        zero[SELECTED_MAP_PAYLOAD..SELECTED_MAP_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_selected_map(&zero), None, "a zero-length name is no preference");
    }

    /// The mint rule's four cases, plus the two invariants the 2026-07-12 review added. `FRESH` is
    /// the TRNG word the board draws; the pure function never draws it, so the test is deterministic.
    #[test]
    fn store_epoch_mint_rule() {
        const FRESH: u32 = 0x1234_5678;
        let floor = IdMarks { next_route_id: 9 };

        // Steady state: a valid card epoch + valid floors → keep the card's epoch, write nothing.
        assert_eq!(store_epoch_mint(Some(0xABCD), Some(floor), FRESH), None);

        // Clause 1 only (card epoch absent/torn, id-marks *intact*): mint a fresh epoch but keep the
        // existing floors verbatim — a torn/absent epoch file must never cost the durable id floor.
        assert_eq!(store_epoch_mint(None, Some(floor), FRESH), Some((FRESH, floor)));

        // Clause 2 (id-marks blank/torn, card epoch intact): a lost floor is a new era → mint a fresh
        // epoch even though the card's was valid, and (re)seed the floor to "no floor" (default),
        // which the store re-derives from the card scan via `max(scan_max + 1, floor)`.
        assert_eq!(store_epoch_mint(Some(0xABCD), None, FRESH), Some((FRESH, IdMarks::default())));

        // Fresh device (no card epoch + no floor): mint + seed default floors.
        assert_eq!(store_epoch_mint(None, None, FRESH), Some((FRESH, IdMarks::default())));
    }

    /// Card-swap semantics — the whole point of #776 (a pure-function pin). The epoch now rides the
    /// card, so swapping cards **transplants** the store identity with no mint, and swapping back
    /// restores the original era. The device's own RRAM floor stays intact throughout (a card swap is
    /// never an era event by itself — clause 2 is only about a *lost* floor).
    #[test]
    fn store_epoch_card_swap_transplants_the_era() {
        const FRESH: u32 = 0xDEAD_0001; // never consumed: every step below is a "keep"
        let floor = IdMarks { next_route_id: 3 };
        let e_a = 0xAAAA_1111u32; // card A's epoch
        let e_b = 0xBBBB_2222u32; // card B's epoch

        // Card A mounted, steady state → no mint, the served epoch is card A's.
        assert_eq!(store_epoch_mint(Some(e_a), Some(floor), FRESH), None, "card A steady: no mint");

        // Swap to card B (its own valid epoch, RRAM floor unchanged) → no mint, the store transplants
        // to card B's era. The served epoch is now e_b — a *different* store identity on the wire.
        assert_eq!(store_epoch_mint(Some(e_b), Some(floor), FRESH), None, "card B adopted verbatim — transplant");

        // Swap back to card A → no mint again, e_a served. The original era is restored intact.
        assert_eq!(store_epoch_mint(Some(e_a), Some(floor), FRESH), None, "swap-back restores card A's era");
    }

    /// The invariant *valid epoch ⇒ valid id-marks at mint*: after a mint the caller persists both
    /// (the epoch to the card file, the marks to the RRAM line), and a re-decode of what it wrote
    /// leaves **both** valid — so the next boot can't mistake the fresh state for a torn one.
    #[test]
    fn store_epoch_mint_writes_a_valid_marks_line() {
        const FRESH: u32 = 0x0BAD_F00D;
        // Clause-2 mint (blank id-marks + intact card epoch), the review's headline case.
        let (new_epoch, new_marks) = store_epoch_mint(Some(0x55), None, FRESH).expect("clause 2 mints");
        // Persist-then-reload both records exactly as the board does.
        assert_eq!(decode_store_epoch(&encode_store_epoch(new_epoch)), Some(FRESH), "epoch file valid post-mint");
        assert_eq!(decode_id_marks(&encode_id_marks(&new_marks)), Some(new_marks), "id-marks line valid post-mint");
    }

    /// Fresh-device stability: a device that never saves a ride or uploads a route mints **once**,
    /// and every subsequent boot (its epoch file + id-marks line now valid) keeps that same epoch —
    /// no clause-2 churn.
    #[test]
    fn store_epoch_fresh_device_stability() {
        const FRESH: u32 = 0xFEED_BEEF;
        // Boot 1: no card epoch + no floor → mint.
        let (epoch, marks) = store_epoch_mint(None, None, FRESH).expect("first boot mints");
        // The board writes both records; model them as the encoded card file + RRAM line it persisted.
        let epoch_line = encode_store_epoch(epoch);
        let marks_line = encode_id_marks(&marks);

        // Boots 2..N with no route allocations: both read back valid, so the decision is "keep" —
        // a *different* TRNG word each boot is irrelevant because the function never reaches it.
        for boot_fresh in [0x1111_1111u32, 0x2222_2222, 0x3333_3333] {
            let e = decode_store_epoch(&epoch_line);
            let m = decode_id_marks(&marks_line);
            assert_eq!(store_epoch_mint(e, m, boot_fresh), None, "a settled fresh device never re-mints");
        }
        assert_eq!(decode_store_epoch(&epoch_line), Some(epoch), "and the epoch is stable across boots");
    }

    /// The remaining FAT route floor never reuses a deleted filename id. Rides are intentionally
    /// absent: their full-width ids come from the flat catalog cursor.
    #[test]
    fn id_allocation_never_reuses_after_delete() {
        let mut card: heapless::Vec<u16, 8> = heapless::Vec::new(); // live RT{id} files
        let mut marks = IdMarks::default(); // fresh device: no floor
        let scan_next = |card: &[u16]| card.iter().max().map_or(0, |m| m + 1);

        // Three routes saved: 0, 1, 2 — identical to scan-max+1 while nothing deletes.
        for want in 0..3u16 {
            let id = marks.alloc_route(scan_next(&card));
            assert_eq!(id, want);
            let _ = card.push(id);
        }

        // Delete the highest (id 2) — the trap: scan-max+1 alone would re-issue 2.
        card.retain(|&id| id != 2);
        // "Reboot": the floor survives in RRAM (marks kept), the scan is rebuilt from the card.
        let mut rebooted = decode_id_marks(&encode_id_marks(&marks)).expect("persisted floor survives");
        let id = rebooted.alloc_route(scan_next(&card));
        assert_eq!(id, 3, "the deleted id 2 is never reused");
        let _ = card.push(id);

        // A torn floor line falls back cleanly: allocation degrades to scan-max+1 (no floor) —
        // ids can collide with tombstones again, but only exactly as they did before the marks.
        let mut torn = encode_id_marks(&rebooted);
        torn[7] ^= 0x55;
        let mut no_floor = decode_id_marks(&torn).unwrap_or_default();
        assert_eq!(no_floor.alloc_route(scan_next(&card)), 4, "torn line → scan-max+1");
    }
}
