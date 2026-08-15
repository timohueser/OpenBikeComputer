//! Host-tested transition latches for the device ride loop's route/storage phase.
//!
//! The large index, hardware resources, guards, weather adapter, and async scheduling stay on the
//! board. This owner only remembers what was reconciled and whether that route has a usable index.

/// The route/storage transition latches resident in the board's ride-loop future.
pub struct RideRuntime {
    reconciled_route: Option<usize>,
    reconciled_session: Option<u32>,
    indexed_route: Option<usize>,
}

impl RideRuntime {
    /// Boot state: the first pass must reconcile storage, and no route index is usable.
    pub const fn new() -> Self {
        RideRuntime { reconciled_route: None, reconciled_session: None, indexed_route: None }
    }

    /// Whether route/track storage must be reconciled this pass.
    #[inline]
    pub fn storage_reconcile_due(
        &self,
        active_route: Option<usize>,
        session: Option<u32>,
        finish_pending: bool,
    ) -> bool {
        active_route != self.reconciled_route || session != self.reconciled_session || finish_pending
    }

    /// Record a completed route/track reconcile.
    #[inline]
    pub fn storage_reconciled(&mut self, active_route: Option<usize>, session: Option<u32>) {
        self.reconciled_route = active_route;
        self.reconciled_session = session;
    }

    /// Invalidate route-derived state after a catalog rescan or computed-route replacement.
    #[inline]
    pub fn invalidate_route(&mut self) {
        self.reconciled_route = None;
        self.indexed_route = None;
    }

    /// Whether the resident chunk-index slot must be rebuilt for `active_route`.
    #[inline]
    pub fn route_index_needs_rebuild(&self, active_route: Option<usize>) -> bool {
        self.indexed_route != active_route
    }

    /// Record that the board's placement-sensitive slot now contains `active_route`'s index.
    #[inline]
    pub fn route_index_built(&mut self, active_route: Option<usize>) {
        debug_assert!(active_route.is_some());
        self.indexed_route = active_route;
    }

    /// Record no usable index. An active route retries next pass; no active route stays quiet.
    #[inline]
    pub fn route_index_unavailable(&mut self) {
        self.indexed_route = None;
    }

    /// Whether the board may pair its resident index slot with the current route source.
    #[inline]
    pub fn route_index_ready(&self) -> bool {
        self.indexed_route.is_some()
    }
}

impl Default for RideRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::RideRuntime;

    #[test]
    fn boot_and_each_independent_edge_require_storage_reconcile() {
        let mut runtime = RideRuntime::new();
        assert!(!runtime.storage_reconcile_due(None, None, false));
        assert!(runtime.storage_reconcile_due(None, Some(0), false));

        runtime.storage_reconciled(None, Some(0));
        assert!(!runtime.storage_reconcile_due(None, Some(0), false));
        assert!(runtime.storage_reconcile_due(Some(2), Some(0), false));
        assert!(runtime.storage_reconcile_due(None, Some(1), false));
        assert!(runtime.storage_reconcile_due(None, Some(0), true));
    }

    #[test]
    fn index_result_is_one_key_without_a_duplicate_validity_flag() {
        let mut runtime = RideRuntime::new();
        assert!(runtime.route_index_needs_rebuild(Some(2)));
        assert!(!runtime.route_index_ready());

        runtime.route_index_unavailable();
        assert!(runtime.route_index_needs_rebuild(Some(2)));

        runtime.route_index_built(Some(2));
        assert!(runtime.route_index_ready());
        assert!(!runtime.route_index_needs_rebuild(Some(2)));
        assert!(runtime.route_index_needs_rebuild(Some(3)));

        runtime.route_index_unavailable();
        assert!(!runtime.route_index_needs_rebuild(None));
        assert!(!runtime.route_index_ready());

        runtime.storage_reconciled(Some(4), Some(7));
        runtime.route_index_built(Some(4));
        runtime.invalidate_route();
        assert!(runtime.storage_reconcile_due(Some(4), Some(7), false));
        assert!(runtime.route_index_needs_rebuild(Some(4)));
        assert!(!runtime.route_index_ready());
    }
}
