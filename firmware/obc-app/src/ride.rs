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

use obc_route::{RideInfo, NAME_CAP};

/// Maximum rides the resident menu catalog holds. Rides accumulate (the device keeps every tracked
/// ride until a manual delete), so this is roomier than [`MAX_ROUTES`](crate::route::MAX_ROUTES);
/// past it the newest rides simply aren't listed until the card is tidied.
pub const MAX_RIDES: usize = 128;

/// The app's resident ride catalog: the summaries the Rides screen lists.
pub type RideCatalog = heapless::Vec<RideSummary, MAX_RIDES>;

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
}

impl RideSummary {
    /// Build a summary from a stored ride's [`RideInfo`] header and its device-local synced flag.
    pub fn from_info(info: &RideInfo, synced: bool) -> Self {
        RideSummary {
            name: info.name.clone(),
            start_time: info.start_time,
            distance_m: info.distance_m,
            moving_time_s: info.moving_time_s,
            climb_m: info.climb_m,
            synced,
        }
    }
}
