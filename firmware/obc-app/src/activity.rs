//! The route/tracking model — what the device is *pointed at*.
//!
//! [`Activity`] holds the operating [`Mode`], which route is loaded, which ride is being viewed,
//! the live map-match result (riding cursor + off-route readout) and the derived route readouts
//! (active climb, next waypoint) — plus the delete/seam/scan one-shots the screens raise.
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

use obc_route::Match;

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

/// A **seam re-anchor** waiting for the next route-aware tick: after a detour commit re-adopts
/// the spliced route (#882), the matcher's progress + forward-only floor are installed at the
/// splice seam (`anchor_m` — the rider's frozen along-route position, which the splice
/// guarantees is exactly the head/detour boundary). Queued by the commit handler, never by a
/// screen: the tick owns the `RouteReader` the install needs. Keyed to the catalog slot so a
/// racing route swap cannot apply the anchor to unrelated geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SeamRequest {
    pub route: usize,
    pub anchor_m: u32,
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

/// The route model: the [`Mode`], which route is loaded, which ride is being viewed, the live
/// map-match and the readouts derived from it. Small and `Copy` — the screens read it by value.
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    /// The operating mode. `pub(crate)`: the screens flip it through their `Ctx`, and the
    /// in-crate screen-harness tests (`src/harness/screens.rs`) stage and assert it directly (#803
    /// narrowed the draw/prepare contexts; #812 relocated that bare-`Activity` harness in-crate so
    /// this field needs no public accessor). Hosts read it through [`App::mode`](crate::App::mode).
    pub(crate) mode: Mode,
    /// Index into the route [`Catalog`](crate::route::Catalog) of the loaded route, or `None` when
    /// idle. The geometry is opened separately by the host (only the active route is resident).
    /// `pub(crate)`: hosts are fully covered by
    /// [`App::active_route_index`](crate::App::active_route_index) /
    /// [`App::activate_route`](crate::App::activate_route) and use them; the in-crate screen-harness
    /// tests (`src/harness/screens.rs`) preload it exactly as the Route menu's `Ctx` write does
    /// (#812 relocated that harness in-crate — see [`mode`](Activity::mode)).
    pub(crate) active_route: Option<usize>,
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
    /// A seam re-anchor queued by the detour commit handler (#882);
    /// [`RideEngine`](crate::ride_engine::RideEngine) consumes it on the next tick that has the
    /// matching active geometry, installing matcher progress + floor at the splice seam.
    seam_request: Option<SeamRequest>,
    /// The **sensor scan mode** level (BLE sensors epic #707, SE7): raised by the Sensors screen while
    /// a scan-list sub-screen is open (entering a HR/power/cadence row) and lowered on exit/Back.
    /// Unlike the delete/scan *one-shots* this is a **level**, not a drained edge — the enter/leave
    /// framing of `request_sensor_scan(bool)`: the host polls it each pass ([`App::sensor_scan_active`]),
    /// keeps a discovery scan running while it is `true`, and clears the app scan list when it falls
    /// (the `set_radio_enabled` shape, not the `take_*` shape).
    sensor_scan: bool,

    // live map-match (from the GPS fix)
    /// Total distance of the active route (m), mirrored from its header so the riding views can
    /// compute the progress fraction without re-reading the route. `0` when none loaded.
    pub(crate) route_total_m: u32,
    /// Matched distance along the route (m): the riding cursor / progress bar. Frozen while
    /// off-route. Written internally via [`apply_match`](Activity::apply_match); `pub(crate)` — the
    /// in-crate upload harness (`src/harness/upload.rs`) stages **and** asserts the three match
    /// readouts directly (#812 relocated it in-crate — see [`mode`](Activity::mode)).
    pub(crate) progress_m: u32,
    /// Whether the rider is currently off-route. Match readout — see [`progress_m`](Activity::progress_m).
    pub(crate) off_route: bool,
    /// Live cross-track distance to the route (m) — the "off route · NNN m" readout. Match readout —
    /// see [`progress_m`](Activity::progress_m).
    pub(crate) dist_to_route_m: u32,
    /// Index into the App-owned [`Climbs`](obc_route::Climbs) list of the climb the rider is
    /// currently on, or `None` when between climbs / off any climb. Set by
    /// [`App::update_active_climb`](crate::App::update_active_climb) on each matched fix — with
    /// enter/exit hysteresis so it can't flap at a climb boundary — since `App`, not `Activity`,
    /// owns the climbs list and the resident detail buffer keyed on this. The riding views (and, in
    /// C4, the Climb screen) read it to decide whether a climb is being tracked. Cleared on every
    /// route swap / unload / replace, alongside `progress_m` / `off_route`.
    pub(crate) active_climb: Option<usize>,
    /// Index into the App-owned [`Waypoints`](obc_route::Waypoints) table of the next waypoint ahead
    /// on the route, or `None` when past the last (or no route). Set by
    /// [`App::update_next_waypoint`](crate::App::update_next_waypoint) on each matched fix — with a
    /// distance-linger so it can't flap around a waypoint — since `App`, not `Activity`, owns the
    /// table. The riding views (the map chip / stat fields, later in the epic) read it for the "next
    /// waypoint" readouts. Cleared on every route swap / unload / replace, alongside `active_climb`.
    pub(crate) next_waypoint: Option<usize>,
    /// Number of entries in the App-owned resident waypoint table. Mirrored with the cache so a
    /// waypoint-list gesture can wrap its cursor without putting draw-only row data in `Ctx`.
    /// Normally this is the complete plan (≤32); on an oversized route it is the current window.
    pub(crate) waypoint_count: usize,
}

impl Activity {
    /// A fresh activity in the given mode, no route loaded and no ride recorded.
    pub fn new(mode: Mode) -> Self {
        Activity { mode, ..Default::default() }
    }

    /// The rider's matched along-route progress, meters (frozen while off-route) — the read the
    /// hosts' flow tests pin the detour seam re-anchor by; in-crate readers use the field.
    pub fn progress_m(&self) -> u32 {
        self.progress_m
    }

    /// Whether the matcher currently places the rider **off** the route corridor — the fact behind
    /// the Map's "off route" chip and the frozen progress above. Read by the hosts' flow tests
    /// (`dirty_parity` pins that its off-route excursion is genuinely off the corridor rather than a
    /// pair of identical nothings); in-crate readers use the field.
    pub fn off_route(&self) -> bool {
        self.off_route
    }

    /// Queue a seam re-anchor at `anchor_m` on `route` (#882): the route-aware tick atomically
    /// installs matcher progress + the forward-only floor at the splice seam before processing
    /// its next fresh fix. Stored verbatim — the tick's `locate_progress` clamps against the
    /// (just-adopted) route's real length, which this `Activity` may not mirror yet.
    pub(crate) fn request_seam(&mut self, route: usize, anchor_m: u32) {
        self.seam_request = Some(SeamRequest { route, anchor_m });
    }

    pub(crate) fn pending_seam(&self) -> Option<SeamRequest> {
        self.seam_request
    }

    pub(crate) fn clear_seam(&mut self) {
        self.seam_request = None;
    }

    /// Follow a queued seam re-anchor through a route-catalog rescan by durable identity. If
    /// its route vanished, drop the one-tick request rather than letting its old index alias a
    /// surviving neighbour.
    pub(crate) fn remap_seam_route(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.seam_request = self
            .seam_request
            .and_then(|req| remap(req.route).map(|route| SeamRequest { route, anchor_m: req.anchor_m }));
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

    /// Clear the route-relative readouts a new ride re-derives (keeps `mode`/`active_route`).
    /// Called on a session edge, beside [`RecorderMachine::reset_totals`](crate::recorder::RecorderMachine).
    pub(crate) fn reset_ride(&mut self) {
        self.seam_request = None;
        self.progress_m = 0;
        self.off_route = false;
        self.dist_to_route_m = 0;
        // A fresh session restarts at progress 0; drop any on-climb / next-waypoint state so the next
        // matched fix re-derives both from the reset cursor rather than holding the old ride's.
        self.active_climb = None;
        self.next_waypoint = None;
    }

    /// Store the latest map-match result (cursor + off-route readout).
    pub(crate) fn apply_match(&mut self, m: Match) {
        self.progress_m = m.progress_m;
        self.off_route = m.off_route;
        self.dist_to_route_m = m.dist_m;
    }
}
