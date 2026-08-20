//! Rides — the recorded rides shown in the Rides screen (epic #447, P7 / #454).
//!
//! A stored Ride object is described to the UI by a [`RideSummary`]: the v3 footer facts
//! ([`obc_route::RideInfo`]) — name + start time + totals — with no resident track geometry. The
//! host lists the device's flat catalog (the simulator may use files as its stand-in) and hands the
//! summaries to [`App::set_rides`](crate::App::set_rides).
//!
//! Each summary carries a host-supplied `synced` flag. Its future flat ride-domain persistence is
//! #1398's boundary; FS8 intentionally has no FAT sidecar compatibility path.

use heapless::String;

use obc_formats::obcr::NAME_CAP;
use obc_route::RideInfo;

/// Maximum rides the host-facing inventory can hand to retention/catalog logic. The resident menu
/// catalog is deliberately smaller ([`UI_RIDES_CAP`]).
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
