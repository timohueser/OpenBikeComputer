//! Rides — the recorded rides shown in the Rides screen.
//!
//! A stored Ride object is described to the UI by a [`RideSummary`]: the footer facts
//! ([`obc_route::RideInfo`]) with no resident track geometry. The host lists the device's flat
//! catalog and hands the paired identities and summaries to
//! [`App::set_rides`](crate::App::set_rides).

use heapless::String;

use obc_formats::obcr::NAME_CAP;
use obc_formats::ride::TripRef;
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

// The board holds `UI_RIDES_CAP` entries resident; a field that grows one costs 32 times its size.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<RideEntry>() == 112);

/// A stored ride's header facts for the Rides screen, plus the device-local `synced` flag the
/// unsynced-delete guard keys on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// The trip day the ride started on. The Rides screen groups a trip's rides into one folder,
    /// named from [`RideTrips`].
    pub trip: Option<TripRef>,
    pub avg_hr: Option<u8>,
    pub avg_cadence: Option<u8>,
    pub avg_power: Option<u16>,
    pub energy_kj: Option<u32>,
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
            trip: info.trip,
            avg_hr: info.avg_hr,
            avg_cadence: info.avg_cadence,
            avg_power: info.avg_power,
            energy_kj: info.energy_kj,
        }
    }
}

/// How many trip folders of the ride catalog carry their trip's name: one for every four rides of
/// [`UI_RIDES_CAP`], because a trip is several days. A folder past the cap shows its newest ride's
/// name instead.
pub const RIDE_TRIPS_CAP: usize = UI_RIDES_CAP / 4;

/// The names of the ride catalog's trips, one per trip key.
pub type RideTrips = heapless::Vec<RideTrip, RIDE_TRIPS_CAP>;

/// A trip's name as a ride footer stores it, so a folder keeps its name after the trip is deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideTrip {
    pub key: u64,
    pub name: String<NAME_CAP>,
}

impl RideTrip {
    /// Note the trip name of a ride in the catalog. Feed the rides newest first: the first
    /// non-empty name of a trip wins, and a full table keeps what it has.
    pub fn note(trips: &mut RideTrips, info: &RideInfo) {
        let Some(trip) = info.trip else { return };
        if info.trip_name.is_empty() || trips.iter().any(|t| t.key == trip.key()) {
            return;
        }
        let _ = trips.push(RideTrip { key: trip.key(), name: info.trip_name.clone() });
    }
}
