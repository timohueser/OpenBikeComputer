use super::*;
use crate::{
    flat_rides::tests::{receipt, seed},
    flat_store::HostStore,
    FlatRideStore, FlatRouteStore, MemTrackStore,
};
use obc_app::{
    device_core::{CatalogTag, RetentionTag, TokenSource},
    retention::RetentionError,
    AppState,
};
use obc_ports::{InputClock, LocationSource, RideClock};

struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<obc_ports::Fix> {
        None
    }
}

struct Rig {
    owner: HostStore,
    app: App,
    host: HostLoop,
    routes: FlatRouteStore,
    rides: FlatRideStore,
    now: u32,
    stamps: usize,
    removals: usize,
}
impl Rig {
    fn new(owner: HostStore) -> Self {
        Self {
            routes: FlatRouteStore::new(HostStore(owner.0.clone()), &[]).unwrap(),
            rides: FlatRideStore::new(HostStore(owner.0.clone())).unwrap(),
            owner,
            app: App::new_idle(AppState::new(0, 0, 1.0)),
            host: HostLoop::new(),
            now: 0,
            stamps: 0,
            removals: 0,
        }
    }
    fn step(&mut self) {
        self.now += 1000;
        self.host.facts().note_store_revision(self.routes.store_scope().unwrap());
        let mut loc = NoFix;
        let mut plan = self.host.pass(
            &mut self.app,
            PassClock { ride: RideClock(self.now), ui: InputClock(self.now) },
            &[],
            Sensors::new(&mut loc),
            None,
            None,
            PlatformSupport { retention_metadata: true, ..Default::default() },
        );
        if let Some(effect) = plan.effects.retention.take() {
            if let RetentionEffect::WriteRideMetadata { .. } = effect {
                self.stamps += 1;
            }
            plan.effects.retention.try_put(effect).unwrap();
        }
        if let Some(effect) = plan.effects.catalog.take() {
            if let CatalogEffect::ExpireObject { .. } = effect {
                self.removals += 1;
            }
            plan.effects.catalog.try_put(effect).unwrap();
        }
        self.host.serve_effects(
            &mut self.app,
            &mut plan,
            &mut self.routes,
            &mut self.rides,
            &mut (),
            &mut MemTrackStore::new(),
            &mut (),
        );
    }
    fn settle(&mut self) {
        for _ in 0..12 {
            self.step();
        }
    }
}

#[test]
fn actual_receipt_enters_host_policy_only_after_durable_reload_and_expires_on_its_original_clock() {
    let owner = HostStore::memory().unwrap();
    let archived = seed(&owner);
    let unsynced = seed(&owner);
    assert_eq!(receipt(&owner, archived), 0);
    let mut rig = Rig::new(owner);
    rig.settle();
    assert_eq!(rig.stamps, 0);
    assert_eq!(rig.removals, 0);
    assert!(rig.app.rides().iter().find(|ride| ride.id == archived.id.0).unwrap().summary.synced);
    assert!(!rig.app.rides().iter().find(|ride| ride.id == unsynced.id.0).unwrap().summary.synced);
    rig.app.stamp_clock_ble(1_700_000_000, 0);
    // The executor's successful write alone cannot modify the resident summary.
    while rig.stamps == 0 {
        rig.step();
        assert!(rig.now < 60_000);
    }
    assert_eq!(rig.app.rides().iter().find(|ride| ride.id == archived.id.0).unwrap().summary.synced_at_utc, 0);
    rig.settle();
    assert_eq!(rig.stamps, 1);
    let stamp = rig.app.rides().iter().find(|ride| ride.id == archived.id.0).unwrap().summary.synced_at_utc;
    assert!(stamp >= 1_700_000_000);
    assert_eq!(receipt(&rig.owner, archived), stamp);
    let scope = rig.routes.store_scope().unwrap();
    let mut ops = TokenSource::<RetentionTag>::new();
    rig.rides
        .write_metadata(RetentionEffect::WriteRideMetadata {
            token: ops.issue(),
            scope: Some(scope),
            id: archived.id.0,
            synced_at: stamp + 1000,
        })
        .unwrap();
    assert_eq!(rig.routes.store_scope(), Some(scope), "duplicate clock start makes no commit");
    assert_eq!(receipt(&rig.owner, archived), stamp);
    rig.app.stamp_clock_ble(stamp + 7 * 86400 - 30, 0);
    rig.settle();
    assert_eq!(rig.removals, 0);
    rig.app.stamp_clock_ble(stamp + 7 * 86400, 0);
    rig.app.force_retention_sweep();
    rig.settle();
    assert_eq!(rig.removals, 1);
    assert_eq!(rig.app.rides().len(), 1);
    assert_eq!(rig.app.rides()[0].id, unsynced.id.0);
}

#[test]
fn host_refuses_mixed_card_catalogs_and_stale_stamp_and_expiry_effects() {
    let owner = HostStore::memory().unwrap();
    let archived = seed(&owner);
    receipt(&owner, archived);
    let mut rig = Rig::new(owner);
    rig.settle();
    let scope = rig.routes.store_scope().unwrap();
    let mut ops = TokenSource::<RetentionTag>::new();
    let effect = RetentionEffect::WriteRideMetadata {
        token: ops.issue(),
        scope: Some(scope),
        id: archived.id.0,
        synced_at: 1_700_000_000,
    };
    seed(&rig.owner);
    assert_eq!(rig.rides.write_metadata(effect), Err(RetentionError::Stale));
    assert_eq!(rig.rides.expire_ride(archived.id.0, scope), Err(CatalogError::Stale));
    let other = HostStore::memory().unwrap();
    let mut other_rides = FlatRideStore::new(other).unwrap();
    let mut catalogs = TokenSource::<CatalogTag>::new();
    let outcome = rig.host.serve_catalog(
        &mut rig.app,
        CatalogEffect::ReadCatalog { token: catalogs.issue() },
        &mut rig.routes,
        &mut other_rides,
        &mut (),
    );
    assert!(matches!(outcome, CatalogOutcome::Failed { error: CatalogError::Stale, .. }));
    assert!(!rig.app.retention_expiry_due(archived.id.0, scope));
}
