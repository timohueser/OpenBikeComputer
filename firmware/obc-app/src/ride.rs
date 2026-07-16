//! Rides — the recorded rides shown in the Rides screen (epic #447, P7 / #454).
//!
//! A stored ride (`/tracks/RD{id}.ORD` on the device) is described to the UI by a [`RideSummary`]:
//! the same header facts the BLE `rideList` serves ([`obc_route::RideInfo`]) — name + start time +
//! the ride totals — with no track geometry (the Rides screen is **see + delete**, not a browser).
//! The **catalog** of summaries is produced by the host (the firmware scans `/tracks`, the
//! simulator scans its tracks folder) and handed to [`App::set_rides`](crate::App::set_rides); the
//! app owns a copy and the Rides screen reads it through [`Render`](crate::screen::Render).
//!
//! Each summary carries a `synced` flag — whether the phone has downloaded this ride at least once
//! (the "unsynced guard", locked option b). The device persists the synced-id set in an SD sidecar
//! (`/tracks/SYNCED.SET`, see [`synced_rides`](crate::settings)); the host stamps the flag onto each
//! summary as it builds the catalog. An unsynced ride's delete footer renders warning-red with a
//! "not synced" cue — still deletable, just informed.

use heapless::String;

use obc_formats::obcr::NAME_CAP;
use obc_route::RideInfo;

/// Maximum rides the **store** tracks — the synced-set sidecar's id capacity and the BLE
/// `rideList`'s sizing peer (the board's wire cap is its own 128). Storage-side only: the resident
/// menu catalog is deliberately smaller ([`UI_RIDES_CAP`]).
pub const MAX_RIDES: usize = 128;

/// Maximum rides the **resident** menu catalog holds — the newest [`UI_RIDES_CAP`] of however many
/// the card stores. Deliberately far below [`MAX_RIDES`]: a resident 128-ride catalog (plus the
/// board's parallel filename/id tables) cost ~8 KB of `.bss`, and on the 256 KB part stack and
/// statics are zero-sum — that growth ate the deep-render path's ~1.6 KB stack margin and
/// hard-faulted at boot (#454 review). The Rides screen is see-and-delete, not an archive browser;
/// 32 newest rows cover it, and the cap can relax on the 512 KB LM20.
pub const UI_RIDES_CAP: usize = 32;

/// The app's resident ride catalog: the summaries the Rides screen lists (newest first, capped at
/// [`UI_RIDES_CAP`]).
pub type RideCatalog = heapless::Vec<RideSummary, UI_RIDES_CAP>;

/// A stored ride's header facts for the Rides screen — the `rideList` header without the track
/// points, plus the device-local `synced` flag the unsynced-delete guard keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideSummary {
    /// Ride name (truncated to [`NAME_CAP`] on a char boundary), for the row's first line.
    pub name: String<NAME_CAP>,
    /// Ride start, unix seconds — the row's date line and the list sort key.
    pub start_time: u32,
    /// Ridden distance, metres — the compact stats line.
    pub distance_m: u32,
    /// Moving time, seconds — the compact stats line.
    pub moving_time_s: u32,
    /// Total ascent, metres — the compact stats line.
    pub climb_m: u16,
    /// Whether the phone has downloaded this ride at least once. `false` (not synced) renders the
    /// warning-red delete footer with the "not synced" cue; the ride is still deletable.
    pub synced: bool,
    /// When this ride was first verifiably synced to the phone, as UTC unix seconds — `0` means
    /// unstamped (never synced, or a legacy sidecar written before `synced_at` existed). The
    /// auto-expiry sweep (epic #638, S3) deletes a ride only once `now ≥ synced_at + ride_retention`,
    /// and a synced ride with `synced_at == 0` is stamped `now` (the countdown starts) rather than
    /// deleted on sight — so a legacy synced ride is never surprise-deleted.
    pub synced_at_utc: u32,
}

impl RideSummary {
    /// Build a summary from a stored ride's [`RideInfo`] header, its device-local synced flag, and
    /// its `synced_at` UTC stamp (`0` when unsynced or unstamped — see
    /// [`synced_at_utc`](RideSummary::synced_at_utc)).
    pub fn from_info(info: &RideInfo, synced: bool, synced_at_utc: u32) -> Self {
        RideSummary {
            name: info.name.clone(),
            start_time: info.start_time,
            distance_m: info.distance_m,
            moving_time_s: info.moving_time_s,
            climb_m: info.climb_m,
            synced,
            synced_at_utc,
        }
    }
}

// ==================== synced-ride sidecar (#454) ====================
//
// The unsynced-ride delete guard (epic #447 P7, locked option b): the device records "the phone
// has downloaded this ride at least once" per ride, set when a ride-object download **completes**.
// It's persisted in a small SD **sidecar file in /tracks** (`SYNCED.SET`) so it survives a reflash
// and travels with the card/rides — deliberately *not* the RRAM settings carve (which a reflash may
// wipe and which doesn't move with the card). The Rides screen renders an unsynced ride's delete
// footer warning-red with a "not synced" cue; a synced ride gets the standard footer.
//
// The codec lives here — beside the id-marks + settings codecs, the established host-testable
// precedent — so the "torn/missing sidecar = nothing synced, never a crash" contract is unit-tested
// without the board crate. The format is a magic + version + a `u16` count + that many entries + a
// trailing CRC-16 over everything before it. A blank page, a short slice, a torn write, or an
// unknown version all decode to the **empty** set — which reads as "nothing synced", the safe
// default (every ride shows the warning footer, all deletable).
//
// **v2 (epic #638, S3)** grows each entry from a bare `u16` id to `id u16 · synced_at u32` — the
// UTC instant the ride was first synced, the auto-expiry sweep's countdown anchor. A **v1** sidecar
// (id-only) still decodes: every entry reads `synced_at = 0` ("legacy synced", which the sweep
// stamps `now` rather than deleting on sight), so upgrading a device never surprise-deletes a ride.

/// The sidecar magic tag; anything else there decodes to the empty synced set.
const SYNCED_MAGIC: [u8; 4] = *b"OBCS";
/// The v1 layout version — id-only entries (2 bytes each). Still decoded (with `synced_at = 0`) for
/// backward compatibility; never written.
const SYNCED_VERSION_V1: u8 = 1;
/// The current layout version — `id u16 · synced_at u32` entries (6 bytes each).
const SYNCED_VERSION: u8 = 2;
/// Fixed header bytes before the entry list: `magic(4) · version(1) · pad(1) · count u16 LE`.
const SYNCED_HEADER_LEN: usize = 8;
/// Bytes per v2 entry: `id u16 LE · synced_at u32 LE`.
const SYNCED_ENTRY_LEN: usize = 6;

/// One synced ride: its id and the UTC instant it was first synced (`0` = legacy unstamped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncedRide {
    id: u16,
    synced_at: u32,
}

/// The persisted set of rides the phone has downloaded at least once, each with its `synced_at`
/// stamp (epic #638). Bounded by [`MAX_RIDES`](MAX_RIDES) (a ride can only be synced if it's
/// stored). `Default` is the empty set — "nothing synced".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncedRides {
    rides: heapless::Vec<SyncedRide, { MAX_RIDES }>,
}

impl SyncedRides {
    /// An empty synced set.
    pub fn new() -> Self {
        SyncedRides::default()
    }

    /// Whether ride `id` has been downloaded at least once.
    pub fn contains(&self, id: u16) -> bool {
        self.rides.iter().any(|r| r.id == id)
    }

    /// Ride `id`'s `synced_at` UTC stamp, or `0` when unsynced or legacy-unstamped.
    pub fn synced_at(&self, id: u16) -> u32 {
        self.rides.iter().find(|r| r.id == id).map(|r| r.synced_at).unwrap_or(0)
    }

    /// Record ride `id` as synced at `synced_at` (UTC unix seconds). Returns `true` if it was newly
    /// added (so the caller only rewrites the sidecar on an actual change). Idempotent: an
    /// already-synced ride keeps its **original** stamp (sync time is first-sync, not last), and a
    /// full set silently ignores a new id.
    pub fn insert(&mut self, id: u16, synced_at: u32) -> bool {
        if self.contains(id) {
            return false;
        }
        self.rides.push(SyncedRide { id, synced_at }).is_ok()
    }

    /// Stamp a **present** ride's `synced_at` — the sweep's "start the countdown on a legacy
    /// synced-without-stamp ride" path. Only ever moves a `0` stamp forward (never re-stamps an
    /// already-stamped ride), so the countdown anchor is stable. Returns whether the set changed.
    pub fn stamp_synced_at(&mut self, id: u16, synced_at: u32) -> bool {
        if let Some(r) = self.rides.iter_mut().find(|r| r.id == id && r.synced_at == 0) {
            r.synced_at = synced_at;
            true
        } else {
            false
        }
    }

    /// Drop ride `id` from the synced set (a deleted ride's id is retired so a later scan doesn't
    /// carry a stale flag — though ids never reuse, so this is belt-and-braces). Returns `true` if it
    /// was present.
    pub fn remove(&mut self, id: u16) -> bool {
        if let Some(pos) = self.rides.iter().position(|r| r.id == id) {
            self.rides.swap_remove(pos);
            true
        } else {
            false
        }
    }

    /// The synced ids, for tests.
    pub fn ids(&self) -> impl Iterator<Item = u16> + '_ {
        self.rides.iter().map(|r| r.id)
    }

    /// How many rides are synced.
    pub fn len(&self) -> usize {
        self.rides.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.rides.is_empty()
    }
}

/// The encoded (v2) sidecar's byte length for `count` synced entries: the fixed header, the entry
/// list, then the trailing CRC-16.
pub const fn synced_rides_len(count: usize) -> usize {
    SYNCED_HEADER_LEN + count * SYNCED_ENTRY_LEN + 2
}

/// The largest an encoded sidecar can be (a full synced set) — the buffer a host reserves to write it.
pub const SYNCED_RIDES_MAX_LEN: usize = synced_rides_len(MAX_RIDES);

/// Pack the synced-ride set into `out` (v2), returning the encoded byte length. `out` must be at
/// least [`synced_rides_len`]`(set.len())` (use a [`SYNCED_RIDES_MAX_LEN`] buffer). Inverse of
/// [`decode_synced_rides`].
pub fn encode_synced_rides(set: &SyncedRides, out: &mut [u8]) -> usize {
    let len = synced_rides_len(set.rides.len());
    out[0..4].copy_from_slice(&SYNCED_MAGIC);
    out[4] = SYNCED_VERSION;
    out[5] = 0;
    out[6..8].copy_from_slice(&(set.rides.len() as u16).to_le_bytes());
    for (i, r) in set.rides.iter().enumerate() {
        let o = SYNCED_HEADER_LEN + i * SYNCED_ENTRY_LEN;
        out[o..o + 2].copy_from_slice(&r.id.to_le_bytes());
        out[o + 2..o + 6].copy_from_slice(&r.synced_at.to_le_bytes());
    }
    let crc = crate::store_meta::crc16(&out[..len - 2]);
    out[len - 2..len].copy_from_slice(&crc.to_le_bytes());
    len
}

/// Decode a synced-ride sidecar (v2 or the legacy v1), always returning a set — a blank page, a
/// short slice, a torn write, an unknown version, a count that overruns the slice, or a CRC mismatch
/// all yield the **empty** set ("nothing synced", the safe default). A **v1** sidecar decodes with
/// every entry's `synced_at = 0` (legacy — the sweep stamps rather than deletes). Never panics.
pub fn decode_synced_rides(bytes: &[u8]) -> SyncedRides {
    let empty = SyncedRides::new();
    if bytes.len() < SYNCED_HEADER_LEN + 2 {
        return empty; // shorter than an empty-set sidecar → treat as absent
    }
    if bytes[0..4] != SYNCED_MAGIC {
        return empty;
    }
    let (entry_len, has_stamp) = match bytes[4] {
        SYNCED_VERSION => (SYNCED_ENTRY_LEN, true),
        SYNCED_VERSION_V1 => (2, false),
        _ => return empty,
    };
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let len = SYNCED_HEADER_LEN + count * entry_len + 2;
    if count > MAX_RIDES || bytes.len() < len {
        return empty; // a count that claims more entries than the slice (or the cap) holds is corrupt
    }
    let crc = u16::from_le_bytes([bytes[len - 2], bytes[len - 1]]);
    if crc != crate::store_meta::crc16(&bytes[..len - 2]) {
        return empty;
    }
    let mut set = SyncedRides::new();
    for i in 0..count {
        let o = SYNCED_HEADER_LEN + i * entry_len;
        let id = u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let synced_at =
            if has_stamp { u32::from_le_bytes([bytes[o + 2], bytes[o + 3], bytes[o + 4], bytes[o + 5]]) } else { 0 };
        let _ = set.insert(id, synced_at);
    }
    set
}

#[cfg(test)]
mod synced_rides_tests {
    use super::*;

    /// A synced set round-trips through the v2 sidecar codec — membership, exact ids, `synced_at`.
    #[test]
    fn synced_rides_codec_round_trips() {
        let mut set = SyncedRides::new();
        assert!(set.insert(3, 5_000));
        assert!(set.insert(7, 6_000));
        assert!(set.insert(41, 7_000));
        assert!(!set.insert(7, 9_999), "a duplicate insert is a no-op — keeps the first-sync stamp");
        assert_eq!(set.synced_at(7), 6_000, "the original stamp is preserved");
        assert!(set.contains(3) && set.contains(7) && set.contains(41));
        assert!(!set.contains(4));
        assert_eq!(set.synced_at(4), 0, "absent → 0");

        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(&set, &mut buf);
        assert_eq!(n, synced_rides_len(3));
        let got = decode_synced_rides(&buf[..n]);
        assert_eq!(got, set);

        // The empty set is a valid, non-crashing round-trip too.
        let empty = SyncedRides::new();
        let n = encode_synced_rides(&empty, &mut buf);
        assert_eq!(decode_synced_rides(&buf[..n]), empty);
    }

    /// A legacy **v1** (id-only) sidecar still decodes — every ride reads `synced_at = 0` ("legacy
    /// synced"), so upgrading a device never surprise-deletes a ride; the sweep stamps it instead.
    #[test]
    fn synced_rides_v1_decodes_with_zero_stamp() {
        // Forge a v1 sidecar by hand: header + two id-only entries + CRC.
        let ids = [9u16, 12];
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(&SYNCED_MAGIC);
        buf[4] = SYNCED_VERSION_V1;
        buf[6..8].copy_from_slice(&(ids.len() as u16).to_le_bytes());
        for (i, &id) in ids.iter().enumerate() {
            let o = SYNCED_HEADER_LEN + i * 2;
            buf[o..o + 2].copy_from_slice(&id.to_le_bytes());
        }
        let len = SYNCED_HEADER_LEN + ids.len() * 2 + 2;
        let crc = crate::store_meta::crc16(&buf[..len - 2]);
        buf[len - 2..len].copy_from_slice(&crc.to_le_bytes());

        let set = decode_synced_rides(&buf[..len]);
        assert!(set.contains(9) && set.contains(12), "v1 ids decode");
        assert_eq!(set.synced_at(9), 0, "v1 has no stamp → 0 (legacy synced)");
    }

    /// `stamp_synced_at` starts a legacy ride's countdown, but never re-stamps an already-stamped one.
    #[test]
    fn synced_rides_stamp_starts_legacy_countdown_once() {
        let mut set = SyncedRides::new();
        set.insert(1, 0); // legacy synced, unstamped
        set.insert(2, 5_000); // already stamped
        assert!(set.stamp_synced_at(1, 8_000), "legacy ride gets its countdown started");
        assert_eq!(set.synced_at(1), 8_000);
        assert!(!set.stamp_synced_at(1, 9_000), "a stamped ride is never re-stamped");
        assert_eq!(set.synced_at(1), 8_000);
        assert!(!set.stamp_synced_at(2, 9_000), "an already-stamped ride is untouched");
        assert_eq!(set.synced_at(2), 5_000);
    }

    /// The DoD guarantee: a torn, blank, short, or foreign sidecar decodes to "nothing synced" —
    /// never a crash, never a false positive that would drop the warning footer on an unsynced ride.
    #[test]
    fn synced_rides_torn_or_missing_reads_as_nothing_synced() {
        let mut set = SyncedRides::new();
        set.insert(9, 100);
        set.insert(12, 200);
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(&set, &mut buf);

        assert_eq!(decode_synced_rides(&[]), SyncedRides::new(), "an absent sidecar → nothing synced");
        assert_eq!(decode_synced_rides(&[0u8; 4]), SyncedRides::new(), "a runt slice → nothing synced");
        assert_eq!(decode_synced_rides(&[0u8; SYNCED_HEADER_LEN + 2]), SyncedRides::new(), "a blank page");
        assert_eq!(decode_synced_rides(&[0xFF; 64]), SyncedRides::new(), "an erased page → nothing synced");

        let mut torn = buf;
        torn[SYNCED_HEADER_LEN] ^= 0xFF; // flip an id byte without fixing the CRC
        assert_eq!(decode_synced_rides(&torn[..n]), SyncedRides::new(), "a CRC mismatch → nothing synced");

        let mut bad_count = buf;
        bad_count[6..8].copy_from_slice(&0xFFFFu16.to_le_bytes()); // claim more ids than the slice holds
        assert_eq!(decode_synced_rides(&bad_count[..n]), SyncedRides::new(), "an overrunning count → nothing");

        let mut old = buf;
        old[4] = SYNCED_VERSION + 1;
        assert_eq!(decode_synced_rides(&old[..n]), SyncedRides::new(), "a foreign version → nothing synced");
    }

    /// `remove` retires an id (the deleted-ride cleanup) without disturbing the rest.
    #[test]
    fn synced_rides_remove_retires_one_id() {
        let mut set = SyncedRides::new();
        set.insert(1, 10);
        set.insert(2, 20);
        set.insert(3, 30);
        assert!(set.remove(2));
        assert!(!set.remove(2), "removing an absent id is a no-op");
        assert!(set.contains(1) && !set.contains(2) && set.contains(3));
    }
}
