//! App activity that is not owned by a product domain.
//!
//! [`Activity`] holds the operating [`Mode`], the ride being viewed, delete requests, and the
//! sensor-scan level. Navigator owns active-route and guidance state; Recorder owns ride state and
//! the ride's own numbers. [`Activity`] stays separate from [`AppState`](crate::AppState), the
//! camera, because the operating mode outlives any one screen and several screens read it.
//!
//! [`Mode`] stays here because Recorder does not subsume it. A session is open or it is not, while
//! both of the modes that are not [`Riding`](Mode::Riding) can hold with a session open or without
//! one: a [`Paused`](Mode::Paused) ride is still recording, and a ride the card refused still shows
//! its distance with no session at all. So the pass tells Recorder whether the ride is running.

/// The device's operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// No route active — the Home screensaver.
    #[default]
    Idle,
    /// A route is loaded and tracking is running — Map / Elevation.
    Riding,
    /// Tracking paused — the Ride control overlay is up.
    Paused,
}

/// A detour-plan request: the Detour chooser's Press asks the planner for an A* detour from the
/// rider's fix to the rejoin point at `target_m`, blacklisting the corridor around the skipped span
/// `[progress_m, target_m]`. The executor resolves the rejoin coordinate itself, because it owns the
/// active `RouteReader`; the screen carries only distances, so the request stays small and `Copy`.
///
/// [`NavigatorMachine`](crate::navigator::NavigatorMachine) holds the request until an executor
/// takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetourRequest {
    /// The active catalog slot the request is keyed to (durable-remapped across rescans).
    pub route: usize,
    /// The rider's fix at Press, `(lon, lat)` µdeg — the detour's start.
    pub from: (i32, i32),
    /// The rider's along-route projection at Press — the corridor's frozen start anchor and the
    /// splice's seam.
    pub progress_m: u32,
    /// The chosen rejoin distance along the route — the corridor's end and the splice point.
    pub target_m: u32,
    /// What the leg does to the route. An approach has no corridor and no trim, and its splice puts
    /// it in front of the whole route.
    pub leg: obc_route::Leg,
}

impl DetourRequest {
    /// Ride to start: the way from `from` to the start of `route`.
    pub fn approach(route: usize, from: (i32, i32)) -> Self {
        DetourRequest { route, from, progress_m: 0, target_m: 0, leg: obc_route::Leg::Approach }
    }

    /// The rest of the day before `route`, from `from_m` on that day's route, then `route`. The
    /// executor reads where the days leave and join the trip's line, and clamps `to_m` to the
    /// leave point.
    pub fn rest(route: usize, from_m: u32) -> Self {
        DetourRequest {
            route,
            from: (0, 0),
            progress_m: 0,
            target_m: 0,
            leg: obc_route::Leg::Rest { from_m, to_m: u32::MAX },
        }
    }
}

/// Which phase of the firmware update [`DfuState`](crate::dfu::DfuState) asks the board
/// to run. The two are separate so the UI can confirm before arming: [`Scan`](DfuAction::Scan) is
/// read-only and answers a [`DfuScanReport`](crate::dfu::DfuScanReport), and
/// [`Install`](DfuAction::Install) is the irreversible arm-and-reboot. The `dfu-install` debug
/// command posts `Install` directly, with no confirm screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuAction {
    /// Validate the staged update package (header, full CRC-32, extents) without touching
    /// anything; the board answers through the pass's fact stage.
    Scan,
    /// Arm the update: write the running image into a rollback reserve, write the `Armed`
    /// boot-state record, and reboot into the bootloader. On success the board never returns.
    Install,
}

/// A route-planning request: the POI create-route confirm asks the on-device router for a route
/// from the rider's fix to the POI. Coordinates are `(lon, lat)` microdegrees; the name is the POI's
/// stored name, or its subtype fallback label, in a fixed inline buffer so the request stays `Copy`
/// and bounded.
///
/// Lives with [`NavigatorMachine`](crate::navigator::NavigatorMachine) until an executor takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavRequest {
    /// The rider's fix, `(lon, lat)` µdeg — the route's start.
    pub from: (i32, i32),
    /// The POI coordinate, `(lon, lat)` µdeg — the route's goal.
    pub to: (i32, i32),
    /// The route name bytes (UTF-8, `name_len` valid) — read via [`name`](NavRequest::name).
    name: [u8; obc_formats::obcm::POI_NAME_LEN],
    name_len: u8,
}

impl NavRequest {
    /// Build a request, truncating `name` to the POI name cap on a char boundary.
    pub fn new(from: (i32, i32), to: (i32, i32), name: &str) -> Self {
        let mut buf = [0u8; obc_formats::obcm::POI_NAME_LEN];
        let mut len = 0usize;
        for ch in name.chars() {
            let n = ch.len_utf8();
            if len + n > buf.len() {
                break;
            }
            ch.encode_utf8(&mut buf[len..]);
            len += n;
        }
        NavRequest { from, to, name: buf, name_len: len as u8 }
    }

    /// The route name to bake into the emitted OBCR (what the catalog then lists).
    pub fn name(&self) -> &str {
        // The buffer was filled from `&str` chars, so it is valid UTF-8 by construction.
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

/// The operating mode and small app-owned requests that do not belong to a product domain.
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    /// The operating mode. It is `pub(crate)` because the screens flip it through their `Ctx` and
    /// the in-crate screen harness stages it directly. Hosts read it through
    /// [`App::mode`](crate::App::mode).
    pub(crate) mode: Mode,
    /// Index into the ride catalog of the ride whose detail screen is open. Set on detail entry,
    /// cleared on exit, and the key the host's one-shot track-profile fill hangs off.
    pub(crate) viewed_ride: Option<usize>,

    /// A one-shot route-delete request: the catalog index of a route the Route menu's
    /// hold-to-delete footer asked to remove, drained through the pass, which translates it to the
    /// route's durable object id. An index, not the id, because the screen holds indices; the lookup
    /// is `App`'s, which owns the parallel [`route_ids`](crate::App::route_ids) table.
    delete_route: Option<usize>,
    /// A one-shot ride-delete request: the twin of [`delete_route`](Activity::delete_route),
    /// resolved through the parallel [`ride_ids`](crate::App::ride_ids) table.
    delete_ride: Option<usize>,
    /// A one-shot trip-delete request: the durable object id of the trip the Route menu's confirm
    /// dialog asked to cascade-delete, the trip and all its member routes. It is the id, not an
    /// index, because a trip id is already durable, so a trip that vanished in a racing rescan
    /// drains to a no-op at the host.
    delete_trip: Option<crate::CatalogObjectId>,
    pub(crate) cleanup_routes: Option<crate::catalog_state::CatalogIntent>,
    /// The sensor scan mode level, raised by the Sensors screen while a scan-list sub-screen is
    /// open and lowered on exit. A level, not a drained edge: the host polls it each pass, keeps a
    /// discovery scan running while it is `true`, and clears the app scan list when it falls.
    sensor_scan: bool,
}

impl Activity {
    /// A fresh activity in the given mode, no route loaded and no ride recorded.
    pub fn new(mode: Mode) -> Self {
        Activity { mode, ..Default::default() }
    }

    /// Record a one-shot request to delete the catalog route at `index`. The index is resolved to
    /// the route's durable object id at drain, so a rescan racing between the hold and the drain
    /// cannot delete the wrong route.
    pub(crate) fn request_route_delete(&mut self, index: usize) {
        self.delete_route = Some(index);
    }

    /// Take the pending route-delete request's catalog index. `App` maps it to a durable object id
    /// for the host to delete.
    pub(crate) fn take_route_delete(&mut self) -> Option<usize> {
        self.delete_route.take()
    }

    /// Record a one-shot request to delete the ride-catalog entry at `index`. The index is resolved
    /// to the ride's durable object id at drain, so a rescan racing the hold cannot delete the wrong
    /// ride.
    pub(crate) fn request_ride_delete(&mut self, index: usize) {
        self.delete_ride = Some(index);
    }

    /// Take the pending ride-delete request's catalog index. `App` maps it to a durable object id
    /// for the host to delete.
    pub(crate) fn take_ride_delete(&mut self) -> Option<usize> {
        self.delete_ride.take()
    }

    /// Record a one-shot request to cascade-delete the trip with durable object `id`. The id, not
    /// an index, because a trip id is durable.
    pub(crate) fn request_trip_delete(&mut self, id: crate::CatalogObjectId) {
        self.delete_trip = Some(id);
    }

    /// Take the pending trip-delete request's durable object id. The pass drains it into a
    /// [`CatalogIntent::DeleteTrip`](crate::catalog_state::CatalogIntent), and `CatalogMachine` owns
    /// the member-then-folder order from there.
    pub(crate) fn take_trip_delete(&mut self) -> Option<crate::CatalogObjectId> {
        self.delete_trip.take()
    }

    /// Set the sensor scan mode level: `true` when the scan-list screen opens on a sensor row,
    /// `false` on exit. The host polls it each pass.
    pub(crate) fn request_sensor_scan(&mut self, on: bool) {
        self.sensor_scan = on;
    }

    /// Whether sensor scan mode is on — the host's per-pass read.
    pub(crate) fn sensor_scan_active(&self) -> bool {
        self.sensor_scan
    }
}
