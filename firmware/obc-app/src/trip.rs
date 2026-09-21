//! Trips — the grouped-route folders shown above the loose routes in the Route menu.
//!
//! A trip is a small metadata object ([`obc_route::TripMeta`], `TP{id}.OBT` on the device) that
//! references route object ids in ride order. The app resolves those ids against its resident route
//! [`Catalog`](crate::route::Catalog) into a [`TripSummary`]: the stage indices into the catalog, in
//! ride order, plus the summed distance and climb over the resolvable stages.
//!
//! A route a stored trip references is filed and shows only inside its folder. A dangling ref
//! resolves to nothing and drops from `stage_indices`, but a trip whose every ref dangles still
//! lists, so it can be deleted on-device.

use heapless::{String, Vec};

use obc_formats::obcr::NAME_CAP;
use obc_route::MAX_TRIP_STAGES;

use crate::route::RouteSummary;
use crate::CatalogObjectId;

/// Maximum trips the resident menu catalog holds. Each [`TripSummary`] costs a name and two small
/// stage `Vec`s, so the table is a couple of KB of static RAM.
pub const MAX_TRIPS: usize = 16;

/// The app's resident trip catalog: the folders the Route menu lists above the unfiled routes.
pub type Trips = heapless::Vec<TripSummary, MAX_TRIPS>;

/// A host-scanned trip handed to [`App::set_trips`](crate::App::set_trips): the trip's durable
/// object id, its name, and its stage route ids in ride order, as stored. The host owns only the raw
/// metadata; the app resolves the ids against the live route catalog.
#[derive(Debug, Clone, Copy)]
pub struct TripInput<'a> {
    pub id: CatalogObjectId,
    pub name: &'a str,
    pub stage_ids: &'a [CatalogObjectId],
}

/// A resolved trip: its identity and name, the route object ids it references, the resolved catalog
/// indices in ride order, and the summed stats over the resolvable stages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripSummary {
    /// The trip's durable object id (its own device counter, separate from routes/rides).
    pub id: CatalogObjectId,
    pub name: String<NAME_CAP>,
    /// The stage route ids as stored, in ride order. They are the resolution source of truth on a
    /// catalog rescan, and for a fully-dangling trip the only thing left to key a delete on.
    pub stage_ids: Vec<CatalogObjectId, MAX_TRIP_STAGES>,
    /// The resolved catalog indices, ride order — one per resolvable stage, so a dangling id makes
    /// this shorter than [`stage_ids`](TripSummary::stage_ids).
    pub stage_indices: Vec<u16, MAX_TRIP_STAGES>,
    /// Summed distance over the resolvable stages, km — the catalog's display unit.
    pub distance_km: u32,
    pub climb_m: u32,
}

impl TripSummary {
    /// Whether every stored stage dangled. An empty folder is still listed, so it can be deleted
    /// on-device.
    pub fn is_empty_folder(&self) -> bool {
        self.stage_indices.is_empty()
    }

    /// Build a resolved trip from a host [`TripInput`] against the route catalog: `catalog[i]` is
    /// the summary whose durable id is `catalog_ids[i]`. A dangling stage id is dropped from the
    /// resolved list but stays in `stage_ids`.
    pub fn resolve(input: &TripInput, catalog: &[RouteSummary], catalog_ids: &[CatalogObjectId]) -> TripSummary {
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

    /// Re-resolve this trip's [`stage_indices`](TripSummary::stage_indices) and stats from
    /// [`stage_ids`](TripSummary::stage_ids). A route rescan calls it, so a route that appeared or
    /// vanished re-files without the host re-feeding the trips.
    pub fn reresolve(&mut self, catalog: &[RouteSummary], catalog_ids: &[CatalogObjectId]) {
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
