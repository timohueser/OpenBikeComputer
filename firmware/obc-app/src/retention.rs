//! Route auto-expiry + ride auto-delete — the retention vocabulary, the SD **route-retention
//! sidecar** codec, and the portable **expiry sweep** (epic #638, S3 #643).
//!
//! Everything here is host-agnostic `no_std`: the same types drive the simulator, the board, and
//! the unit tests. The *policy* — what expires, what re-stamps, and when either may happen — has
//! exactly one owner, [`RetentionMachine`]; [`collect_sweep_actions`] is the pure discovery pass
//! inside it, so the safety invariants stay testable without hardware. The machine never deletes:
//! it emits a [`RetentionEffect`] for its own device-local sidecar writes and a
//! [`CatalogIntent`](crate::catalog_state::CatalogIntent) for every expiry, so an auto-expired
//! object leaves through exactly the path a rider-deleted one does and **no platform executor ever
//! decides whether an object is expired**.
//!
//! ## The safety core
//!
//! The device has no RTC. Nothing here ever runs unless the wall clock was established from a real
//! time source **this boot** ([`App::clock_trusted`](crate::App::clock_trusted)) — a stale or
//! fat-fingered clock can never drive a deletion. On top of that, the sweep holds the epic's seven
//! invariants (see [`collect_sweep_actions`]): the active route is never deleted (it re-stamps), an
//! unknown `last_used` is stamped rather than deleted, unsynced rides are never touched, and a
//! `Never` retention deletes nothing.
//!
//! ## Where the state lives
//!
//! Retention is mutable **device-local** state, never baked into the byte-pinned OBCR route file.
//! Per route it is a [`RouteRetentionMeta`] (a retention level + a `last_used` UTC stamp), carried
//! alongside the route catalog and persisted host-side in the [route-retention
//! sidecar](RouteRetentionStore) — the direct analogue of the ride synced-set sidecar
//! ([`SyncedRides`](crate::ride::SyncedRides)). A torn or absent sidecar decodes **empty** → every
//! route reads `Never` → nothing deletes (the safe direction; the app re-pushes retention at
//! reconcile in S7, so it self-heals).

use crate::ride::UI_RIDES_CAP;
use crate::route::MAX_ROUTES;

/// Seconds in a day — the retention arithmetic unit (`expires_at = last_used + days · DAY_SECS`).
pub const DAY_SECS: u32 = 86_400;

/// Per-route **retention level** — the shared wire/storage value (epic #638). A `u8` on the wire
/// (the `setRouteRetention` command, S4) and in the sidecar; **an unknown byte decodes to
/// [`Never`](Retention::Never)** so a forward-compat value can never surprise-delete a route.
///
/// The discriminants are a stable on-disk/on-wire contract — appended, never renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Retention {
    /// The route never auto-expires. The default for a pre-existing (pre-feature) route and for any
    /// unknown stored/wire byte — invariant 6: shipping this feature surprise-deletes nothing.
    #[default]
    Never = 0,
    /// Delete once unused for 1 day.
    Day1 = 1,
    /// Delete once unused for 1 week.
    Week1 = 2,
    /// Delete once unused for 2 weeks.
    Week2 = 3,
    /// Delete once unused for 1 month (30 days).
    Month1 = 4,
    /// Delete once unused for 2 months (60 days).
    Month2 = 5,
}

impl Retention {
    /// The retention window in **days**, or `None` for [`Never`](Retention::Never) (no expiry). The
    /// sweep multiplies by [`DAY_SECS`] for the UTC-seconds deadline.
    pub const fn days(self) -> Option<u32> {
        match self {
            Retention::Never => None,
            Retention::Day1 => Some(1),
            Retention::Week1 => Some(7),
            Retention::Week2 => Some(14),
            Retention::Month1 => Some(30),
            Retention::Month2 => Some(60),
        }
    }

    /// Rebuild from a stored/wire byte, sanitising an unknown value to [`Never`](Retention::Never)
    /// — the safe direction (an unrecognised level never deletes).
    pub const fn from_u8(b: u8) -> Self {
        match b {
            1 => Retention::Day1,
            2 => Retention::Week1,
            3 => Retention::Week2,
            4 => Retention::Month1,
            5 => Retention::Month2,
            _ => Retention::Never,
        }
    }

    /// The stored/wire byte.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// The device's **ride-retention setting** (epic #638): delete a ride this long after it was
/// verifiably synced to the phone. A deliberately **smaller** menu than [`Retention`] (the route
/// levels) — Never / 1 day / 1 week / 1 month — because it is one global stepper, not a per-object
/// choice. **Default 1 week.** S5 adds the settings screen; here it is the field, the default, the
/// codec byte, and the picker walk.
///
/// The discriminants are a stable on-disk contract — appended, never renumbered; an unknown byte
/// decodes to the default ([`Week1`](RideRetention::Week1)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RideRetention {
    /// Never auto-delete a synced ride (the rider prunes rides by hand).
    Never = 0,
    /// Delete 1 day after sync.
    Day1 = 1,
    /// Delete 1 week after sync — **the default**.
    Week1 = 2,
    /// Delete 1 month (30 days) after sync.
    Month1 = 3,
}

impl Default for RideRetention {
    /// **1 week** out of the box — long enough that a synced ride survives a normal review window,
    /// short enough that the Rides list cleans itself up over a season.
    fn default() -> Self {
        RideRetention::Week1
    }
}

impl RideRetention {
    /// The ordered picker values (the settings stepper's left/right walk order), shortest window to
    /// the longest, `Never` first.
    const ORDER: [RideRetention; 4] =
        [RideRetention::Never, RideRetention::Day1, RideRetention::Week1, RideRetention::Month1];

    /// The retention window in **days**, or `None` for [`Never`](RideRetention::Never).
    pub const fn days(self) -> Option<u32> {
        match self {
            RideRetention::Never => None,
            RideRetention::Day1 => Some(1),
            RideRetention::Week1 => Some(7),
            RideRetention::Month1 => Some(30),
        }
    }

    /// Rebuild from a stored byte, sanitising an unknown value to the default
    /// ([`Week1`](RideRetention::Week1)) — the decode-side clamp, exactly like the other settings
    /// enum fields.
    pub const fn from_u8(b: u8) -> Self {
        match b {
            0 => RideRetention::Never,
            1 => RideRetention::Day1,
            3 => RideRetention::Month1,
            _ => RideRetention::Week1,
        }
    }

    /// The stored byte.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Walk the picker `n` steps through [`ORDER`](RideRetention::ORDER), wrapping at both ends —
    /// the Auto-delete settings row's left/right value step (S5). Falls back to the default
    /// [`Week1`](RideRetention::Week1) index.
    pub fn stepped(self, n: i32) -> Self {
        let i = Self::ORDER.iter().position(|&v| v == self).unwrap_or(2);
        Self::ORDER[(i as i32 + n).rem_euclid(Self::ORDER.len() as i32) as usize]
    }
}

/// One route's device-local retention state: the [`Retention`] level and the UTC-unix-seconds
/// `last_used` stamp the expiry math anchors on. `Copy`, so the whole per-route column stays cheap
/// to carry alongside the route catalog.
///
/// `last_used == 0` means **unknown / never stamped** (invariant 2): the sweep starts the clock
/// (stamps `now`) instead of ever treating it as "used at the epoch" and deleting on sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteRetentionMeta {
    /// This route's retention level (default [`Never`](Retention::Never) — never expires).
    pub retention: Retention,
    /// When the route was last *used* — a route becoming the active nav route, an upload commit, or
    /// a sweep re-stamp — as UTC unix seconds. `0` = unknown (never stamped this device).
    pub last_used_utc: u32,
}

impl RouteRetentionMeta {
    /// A route with an explicit retention level and last-used stamp.
    pub const fn new(retention: Retention, last_used_utc: u32) -> Self {
        RouteRetentionMeta { retention, last_used_utc }
    }

    /// The UTC-seconds instant this route expires (`last_used + days · DAY_SECS`), or `None` when it
    /// never expires — a [`Never`](Retention::Never) level, or an **unknown** (`0`) `last_used`
    /// (invariant 2: unknown is stamped, never deleted). Saturating so a near-`u32::MAX` stamp can't
    /// wrap the deadline back into the past.
    pub const fn expires_at(self) -> Option<u32> {
        match self.retention.days() {
            Some(days) if self.last_used_utc != 0 => Some(self.last_used_utc.saturating_add(days * DAY_SECS)),
            _ => None,
        }
    }

    /// Whether this route is expired at `now_utc` — `now ≥ expires_at`, exact-boundary inclusive.
    /// Always `false` for `Never` and for an unknown (`0`) `last_used`.
    pub const fn is_expired(self, now_utc: u32) -> bool {
        match self.expires_at() {
            Some(deadline) => now_utc >= deadline,
            None => false,
        }
    }

    /// Whether this route has a retention level set but an **unknown** `last_used` (`0`) — the sweep
    /// stamps it `now` to start the clock (invariant 2), rather than deleting it.
    pub const fn needs_clock_started(self) -> bool {
        !matches!(self.retention, Retention::Never) && self.last_used_utc == 0
    }
}

// ==================== the route-retention SD sidecar ====================
//
// route object id → (retention, last_used), CRC-framed exactly like the ride synced-set sidecar
// ([`SyncedRides`](crate::ride)). The codec lives here (host-testable, off-target) so the
// "torn/absent → empty → nothing deletes" contract is unit-tested without the board crate; the
// board only does the file read/write. A blank page, a short slice, a torn write, an unknown
// version, or an overrunning count all decode to the **empty** store — every route reads `Never`,
// the safe default that deletes nothing and self-heals when the app re-pushes retention (S7).

/// The sidecar magic tag; anything else there decodes to the empty store.
const RET_MAGIC: [u8; 4] = *b"OBRR";
/// Sidecar layout version — bump on any format change (an old version reads as empty).
const RET_VERSION: u8 = 1;
/// Fixed header bytes before the entry list: `magic(4) · version(1) · pad(1) · count u16 LE`.
const RET_HEADER_LEN: usize = 8;
/// Bytes per entry: `id u16 LE · retention u8 · last_used u32 LE`.
const RET_ENTRY_LEN: usize = 7;

/// One persisted route-retention row: a durable route id and its [`RouteRetentionMeta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetEntry {
    id: u16,
    meta: RouteRetentionMeta,
}

/// The persisted route-retention set: route object id → [`RouteRetentionMeta`]. Bounded by
/// [`MAX_ROUTES`] (a retention can only exist for a stored route). `Default` is empty — every route
/// reads [`Never`](Retention::Never).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteRetentionStore {
    entries: heapless::Vec<RetEntry, MAX_ROUTES>,
}

impl RouteRetentionStore {
    /// An empty store — nothing has a retention set (every route reads `Never`).
    pub fn new() -> Self {
        RouteRetentionStore::default()
    }

    /// This route's retention meta, or the default ([`Never`](Retention::Never), `last_used = 0`)
    /// when the route has no stored entry.
    pub fn get(&self, id: u16) -> RouteRetentionMeta {
        self.entries.iter().find(|e| e.id == id).map(|e| e.meta).unwrap_or_default()
    }

    /// Set (or replace) route `id`'s retention meta. Returns `true` when the store actually changed
    /// (so the host only rewrites the sidecar on a real edit). A `Never` + `last_used == 0` write
    /// drops the entry (the empty default already reads that way — keeps the sidecar tight).
    pub fn set(&mut self, id: u16, meta: RouteRetentionMeta) -> bool {
        let default_row = meta == RouteRetentionMeta::default();
        match self.entries.iter().position(|e| e.id == id) {
            Some(pos) if self.entries[pos].meta == meta => false,
            Some(pos) if default_row => {
                // A row that reverted to the default carries no information — drop it.
                self.entries.swap_remove(pos);
                true
            }
            Some(pos) => {
                self.entries[pos].meta = meta;
                true
            }
            None if default_row => false, // nothing stored, nothing to store
            None => self.entries.push(RetEntry { id, meta }).is_ok(),
        }
    }

    /// Stamp route `id`'s `last_used` (keeping its retention level), inserting a default-retention
    /// row if the route had none. Returns whether the store changed. The sweep / upload / activation
    /// stamp path.
    pub fn stamp_last_used(&mut self, id: u16, last_used_utc: u32) -> bool {
        let meta = RouteRetentionMeta { retention: self.get(id).retention, last_used_utc };
        self.set(id, meta)
    }

    /// Drop every entry whose id is **not** in `live_ids` — the tidy-up a rescan/delete runs so the
    /// sidecar never carries retention for a route that no longer exists (ids never reuse, so this
    /// is belt-and-braces, mirroring the synced-set's `remove` on delete). Returns whether anything
    /// was dropped.
    pub fn retain_ids(&mut self, live_ids: &[u16]) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| live_ids.contains(&e.id));
        self.entries.len() != before
    }

    /// The stored rows, for the codec / tests.
    fn rows(&self) -> &[RetEntry] {
        &self.entries
    }
}

/// The encoded sidecar's byte length for `count` entries: the fixed header, the entry list, then the
/// trailing CRC-16.
pub const fn route_retention_len(count: usize) -> usize {
    RET_HEADER_LEN + count * RET_ENTRY_LEN + 2
}

/// The largest an encoded route-retention sidecar can be (a full store) — the buffer a host reserves.
pub const ROUTE_RETENTION_MAX_LEN: usize = route_retention_len(MAX_ROUTES);

/// Pack the route-retention store into `out`, returning the encoded byte length. `out` must be at
/// least [`route_retention_len`]`(store.len())` (use a [`ROUTE_RETENTION_MAX_LEN`] buffer). Inverse
/// of [`decode_route_retention`].
pub fn encode_route_retention(store: &RouteRetentionStore, out: &mut [u8]) -> usize {
    let rows = store.rows();
    let len = route_retention_len(rows.len());
    out[0..4].copy_from_slice(&RET_MAGIC);
    out[4] = RET_VERSION;
    out[5] = 0;
    out[6..8].copy_from_slice(&(rows.len() as u16).to_le_bytes());
    for (i, row) in rows.iter().enumerate() {
        let o = RET_HEADER_LEN + i * RET_ENTRY_LEN;
        out[o..o + 2].copy_from_slice(&row.id.to_le_bytes());
        out[o + 2] = row.meta.retention.as_u8();
        out[o + 3..o + 7].copy_from_slice(&row.meta.last_used_utc.to_le_bytes());
    }
    let crc = crate::store_meta::crc16(&out[..len - 2]);
    out[len - 2..len].copy_from_slice(&crc.to_le_bytes());
    len
}

/// Decode a route-retention sidecar, always returning a store — a blank page, a short slice, a torn
/// write, an unknown version, an overrunning count, or a CRC mismatch all yield the **empty** store
/// (every route reads `Never`, the safe default). Never panics on malformed input. Retention bytes
/// are sanitised through [`Retention::from_u8`] (unknown → `Never`), so a forward-compat level never
/// deletes.
pub fn decode_route_retention(bytes: &[u8]) -> RouteRetentionStore {
    let empty = RouteRetentionStore::new();
    if bytes.len() < RET_HEADER_LEN + 2 {
        return empty; // shorter than an empty-store sidecar → treat as absent
    }
    if bytes[0..4] != RET_MAGIC || bytes[4] != RET_VERSION {
        return empty;
    }
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let len = route_retention_len(count);
    if count > MAX_ROUTES || bytes.len() < len {
        return empty; // a count that claims more entries than the slice (or the cap) holds is corrupt
    }
    let crc = u16::from_le_bytes([bytes[len - 2], bytes[len - 1]]);
    if crc != crate::store_meta::crc16(&bytes[..len - 2]) {
        return empty;
    }
    let mut store = RouteRetentionStore::new();
    for i in 0..count {
        let o = RET_HEADER_LEN + i * RET_ENTRY_LEN;
        let id = u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let retention = Retention::from_u8(bytes[o + 2]);
        let last_used_utc = u32::from_le_bytes([bytes[o + 3], bytes[o + 4], bytes[o + 5], bytes[o + 6]]);
        let _ = store.set(id, RouteRetentionMeta { retention, last_used_utc });
    }
    store
}

// ==================== the expiry sweep ====================

/// One thing the sweep wants the host to do, drained into the typed [`HostCommand`] protocol. The
/// stamps carry the id only — the [`utc`] is filled from the wall clock at drain time (day-grain
/// expiry is indifferent to the few seconds between the sweep and the drain), keeping this a
/// pocket-sized 4-byte value so the pending queue stays cheap.
///
/// [`utc`]: crate::App::wall_unix_now
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepAction {
    /// Delete the expired route with this durable id (via the existing
    /// [`HostCommand::DeleteRoute`](crate::HostCommand::DeleteRoute)).
    DeleteRoute(crate::CatalogObjectId),
    /// Stamp this route's `last_used` to now — start the clock on an unknown stamp, or re-stamp the
    /// active route so it never expires underneath a ride
    /// ([`HostCommand::StampRouteUsed`](crate::HostCommand::StampRouteUsed)).
    StampRoute(crate::CatalogObjectId),
    /// Delete the synced-and-aged-out ride with this durable id
    /// ([`HostCommand::DeleteRide`](crate::HostCommand::DeleteRide)).
    DeleteRide(crate::CatalogObjectId),
    /// Stamp this ride's `synced_at` to now — start the countdown on a legacy synced-without-stamp
    /// ride ([`HostCommand::StampRideSynced`](crate::HostCommand::StampRideSynced)).
    StampRide(crate::CatalogObjectId),
}

/// One stored ride's **retention-relevant** facts — the compact, board-agnostic inventory the sweep
/// reads *instead of* the 32-row UI ride catalog (finding #876-2). Storage holds up to
/// [`MAX_RIDES`] rides but the resident [`RideCatalog`](crate::ride) only ever surfaces the newest
/// [`UI_RIDES_CAP`]; feeding retention from that catalog left older synced rides invisible to expiry
/// forever. Retention needs only these three facts (never the name/distance/profile the UI carries),
/// so the host streams **every** stored ride's `id + synced + synced_at` here, independent of the
/// display catalog.
///
/// [`MAX_RIDES`]: crate::ride::MAX_RIDES
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideRetentionRecord {
    /// The ride's durable object id.
    pub id: crate::CatalogObjectId,
    /// Whether the phone has a durable copy (the synced-set flag). An unsynced ride is never touched.
    pub synced: bool,
    /// The UTC-seconds instant the ride was first synced (`0` = legacy synced-without-stamp — the
    /// eager stamp starts its countdown; never deleted on sight).
    pub synced_at_utc: u32,
}

/// The read-only inputs one sweep pass evaluates — the resident route catalog + the full compact
/// ride inventory and their retention state, plus the current UTC instant and the ride-retention
/// setting. Borrowed (no copies of the catalogs), so the sweep is allocation-free: it only compares
/// a handful of `u32`s per entry.
pub struct SweepInputs<'a> {
    /// Current UTC unix seconds (only ever passed when the clock is trusted — the caller gates it).
    pub now_utc: u32,
    /// Durable route ids, pairwise with [`route_metas`](SweepInputs::route_metas).
    pub route_ids: &'a [crate::CatalogObjectId],
    /// Each route's retention meta, pairwise with [`route_ids`](SweepInputs::route_ids).
    pub route_metas: &'a [RouteRetentionMeta],
    /// The active-navigation route's catalog index, or `None` — never deleted (re-stamped instead).
    pub active_route: Option<usize>,
    /// The **full** compact ride inventory — every stored ride's retention facts (finding #876-2),
    /// not just the newest [`UI_RIDES_CAP`] the menu shows.
    pub ride_records: &'a [RideRetentionRecord],
    /// The device ride-retention setting.
    pub ride_retention: RideRetention,
}

/// Evaluate one sweep pass, pushing every warranted [`SweepAction`] into `out` (bounded; overflow is
/// silently dropped). With the full [`MAX_RIDES`](crate::ride::MAX_RIDES)-deep ride inventory
/// (finding #876-2), a bulk-expiry backlog *can* exceed [`SWEEP_QUEUE_CAP`] — that is deliberate
/// **wave behavior**: the caller only re-sweeps once the queue has drained, so the next sweep picks
/// up whatever is still due. The direction is benign — a full queue only delays stamps/deletes to a
/// later wave, never mis-deletes (dispatch re-validates every candidate). **Pure** over its inputs —
/// the whole safety policy in one testable place. Holds the epic's invariants:
///
/// 1. *(caller-gated)* nothing runs without a trusted clock — the caller only builds [`SweepInputs`]
///    when [`clock_trusted`](crate::App::clock_trusted) and no ride is recording.
/// 2. an unknown `last_used` (`0`) on a retention≠Never route is **stamped**, never deleted.
/// 3. the **active** route is never deleted — it re-stamps when it would otherwise expire.
/// 6. a `Never` route (the migration default for pre-feature routes) is never touched.
/// 5. a ride is deleted only when `synced` **and** `synced_at > 0` **and** `now ≥ synced_at +
///    retention`; an unsynced ride is never touched regardless of age.
///
/// **Ride `synced_at` stamps are not emitted here** — they are stamped *eagerly* at ack-time (see
/// [`RetentionMachine::advance`]), not deferred to this recording-gated hourly sweep, so
/// a ride acked mid-tour starts its countdown immediately rather than at recording-end. This
/// function only ever *deletes* a ride whose `synced_at` is already set.
pub fn collect_sweep_actions<const N: usize>(inputs: &SweepInputs, out: &mut heapless::Vec<SweepAction, N>) {
    let now = inputs.now_utc;

    // ── Routes ──
    for (idx, (&id, &meta)) in inputs.route_ids.iter().zip(inputs.route_metas).enumerate() {
        let is_active = inputs.active_route == Some(idx);
        if is_active {
            // Invariant 3: never delete the active route. Re-stamp it only when it *would* have
            // expired (or its clock was never started) — so a long tour renews the route it is
            // navigating instead of deleting it, without an RRAM write every sweep otherwise.
            if !matches!(meta.retention, Retention::Never) && (meta.last_used_utc == 0 || meta.is_expired(now)) {
                let _ = out.push(SweepAction::StampRoute(id));
            }
            continue;
        }
        if matches!(meta.retention, Retention::Never) {
            continue; // invariant 6: Never expires nothing
        }
        if meta.needs_clock_started() {
            // Invariant 2: unknown last_used starts the clock, it is never deleted on sight.
            let _ = out.push(SweepAction::StampRoute(id));
        } else if meta.is_expired(now) {
            let _ = out.push(SweepAction::DeleteRoute(id));
        }
    }

    // ── Rides (delete only; the `synced_at` stamp is eager — see the fn doc) ──
    // Evaluated over the **full** compact inventory (finding #876-2), so a synced+aged ride is
    // reachable regardless of whether it sits in the newest-32 UI catalog.
    let ride_days = inputs.ride_retention.days();
    for ride in inputs.ride_records {
        // Invariant 5: only a synced ride whose countdown has *started* (`synced_at > 0`) and fully
        // elapsed is deleted. `synced_at == 0` (never stamped — e.g. acked while untrusted) is left
        // for the eager stamp to start; an unsynced ride is never touched, at any age. `ride_days ==
        // None` (`ride_retention` Never) deletes nothing.
        if !ride.synced || ride.synced_at_utc == 0 {
            continue;
        }
        let Some(days) = ride_days else {
            continue; // ride_retention == Never → nothing deletes
        };
        if now >= ride.synced_at_utc.saturating_add(days * DAY_SECS) {
            let _ = out.push(SweepAction::DeleteRide(ride.id));
        }
    }
}

/// The pending-candidate queue's capacity. Deliberately **smaller** than the theoretical worst case
/// now that the sweep reads the full [`MAX_RIDES`](crate::ride::MAX_RIDES)-deep ride inventory
/// (finding #876-2): a pathological store (every route expired + >32 rides due at once) can emit
/// more candidates than fit. That overflow is **wave behavior, not loss** — `collect_sweep_actions`
/// drops the excess silently, and because [`RetentionMachine::advance`] only re-sweeps once the
/// queue has drained, the next sweep re-discovers whatever is still due. The failure direction is
/// benign by construction: a full queue can only *delay* stamps and deletes to a later wave, never
/// mis-delete (every candidate is still re-validated at dispatch). Sized for the realistic case —
/// a full route catalog plus a UI-page of rides — so waves only occur under bulk-expiry backlogs.
pub(crate) const SWEEP_QUEUE_CAP: usize = MAX_ROUTES + UI_RIDES_CAP;

/// Bounded pacing between re-dispatches of a delete **candidate** that storage has not yet confirmed
/// gone (map-plane millis, finding #876-1/3). A delete candidate is *kept* — not consumed — when it
/// is dispatched, so it re-dispatches until the authoritative rescan shows the object absent (success)
/// or a live recheck cancels it. This backoff keeps a genuinely-failing write (rare — a dead SD) from
/// re-firing every frame while the far more common success path (rescan lands in a frame or two, the
/// candidate is cancelled as "already absent") is never gated by it.
pub(crate) const RETENTION_DELETE_BACKOFF_MS: u32 = 3_000;

/// Everything one retention decision reads, borrowed for the length of the call — the catalogs and
/// their metadata columns, the live gates, and the two clocks.
///
/// It exists so the *whole* policy can live in [`RetentionMachine`] without the machine owning a
/// copy of the catalogs: `App` assembles this view from its components and the machine reads it.
/// Building it is free — three slice borrows and five scalars — so discovery and the just-in-time
/// recheck read exactly the same picture rather than two hand-assembled ones.
pub(crate) struct RetentionView<'a> {
    /// Current UTC unix seconds, or `None` when no real time source established the clock **this
    /// boot** — invariant 1. A `None` here is the whole safety core: nothing stamps, nothing
    /// deletes, nothing sweeps.
    pub now_utc: Option<u32>,
    /// Map-plane millis — the timebase of the bounded delete re-dispatch backoff only.
    pub now_ms: u32,
    /// Whether a ride is recording (invariant 4: deletions defer; metadata stamps do not).
    pub recording: bool,
    /// Durable route ids, pairwise with [`route_metas`](RetentionView::route_metas).
    pub route_ids: &'a [crate::CatalogObjectId],
    /// Each route's retention meta, pairwise with [`route_ids`](RetentionView::route_ids).
    pub route_metas: &'a [RouteRetentionMeta],
    /// The active-navigation route's catalog index, or `None` — never deleted (re-stamped instead).
    pub active_route: Option<usize>,
    /// The **full** compact ride inventory (finding #876-2), not just the newest UI page.
    pub ride_records: &'a [RideRetentionRecord],
    /// The device ride-retention setting.
    pub ride_retention: RideRetention,
}

impl RetentionView<'_> {
    /// Route `id`'s catalog index and retention meta, or `None` when it is not resident.
    fn route(&self, id: crate::CatalogObjectId) -> Option<(usize, RouteRetentionMeta)> {
        let idx = self.route_ids.iter().position(|&x| x == id)?;
        Some((idx, self.route_metas.get(idx).copied().unwrap_or_default()))
    }

    /// The active route's durable id, but only while it is a route that can actually expire. A
    /// `Never` route (the migration default, and every route until a rider sets a level) needs no
    /// `last_used`, so stamping it on each activation would churn the sidecar for no benefit.
    fn expiring_active_route(&self) -> Option<crate::CatalogObjectId> {
        let idx = self.active_route?;
        self.route_metas.get(idx)?.retention.days()?;
        self.route_ids.get(idx).copied()
    }
}

/// The **one owner** of retention policy (epic #1433 §5, #1437): the trusted-clock gate, the hourly
/// sweep gate, the active-route and ride-sync stamps, expiry discovery, the live revalidation that
/// stands between a candidate and a deletion, and the per-kind delete retry pacing.
///
/// It produces exactly two things, and never a deletion itself:
///
/// - [`RetentionEffect`] — the two bounded device-local sidecar writes, through
///   [`next_metadata_effect`](RetentionMachine::next_metadata_effect).
/// - [`CatalogIntent`] — an expiry, handed to `CatalogMachine` through
///   [`next_expiry`](RetentionMachine::next_expiry), so an auto-expired object leaves by exactly the
///   path a rider-deleted one does. **No platform executor decides whether an object is expired.**
///
/// The queue holds **candidates**, not decisions. A `Delete*` entry means only "this id looked worth
/// deleting when the sweep reached it"; [`next_expiry`](RetentionMachine::next_expiry) re-derives
/// the whole rule from the live [`RetentionView`] immediately before it emits an intent, and keeps
/// the candidate until storage confirms the object gone — so a transient failure retries without
/// waiting for the next hourly sweep, and a route activated or a ride re-synced after discovery is
/// protected by the recheck.
pub(crate) struct RetentionMachine {
    /// The batch of candidate [`SweepAction`]s a sweep pass emitted. A stamp leaves the queue when
    /// its effect is issued (and is re-queued only if the write comes back failed); delete
    /// candidates are retained until storage confirms the object absent, or a live recheck cancels
    /// them. A sweep only refills it when it is empty (so a batch is never double-enqueued and at
    /// most one batch drains at a time).
    queue: heapless::Vec<SweepAction, SWEEP_QUEUE_CAP>,
    /// The wall-clock hour (`utc / 3600`) of the last completed sweep — `None` until the first
    /// trusted sweep this boot. The sweep re-runs when the current hour differs (roughly hourly)
    /// or on the first eligible tick after trust is established.
    last_sweep_hour: Option<u32>,
    /// The active route id whose activation was last stamped — so the once-per-activation
    /// `last_used` stamp fires when the active route *changes*, not every tick it stays active.
    /// Only advanced once the stamp is actually queued (never lost to capacity pressure —
    /// finding #876-1).
    last_active_stamped: Option<crate::CatalogObjectId>,
    /// Per-kind in-flight **route** / **ride** delete: the dispatched candidate's id plus the
    /// map-plane-millis instant it may be re-dispatched (the bounded
    /// [`RETENTION_DELETE_BACKOFF_MS`] pacing); `None` = nothing of that kind is in flight, the head
    /// candidate may dispatch now. Keyed per kind so a route delete never blocks a ride delete —
    /// "one delete in flight" is per kind, matching the board's per-kind delete channels. Carrying
    /// the **id** (not just the deadline) is what keeps the one-in-flight property honest: only a
    /// cancel of *the dispatched id itself* (its rescan confirmed it gone, or a live recheck retired
    /// it) clears the slot — cancelling some *other* queued-but-never-dispatched candidate of the
    /// same kind (e.g. an activation retiring its own delete while a different route's delete is
    /// outstanding) must not re-open the dispatch window mid-flight.
    route_delete_inflight: Option<(crate::CatalogObjectId, u32)>,
    ride_delete_inflight: Option<(crate::CatalogObjectId, u32)>,
    /// The token source for the sidecar writes. One write is in flight at a time (the domain's
    /// single effect slot), so one source is all the domain needs.
    ops: crate::device_core::TokenSource<crate::device_core::RetentionTag>,
    /// The stamp currently out with the executor, so a failure can be re-queued for the id it was
    /// actually about (the outcome carries the token, not the subject).
    inflight_write: Option<(SweepKind, crate::CatalogObjectId)>,
}

impl RetentionMachine {
    /// The boot state: nothing queued, no sweep run yet, no activation stamped.
    pub(crate) const fn new() -> Self {
        RetentionMachine {
            queue: heapless::Vec::new(),
            last_sweep_hour: None,
            last_active_stamped: None,
            route_delete_inflight: None,
            ride_delete_inflight: None,
            ops: crate::device_core::TokenSource::new(),
            inflight_write: None,
        }
    }

    /// Advance the domain one pass: the once-per-activation active-route stamp, the eager ride
    /// `synced_at` stamps, and the roughly-hourly expiry sweep.
    ///
    /// **The safety core lives here, not in the caller.** An untrusted clock returns immediately:
    /// no stamp, no sweep, no candidate (invariant 1). The sweep additionally waits for the ride to
    /// end (invariant 4) — a metadata stamp does not, so a ride acked mid-tour starts its countdown
    /// at ack time instead of at the end of the tour.
    pub(crate) fn advance(&mut self, view: &RetentionView) {
        let Some(now) = view.now_utc else {
            return; // invariant 1: no trusted clock this boot → nothing runs
        };

        // Once-per-activation stamp: when the active route *changes*, mark it used now so a freshly
        // started route cannot expire underneath the ride it is guiding (invariant 3's companion).
        self.note_active_route(view.expiring_active_route());

        // The eager `synced_at` stamp: a ride acked under a trusted clock starts its delete
        // countdown at ~ack time. Safe mid-ride — invariant 4 gates *deletions*, not stamps.
        self.stamp_synced_rides(view.ride_records);

        if view.recording {
            return; // invariant 4: no sweep while a ride records
        }
        self.maybe_sweep(now, |queue| {
            let inputs = SweepInputs {
                now_utc: now,
                route_ids: view.route_ids,
                route_metas: view.route_metas,
                active_route: view.active_route,
                ride_records: view.ride_records,
                ride_retention: view.ride_retention,
            };
            collect_sweep_actions(&inputs, queue);
        });
    }

    /// Take the next queued **metadata write** of `kind` (a stamp class) as a typed effect, or
    /// `None` when nothing of that kind is queued, the clock is untrusted, or a write is already in
    /// flight — the domain's single-operation rule.
    ///
    /// The stamp carries the UTC instant at *emission*, which is what makes the queue entry a bare
    /// id: day-grain expiry is indifferent to the seconds between discovery and dispatch.
    pub(crate) fn next_metadata_effect(&mut self, kind: SweepKind, view: &RetentionView) -> Option<RetentionEffect> {
        let now = view.now_utc?;
        if self.inflight_write.is_some() {
            return None; // one sidecar write at a time
        }
        let id = self.peek(kind)?;
        let effect = match kind {
            SweepKind::StampRoute => {
                // A retention *level* is never touched by a use stamp — read the live level back and
                // write it beside the fresh `last_used`.
                let retention = view.route(id).map(|(_, meta)| meta.retention).unwrap_or_default();
                RetentionEffect::WriteRouteMetadata {
                    token: self.ops.issue(),
                    id,
                    meta: RouteRetentionMeta { retention, last_used_utc: now },
                }
            }
            SweepKind::StampRide => RetentionEffect::WriteRideMetadata { token: self.ops.issue(), id, synced_at: now },
            SweepKind::DeleteRoute | SweepKind::DeleteRide => return None, // deletions are intents
        };
        self.take(kind); // the stamp leaves the queue; a failure re-queues it in `apply_outcome`
        self.inflight_write = Some((kind, id));
        Some(effect)
    }

    /// Consume the answer to a [`RetentionEffect`]. A stale token (the write was superseded or
    /// already accounted for) changes nothing.
    ///
    /// A failed or abandoned write **re-queues its stamp**. The direction is safe either way — an
    /// unwritten stamp reads back as unknown, which never deletes — but re-queuing means the retry
    /// costs one pass rather than waiting for the next hourly sweep to rediscover it.
    pub(crate) fn apply_outcome(&mut self, outcome: RetentionOutcome) {
        if !self.ops.is_current(outcome.token()) {
            return;
        }
        self.ops.invalidate(); // terminal: a repeat of this outcome is no longer current
        let Some((kind, id)) = self.inflight_write.take() else {
            return;
        };
        if matches!(outcome, RetentionOutcome::Failed { .. } | RetentionOutcome::Cancelled { .. }) {
            let _ = match kind {
                SweepKind::StampRoute => self.ensure_stamp_route(id),
                _ => self.ensure_stamp_ride(id),
            };
        }
    }

    /// The next **expiry intent** of `kind` for `CatalogMachine`, with the whole policy re-derived
    /// from live state at this instant (finding #876-1). A candidate is only a suggestion; what
    /// leaves here is a decision:
    ///
    /// - no trusted clock, or a ride recording → **defer** (candidates stay queued — invariants 1 & 4);
    /// - the route is now active → **never delete**; re-stamp it instead (invariant 3);
    /// - the subject vanished, went `Never`, has an unknown `last_used`, was re-synced, or is no
    ///   longer expired → **cancel** the candidate and try the next (invariants 2, 5 & 6);
    /// - otherwise **emit** the intent and **keep** the candidate — it is retired only when the
    ///   authoritative rescan drops the id — paced by the bounded re-dispatch backoff so a failing
    ///   write retries without hammering (finding #876-3).
    pub(crate) fn next_expiry(&mut self, kind: SweepKind, view: &RetentionView) -> Option<CatalogIntent> {
        if view.now_utc.is_none() || view.recording {
            return None; // defer — invariants 1 & 4, evaluated at execution time
        }
        loop {
            let id = self.peek(kind)?;
            if self.still_due(kind, id, view) {
                if !self.delete_dispatch_ready(kind, id, view.now_ms) {
                    return None; // one delete of this kind in flight — wait out the bounded backoff
                }
                self.mark_delete_dispatched(kind, id, view.now_ms);
                return Some(match kind {
                    SweepKind::DeleteRide => CatalogIntent::DeleteRide { id },
                    _ => CatalogIntent::DeleteRoute { id },
                });
            }
            self.cancel(kind, id); // invalid / already applied: drop it and try the next
        }
    }

    /// The just-in-time delete predicate: whether `id` is *still* a legal auto-delete right now.
    /// Converts an active or unknown-stamp route into a re-stamp instead of a delete (so the safety
    /// stamp is never lost), and reports `false` for anything that vanished or stopped qualifying.
    fn still_due(&mut self, kind: SweepKind, id: crate::CatalogObjectId, view: &RetentionView) -> bool {
        let Some(now) = view.now_utc else { return false };
        match kind {
            SweepKind::DeleteRide => {
                let Some(rec) = view.ride_records.iter().find(|r| r.id == id) else {
                    return false; // gone from the inventory — already absent
                };
                if !rec.synced || rec.synced_at_utc == 0 {
                    return false; // unsynced, or the countdown has not started (invariant 5)
                }
                let Some(days) = view.ride_retention.days() else {
                    return false; // ride_retention == Never
                };
                now >= rec.synced_at_utc.saturating_add(days * DAY_SECS)
            }
            _ => {
                let Some((idx, meta)) = view.route(id) else {
                    return false; // route gone — already absent
                };
                // Invariant 3: the active route is never deleted — re-stamp it instead.
                if view.active_route == Some(idx) {
                    if !matches!(meta.retention, Retention::Never) {
                        self.ensure_stamp_route(id);
                    }
                    return false;
                }
                if matches!(meta.retention, Retention::Never) {
                    return false; // invariant 6
                }
                if meta.needs_clock_started() {
                    self.ensure_stamp_route(id); // invariant 2: stamp, never delete on sight
                    return false;
                }
                meta.is_expired(now)
            }
        }
    }

    /// The per-kind in-flight slot (`None` for the non-delete kinds — the stamp classes are
    /// fire-and-forget and never paced).
    fn inflight_slot(&mut self, kind: SweepKind) -> Option<&mut Option<(crate::CatalogObjectId, u32)>> {
        match kind {
            SweepKind::DeleteRoute => Some(&mut self.route_delete_inflight),
            SweepKind::DeleteRide => Some(&mut self.ride_delete_inflight),
            SweepKind::StampRoute | SweepKind::StampRide => None,
        }
    }

    /// Whether an action of `kind` is queued (the drain's backpressure peek).
    pub(crate) fn has(&self, kind: SweepKind) -> bool {
        self.queue.iter().any(|a| kind.matches(*a))
    }

    /// Pop the first queued action of `kind`, or `None`. Removes it (each is a distinct one-shot).
    /// The **stamp** drain path — a delete candidate is never consumed this way (it is retained
    /// until confirmed; see [`peek`](Self::peek) / [`cancel`](Self::cancel)).
    fn take(&mut self, kind: SweepKind) -> Option<crate::CatalogObjectId> {
        let pos = self.queue.iter().position(|a| kind.matches(*a))?;
        Some(self.queue.remove(pos).id())
    }

    /// The first queued candidate id of `kind` **without** removing it — the delete drain peeks,
    /// re-derives the live decision, and only then either [`cancel`](Self::cancel)s it (invalid /
    /// already applied) or dispatches it (kept for retry).
    pub(crate) fn peek(&self, kind: SweepKind) -> Option<crate::CatalogObjectId> {
        self.queue.iter().find(|a| kind.matches(**a)).map(|a| a.id())
    }

    /// Remove the queued candidate of `kind` with this id (the live recheck cancelled it — invalid,
    /// or storage confirmed it gone). Clears the in-flight slot **only when the cancelled id is the
    /// dispatched one** — that operation has resolved, so the next head may dispatch at once.
    /// Cancelling a queued-but-never-dispatched candidate (an activation retiring its own delete
    /// while a *different* id's delete is outstanding) leaves the in-flight slot alone, preserving
    /// the per-kind one-in-flight property: the outstanding id is not re-dispatched mid-flight by an
    /// unrelated cancel. Returns whether anything was removed.
    fn cancel(&mut self, kind: SweepKind, id: crate::CatalogObjectId) -> bool {
        let Some(pos) = self.queue.iter().position(|a| kind.matches(*a) && a.id() == id) else {
            return false;
        };
        self.queue.remove(pos);
        if let Some(slot) = self.inflight_slot(kind) {
            if matches!(slot, Some((inflight, _)) if *inflight == id) {
                *slot = None; // the dispatched op resolved — the next head may dispatch now
            }
        }
        true
    }

    /// Whether delete candidate `id` of `kind` may be **(re-)dispatched** now — `true` when nothing
    /// of that kind is in flight, or when `id` *is* the in-flight one and its bounded
    /// [`RETENTION_DELETE_BACKOFF_MS`] window has elapsed (the retry path). A *different* id stays
    /// blocked until the in-flight one resolves (cancel on rescan/recheck) — one delete in flight
    /// per kind. Cancellation of the dispatched id clears the slot, so the common success path
    /// (rescan drops the id within a frame or two) is never gated; only a genuinely-failing write
    /// waits out the backoff before it re-fires.
    fn delete_dispatch_ready(&mut self, kind: SweepKind, id: crate::CatalogObjectId, now_ms: u32) -> bool {
        match self.inflight_slot(kind).and_then(|s| *s) {
            None => true,
            Some((inflight, at)) => inflight == id && now_ms.wrapping_sub(at) < 0x8000_0000,
        }
    }

    /// Record that delete candidate `id` of `kind` was just dispatched: own the per-kind in-flight
    /// slot and arm the bounded re-dispatch backoff so a still-present (failed) object doesn't
    /// re-fire every frame. The candidate itself stays queued (retained until storage confirms it
    /// gone).
    fn mark_delete_dispatched(&mut self, kind: SweepKind, id: crate::CatalogObjectId, now_ms: u32) {
        if let Some(slot) = self.inflight_slot(kind) {
            *slot = Some((id, now_ms.wrapping_add(RETENTION_DELETE_BACKOFF_MS)));
        }
    }

    /// Enqueue a stamp for the active route becoming `active_id` — but only **once per activation**
    /// (when it differs from the last stamped active id). Called each trusted tick; a no-op while
    /// the active route is unchanged. `active_id == None` clears the memory so re-activating the
    /// same route later re-stamps.
    ///
    /// Two safety duties beyond the stamp (finding #876-1):
    /// - it **cancels any queued delete candidate for the now-active route** immediately, so an
    ///   activation that races an already-discovered delete can never be beaten to dispatch (the
    ///   live drain recheck is the primary guard; this closes the window belt-and-braces);
    /// - it advances `last_active_stamped` **only once the stamp is actually queued**, so a full
    ///   queue can never drop the activation stamp and then suppress the retry — a later tick re-tries.
    fn note_active_route(&mut self, active_id: Option<crate::CatalogObjectId>) {
        match active_id {
            Some(id) => {
                // Invariant 3: an activated route must never be deleted by an earlier sweep decision.
                self.cancel(SweepKind::DeleteRoute, id);
                if self.last_active_stamped != Some(id) {
                    // Advance the once-per-activation memory only if the stamp is (or is already)
                    // queued — never lose it to capacity pressure.
                    if self.ensure_stamp_route(id) {
                        self.last_active_stamped = Some(id);
                    }
                }
            }
            None => self.last_active_stamped = None,
        }
    }

    /// Ensure a `StampRoute(id)` is queued (idempotent — skips a duplicate). Returns whether one is
    /// queued afterwards (`false` only when the queue was full and the push failed).
    fn ensure_stamp_route(&mut self, id: crate::CatalogObjectId) -> bool {
        if self.queue.iter().any(|a| matches!(a, SweepAction::StampRoute(q) if *q == id)) {
            return true;
        }
        self.queue.push(SweepAction::StampRoute(id)).is_ok()
    }

    /// Enqueue a `last_used` stamp for a route a BLE upload just committed (auto-expiry epic #638 S4):
    /// a fresh or replace upload is a "use", so its expiry clock should anchor at **upload time** — the
    /// precise stamp the sweep otherwise only approximates (invariant 2 starts an unknown `last_used`
    /// at the *next sweep*, up to an hour later). Called from `on_route_uploaded` **only when the clock
    /// is trusted** (an untrusted upload leaves `last_used == 0`, which the sweep starts later — the
    /// safe fallback). Idempotent: skips an id already queued (the drain mirrors `last_used` into the
    /// resident meta, so a stamped route stops re-enqueuing). Reuses the `StampRouteUsed` host path (the
    /// same sidecar write a sweep / activation stamp takes) — no new channel.
    pub(crate) fn note_route_uploaded(&mut self, id: crate::CatalogObjectId) {
        self.ensure_stamp_route(id);
    }

    /// Eagerly enqueue a `synced_at` stamp for every resident ride that is `synced` but not yet
    /// stamped (`synced_at == 0`) — the ack-time countdown start (epic #638, S3). Called each
    /// **trusted** tick **regardless of recording**: a metadata stamp is safe mid-ride (invariant 4
    /// gates deletions, not stamps), so a ride acked mid-tour starts its countdown at ~ack-time
    /// instead of deferring to the recording-end sweep. Idempotent: skips an id already queued (the
    /// caller also mirrors `synced_at` on drain, so a stamped ride stops matching next tick). `rides`
    /// is the resident catalog pairwise with `ride_ids`.
    fn stamp_synced_rides(&mut self, rides: &[RideRetentionRecord]) {
        for ride in rides {
            if ride.synced && ride.synced_at_utc == 0 {
                self.ensure_stamp_ride(ride.id);
            }
        }
    }

    /// Ensure a `StampRide(id)` is queued (idempotent) — the ride twin of
    /// [`ensure_stamp_route`](Self::ensure_stamp_route). Returns whether one is queued afterwards.
    fn ensure_stamp_ride(&mut self, id: crate::CatalogObjectId) -> bool {
        if self.queue.iter().any(|a| matches!(a, SweepAction::StampRide(q) if *q == id)) {
            return true;
        }
        self.queue.push(SweepAction::StampRide(id)).is_ok()
    }

    /// Run a sweep pass if due, filling the queue. [`advance`](Self::advance) has already applied the
    /// trust and recording gates; here the hourly cadence + empty-queue precondition are checked.
    /// `now_utc` is the trusted UTC now.
    fn maybe_sweep(&mut self, now_utc: u32, build: impl FnOnce(&mut heapless::Vec<SweepAction, SWEEP_QUEUE_CAP>)) {
        if !self.queue.is_empty() {
            return; // a prior batch is still draining — don't stack a second
        }
        let hour = now_utc / 3600;
        if self.last_sweep_hour == Some(hour) {
            return; // already swept this wall-clock hour
        }
        self.last_sweep_hour = Some(hour);
        build(&mut self.queue);
    }

    /// Force the next eligible tick to sweep (drops the hourly memory). Used by the sim's "+1 day"
    /// control so a fast-forwarded clock sweeps immediately instead of waiting for the wall-clock
    /// hour to roll.
    pub(crate) fn force_next_sweep(&mut self) {
        self.last_sweep_hour = None;
    }

    /// Test seam: enqueue an action directly, standing in for a sweep the protocol-ordering test
    /// doesn't replay in full.
    #[cfg(test)]
    pub(crate) fn test_push(&mut self, a: SweepAction) {
        let _ = self.queue.push(a);
    }
}

/// A [`SweepAction`] discriminator for the per-class drain (which host command a queued action
/// becomes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SweepKind {
    DeleteRoute,
    StampRoute,
    DeleteRide,
    StampRide,
}

impl SweepKind {
    fn matches(self, a: SweepAction) -> bool {
        matches!(
            (self, a),
            (SweepKind::DeleteRoute, SweepAction::DeleteRoute(_))
                | (SweepKind::StampRoute, SweepAction::StampRoute(_))
                | (SweepKind::DeleteRide, SweepAction::DeleteRide(_))
                | (SweepKind::StampRide, SweepAction::StampRide(_))
        )
    }
}

impl SweepAction {
    /// The durable object id this action targets.
    fn id(self) -> crate::CatalogObjectId {
        match self {
            SweepAction::DeleteRoute(id)
            | SweepAction::StampRoute(id)
            | SweepAction::DeleteRide(id)
            | SweepAction::StampRide(id) => id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::MAX_RIDES;

    #[test]
    fn retention_days_and_unknown_decode() {
        assert_eq!(Retention::Never.days(), None);
        assert_eq!(Retention::Day1.days(), Some(1));
        assert_eq!(Retention::Month2.days(), Some(60));
        assert_eq!(Retention::from_u8(0), Retention::Never);
        assert_eq!(Retention::from_u8(5), Retention::Month2);
        assert_eq!(Retention::from_u8(200), Retention::Never, "an unknown byte is Never — never deletes");
        for r in [
            Retention::Never,
            Retention::Day1,
            Retention::Week1,
            Retention::Week2,
            Retention::Month1,
            Retention::Month2,
        ] {
            assert_eq!(Retention::from_u8(r.as_u8()), r, "round-trips");
        }
    }

    #[test]
    fn ride_retention_default_and_step() {
        assert_eq!(RideRetention::default(), RideRetention::Week1, "default 1 week");
        assert_eq!(RideRetention::default().days(), Some(7));
        assert_eq!(RideRetention::Never.days(), None);
        assert_eq!(RideRetention::from_u8(99), RideRetention::Week1, "unknown → default");
        // The picker wraps Never → 1d → 1wk → 1mo → Never.
        assert_eq!(RideRetention::Never.stepped(1), RideRetention::Day1);
        assert_eq!(RideRetention::Month1.stepped(1), RideRetention::Never);
        assert_eq!(RideRetention::Never.stepped(-1), RideRetention::Month1);
    }

    #[test]
    fn meta_expiry_math() {
        let m = RouteRetentionMeta::new(Retention::Day1, 1_000);
        assert_eq!(m.expires_at(), Some(1_000 + DAY_SECS));
        assert!(!m.is_expired(1_000 + DAY_SECS - 1));
        assert!(m.is_expired(1_000 + DAY_SECS), "exact boundary is expired");
        // Unknown last_used never expires — it needs the clock started first.
        let unknown = RouteRetentionMeta::new(Retention::Day1, 0);
        assert_eq!(unknown.expires_at(), None);
        assert!(!unknown.is_expired(u32::MAX));
        assert!(unknown.needs_clock_started());
        // Never never expires.
        let never = RouteRetentionMeta::new(Retention::Never, 1_000);
        assert_eq!(never.expires_at(), None);
        assert!(!never.needs_clock_started());
        // Saturating deadline can't wrap into the past.
        let late = RouteRetentionMeta::new(Retention::Month2, u32::MAX - 10);
        assert_eq!(late.expires_at(), Some(u32::MAX));
    }

    #[test]
    fn sidecar_round_trips() {
        let mut store = RouteRetentionStore::new();
        assert!(store.set(3, RouteRetentionMeta::new(Retention::Week1, 5_000)));
        assert!(store.set(7, RouteRetentionMeta::new(Retention::Month1, 9_999)));
        assert!(store.stamp_last_used(3, 6_000), "stamp updates last_used");
        assert_eq!(store.get(3), RouteRetentionMeta::new(Retention::Week1, 6_000));
        assert_eq!(store.get(42), RouteRetentionMeta::default(), "absent → Never/0");

        let mut buf = [0u8; ROUTE_RETENTION_MAX_LEN];
        let n = encode_route_retention(&store, &mut buf);
        assert_eq!(decode_route_retention(&buf[..n]), store);

        // Empty store is a valid, non-crashing round-trip.
        let empty = RouteRetentionStore::new();
        let n = encode_route_retention(&empty, &mut buf);
        assert_eq!(decode_route_retention(&buf[..n]), empty);
    }

    #[test]
    fn sidecar_torn_or_missing_decodes_empty() {
        let mut store = RouteRetentionStore::new();
        store.set(9, RouteRetentionMeta::new(Retention::Day1, 100));
        store.set(12, RouteRetentionMeta::new(Retention::Week2, 200));
        let mut buf = [0u8; ROUTE_RETENTION_MAX_LEN];
        let n = encode_route_retention(&store, &mut buf);

        assert_eq!(decode_route_retention(&[]), RouteRetentionStore::new(), "absent → empty");
        assert_eq!(decode_route_retention(&[0u8; 4]), RouteRetentionStore::new(), "runt → empty");
        assert_eq!(decode_route_retention(&[0u8; RET_HEADER_LEN + 2]), RouteRetentionStore::new(), "blank page");
        assert_eq!(decode_route_retention(&[0xFF; 80]), RouteRetentionStore::new(), "erased page → empty");

        let mut torn = buf;
        torn[RET_HEADER_LEN] ^= 0xFF; // flip an id byte, don't fix the CRC
        assert_eq!(decode_route_retention(&torn[..n]), RouteRetentionStore::new(), "CRC mismatch → empty");

        let mut bad_count = buf;
        bad_count[6..8].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert_eq!(decode_route_retention(&bad_count[..n]), RouteRetentionStore::new(), "overrunning count → empty");

        let mut old = buf;
        old[4] = RET_VERSION + 1;
        assert_eq!(decode_route_retention(&old[..n]), RouteRetentionStore::new(), "foreign version → empty");
    }

    #[test]
    fn sidecar_retain_ids_drops_absent() {
        let mut store = RouteRetentionStore::new();
        store.set(1, RouteRetentionMeta::new(Retention::Day1, 10));
        store.set(2, RouteRetentionMeta::new(Retention::Day1, 20));
        store.set(3, RouteRetentionMeta::new(Retention::Day1, 30));
        assert!(store.retain_ids(&[1, 3]), "dropped id 2");
        assert_eq!(store.get(2), RouteRetentionMeta::default());
        assert_eq!(store.get(1).last_used_utc, 10);
        assert!(!store.retain_ids(&[1, 3]), "idempotent — nothing more to drop");
    }

    /// Unknown retention byte in the sidecar decodes to Never (never deletes) — the forward-compat
    /// safety property carried through the codec, not just the enum.
    #[test]
    fn sidecar_unknown_retention_byte_reads_never() {
        let mut store = RouteRetentionStore::new();
        store.set(5, RouteRetentionMeta::new(Retention::Week1, 123));
        let mut buf = [0u8; ROUTE_RETENTION_MAX_LEN];
        let n = encode_route_retention(&store, &mut buf);
        buf[RET_HEADER_LEN + 2] = 0x7F; // forge an unknown retention level on the one entry
        let crc = crate::store_meta::crc16(&buf[..n - 2]);
        buf[n - 2..n].copy_from_slice(&crc.to_le_bytes()); // re-CRC so it passes framing
        let got = decode_route_retention(&buf[..n]);
        assert_eq!(got.get(5).retention, Retention::Never, "unknown level → Never");
    }

    fn ins() -> SweepInputs<'static> {
        SweepInputs {
            now_utc: 0,
            route_ids: &[],
            route_metas: &[],
            active_route: None,
            ride_records: &[],
            ride_retention: RideRetention::Week1,
        }
    }

    fn ride(id: crate::CatalogObjectId, synced: bool, synced_at_utc: u32) -> RideRetentionRecord {
        RideRetentionRecord { id, synced, synced_at_utc }
    }

    #[test]
    fn sweep_deletes_expired_keeps_fresh_and_never() {
        let ids = [10u64, 11, 12];
        let now = 1_000_000;
        let metas = [
            RouteRetentionMeta::new(Retention::Day1, now - 2 * DAY_SECS), // expired
            RouteRetentionMeta::new(Retention::Day1, now - DAY_SECS / 2), // fresh
            RouteRetentionMeta::new(Retention::Never, now - 10 * DAY_SECS), // never
        ];
        let inputs = SweepInputs { now_utc: now, route_ids: &ids, route_metas: &metas, ..ins() };
        let mut out: heapless::Vec<SweepAction, 8> = heapless::Vec::new();
        collect_sweep_actions(&inputs, &mut out);
        assert_eq!(&out[..], &[SweepAction::DeleteRoute(10)], "only the expired non-Never route is deleted");
    }

    #[test]
    fn sweep_stamps_unknown_last_used() {
        let ids = [10u64];
        let metas = [RouteRetentionMeta::new(Retention::Day1, 0)]; // retention set, clock not started
        let inputs = SweepInputs { now_utc: 1_000_000, route_ids: &ids, route_metas: &metas, ..ins() };
        let mut out: heapless::Vec<SweepAction, 8> = heapless::Vec::new();
        collect_sweep_actions(&inputs, &mut out);
        assert_eq!(&out[..], &[SweepAction::StampRoute(10)], "unknown last_used is stamped, not deleted");
    }

    #[test]
    fn sweep_re_stamps_active_route_past_expiry() {
        let ids = [10u64, 11];
        let now = 1_000_000;
        let metas = [
            RouteRetentionMeta::new(Retention::Day1, now - 5 * DAY_SECS), // active + expired
            RouteRetentionMeta::new(Retention::Day1, now - 5 * DAY_SECS), // inactive + expired
        ];
        let inputs = SweepInputs { now_utc: now, route_ids: &ids, route_metas: &metas, active_route: Some(0), ..ins() };
        let mut out: heapless::Vec<SweepAction, 8> = heapless::Vec::new();
        collect_sweep_actions(&inputs, &mut out);
        assert!(out.contains(&SweepAction::StampRoute(10)), "active route re-stamped, not deleted");
        assert!(out.contains(&SweepAction::DeleteRoute(11)), "the inactive expired route is deleted");
        assert!(!out.contains(&SweepAction::DeleteRoute(10)), "the active route is never a delete");
    }

    /// The **delete** sweep only ever deletes a synced+stamped+aged ride — it never stamps (that is
    /// the eager step's job, see `runtime_stamps_synced_rides_eagerly`). A `synced_at == 0` ride is
    /// left untouched here (the eager stamp starts it), and an unsynced ride is never touched.
    #[test]
    fn sweep_ride_rules() {
        let now = 1_000_000;
        let rides = [
            ride(1, true, now - 8 * DAY_SECS), // synced 8 days ago → expired (>7)
            ride(2, true, now - DAY_SECS),     // synced yesterday → kept
            ride(3, true, 0),                  // legacy synced, unstamped → NOT the sweep's job
            ride(4, false, 0),                 // unsynced → untouched (age irrelevant)
        ];
        let inputs = SweepInputs {
            now_utc: now,
            ride_records: &rides,
            ride_retention: RideRetention::Week1, // 7 days
            ..ins()
        };
        let mut out: heapless::Vec<SweepAction, 8> = heapless::Vec::new();
        collect_sweep_actions(&inputs, &mut out);
        assert!(out.contains(&SweepAction::DeleteRide(1)), "synced 8 days ago → deleted (>7)");
        assert!(!out.contains(&SweepAction::DeleteRide(2)), "synced yesterday → kept");
        assert!(!out.iter().any(|a| matches!(a, SweepAction::StampRide(_))), "the delete sweep never stamps");
        assert!(
            !out.iter().any(|a| matches!(a, SweepAction::DeleteRide(3) | SweepAction::DeleteRide(4))),
            "an unstamped-synced (3) and an unsynced (4) ride are never deleted by the sweep"
        );
    }

    /// Finding #876-2: the sweep evaluates the **full** compact inventory, not the newest-32 UI
    /// catalog — an older synced+expired ride (one that would never sit in the display list) is
    /// still selected for deletion, and a legacy `synced_at == 0` ride anywhere in the inventory is
    /// eagerly stamped.
    #[test]
    fn sweep_and_stamp_cover_full_inventory_beyond_ui_cap() {
        let now = 10_000_000;
        // Build an inventory larger than the UI cap: the newest UI_RIDES_CAP are fresh/unsynced, an
        // old one (index past the cap) is synced + expired, and another old one is legacy-unstamped.
        let mut recs: heapless::Vec<RideRetentionRecord, MAX_RIDES> = heapless::Vec::new();
        for i in 0..UI_RIDES_CAP as crate::CatalogObjectId {
            let _ = recs.push(ride(100 + i, false, 0)); // the "visible" newest 32: unsynced
        }
        let _ = recs.push(ride(7, true, now - 30 * DAY_SECS)); // old, synced long ago → expired
        let _ = recs.push(ride(8, true, 0)); // old, legacy synced-without-stamp → needs the eager stamp
        assert!(recs.len() > UI_RIDES_CAP, "the inventory is larger than the display catalog");

        let inputs = SweepInputs { now_utc: now, ride_records: &recs, ride_retention: RideRetention::Week1, ..ins() };
        let mut out: heapless::Vec<SweepAction, SWEEP_QUEUE_CAP> = heapless::Vec::new();
        collect_sweep_actions(&inputs, &mut out);
        assert!(out.contains(&SweepAction::DeleteRide(7)), "an older-than-UI-cap synced+expired ride is deleted");

        // The eager stamp reaches every legacy ride in the inventory, not just the UI-resident ones.
        let mut rt = RetentionMachine::new();
        rt.stamp_synced_rides(&recs);
        assert_eq!(rt.take(SweepKind::StampRide), Some(8), "the legacy synced-unstamped ride is stamped");
        assert_eq!(rt.take(SweepKind::StampRide), None, "only the one unstamped ride needs a stamp");
    }

    /// The eager `synced_at` stamp: every trusted tick enqueues one `StampRide` per synced-and-unstamped
    /// ride (idempotently — never a second for the same id), and leaves stamped/unsynced rides alone.
    #[test]
    fn runtime_stamps_synced_rides_eagerly() {
        let rides = [
            ride(1, true, 0),     // synced, unstamped → stamp
            ride(2, true, 5_000), // synced, already stamped → leave
            ride(3, false, 0),    // unsynced → never
        ];
        let mut rt = RetentionMachine::new();
        rt.stamp_synced_rides(&rides);
        rt.stamp_synced_rides(&rides); // a second tick must not double-enqueue id 1
        assert_eq!(rt.take(SweepKind::StampRide), Some(1), "the unstamped synced ride is stamped");
        assert_eq!(rt.take(SweepKind::StampRide), None, "exactly one stamp, and only for the unstamped ride");
    }

    #[test]
    fn sweep_ride_retention_never_deletes_nothing() {
        let rides = [ride(1, true, 1)]; // synced ages ago
        let inputs =
            SweepInputs { now_utc: u32::MAX, ride_records: &rides, ride_retention: RideRetention::Never, ..ins() };
        let mut out: heapless::Vec<SweepAction, 8> = heapless::Vec::new();
        collect_sweep_actions(&inputs, &mut out);
        assert!(out.is_empty(), "ride_retention Never → nothing");
    }

    #[test]
    fn runtime_active_stamp_fires_once_per_activation() {
        let mut rt = RetentionMachine::new();
        rt.note_active_route(Some(7));
        rt.note_active_route(Some(7)); // unchanged → no second stamp
        assert!(rt.has(SweepKind::StampRoute));
        assert_eq!(rt.take(SweepKind::StampRoute), Some(7));
        assert_eq!(rt.take(SweepKind::StampRoute), None, "only one stamp per activation");
        rt.note_active_route(Some(7)); // still 7 → still no re-stamp
        assert!(!rt.has(SweepKind::StampRoute));
        rt.note_active_route(None); // clear memory
        rt.note_active_route(Some(7)); // re-activating 7 stamps again
        assert_eq!(rt.take(SweepKind::StampRoute), Some(7));
    }

    /// Finding #876-1: the activation safety stamp is **never lost to capacity pressure**. When the
    /// candidate queue is full, `note_active_route` cannot queue the stamp and must *not* advance its
    /// once-per-activation memory — so a later tick (after a slot frees) retries and lands it.
    #[test]
    fn activation_stamp_survives_capacity_pressure() {
        let mut rt = RetentionMachine::new();
        // Saturate the queue with delete candidates.
        for i in 0..SWEEP_QUEUE_CAP as crate::CatalogObjectId {
            rt.test_push(SweepAction::DeleteRoute(i));
        }
        rt.note_active_route(Some(9_999));
        assert!(!rt.has(SweepKind::StampRoute), "no room for the activation stamp yet");
        // A delete drains/cancels, freeing a slot — the next activation tick retries the stamp.
        assert!(rt.cancel(SweepKind::DeleteRoute, 0), "free one slot");
        rt.note_active_route(Some(9_999));
        assert_eq!(rt.take(SweepKind::StampRoute), Some(9_999), "the activation stamp is retried, not lost");
    }

    /// Finding #876-1: `note_active_route` immediately cancels a queued delete candidate for the
    /// route that just became active (belt-and-braces alongside the live drain recheck).
    #[test]
    fn activation_cancels_a_queued_delete_for_that_route() {
        let mut rt = RetentionMachine::new();
        rt.test_push(SweepAction::DeleteRoute(5));
        rt.test_push(SweepAction::DeleteRoute(6));
        rt.note_active_route(Some(5));
        assert_eq!(rt.peek(SweepKind::DeleteRoute), Some(6), "route 5's delete candidate was cancelled on activation");
        assert!(rt.has(SweepKind::StampRoute), "and it is queued for a re-stamp instead");
    }

    #[test]
    fn runtime_hourly_gate_and_empty_precondition() {
        let mut rt = RetentionMachine::new();
        let mut runs = 0;
        // First eligible sweep this boot runs (last_sweep_hour is None).
        rt.maybe_sweep(3600, |q| {
            runs += 1;
            let _ = q.push(SweepAction::DeleteRoute(1));
        });
        assert_eq!(runs, 1);
        // Same hour, queue non-empty → no run.
        rt.maybe_sweep(3600 + 10, |_| runs += 1);
        assert_eq!(runs, 1, "queue not empty and same hour → skipped");
        // Drain, same hour → still skipped (hourly gate).
        assert_eq!(rt.take(SweepKind::DeleteRoute), Some(1));
        rt.maybe_sweep(3600 + 20, |_| runs += 1);
        assert_eq!(runs, 1, "same hour → skipped even with empty queue");
        // Next hour, empty queue → runs.
        rt.maybe_sweep(7200, |_| runs += 1);
        assert_eq!(runs, 2, "next wall-clock hour → sweeps again");
    }

    /// The `setRouteRetention` idempotence pin (epic #638 S4): `set` reports a change only on a real
    /// edit — the board's command handler bumps the route revision on exactly that, so setting the
    /// same value twice is `ok` with **no** bump. A retention change **preserves `last_used`** (the
    /// command must never reset the usage clock): the board sets `{new_level, existing last_used}`.
    #[test]
    fn set_route_retention_change_semantics() {
        let mut store = RouteRetentionStore::new();
        // First set of a level is a change.
        assert!(store.set(7, RouteRetentionMeta::new(Retention::Week2, 1_000)), "first set changes the store");
        // Same value again → no change (the no-bump idempotence pin).
        assert!(!store.set(7, RouteRetentionMeta::new(Retention::Week2, 1_000)), "same value twice → no change");
        // The board's `set_route_retention_level` preserves last_used: read it, set {new, last_used}.
        let preserved = store.get(7).last_used_utc;
        assert_eq!(preserved, 1_000);
        assert!(store.set(7, RouteRetentionMeta::new(Retention::Day1, preserved)), "a real level change is reported");
        assert_eq!(store.get(7), RouteRetentionMeta::new(Retention::Day1, 1_000), "level changed, last_used kept");
        // Reverting to the same new level again → no change.
        assert!(!store.set(7, RouteRetentionMeta::new(Retention::Day1, preserved)), "unchanged again → no change");
    }

    // ==================== the domain boundary (#1437) ====================

    fn view<'a>(
        now_utc: Option<u32>,
        route_ids: &'a [crate::CatalogObjectId],
        route_metas: &'a [RouteRetentionMeta],
        ride_records: &'a [RideRetentionRecord],
    ) -> RetentionView<'a> {
        RetentionView {
            now_utc,
            now_ms: 0,
            recording: false,
            route_ids,
            route_metas,
            active_route: None,
            ride_records,
            ride_retention: RideRetention::Week1,
        }
    }

    /// The safety core is the domain's, not the caller's: without a clock established this boot,
    /// `advance` stamps nothing, sweeps nothing and discovers nothing — however expired the data
    /// looks (invariant 1).
    #[test]
    fn the_machine_does_nothing_without_a_trusted_clock() {
        let ids = [10u64];
        let metas = [RouteRetentionMeta::new(Retention::Day1, 1)]; // "used" at unix 1 → ancient
        let rides = [ride(7, true, 1)];
        let mut rt = RetentionMachine::new();

        rt.advance(&view(None, &ids, &metas, &rides));
        assert!(!rt.has(SweepKind::DeleteRoute) && !rt.has(SweepKind::StampRide), "no clock, no candidates");
        assert!(rt.next_metadata_effect(SweepKind::StampRide, &view(None, &ids, &metas, &rides)).is_none());
        assert!(rt.next_expiry(SweepKind::DeleteRoute, &view(None, &ids, &metas, &rides)).is_none());
    }

    /// A discovered expiry leaves as a **catalog intent**, never as a delete the domain performs:
    /// the route and the ride namespaces each become the intent `CatalogMachine` already accepts
    /// from a rider's own delete.
    #[test]
    fn an_expiry_leaves_as_a_catalog_intent() {
        let now = 10_000_000;
        let ids = [10u64];
        let metas = [RouteRetentionMeta::new(Retention::Day1, now - 3 * DAY_SECS)];
        let rides = [ride(7, true, now - 30 * DAY_SECS)];
        let mut rt = RetentionMachine::new();
        rt.advance(&view(Some(now), &ids, &metas, &rides));

        let v = view(Some(now), &ids, &metas, &rides);
        assert_eq!(rt.next_expiry(SweepKind::DeleteRoute, &v), Some(CatalogIntent::DeleteRoute { id: 10 }));
        assert_eq!(rt.next_expiry(SweepKind::DeleteRide, &v), Some(CatalogIntent::DeleteRide { id: 7 }));
        // The candidate is *kept* until storage confirms it gone, and the bounded backoff paces the
        // retry rather than letting it re-fire every pass.
        assert!(rt.next_expiry(SweepKind::DeleteRoute, &v).is_none(), "the dispatched delete waits out the backoff");
        let later = RetentionView { now_ms: RETENTION_DELETE_BACKOFF_MS, ..view(Some(now), &ids, &metas, &rides) };
        assert_eq!(
            rt.next_expiry(SweepKind::DeleteRoute, &later),
            Some(CatalogIntent::DeleteRoute { id: 10 }),
            "and then the same candidate retries — storage never confirmed it gone"
        );
    }

    /// A stamp leaves as a typed metadata effect carrying the live retention *level* beside a fresh
    /// `last_used` — a use stamp never rewrites the level — and a failed write re-queues it.
    #[test]
    fn a_metadata_effect_carries_the_level_and_a_failure_requeues_it() {
        let now = 500_000;
        let ids = [10u64];
        let metas = [RouteRetentionMeta::new(Retention::Week2, 0)]; // level set, clock never started
        let rides: [RideRetentionRecord; 0] = [];
        let mut rt = RetentionMachine::new();
        rt.advance(&view(Some(now), &ids, &metas, &rides));

        let effect = rt.next_metadata_effect(SweepKind::StampRoute, &view(Some(now), &ids, &metas, &rides));
        let Some(RetentionEffect::WriteRouteMetadata { token, id, meta }) = effect else {
            panic!("the unknown last_used is stamped, not deleted")
        };
        assert_eq!(id, 10);
        assert_eq!(meta, RouteRetentionMeta::new(Retention::Week2, now), "the level survives a use stamp");
        assert!(
            rt.next_metadata_effect(SweepKind::StampRoute, &view(Some(now), &ids, &metas, &rides)).is_none(),
            "one sidecar write in flight at a time"
        );

        // A failed write is retried on a later pass rather than waiting for the next hourly sweep.
        rt.apply_outcome(RetentionOutcome::Failed { token, error: RetentionError::WriteFailed });
        assert!(rt.has(SweepKind::StampRoute), "the failed stamp is queued again");
        let retry = rt.next_metadata_effect(SweepKind::StampRoute, &view(Some(now), &ids, &metas, &rides));
        let Some(RetentionEffect::WriteRouteMetadata { token: retry_token, .. }) = retry else {
            panic!("the retry goes out")
        };

        // The success is terminal, and a repeat of it is stale — nothing is re-queued twice.
        rt.apply_outcome(RetentionOutcome::RouteMetadataWritten { token: retry_token, id: 10 });
        rt.apply_outcome(RetentionOutcome::Failed { token: retry_token, error: RetentionError::WriteFailed });
        assert!(!rt.has(SweepKind::StampRoute), "a repeated outcome cannot resurrect a written stamp");
    }

    /// The route-upload `last_used` stamp (epic #638 S4): `note_route_uploaded` enqueues exactly one
    /// `StampRoute` per uploaded id, idempotently (a re-fire before the drain must not double-enqueue).
    #[test]
    fn runtime_note_route_uploaded_enqueues_once() {
        let mut rt = RetentionMachine::new();
        rt.note_route_uploaded(7);
        rt.note_route_uploaded(7); // a second call before draining must not stack a duplicate
        assert_eq!(rt.take(SweepKind::StampRoute), Some(7), "the uploaded route is stamped");
        assert_eq!(rt.take(SweepKind::StampRoute), None, "exactly one stamp per upload");
        // A different id enqueues its own stamp.
        rt.note_route_uploaded(8);
        assert_eq!(rt.take(SweepKind::StampRoute), Some(8));
    }
}

// ==================== the Retention domain protocol (#1436) ====================
//
// RetentionMachine owns the stamps, the deadlines and the sweep. It advances *before*
// CatalogMachine, so an expiry it decides on this pass reaches the catalog as a `CatalogIntent`
// in the same pass — the deletion itself is never a retention effect. What is left here is the
// device-local sidecar write: two bounded metadata writes that bump no store revision.

use crate::catalog_state::CatalogIntent;
use crate::device_core::{OperationToken, RetentionTag};
use crate::CatalogObjectId;

/// What the rest of the device asks of retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionIntent {
    /// Re-run the sweep. Raised when an input the policy depends on moves: the clock becomes
    /// trusted, the catalog changes, a ride ends, or the day rolls over.
    Recheck,
    /// A route's retention metadata changed — the rider picked a level, or the route was used.
    RouteMetadataChanged { id: CatalogObjectId, meta: RouteRetentionMeta },
    /// A ride's sync stamp changed — the companion acknowledged the ride.
    RideMetadataChanged { id: CatalogObjectId, synced_at: u32 },
}

/// One bounded sidecar write, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionEffect {
    /// Write route `id`'s retention level and last-used stamp to the route-retention sidecar.
    WriteRouteMetadata { token: OperationToken<RetentionTag>, id: CatalogObjectId, meta: RouteRetentionMeta },
    /// Write ride `id`'s `synced_at` stamp to the synced-set sidecar.
    WriteRideMetadata { token: OperationToken<RetentionTag>, id: CatalogObjectId, synced_at: u32 },
}

impl RetentionEffect {
    /// The operation this effect belongs to.
    pub fn token(&self) -> OperationToken<RetentionTag> {
        match self {
            RetentionEffect::WriteRouteMetadata { token, .. } | RetentionEffect::WriteRideMetadata { token, .. } => {
                *token
            }
        }
    }
}

/// Why a metadata write failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionError {
    /// The sidecar write failed. Retention is the safe-direction store: an unwritten stamp reads
    /// back as unknown, which never deletes — so this is retried, never escalated.
    WriteFailed,
}

/// The result of one [`RetentionEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionOutcome {
    /// Route `id`'s metadata is durable.
    RouteMetadataWritten { token: OperationToken<RetentionTag>, id: CatalogObjectId },
    /// Ride `id`'s metadata is durable.
    RideMetadataWritten { token: OperationToken<RetentionTag>, id: CatalogObjectId },
    /// The write failed.
    Failed { token: OperationToken<RetentionTag>, error: RetentionError },
    /// The executor abandoned the write without completing it.
    Cancelled { token: OperationToken<RetentionTag> },
}

impl RetentionOutcome {
    /// The operation this outcome answers.
    pub fn token(&self) -> OperationToken<RetentionTag> {
        match self {
            RetentionOutcome::RouteMetadataWritten { token, .. }
            | RetentionOutcome::RideMetadataWritten { token, .. }
            | RetentionOutcome::Failed { token, .. }
            | RetentionOutcome::Cancelled { token } => *token,
        }
    }
}

// Layout tripwires: an identity plus the two-field sidecar record, never the sidecar itself.
const _: () = assert!(core::mem::size_of::<RetentionIntent>() <= 24, "an identity and a stamp");
const _: () = assert!(core::mem::size_of::<RetentionEffect>() <= 24, "a token, an identity and a stamp");
const _: () = assert!(core::mem::size_of::<RetentionOutcome>() <= 16, "a token and an identity");
const _: () = assert!(core::mem::size_of::<RetentionError>() <= 1, "a verdict, not a report");
