//! Same-pass messages between device domains.
use super::Slot;
use crate::{catalog_state::CatalogIntent, screen::WarningFlags, CatalogObjectId};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveRouteRemoved {
    pub route: CatalogObjectId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideFinalized {
    pub ride: CatalogObjectId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultNotices(WarningFlags);

impl FaultNotices {
    pub const NONE: FaultNotices = FaultNotices(WarningFlags::NONE);

    /// Raise `flags`. Never displaces what another domain already raised.
    pub fn raise(&mut self, flags: WarningFlags) {
        self.0 |= flags;
    }

    pub fn take(&mut self) -> WarningFlags {
        core::mem::replace(&mut self.0, WarningFlags::NONE)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Connections {
    pub ui_catalog: Slot<CatalogIntent>,
    pub active_route_removed: Slot<ActiveRouteRemoved>,
    pub ride_finalized: Slot<RideFinalized>,
    pub faults: FaultNotices,
}
impl Connections {
    pub const fn new() -> Self {
        Self {
            ui_catalog: Slot::new(),
            active_route_removed: Slot::new(),
            ride_finalized: Slot::new(),
            faults: FaultNotices::NONE,
        }
    }
}
impl Default for Connections {
    fn default() -> Self {
        Self::new()
    }
}
