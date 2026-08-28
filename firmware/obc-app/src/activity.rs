//! App activity that is not owned by a product domain.
//!
//! [`Activity`] holds the operating [`Mode`], the ride being viewed, delete requests, and the
//! sensor-scan level. Navigator owns active-route and guidance state; Recorder owns ride state.
//!
//! **The ride's own numbers are not here.** Distance, moving time, climb, the live sensor values
//! and the per-ride summary belong to [`RecorderMachine`](crate::recorder::RecorderMachine), which
//! is what decides when a ride starts and what closes it (#1398 R1). Kept separate from
//! [`AppState`](crate::AppState) (the camera) because the mode and the route model outlive any one
//! screen and several screens read them.
//!
//! ## Why [`Mode`] stays here
//!
//! Recorder does not subsume it. A session is open or it is not; [`Mode`] is three states, and both
//! of the two that are not [`Riding`](Mode::Riding) can hold with a session open *or* without one. A
//! [`Paused`](Mode::Paused) ride is still recording — the rider stopped the totals, not the ride —
//! and a ride the card refused still shows its distance with no session at all. So the pass tells
//! Recorder whether the ride is running rather than Recorder deciding it, and folding the two is
//! #1398 R5's question, not this slice's.

/// The device's operating mode (`docs/content/software/ui.md` §"The whole flow").
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

/// A **detour-plan request** (#882): the Detour chooser's Press asks the planner for an A* detour
/// from the rider's fix to the rejoin point at `target_m`, blacklisting the corridor around the
/// skipped span `[progress_m, target_m]`. The executor resolves the rejoin *coordinate* itself
/// (`position_at(target_m)`) — it owns the active `RouteReader`; the screen deliberately carries
/// only distances, keeping the request tiny and `Copy`.
///
/// Lives with [`NavigatorMachine`](crate::navigator::NavigatorMachine), which is where the rider's
/// request waits until an executor takes it. The type stays here because [`Activity`] is the ride
/// model the chooser measures it against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetourRequest {
    /// The active catalog slot the request is keyed to (durable-remapped across rescans).
    pub route: usize,
    /// The rider's fix at Press, `(lon, lat)` µdeg — the detour's start.
    pub from: (i32, i32),
    /// The rider's along-route projection at Press — the corridor's (frozen) start anchor and
    /// the splice's head/detour seam.
    pub progress_m: u32,
    /// The chosen rejoin distance along the route — the corridor's end and the splice point.
    pub target_m: u32,
}

/// Which phase of the SD-sideload firmware update (epic #615 S5, #620)
/// [`DfuState`](crate::dfu::DfuState) asks the board to run. The two phases are separate so the UI can **confirm before arming**:
/// [`Scan`](DfuAction::Scan) is read-only (validate `UPDATE.BIN`, cost nothing on failure) and
/// answers a [`DfuScanReport`](crate::dfu::DfuScanReport); [`Install`](DfuAction::Install) is the
/// irreversible arm-and-reboot (snapshot the rollback, write the boot-state page, reset into the
/// bootloader). The `dfu-install` debug command posts `Install` directly (no confirm screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuAction {
    /// Validate the staged `UPDATE.BIN` (header, full CRC-32, extents) without touching anything;
    /// the board answers through the pass's fact stage.
    Scan,
    /// Arm the update: snapshot the running image to `ROLLBACK.BIN`, write the `Armed` boot-state
    /// record, and reboot into the bootloader. On success the board never returns (it resets).
    Install,
}

/// A **route-planning request** (epic #116, R4): the POI create-route confirm asks the on-device
/// router for a route from the rider's fix to the POI. Coordinates are `(lon, lat)` microdegrees
/// (the OBCM/renderer convention); the name is the POI's stored name (or its subtype fallback
/// label, matching the list row), carried as a fixed inline buffer so the request stays `Copy` and
/// bounded.
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
    /// The operating mode. `pub(crate)`: the screens flip it through their `Ctx`, and the
    /// in-crate screen-harness tests (`src/harness/screens.rs`) stage and assert it directly (#803
    /// narrowed the draw/prepare contexts; #812 relocated that bare-`Activity` harness in-crate so
    /// this field needs no public accessor). Hosts read it through [`App::mode`](crate::App::mode).
    pub(crate) mode: Mode,
    /// Index into the ride catalog of the ride whose **detail screen** is open, or `None` (epic
    /// #678 T2 / #680) — the ride namespace's `active_route`: set on detail entry, cleared on
    /// exit, and the key the host's one-shot track-profile fill hangs off
    /// ([`App::ride_track_request`](crate::App::ride_track_request) → the host streams the Ride
    /// object once → the keyed ride-track answer).
    pub(crate) viewed_ride: Option<usize>,

    /// A one-shot **route-delete request** (epic #447, P6): the catalog *index* of a route the Route
    /// menu's hold-to-delete footer asked to remove, drained by the host via
    /// the pass which translates it to the route's
    /// durable object id. An index (not the id) because the screen holds indices; the id lookup is
    /// `App`'s, which owns the parallel [`route_ids`](crate::App::route_ids) table. Recorded by
    /// [`request_route_delete`](Activity::request_route_delete); the actual file delete + rescan is
    /// the host's, and the resulting store-changed edge re-feeds the catalog with the route gone.
    delete_route: Option<usize>,
    /// A one-shot **ride-delete request** (epic #447, P7): the catalog *index* of a ride the Rides
    /// screen's hold-to-delete footer asked to remove, drained by
    /// the pass which translates it to the ride's
    /// durable object id (the parallel [`ride_ids`](crate::App::ride_ids) table). The ride namespace's
    /// twin of [`delete_route`](Activity::delete_route); the host deletes the ride object + rescans,
    /// and the resulting store-changed edge re-feeds the ride catalog with it gone.
    delete_ride: Option<usize>,
    /// A one-shot **trip-delete request** (epic #526, TR3): the trip's durable **object id** the
    /// Route menu's long-press → confirm dialog asked to cascade-delete (the trip **and** all its
    /// member routes — locked). Drained by the host via
    /// the pass. Unlike
    /// [`delete_route`](Activity::delete_route) this is the id, not an index: a trip id is already
    /// durable (its own device counter), the confirm screen carries it verbatim, and a trip that
    /// vanished in a racing rescan simply drains to a no-op at the host. The host deletes the
    /// `TP{id}.OBT` **and** every member route file, then rescans + re-feeds trips + routes.
    delete_trip: Option<crate::CatalogObjectId>,
    /// The **sensor scan mode** level (BLE sensors epic #707, SE7): raised by the Sensors screen while
    /// a scan-list sub-screen is open (entering a HR/power/cadence row) and lowered on exit/Back.
    /// Unlike the delete/scan *one-shots* this is a **level**, not a drained edge — the enter/leave
    /// framing of `request_sensor_scan(bool)`: the host polls it each pass ([`App::sensor_scan_active`]),
    /// keeps a discovery scan running while it is `true`, and clears the app scan list when it falls
    /// (the `set_radio_enabled` shape, not the `take_*` shape).
    sensor_scan: bool,
}

impl Activity {
    /// A fresh activity in the given mode, no route loaded and no ride recorded.
    pub fn new(mode: Mode) -> Self {
        Activity { mode, ..Default::default() }
    }

    /// Record a one-shot request to delete the catalog route at `index` (epic #447, P6) — set by the
    /// Route menu's hold-to-delete footer, drained by the pass.
    /// The index is resolved to the route's durable object id at drain, so a rescan racing between the
    /// hold and the drain can't delete the wrong route (a vanished route resolves to nothing).
    pub(crate) fn request_route_delete(&mut self, index: usize) {
        self.delete_route = Some(index);
    }

    /// Take (and clear) the pending route-delete request's catalog **index**, if any — `App` drains
    /// this and maps the index to its durable object id for the host to delete.
    pub(crate) fn take_route_delete(&mut self) -> Option<usize> {
        self.delete_route.take()
    }

    /// Record a one-shot request to delete the ride-catalog entry at `index` (epic #447, P7) — set by
    /// the Rides screen's hold-to-delete footer, drained by
    /// the pass, which resolves it to the ride's durable
    /// object id at drain (so a rescan racing the hold can't delete the wrong ride).
    pub(crate) fn request_ride_delete(&mut self, index: usize) {
        self.delete_ride = Some(index);
    }

    /// Take (and clear) the pending ride-delete request's catalog **index**, if any — `App` drains
    /// this and maps the index to its durable object id for the host to delete.
    pub(crate) fn take_ride_delete(&mut self) -> Option<usize> {
        self.delete_ride.take()
    }

    /// Record a one-shot request to cascade-delete the trip with durable object `id` (epic #526,
    /// TR3) — set by the Route menu's long-press → confirm dialog, drained by the pass. The id (not
    /// an index) because a trip id is durable.
    pub(crate) fn request_trip_delete(&mut self, id: crate::CatalogObjectId) {
        self.delete_trip = Some(id);
    }

    /// Take (and clear) the pending trip-delete request's durable **object id**, if any — the pass
    /// drains this into a [`CatalogIntent::DeleteTrip`](crate::catalog_state::CatalogIntent), and
    /// `CatalogMachine` owns the member-then-folder order from there (#1491).
    pub(crate) fn take_trip_delete(&mut self) -> Option<crate::CatalogObjectId> {
        self.delete_trip.take()
    }

    /// Set the **sensor scan mode** level (BLE sensors epic #707, SE7): `true` when the scan-list
    /// screen opens on a HR/power/cadence row, `false` on exit/Back. A level, not a one-shot — the host
    /// polls it via [`App::sensor_scan_active`](crate::App::sensor_scan_active) each pass.
    pub(crate) fn request_sensor_scan(&mut self, on: bool) {
        self.sensor_scan = on;
    }

    /// Whether sensor scan mode is on — the host's per-pass read (keeps a discovery scan running while
    /// `true`, clears the app scan list when it falls).
    pub(crate) fn sensor_scan_active(&self) -> bool {
        self.sensor_scan
    }
}
