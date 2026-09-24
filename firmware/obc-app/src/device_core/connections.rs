//! Same-pass messages between device domains.
use super::Slot;
use crate::{catalog_state::CatalogIntent, Alerts, CatalogObjectId};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveRouteRemoved {
    pub route: CatalogObjectId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideFinalized {
    pub ride: CatalogObjectId,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Connections {
    pub ui_catalog: Slot<CatalogIntent>,
    pub active_route_removed: Slot<ActiveRouteRemoved>,
    pub ride_finalized: Slot<RideFinalized>,
    /// Every alert a domain raised this pass, delivered together at the end of it.
    pub alerts: Alerts,
}
impl Connections {
    pub const fn new() -> Self {
        Self {
            ui_catalog: Slot::new(),
            active_route_removed: Slot::new(),
            ride_finalized: Slot::new(),
            alerts: Alerts::NONE,
        }
    }
}
impl Default for Connections {
    fn default() -> Self {
        Self::new()
    }
}
