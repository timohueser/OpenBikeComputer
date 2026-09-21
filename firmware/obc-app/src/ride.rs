//! Rides — the recorded rides shown in the Rides screen.
//!
//! A stored Ride object is described to the UI by a [`RideSummary`]: the footer facts
//! ([`obc_route::RideInfo`]) with no resident track geometry. The host lists the device's flat
//! catalog and hands the paired identities and summaries to
//! [`App::set_rides`](crate::App::set_rides).

use heapless::String;

use obc_formats::obcr::NAME_CAP;
use obc_route::RideInfo;
pub const MAX_RIDES: usize = 128;
pub const UI_RIDES_CAP: usize = 32;

/// The app's resident ride catalog: the entries the Rides screen lists, newest first.
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

/// A stored ride's header facts for the Rides screen, plus the device-local `synced` flag the
/// unsynced-delete guard keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideSummary {
    /// Ride name (truncated to [`NAME_CAP`] on a char boundary), for the row's first line.
    pub name: String<NAME_CAP>,
    /// Ride start, unix seconds — the row's date line and the list sort key.
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub climb_m: u16,
    /// Whether exact durable client archive proof exists. The delete footer warns when false.
    pub synced: bool,
    pub synced_at_utc: u32,
}

impl RideSummary {
    /// `synced_at_utc` is `0` when the ride is unsynced or unstamped.
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
