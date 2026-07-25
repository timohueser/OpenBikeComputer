//! Trips — the grouped-route folders shown above the loose routes in the Route menu (epic #526).
//!
//! A **trip** is a tiny metadata object ([`obc_route::TripMeta`], `TP{id}.OBT` on the device) that
//! references route object ids in ride order. The app resolves those ids against its resident route
//! [`Catalog`](crate::route::Catalog) — whose parallel `catalog_ids` (#450) carry each route's
//! durable object id — into a [`TripSummary`]: the stage *indices* into the catalog (ride order,
//! dangling refs dropped) plus the summed distance / climb over the resolvable stages.
//!
//! Grouping rule (spec §7.7): a route referenced by a stored trip is **filed** and shows only inside
//! its folder; the menu's top level lists trips + unfiled routes. A dangling ref (a member route
//! deleted individually) resolves to nothing and drops from `stage_indices` — the stats sum only
//! what resolves — but a trip whose every ref dangles still lists (empty folder) so it can be
//! deleted on-device.
//!
//! The device UI rows land in TR3; this module only builds the grouped model. The flat Route menu
//! (which reads the full [`Catalog`](crate::route::Catalog)) is untouched, so filed routes keep
//! listing until TR3 wires the folders.

use heapless::{String, Vec};

use obc_formats::obcr::NAME_CAP;
use obc_route::MAX_TRIP_STAGES;

use crate::route::RouteSummary;

/// Maximum trips the resident menu catalog holds (epic #526: cap 16). Each [`TripSummary`] costs
/// a name + two small stage `Vec`s (~`4·MAX_TRIP_STAGES` bytes), so the table is a couple of KB
/// of static RAM.
pub const MAX_TRIPS: usize = 16;

/// The app's resident trip catalog: the folders the Route menu lists above the unfiled routes.
pub type Trips = heapless::Vec<TripSummary, MAX_TRIPS>;

/// A host-scanned trip handed to [`App::set_trips`](crate::App::set_trips): the trip's durable object
/// id, its name, and its stage route ids in ride order (as stored — dangling ids and all). The app
/// resolves the ids against the live route catalog; the host owns only the raw metadata (the sim
/// scans `TP{id}.OBT`, the board its `ObjectStore`).
#[derive(Debug, Clone, Copy)]
pub struct TripInput<'a> {
    pub id: u16,
    pub name: &'a str,
    pub stage_ids: &'a [u16],
}

/// A resolved trip: its identity + name, the route object ids it references (kept verbatim so a
/// catalog rescan can re-resolve and so a fully-dangling trip is still deletable), the resolved
/// catalog **indices** in ride order (dangling refs dropped), and the summed stats over the
/// resolvable stages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripSummary {
    /// The trip's durable object id (its own device counter, separate from routes/rides).
    pub id: u16,
    pub name: String<NAME_CAP>,
    /// The stage route ids **as stored**, ride order — the resolution source of truth (re-run on a
    /// catalog rescan) and, for a fully-dangling trip, the only thing left to key a delete on.
    pub stage_ids: Vec<u16, MAX_TRIP_STAGES>,
    /// The resolved catalog indices, ride order — one per **resolvable** stage (a dangling id is
    /// skipped, so this can be shorter than [`stage_ids`](TripSummary::stage_ids)).
    pub stage_indices: Vec<u16, MAX_TRIP_STAGES>,
    /// Summed distance over the resolvable stages, km (rounded — the catalog's display unit).
    pub distance_km: u32,
    /// Summed ascent over the resolvable stages, m.
    pub climb_m: u32,
}

impl TripSummary {
    /// Whether every stored stage dangled — a folder that resolves to no routes. Still listed (so it
    /// can be deleted on-device) but empty.
    pub fn is_empty_folder(&self) -> bool {
        self.stage_indices.is_empty()
    }

    /// Build a resolved trip from a host [`TripInput`] against the route catalog: `catalog[i]` is the
    /// summary whose durable id is `catalog_ids[i]`. Each stage id is looked up in `catalog_ids`;
    /// a hit contributes its catalog index (ride order) and its distance/climb, a miss (dangling ref)
    /// is dropped from the resolved list but stays in `stage_ids`.
    pub fn resolve(input: &TripInput, catalog: &[RouteSummary], catalog_ids: &[u16]) -> TripSummary {
        let mut name = String::new();
        let _ = name.push_str(truncate_on_char_boundary(input.name, NAME_CAP));

        let mut stage_ids = Vec::new();
        let mut stage_indices = Vec::new();
        let mut distance_km = 0u32;
        let mut climb_m = 0u32;
        for &sid in input.stage_ids.iter().take(MAX_TRIP_STAGES) {
            let _ = stage_ids.push(sid);
            if let Some(idx) = catalog_ids.iter().position(|&x| x == sid) {
                let _ = stage_indices.push(idx as u16);
                if let Some(r) = catalog.get(idx) {
                    distance_km = distance_km.saturating_add(r.distance_km);
                    climb_m = climb_m.saturating_add(r.climb_m);
                }
            }
        }
        TripSummary { id: input.id, name, stage_ids, stage_indices, distance_km, climb_m }
    }

    /// Re-resolve this trip's [`stage_indices`](TripSummary::stage_indices) + stats against a
    /// (possibly changed) catalog, from the verbatim [`stage_ids`](TripSummary::stage_ids). Called
    /// on a route rescan so a route that appeared/vanished re-files correctly without the host having
    /// to re-feed the trips.
    pub fn reresolve(&mut self, catalog: &[RouteSummary], catalog_ids: &[u16]) {
        self.stage_indices.clear();
        self.distance_km = 0;
        self.climb_m = 0;
        for &sid in self.stage_ids.iter() {
            if let Some(idx) = catalog_ids.iter().position(|&x| x == sid) {
                let _ = self.stage_indices.push(idx as u16);
                if let Some(r) = catalog.get(idx) {
                    self.distance_km = self.distance_km.saturating_add(r.distance_km);
                    self.climb_m = self.climb_m.saturating_add(r.climb_m);
                }
            }
        }
    }
}

/// The longest prefix of `s` that fits in `cap` bytes without splitting a multi-byte char.
fn truncate_on_char_boundary(s: &str, cap: usize) -> &str {
    let mut end = s.len().min(cap);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
