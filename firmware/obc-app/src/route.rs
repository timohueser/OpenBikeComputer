//! Routes — the loadable rides shown in the Route menu.
//!
//! A route is described to the UI by a [`RouteSummary`] (name + totals + bbox +
//! start), defined by the [`obc_route`] format crate. The **catalog** of summaries is
//! produced by the host — the simulator scans a folder of `.obcr` files, the firmware
//! scans the SD card — and handed to [`App::set_routes`](crate::App::set_routes); the
//! app owns a copy and the screens read it through [`Ctx`](crate::screen::Ctx) /
//! [`Render`](crate::screen::Render). The heavy route *geometry* (the polyline the Map
//! draws) stays host-owned and is streamed on demand through an
//! [`obc_route::RouteReader`]; only the one active route is opened at a time.
//!
//! [`Activity::active_route`](crate::Activity::active_route) indexes into the catalog.

use obc_render::{OverlayChunk, RouteOverlaySource};
use obc_route::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

pub use obc_route::RouteSummary;

/// Maximum routes the resident menu catalog holds. Each summary is ~80 bytes, so the cap costs a
/// few KB of static RAM — the cap also sizes the board's parallel filename/id tables and the boot
/// scan's stack-side catalog, so each slot pays roughly twice. 64 stays far past any sane card
/// (the board's `scan_routes` warns and lists the first 64 if a card somehow exceeds it).
pub const MAX_ROUTES: usize = 64;

/// The app's resident route catalog: the summaries the Route menu lists and
/// [`Activity::active_route`](crate::Activity::active_route) indexes.
pub type Catalog = heapless::Vec<RouteSummary, MAX_ROUTES>;

/// The route-overlay seam adapter (issue #332): presents a [`RouteReader`] to the renderer as
/// [`obc_render::RouteOverlaySource`] — chunked `(lon, lat)` microdegree polylines with per-chunk
/// bbox + cumulative distance — so `obc-render` never depends on the OBCR format. A zero-cost
/// wrapper (the orphan rule forbids implementing the foreign trait on the foreign reader directly).
pub struct RouteOverlay<'a, 'b>(pub &'a RouteReader<'b>);

/// Decode chunk `k` into `(lon, lat)` pairs. Split out `#[inline(never)]` so the
/// `RoutePoint` decode scratch (~3 KB, the same buffer `draw_route` used to keep in its own
/// frame) lives in a frame that is **popped before** `visit` descends into the deep
/// stroke/fill path — the measured stack peak on the 256 KB DK must not grow.
#[inline(never)]
fn decode_lonlat(rr: &RouteReader, k: usize, out: &mut [(i32, i32); MAX_POINTS_PER_CHUNK]) -> Option<usize> {
    let mut pts = heapless::Vec::<RoutePoint, MAX_POINTS_PER_CHUNK>::new();
    rr.decode_chunk(k, &mut pts).ok()?;
    for (dst, p) in out.iter_mut().zip(pts.iter()) {
        *dst = (p.lon, p.lat);
    }
    Some(pts.len())
}

impl RouteOverlaySource for RouteOverlay<'_, '_> {
    fn chunk_count(&self) -> usize {
        self.0.chunks().len()
    }

    fn chunk(&self, k: usize) -> OverlayChunk {
        let cm = &self.0.chunks()[k];
        OverlayChunk { bbox: cm.bbox, cum_distance_m: cm.cum_distance_m }
    }

    fn total_distance_m(&self) -> u32 {
        self.0.total_distance_m
    }

    fn visit_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        // Stack, not heap (`no_std`): a 2 KB `(lon, lat)` staging array in this frame; the
        // `RoutePoint` decode scratch lives (and dies) in `decode_lonlat`'s frame. A failed
        // decode (flaky SD) skips `visit`, per the trait contract.
        let mut ll = [(0i32, 0i32); MAX_POINTS_PER_CHUNK];
        if let Some(n) = decode_lonlat(self.0, k, &mut ll) {
            visit(&ll[..n]);
        }
    }
}

// ==================== route-CRC sidecar (#632 item 6, V2) ====================
//
// The route-identity content fingerprint (epic #632 item 6, device half): the whole-object CRC-32
// of each stored route's OBCR bytes, keyed by durable object id, so the `routeList` entry can carry
// it and the app can verify *what* a linked id points at (identity-verified badges) and adopt an
// identical unlinked copy. Persisted in a small SD **sidecar in /routes** (`ROUTES.CRC`) so it
// survives a reflash and travels with the card/routes, and is *not* the RRAM settings carve. A BLE upload writes the entry
// at commit (the CRC is already verified there); a side-loaded / pre-v2 route with no entry is filled
// lazily at first list build (one streaming CRC pass, then persisted).
//
// The codec lives here so the "torn/missing sidecar = empty map, never a crash" contract is
// unit-tested without the board crate: a magic + version + a `u16` count + that many
// `(id u16, crc32 u32)` little-endian pairs + a trailing CRC-16 over everything before it. A blank
// page, a short slice, a torn write, an unknown version, an overrunning count, or a CRC mismatch all
// decode to the **empty** map — which serves `0 = unknown` for every route (the safe default; the
// device then re-fills lazily).

/// The sidecar magic tag; anything else there decodes to the empty CRC map.
const ROUTE_CRCS_MAGIC: [u8; 4] = *b"ORCS";
/// Sidecar layout version — bump on any format change (an old version reads as empty).
const ROUTE_CRCS_VERSION: u8 = 1;
/// Fixed header bytes before the entry list: `magic(4) · version(1) · pad(1) · count u16 LE`.
const ROUTE_CRCS_HEADER_LEN: usize = 8;
/// One `(id u16 LE, crc32 u32 LE)` entry.
const ROUTE_CRCS_ENTRY_LEN: usize = 6;

/// The persisted map of route object id → whole-object CRC-32. Bounded by
/// [`MAX_ROUTES`](MAX_ROUTES) (a CRC can only exist for a cataloged route). `Default`
/// is the empty map — "no CRC known for any route".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteCrcs {
    entries: heapless::Vec<(u16, u32), { MAX_ROUTES }>,
}

impl RouteCrcs {
    /// An empty CRC map.
    pub fn new() -> Self {
        RouteCrcs::default()
    }

    /// The stored whole-object CRC-32 for route `id`, or `None` when the map has no entry for it
    /// (the caller then lazily fills it). Note a genuine CRC of `0` is a legal value stored and
    /// returned as `Some(0)` — it is only ever *served* on the wire as `0 = unknown`, never
    /// special-cased here.
    pub fn get(&self, id: u16) -> Option<u32> {
        self.entries.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
    }

    /// Upsert the CRC for route `id`. Returns `true` when the map changed (a new entry, or an
    /// existing entry whose CRC differs) so the caller only rewrites the sidecar on an actual
    /// change. A full map silently ignores a brand-new id.
    pub fn insert(&mut self, id: u16, crc: u32) -> bool {
        if let Some(slot) = self.entries.iter_mut().find(|(i, _)| *i == id) {
            if slot.1 == crc {
                return false;
            }
            slot.1 = crc;
            return true;
        }
        self.entries.push((id, crc)).is_ok()
    }

    /// Retire route `id`'s CRC entry (a deleted route — ids never reuse, so this is belt-and-braces
    /// tidiness). Returns `true` if it was present.
    pub fn remove(&mut self, id: u16) -> bool {
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == id) {
            self.entries.swap_remove(pos);
            true
        } else {
            false
        }
    }

    /// The `(id, crc)` entries, for the codec / tests.
    pub fn entries(&self) -> &[(u16, u32)] {
        &self.entries
    }
}

/// The encoded sidecar's byte length for `count` entries: the fixed header, the entry list, then the
/// trailing CRC-16.
pub const fn route_crcs_len(count: usize) -> usize {
    ROUTE_CRCS_HEADER_LEN + count * ROUTE_CRCS_ENTRY_LEN + 2
}

/// The largest an encoded sidecar can be (a full map) — the buffer a host reserves to write it.
pub const ROUTE_CRCS_MAX_LEN: usize = route_crcs_len(MAX_ROUTES);

/// Pack the route-CRC map into `out`, returning the encoded byte length. `out` must be at least
/// [`route_crcs_len`]`(map.entries().len())` (use a [`ROUTE_CRCS_MAX_LEN`] buffer). Inverse of
/// [`decode_route_crcs`].
pub fn encode_route_crcs(map: &RouteCrcs, out: &mut [u8]) -> usize {
    let entries = map.entries();
    let len = route_crcs_len(entries.len());
    out[0..4].copy_from_slice(&ROUTE_CRCS_MAGIC);
    out[4] = ROUTE_CRCS_VERSION;
    out[5] = 0;
    out[6..8].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    for (i, (id, crc)) in entries.iter().enumerate() {
        let o = ROUTE_CRCS_HEADER_LEN + i * ROUTE_CRCS_ENTRY_LEN;
        out[o..o + 2].copy_from_slice(&id.to_le_bytes());
        out[o + 2..o + 6].copy_from_slice(&crc.to_le_bytes());
    }
    let crc = crate::store_meta::crc16(&out[..len - 2]);
    out[len - 2..len].copy_from_slice(&crc.to_le_bytes());
    len
}

/// Decode a route-CRC sidecar, always returning a map — a blank page, a short slice, a torn write,
/// an unknown version, a count that overruns the slice (or the cap), or a CRC mismatch all yield the
/// **empty** map ("no CRC known", the safe default). Never panics on malformed input.
pub fn decode_route_crcs(bytes: &[u8]) -> RouteCrcs {
    let empty = RouteCrcs::new();
    if bytes.len() < ROUTE_CRCS_HEADER_LEN + 2 {
        return empty; // shorter than an empty-map sidecar → treat as absent
    }
    if bytes[0..4] != ROUTE_CRCS_MAGIC || bytes[4] != ROUTE_CRCS_VERSION {
        return empty;
    }
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let len = route_crcs_len(count);
    if count > MAX_ROUTES || bytes.len() < len {
        return empty; // a count claiming more entries than the slice (or the cap) holds is corrupt
    }
    let crc = u16::from_le_bytes([bytes[len - 2], bytes[len - 1]]);
    if crc != crate::store_meta::crc16(&bytes[..len - 2]) {
        return empty;
    }
    let mut map = RouteCrcs::new();
    for i in 0..count {
        let o = ROUTE_CRCS_HEADER_LEN + i * ROUTE_CRCS_ENTRY_LEN;
        let id = u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let c = u32::from_le_bytes([bytes[o + 2], bytes[o + 3], bytes[o + 4], bytes[o + 5]]);
        let _ = map.insert(id, c);
    }
    map
}

#[cfg(test)]
mod route_crcs_tests {
    use super::*;

    /// The route-CRC sidecar round-trips id → crc32 pairs byte-for-byte, empty included.
    #[test]
    fn route_crcs_codec_round_trips() {
        let mut map = RouteCrcs::new();
        assert!(map.insert(1, 0xDEAD_BEEF));
        assert!(map.insert(7, 0));
        assert!(map.insert(65535, 0x0000_0001));
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = encode_route_crcs(&map, &mut buf);
        assert_eq!(n, route_crcs_len(3));
        let got = decode_route_crcs(&buf[..n]);
        assert_eq!(got, map);
        assert_eq!(got.get(1), Some(0xDEAD_BEEF));
        assert_eq!(got.get(7), Some(0), "a genuine CRC of 0 is a stored, retrievable value");
        assert_eq!(got.get(2), None, "an unlisted route has no CRC (→ lazily filled)");

        let empty = RouteCrcs::new();
        let n = encode_route_crcs(&empty, &mut buf);
        assert_eq!(decode_route_crcs(&buf[..n]), empty);
    }

    /// A torn / missing / foreign route-CRC sidecar decodes to the empty map (serve `0 = unknown`).
    #[test]
    fn route_crcs_torn_or_missing_reads_as_empty() {
        let mut map = RouteCrcs::new();
        map.insert(9, 0x1111_2222);
        map.insert(12, 0x3333_4444);
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = encode_route_crcs(&map, &mut buf);

        assert_eq!(decode_route_crcs(&[]), RouteCrcs::new(), "an absent sidecar → empty");
        assert_eq!(decode_route_crcs(&[0u8; 4]), RouteCrcs::new(), "a runt slice → empty");
        assert_eq!(decode_route_crcs(&[0u8; ROUTE_CRCS_HEADER_LEN + 2]), RouteCrcs::new(), "a blank page");
        assert_eq!(decode_route_crcs(&[0xFF; 64]), RouteCrcs::new(), "an erased page → empty");

        let mut torn = buf;
        torn[ROUTE_CRCS_HEADER_LEN] ^= 0xFF; // flip an id byte without fixing the CRC
        assert_eq!(decode_route_crcs(&torn[..n]), RouteCrcs::new(), "a CRC mismatch → empty");

        let mut bad_count = buf;
        bad_count[6..8].copy_from_slice(&0xFFFFu16.to_le_bytes()); // claim more entries than the slice holds
        assert_eq!(decode_route_crcs(&bad_count[..n]), RouteCrcs::new(), "an overrunning count → empty");

        let mut old = buf;
        old[4] = ROUTE_CRCS_VERSION + 1;
        assert_eq!(decode_route_crcs(&old[..n]), RouteCrcs::new(), "a foreign version → empty");
    }

    /// `insert` upserts (a changed CRC rewrites, an identical one is a no-op) and `remove` retires
    /// one id — the upload-replace + delete cleanup paths.
    #[test]
    fn route_crcs_upsert_and_remove() {
        let mut map = RouteCrcs::new();
        assert!(map.insert(5, 0xAAAA), "a new id changes the map");
        assert!(!map.insert(5, 0xAAAA), "the same id+crc is a no-op");
        assert!(map.insert(5, 0xBBBB), "a replaced route's new crc rewrites in place");
        assert_eq!(map.get(5), Some(0xBBBB));
        assert_eq!(map.entries().len(), 1, "upsert never duplicates an id");
        assert!(map.remove(5));
        assert!(!map.remove(5), "removing an absent id is a no-op");
        assert_eq!(map.get(5), None);
    }
}
