//! The iPhone host's device core: one [`Host`] owns the whole device — the card, the map, the
//! stores, the app, the planner, the RGBA frame — and advances it one display-link frame at a time.
//!
//! **Swift owns the loop; Rust owns the truth.** Per [`tick`](Host::tick): the queued button edges
//! reach the app's own gesture recognizer, one bounded Peak View step runs, one `App::run_pass`
//! decides, the shared executor serves it, and the frame is rendered only when the app says
//! something changed.
//!
//! The card is persistent and this host never creates it: [`import_map`] does, with no host open.
//! Everything here is target-independent and tested natively; the C ABI and the Swift shell are
//! separate.

mod sensors;
#[cfg(test)]
mod tests;

pub use sensors::PhoneSensors;

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::device_core::{PassClock, PlatformSupport};
use obc_app::settings::Settings;
use obc_app::{App, AppState, CameraMode, CatalogObjectId, Screen};
use obc_host_core::flat_map::{FlatMap, MapError};
use obc_host_core::flat_store::HostStore;
use obc_host_core::{
    convert_gpx, initial_camera, ActiveRouteSession, DeviceInput, FileSettingsStore, FlatRideRecorder, FlatRideStore,
    FlatRouteStore, HostLoop, HostPlatform, RgbaFrame, RideRepository, RouteRepository, TrackStore,
};
use obc_ports::{Button, InputClock, RideClock, SettingsSaveError, SettingsStore};
use obc_reader::rgb565_to_device64;
use obc_route::{ElevationSource, NullElevation, RouteReader};
use obc_storage::flat::StoreError;
use std::path::Path;

/// The panel resolution — the one [`obc_display`] frame authority, not re-declared literals.
pub const FRAME_W: u32 = obc_display::ls021::FRAME_W as u32;
pub const FRAME_H: u32 = obc_display::ls021::FRAME_H as u32;

/// What the phone honestly is. It has a card and a settings file, so detours and persisted
/// settings are real; it is not a BLE peripheral, carries no staged firmware, and reports no free
/// space, so the screens behind those hide instead of offering a control that answers nothing.
const SUPPORT: PlatformSupport = PlatformSupport {
    detour: true,
    settings_persistence: true,
    dfu: false,
    bonding: false,
    storage_space_report: false,
};

/// The only platform work the phone does: the settings file behind the app's save.
struct PhonePlatform<'a> {
    settings: &'a mut FileSettingsStore,
}

impl HostPlatform for PhonePlatform<'_> {
    fn persist_settings(&mut self, settings: &Settings, _revision: u16) -> Result<(), SettingsSaveError> {
        self.settings.save(settings)
    }
}

/// What the shell finds at the card path. Asked before opening, so an empty phone offers the
/// import screen instead of reading a reason out of an error string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardState {
    /// No card file yet. The first [`import_map`] creates it.
    Missing,
    /// A card, but no map on it: nothing to show until one is imported.
    NoMap,
    /// A card with a map. [`Host::open`] mounts it.
    Ready,
}

/// What is at `card` right now. An error means a card that exists but cannot be read.
pub fn card_state(card: &Path) -> Result<CardState, String> {
    if !card.exists() {
        return Ok(CardState::Missing);
    }
    let owner = open_card(card)?;
    match FlatMap::open_only_in(&owner) {
        Ok(_) => Ok(CardState::Ready),
        Err(error) if missing_map(&error) => Ok(CardState::NoMap),
        Err(error) => Err(format!("card map: {error}")),
    }
}

/// Put `obcm` on the card at `card`, creating the card when it does not exist yet.
///
/// The only place a card is created, and it runs with **no [`Host`] open**: the map is the one
/// object a running device cannot have replaced underneath it, so the shell closes the host,
/// imports, and opens again.
pub fn import_map(card: &Path, obcm: &Path) -> Result<(), String> {
    let owner = if card.exists() { HostStore::open_file(card) } else { HostStore::create_file(card) }
        .map_err(|error| format!("card {}: {error}", card.display()))?;
    let input = std::fs::File::open(obcm).map_err(|error| format!("read {}: {error}", obcm.display()))?;
    let imported = match FlatMap::open_only_in(&owner) {
        Ok(current) => current.replace_from_file(input),
        Err(error) if missing_map(&error) => FlatMap::from_file_in(&owner, input),
        // The card cannot be read at all, which is not the input file's fault.
        Err(error) => return Err(format!("card {}: {error}", card.display())),
    };
    imported.map(|_| ()).map_err(|error| format!("import {}: {error}", obcm.display()))
}

/// A card that carries no map yet — the one map error that is a state rather than a failure.
fn missing_map(error: &MapError) -> bool {
    matches!(error, MapError::Storage(StoreError::NotFound))
}

fn open_card(card: &Path) -> Result<HostStore, String> {
    HostStore::open_file(card).map_err(|error| format!("card {}: {error}", card.display()))
}

/// The resident active-route parse as a reader, when a route is active. A free function because a
/// `&self` method would borrow the whole host for as long as the reader lives.
fn active_route<'a>(session: &'a ActiveRouteSession, routes: &'a FlatRouteStore) -> Option<RouteReader<'a>> {
    match (session.index(), routes.active_source()) {
        (Some(index), Some(source)) => Some(RouteReader::new(index, source)),
        _ => None,
    }
}

/// The whole device, over one persistent card.
pub struct Host {
    map: FlatMap,
    /// The shared app (~136 KB). Heap-allocated: a by-value `App` temporary is exactly the kind of
    /// silent stack trap the iOS main thread's 1 MiB budget cannot absorb.
    app: Box<App>,
    /// The render path's per-frame scratch (~90 KB), lent to each render call. Boxed for the same
    /// reason as the app.
    scratch: Box<obc_render::RenderScratch>,
    routes: FlatRouteStore,
    rides: FlatRideStore,
    tracks: TrackStore,
    sensors: PhoneSensors,
    /// The four on-screen buttons as raw edges, drained by the app's own recognizer each tick.
    input: DeviceInput,
    settings: FileSettingsStore,
    /// The shared typed executor: the next pass's outcomes and facts, and the in-flight route plan.
    host: HostLoop,
    /// The resident active-route parse, opened once per frame and lent to both the pass and the
    /// render (so the map opens without a per-frame `RouteIndex` reparse).
    session: ActiveRouteSession,
    frame: RgbaFrame,
    photo: obc_host_core::photo::Preparer,
    /// The cooperative panorama build, when the card map carries surface terrain.
    peaks: Option<obc_host_core::peak_view::Runtime>,
    elevation: Box<dyn ElevationSource>,
    /// First frame rendered — the shell's readiness signal.
    ready: bool,
}

impl Host {
    /// Open the device over an existing card. Never creates the card and never imports: a card
    /// without a map is an error the shell answers by asking for one ([`import_map`]).
    ///
    /// `settings` is the settings file (the device's RRAM stand-in) and `exports` the directory a
    /// committed ride's GPX is written into.
    pub fn open(card: &Path, settings: &Path, exports: &Path) -> Result<Box<Self>, String> {
        let owner = open_card(card)?;
        let map = FlatMap::open_only_in(&owner).map_err(|error| format!("card map: {error}"))?;
        let mut routes = FlatRouteStore::new(owner.clone(), &[]).map_err(|error| format!("routes: {error}"))?;
        routes.refresh_metadata().map_err(|error| format!("route metadata: {error:?}"))?;
        let rides = FlatRideStore::new(owner.clone()).map_err(|error| format!("rides: {error:?}"))?;
        // iOS stops an app without warning, so an interrupted ride must come back as the recovery
        // card rather than as a silently lost recording.
        let recorder = FlatRideRecorder::new(owner.clone()).map_err(|error| format!("ride recovery: {error:?}"))?;
        let tracks = TrackStore::new(recorder, owner, exports);
        let mut settings_store = FileSettingsStore::open(settings);
        let boot_settings = settings_store.load().unwrap_or_default();
        // Absent or unreadable terrain is not fatal; routes stay flat.
        let elevation: Box<dyn ElevationSource> = match obc_host_core::terrain::FlatElevation::open(&map) {
            Ok(Some(terrain)) => terrain,
            _ => Box::new(NullElevation),
        };
        let peaks = obc_host_core::peak_view::Runtime::over_map(&map);

        let (cam_lon, cam_lat, zoom) = initial_camera(&map.reader(), FRAME_W);
        let mut state = AppState::new(cam_lon, cam_lat, zoom);
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        let mut app = Box::new(App::new(state));
        // One resident frame, repainted on demand, so every render is a render over the last one.
        app.set_resident_frame(true);
        app.set_nav_profiles(map.tables().nav_profiles());
        app.set_map_nav_graph(map.tables().has_nav_graph());
        app.set_routes_with_ids(routes.catalog(), routes.ids());
        app.set_rides(rides.catalog());
        // The phone runs the settings a rider runs: whatever was saved, else the defaults.
        app.set_settings(boot_settings);
        tracks.offer_recovery(&mut app);

        Ok(Box::new(Host {
            map,
            app,
            scratch: Box::new(obc_render::RenderScratch::new()),
            routes,
            rides,
            tracks,
            sensors: PhoneSensors::default(),
            input: DeviceInput::new(),
            settings: settings_store,
            host: HostLoop::new(),
            session: ActiveRouteSession::new(),
            frame: RgbaFrame::new(FRAME_W, FRAME_H),
            photo: obc_host_core::photo::Preparer::default(),
            peaks,
            elevation,
            ready: false,
        }))
    }

    /// The phone's sensors. The shell pushes each CoreLocation / CoreMotion value as it arrives.
    pub fn sensors(&mut self) -> &mut PhoneSensors {
        &mut self.sensors
    }

    /// One button edge from the shell's touch areas. Held state in, edges out: pressing and holding
    /// queues exactly one Down, and the hold itself is the recognizer's to time.
    pub fn push_button(&mut self, button: Button, down: bool) {
        self.input.set_button(button, down);
    }

    /// Advance one display-link frame; `true` when the frame buffer changed (only then does the
    /// shell need to blit). `now_ms` is the shell's monotonic clock since open.
    pub fn tick(&mut self, now_ms: f64) -> bool {
        let now = now_ms.max(0.0) as u32;
        // Recognition runs every tick, also with no queued edge: that is how a held Select or Back
        // fires its hold. A chord resolves inside `recognize`, above the screen stack.
        let gestures = self.app.recognize(InputClock(now), &mut self.input);
        match &mut self.peaks {
            Some(peaks) => peaks.update(&mut self.app, &self.map.reader()),
            // A card map with no surface terrain can say so instead of waiting forever.
            None if matches!(self.app.top_screen(), Screen::PeakView(_)) => {
                self.app.set_peak_view_status(obc_app::peak_view::runtime::Status::Unavailable)
            }
            None => {}
        }

        // Open the active route once from the resident session and lend it to the pass, so the
        // map-matcher reads the geometry the frame draws.
        self.session.sync(&self.app, &mut self.routes);
        let mut plan = {
            let route = active_route(&self.session, &self.routes);
            self.host.pass(
                &mut self.app,
                PassClock { ride: RideClock(now), ui: InputClock(now) },
                &gestures,
                self.sensors.ports(),
                route.as_ref(),
                SUPPORT,
            )
        };
        // A single-loop host has no second recognizer to cancel, so it consumes the hold-cancel
        // latch the pass may have armed rather than leaving it set for a plane that does not exist.
        let _ = self.app.take_hold_cancel();
        {
            let mut platform = PhonePlatform { settings: &mut self.settings };
            self.host.execute(
                &mut self.app,
                &mut plan,
                &mut self.session,
                &mut self.routes,
                &mut self.rides,
                &mut self.tracks,
                &mut (),
                &self.map,
                &mut *self.elevation,
                &mut platform,
            );
        }
        // The map-referenced altimeter's terrain read, drained once per frame behind the pass.
        self.app.sample_terrain(&mut *self.elevation);

        // `plan.next_wake_ms` and `plan.immediate` are deliberately ignored: the display link paces
        // the loop, and its next frame is already the "come straight back" an immediate wake asks
        // for. Render on demand otherwise — the same signal the firmware gates its repaints on.
        if plan.render.map || plan.render.overlay || !self.ready || self.app.photo_pending() {
            // Re-open the active route: the executor may have committed new geometry under it (a
            // planned route, a spliced detour), and the frame must draw what is there now.
            self.session.sync(&self.app, &mut self.routes);
            let route = active_route(&self.session, &self.routes);
            let reader = self.map.reader();
            self.app.render_scene_map_photo_timed(
                Some(&mut self.scratch),
                &mut self.frame,
                Some(&reader),
                Some(&reader),
                route.as_ref(),
                self.peaks.as_ref().and_then(|peaks| peaks.panorama()),
                FRAME_W as f32,
                FRAME_H as f32,
                |c| {
                    let (r, g, b) = rgb565_to_device64(c);
                    Rgb888::new(r, g, b)
                },
                &obc_render::NoopClock,
                Some(self.photo.interactive(plan.render.map || !self.ready)),
            );
            self.app.render_overlay(&mut self.frame, FRAME_W as f32, FRAME_H as f32, |c| {
                let (r, g, b) = rgb565_to_device64(c);
                Rgb888::new(r, g, b)
            });
            self.ready = true;
            return true;
        }
        false
    }

    /// Import one route file into the card: `.obcr` bytes as they are, `.gpx` converted and
    /// attributed against this map. The catalog re-reads on the next pass.
    pub fn import_route(&mut self, path: &Path) -> Result<CatalogObjectId, String> {
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase();
        let bytes = match extension.as_str() {
            "obcr" => std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?,
            "gpx" => convert_gpx(path, Some((&self.map.reader(), self.attribution_key())))?.0,
            _ => return Err(format!("{}: not an .obcr or .gpx route", path.display())),
        };
        let id = self.routes.import(&bytes).map_err(|error| format!("import {}: {error}", path.display()))?;
        self.host.note_store_commit();
        Ok(id)
    }

    /// The rendered RGBA frame ([`FRAME_W`]`×`[`FRAME_H`]`×4` bytes), for the shell's `CGImage`.
    pub fn frame(&self) -> &[u8] {
        self.frame.as_rgba()
    }

    /// The current (input-receiving) screen's variant name, e.g. `"Map"`, `"Menu"`, `"PeakView"`.
    pub fn screen(&self) -> &'static str {
        self.app.top_screen().name()
    }

    /// Whether a ride is open — the shell keeps the screen awake while it is.
    pub fn recording(&self) -> bool {
        self.app.recording()
    }

    /// The key that binds a converted route to this exact map revision.
    fn attribution_key(&self) -> obc_formats::obcr::RouteSourceKey {
        let source = self.map.source();
        obc_formats::obcr::RouteSourceKey {
            store: source.store_id().0,
            object: source.id().0,
            revision: source.revision().0,
        }
    }
}
