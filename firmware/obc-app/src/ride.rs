//! Rides — the recorded rides shown in the Rides screen (epic #447, P7 / #454).
//!
//! A stored Ride object is described to the UI by a [`RideSummary`]: the v3 footer facts
//! ([`obc_route::RideInfo`]) — name + start time + totals — with no resident track geometry. The
//! host lists the device's flat catalog (the simulator may use files as its stand-in) and hands the
//! paired identities and summaries to [`App::set_rides`](crate::App::set_rides).
//!
//! Each summary carries a host-supplied `synced` flag. Its future flat ride-domain persistence is
//! #1398's boundary; FS8 intentionally has no FAT sidecar compatibility path.

use heapless::String;

use obc_formats::obcr::NAME_CAP;
use obc_route::RideInfo;

/// Maximum rides the host-facing inventory can hand to retention/catalog logic. The resident menu
/// catalog is deliberately smaller ([`UI_RIDES_CAP`]).
pub const MAX_RIDES: usize = 128;

/// Maximum resident menu rows. The separate compact retention inventory can
/// cover older rides without retaining their full summaries.
pub const UI_RIDES_CAP: usize = 32;

/// The app's resident ride catalog: the paired entries the Rides screen lists (newest first, capped at
/// [`UI_RIDES_CAP`]).
pub type RideCatalog = heapless::Vec<RideEntry, UI_RIDES_CAP>;

/// One stored ride's durable identity and menu facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideEntry {
    pub id: crate::CatalogObjectId,
    pub summary: RideSummary,
}

// Pairing must not add per-row padding on the board or host.
const _: () = assert!(
    core::mem::size_of::<RideEntry>()
        == core::mem::size_of::<RideSummary>() + core::mem::size_of::<crate::CatalogObjectId>()
);

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
    /// Whether exact durable client archive proof exists. The delete footer warns when false.
    pub synced: bool,
    /// First trusted UTC retention stamp for the archived ride. Zero protects an unstamped proof;
    /// retention starts its clock through a checked metadata write before it can expire.
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
