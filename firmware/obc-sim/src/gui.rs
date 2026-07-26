//! The eframe host window — the device "screen".
//!
//! The desktop counterpart to the firmware's main loop: each frame it polls the
//! [`SimLocationSource`], advances the shared [`obc_app::App`], renders the firmware-identical
//! path into the resident device-64 frame it owns next to its [`Present`] presenter (the display
//! contracts' render-vs-present split — the same shape as the firmware's map plane), presents it
//! through the presenter's self-diffing engine, and blits the reconstructed texture to a GPU
//! texture at integer scale (nearest-neighbor, so the pixel grid stays crisp). The firmware does
//! the same with a real GPS driver and the LS021B7DD02 panel. Because the GUI now goes through the device-64 backend, the interactive window shows the
//! panel's true 64-colour gamut; `--true-color` (the un-quantized reference) stays on the headless
//! `--png` path.
//!
//! The second "Controls" viewport (the simulated GPS fix + emulated buttons) lives
//! in [`panel`]; its zoom / formatting helpers live in [`units`].

use std::path::Path;

use eframe::egui;
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_app::{App, AppState, CameraMode, Dirty, HostCommand, HostEvent};
use obc_display::FbDevice64;
use obc_host_core::{fill_nav_preview, HostLoop};
use obc_ports::{Button, Fix, InputClock, RideClock, Sensors, SettingsStore};
use obc_reader::{MapCache, MapTables, Reader, SliceSource};
use obc_route::RouteReader;

use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

use crate::device_input::DeviceInput;
use crate::present::Present;
use crate::rides::RideStore;
use crate::routes::RouteStore;
use crate::settings_store::FileSettingsStore;
use crate::sim_compass::SimCompass;
use crate::sim_location::SimLocationSource;
use crate::track::TrackStore;
use crate::trips::TripStore;
use crate::Args;

mod housing;
mod panel;
mod units;

use housing::Colorway;

/// The control panel's editable mirrors. The [`SimLocationSource`] stores the fix as
/// integer microdegrees + `course`; egui widgets need `&mut` floats, so the panel edits
/// these and pushes them into the source each frame.
struct PanelState {
    lat_deg: f64,
    lon_deg: f64,
    heading_deg: f32,
    /// The "Compass" slider — the magnetometer heading orienting a heading-up map while the
    /// rider is stopped (GPS course drops to `None`). Pushed into [`SimCompass`].
    compass_deg: f32,
    /// The injected BLE link state (epic #447): the host→app seam the sim drives from the control
    /// panel, pushed into the app each frame via [`obc_app::App::set_ble_status`]. It's the whole
    /// [`obc_app::BleStatus`] — not just a `connected` bool — so P2/P4 can add passkey / upload
    /// injection by extending this field and its widgets with no restructuring.
    ble: obc_app::BleStatus,
    /// The "Inject upload" combo's selected catalog row (epic #447, P4) — which route the panel's
    /// upload-injection buttons duplicate (new) or rewrite (replace-by-id).
    upload_sel: usize,
    /// The "Delete trip" combo's selected trip row (epic #526, TR2) — which trip the panel's
    /// delete button removes (the `.obt`, non-cascading).
    trip_sel: usize,
    /// **GPS time** (auto-expiry epic #638, S3): when on (the default), the sim feeds the host
    /// wall-clock UTC (plus [`clock_offset_secs`](PanelState::clock_offset_secs)) as a `GpsTime`
    /// poll each tick, so the device boots into a **trusted** clock exactly as a real fix would —
    /// the precondition the deletion sweep gates on. Off leaves the clock untrusted (nothing
    /// auto-deletes), the fresh-device state.
    gps_time: bool,
    /// The accumulated clock fast-forward in seconds (auto-expiry epic #638, S3): the "+1 day"
    /// button adds 86 400 so route/ride expiry is eyeball-testable in seconds rather than days.
    clock_offset_secs: u32,
    /// The "Set route retention" combo's selected catalog row (auto-expiry epic #638, S3) — the
    /// stand-in for the phone's `setRouteRetention` command until S4 gives it a wire.
    retention_route_sel: usize,
    /// The retention level the panel's "Set" button assigns to the selected route.
    retention_level: obc_app::Retention,
    /// The "Mark ride synced" combo's selected Rides row — the `ackRides` stand-in that stamps a
    /// ride's `synced_at` so the sweep can later auto-delete it.
    synced_ride_sel: usize,
}

/// In-progress 1:1 size calibration: the user measures the on-screen reference bar and
/// types the millimetres here. `Some` while the calibration screen is up.
#[derive(Default)]
struct CalibState {
    measured_mm: String,
}

/// The simulator's GPS-time source (auto-expiry epic #638, S3): when `enabled`, each poll resolves
/// the host wall-clock UTC (plus the accumulated `offset_secs` fast-forward) into a [`GpsTime`], so
/// [`App::tick`](obc_app::App::tick) stamps a **trusted** clock exactly as a real fix would. When
/// disabled it yields nothing — the fresh-device untrusted state, where nothing auto-deletes.
struct SimClock {
    enabled: bool,
    offset_secs: u32,
}

impl obc_ports::ClockSource for SimClock {
    fn poll(&mut self) -> Option<obc_ports::GpsTime> {
        if !self.enabled {
            return None;
        }
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0)
            .wrapping_add(self.offset_secs);
        // `DateTime` is minute-resolution; the seconds-into-the-minute ride separately so the wall
        // clock back-dates its epoch exactly like a real fix (see `App::stamp_clock`).
        Some(obc_ports::GpsTime { utc: obc_ports::DateTime::from_unix(unix), second: (unix % 60) as u8 })
    }
}

/// Launch the simulator window. Owns the map bytes for the process lifetime; the
/// [`Reader`] is a cheap view rebuilt each frame over them.
pub fn run(bytes: Vec<u8>, args: Args) -> Result<(), eframe::Error> {
    // The window wraps the whole device (housing + screen + a little backdrop) at `--scale`,
    // so the body has room around the framebuffer.
    let dev = housing::HousingStyle::default().window_size_px(egui::vec2(args.width as f32, args.height as f32));
    let win = [dev.x * args.scale as f32, dev.y * args.scale as f32];
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("OBC Simulator").with_inner_size(win),
        ..Default::default()
    };
    eframe::run_native(
        "OBC Simulator",
        options,
        Box::new(move |_cc| Ok(Box::new(SimGui::new(bytes, args)) as Box<dyn eframe::App>)),
    )
}

/// One frame's device-control hit-test, produced while the housing is drawn
/// ([`SimGui::show_device_image`]) and consumed by [`SimGui::apply_device_input`]. Keeps the draw
/// free of input side effects: it only reports geometry, the caller drives the recognizer.
struct DeviceHit {
    /// UP pressed this frame (housing hit-test OR the ↑ key).
    up_down: bool,
    /// DOWN pressed this frame (housing hit-test OR the ↓ key).
    down_down: bool,
    /// SELECT pressed this frame (housing hit-test OR the Enter key).
    select_down: bool,
    /// BACK pressed this frame (housing hit-test OR the Backspace key).
    back_down: bool,
    /// Selection steps queued this frame by the UP/DOWN pads and the keyboard.
    steps: i32,
    /// Mouse-wheel delta over the screen — the host's stand-in for tapping UP/DOWN.
    scroll_dy: f32,
}

struct SimGui {
    /// Map file bytes; `Reader` borrows these each frame.
    bytes: Vec<u8>,
    /// The immutable map tables (style table + LOD pyramid), parsed once at startup and borrowed
    /// by the cheap per-frame `Reader` — mirroring the device, which parses them once at boot.
    map_tables: MapTables,
    /// The streamed-map cache, kept for the whole session and reused across frames (as the device
    /// holds one in its reserved region). Cross-frame reuse lets a panned-into view warm to 100%
    /// hit, so the "Map SD" stats track real device behaviour rather than a cold ≤75%.
    map_cache: MapCache,
    app: App,
    /// The routes folder (the device-SD stand-in): the menu catalog + active geometry.
    store: RouteStore,
    /// The `.obt` trips beside the routes (epic #526, TR2): the grouped-route folders. Rescanned +
    /// re-fed alongside the route catalog so a rescan re-resolves the trips' stage ids.
    trip_store: TripStore,
    /// The tracks folder as the **ride catalog** (device-SD `/tracks` stand-in): the `RD{id}.ORD`
    /// summaries + synced flags the Rides screen lists (#454). Rescanned when a ride is saved/deleted.
    ride_store: RideStore,
    /// The tracks folder (device-SD `/tracks` stand-in): the `.obct` ride log + saved `RD{id}.ORD` /
    /// `.gpx`. Reconciled to the app's tracking session each frame.
    tracks: TrackStore,
    /// The persisted-settings store (device-RRAM stand-in): seeds the app at boot, written on
    /// each settings change so they survive a relaunch.
    settings_store: FileSettingsStore,
    loc: SimLocationSource,
    /// The resident device-64 frame the app renders the whole frame into — owned by the GUI next
    /// to its presenter (the contracts' borrow split); runtime-sized because the device resolution
    /// is a CLI knob (`--size`).
    fb: Vec<u8>,
    /// The simulator's presenter: [`Present::present_now`] self-diffs the resident frame (pushing only the changed spans into the uploaded texture) under an exact-diff oracle. Its
    /// [`stats`](Present::stats) feed the panel.
    present: Present,
    dev_w: u32,
    dev_h: u32,
    scale: u32,
    /// Saved display calibration (egui points per millimetre), or `None` until the
    /// user calibrates. Loaded from / written to [`crate::calib`].
    points_per_mm: Option<f32>,
    /// Render the device at the panel's true physical size (needs `points_per_mm`).
    physical: bool,
    /// Snap the window to the device's 1:1 size next frame (set when 1:1 turns on).
    physical_resize_pending: bool,
    /// `Some` while the size-calibration screen is shown instead of the device image.
    calib: Option<CalibState>,
    /// Last calibration-save error, surfaced in the control panel.
    calib_error: Option<String>,
    /// Editable mirrors for the control-panel widgets.
    panel: PanelState,
    /// Emulated device controls (four device buttons) → shared gesture recognizer.
    input: DeviceInput,
    /// The loaded GPX replay, if any. When `Some`, it drives the fix instead of the manual
    /// [`SimLocationSource`] (as the device's GPS would). `None` = manual panel control.
    gpx: Option<GpxPlayer>,
    /// Simulated barometer (device altimeter stand-in), fed the replay's elevation on its own
    /// cadence (asynchronous to the GPS fix).
    baro: BaroSensor,
    /// Simulated compass (device magnetometer stand-in) — the panel's "Compass" slider, orienting
    /// a heading-up map while stopped when the GPS has no course.
    compass: SimCompass,
    /// Synthetic BLE sensors (HR / power / cadence) — the panel's "Sensors" section drives them, and
    /// their three source fields feed `Sensors::{hr,power,cadence}` each tick, honouring the ~1 Hz
    /// fresh-mailbox contract so a disabled quantity reads `--` (epic #707 SE8).
    sim_sensors: crate::sim_sensors::SimSensors,
    /// A short "name — N pts, M:SS" status line for the loaded track.
    gpx_label: Option<String>,
    /// The last GPX load error, shown in the panel until the next successful load.
    gpx_error: Option<String>,
    /// Set when the Controls window is closed; quits the whole app next frame.
    quit: bool,
    texture: Option<egui::TextureHandle>,
    /// `--screenshot`: save the first composited frame here, then close.
    screenshot: Option<String>,
    screenshot_requested: bool,
    last_stats: obc_render::RenderStats,
    /// The shared render-on-demand dirty signal ([`App::take_dirty`]), drained once per frame for
    /// the stats panel. The sim always redraws, so this is informational — a live readout of the
    /// signal the firmware gates its renders on. (Mouse pan/zoom bypasses the app's input path, so
    /// it isn't reflected; on the device every camera change goes through a gesture or a fix.)
    last_dirty: Dirty,
    /// The shared host loop (`obc-host-core`): the command/event dispatcher, the in-flight
    /// resumable planner (#499, stepped once per frame — the board's one-step-per-pass shape), and
    /// the resident active-route session (so the Map opens without a per-frame `RouteIndex` reparse).
    /// Every delete/rescan/nav/track sequencing decision lives in the dispatcher, not here.
    host: HostLoop,
    /// The device body color drawn by the housing chrome. Switchable in the control panel.
    colorway: Colorway,
    /// This frame's device-control keyboard state, read at the top of `update` (before a widget
    /// can take focus and swallow the keys), then folded into the on-housing controls. Steps are
    /// edge-counted (how many this frame); the four buttons carry held state.
    kbd_steps: i32,
    kbd_up: bool,
    kbd_down: bool,
    kbd_select: bool,
    kbd_back: bool,
}

impl SimGui {
    fn new(bytes: Vec<u8>, args: Args) -> Self {
        let map_tables = MapTables::parse(&SliceSource(&bytes)).expect("map validated in main()");
        let (cx, cy, zoom) = {
            let cache = MapCache::new();
            let src = SliceSource(&bytes);
            let reader = Reader::new(&src, &map_tables, &cache);
            crate::initial_camera(&reader, args.width)
        };
        let mut state = AppState::new(cx, cy, zoom);
        if let Some(b) = args.battery {
            state.battery_pct = b;
        }
        // Start in Free so the mouse drives the camera; the fix is still seeded (map center)
        // so the loop and user marker have something to track.
        state.mode = CameraMode::Free;
        state.heading_up = args.heading.is_some();
        let loc = SimLocationSource::new(Some(Fix { lat: cy, lon: cx, course: args.heading, speed_mps: None }));

        // Seed the panel mirrors from the initial fix so the widgets open at the real position.
        let panel = match loc.current() {
            Some(f) => PanelState {
                lat_deg: f.lat as f64 / 1e6,
                lon_deg: f.lon as f64 / 1e6,
                heading_deg: f.course.unwrap_or(0.0),
                compass_deg: f.course.unwrap_or(0.0),
                ble: obc_app::BleStatus::DISCONNECTED,
                upload_sel: 0,
                trip_sel: 0,
                gps_time: true,
                clock_offset_secs: 0,
                retention_route_sel: 0,
                retention_level: obc_app::Retention::Day1,
                synced_ride_sel: 0,
            },
            None => PanelState {
                lat_deg: 0.0,
                lon_deg: 0.0,
                heading_deg: 0.0,
                compass_deg: 0.0,
                ble: obc_app::BleStatus::DISCONNECTED,
                upload_sel: 0,
                trip_sel: 0,
                gps_time: true,
                clock_offset_secs: 0,
                retention_route_sel: 0,
                retention_level: obc_app::Retention::Day1,
                synced_ride_sel: 0,
            },
        };

        // Boot at the device's real power-on state (Home / Idle, no route); the headless
        // `--png` path opens straight on the map instead (see `--boot`).
        let mut app = App::new_idle(state);
        if let Some(seed) = args.home_seed {
            app.reseed_home(seed);
        }
        let store = RouteStore::open(args.routes_dir());
        let trip_store = TripStore::open(args.routes_dir());
        let ride_store = RideStore::open(args.tracks_dir());
        let tracks = TrackStore::open(args.tracks_dir());
        // Seed the live settings from the persisted store, falling back to defaults on a first
        // run / unreadable file — the device's boot path.
        let mut settings_store = FileSettingsStore::open(args.settings_path());
        app.set_settings(settings_store.load().unwrap_or_default());
        // Mirror the map's §8.6 routing-profile names into the app for the Bike-type screen +
        // created-route overview label (N5). The map is loaded once in the sim, so this is a one-shot
        // (a device re-runs it on every map load).
        app.set_nav_profiles(map_tables.nav_profiles());
        app.set_map_nav_graph(map_tables.has_nav_graph());
        // Device-info built-ins for the System settings screen (T8 item 6): firmware version (the
        // sim's crate version) + the loaded map's name (filename stem) & OBCM version. The card-free
        // scan is answered per-frame in `update` when the screen posts its on-entry request.
        app.set_fw_version(env!("CARGO_PKG_VERSION"));
        let map_stem = std::path::Path::new(&args.map).file_stem().and_then(|s| s.to_str()).unwrap_or("map");
        app.set_map_info(map_stem, map_tables.version);
        // `--physical` only takes effect with a saved calibration; `--calibrate` opens the screen.
        let points_per_mm = crate::calib::load();
        let physical = args.physical && points_per_mm.is_some();
        let colorway = args.colorway.as_deref().and_then(Colorway::from_label).unwrap_or(Colorway::Forest);
        let mut gui = SimGui {
            app,
            store,
            trip_store,
            ride_store,
            tracks,
            settings_store,
            loc,
            fb: vec![0u8; (args.width * args.height) as usize],
            present: Present::new(args.width, args.height),
            dev_w: args.width,
            dev_h: args.height,
            scale: args.scale,
            points_per_mm,
            physical,
            physical_resize_pending: physical,
            calib: args.calibrate.then(CalibState::default),
            calib_error: None,
            panel,
            input: DeviceInput::new(),
            gpx: None,
            baro: BaroSensor::new(),
            compass: SimCompass::new(),
            sim_sensors: crate::sim_sensors::SimSensors::new(),
            gpx_label: None,
            gpx_error: None,
            quit: false,
            texture: None,
            screenshot: args.screenshot,
            screenshot_requested: false,
            bytes,
            map_tables,
            map_cache: MapCache::new(),
            last_stats: obc_render::RenderStats::default(),
            last_dirty: Dirty::CLEAN,
            host: HostLoop::new(),
            colorway,
            kbd_steps: 0,
            kbd_up: false,
            kbd_down: false,
            kbd_select: false,
            kbd_back: false,
        };
        gui.app.set_routes_with_ids(gui.store.catalog(), gui.store.ids());
        gui.app.set_trips(&gui.trip_store.inputs());
        gui.app.set_rides(gui.ride_store.catalog(), gui.ride_store.ids());
        // `--gpx` opens with a track loaded, paused at the start.
        if let Some(path) = &args.gpx {
            gui.load_gpx(Path::new(path));
        }
        gui
    }

    /// Parse a GPX file and load it as the active replay (paused at the start), or record the
    /// error for the panel (CLI `--gpx`, file-dialog).
    fn load_gpx(&mut self, path: &Path) {
        match Track::load(path) {
            Ok(track) => {
                let player = GpxPlayer::new(track);
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("track.gpx").to_string();
                self.gpx_label =
                    Some(format!("{name} — {} pts, {}", player.point_count(), units::format_clock(player.duration())));
                self.gpx = Some(player);
                self.gpx_error = None;
            }
            Err(e) => {
                self.gpx = None;
                self.gpx_label = None;
                self.gpx_error = Some(e);
            }
        }
    }

    /// Run the shared app for one frame into the backend's device-64 frame, present it through the
    /// seam, then upload the reconstructed texture.
    fn render_to_texture(&mut self, ctx: &egui::Context) {
        // Reuse the session-long cache (see the field doc) so the "Map SD" stats mirror glass.
        let map_src = SliceSource(&self.bytes);
        let reader = Reader::new(&map_src, &self.map_tables, &self.map_cache);

        // Feed the host→app BLE seam (epic #447): the control panel's injected link state, pushed
        // every frame exactly as the board's ride loop feeds its `ble::state` snapshot. Cheap and
        // idempotent — an unchanged status repaints nothing.
        self.app.set_ble_status(self.panel.ble);

        // Feed the host→app BLE **sensor** seam (epic #707, SE7) from a fake central manager, so the
        // Sensors screen is fully drivable without a radio. While the scan list is up, publish a
        // canned hit set (one per kind); for each saved slot, report Connected with a stand-in
        // battery — so pairing a hit (a Settings write) flips its row to Connected next frame, and
        // Forget drops it back to Not set. The board's ride loop drives the real thing the same shape.
        if self.app.sensor_scan_active() {
            self.app.set_sensor_scan_hits(&fake_scan_hits());
        } else {
            self.app.set_sensor_scan_hits(&[]);
        }
        let mut sensor_status = [obc_app::SensorStatus::default(); obc_app::SENSOR_SLOTS];
        for (q, slot) in self.app.settings().saved_sensors.iter().enumerate() {
            if slot.present {
                sensor_status[q] = obc_app::SensorStatus {
                    phase: obc_app::SensorPhase::Connected,
                    battery: Some(78),
                    last_value_ms: 0,
                };
            }
        }
        self.app.set_sensor_status(&sensor_status);

        // Host reconciliation through the shared dispatcher (`obc-host-core`): drain the typed
        // command protocol in canonical order and apply it against the sim's folder-backed stores —
        // the route/ride/trip deletes + catalog re-feeds, the resumable planner's lifecycle (one
        // bounded step per frame), the ride-track fill, and the ride log's session reconcile
        // (finalising a `Save` writes `RD{id}.ORD` and re-feeds the Rides menu). The handful of
        // genuinely host-specific commands go to the closure: the card-free scan (a fixed ~1.2 GB
        // stand-in — the sim has no FAT to scan), the Bluetooth Forget (the "bond" is the injected
        // panel flag), settings persistence (the RRAM stand-in file), and DFU (no real flash in the
        // desktop sim, so a no-op). Everything else — the sequencing — lives in the dispatcher.
        {
            let map_src = SliceSource(&self.bytes);
            let reader = Reader::new(&map_src, &self.map_tables, &self.map_cache);
            let settings_store = &mut self.settings_store;
            let panel = &mut self.panel;
            self.host.reconcile(
                &mut self.app,
                &mut self.store,
                &mut self.ride_store,
                &mut self.tracks,
                &mut self.trip_store,
                &reader,
                |app, cmd| match cmd {
                    HostCommand::ScanCardFree => {
                        app.apply_event(HostEvent::CardScanned { free_bytes: Some(1_288_490_188) });
                    }
                    HostCommand::ForgetBond => panel.ble.paired = false,
                    HostCommand::PersistSettings { revision } => {
                        // Persist to the RRAM stand-in file, then acknowledge the revision so the app
                        // clears its dirty state (or, on failure, keeps it retryable) — #810.
                        let event = match settings_store.save(app.settings()) {
                            Ok(()) => HostEvent::SettingsPersisted { revision },
                            Err(error) => HostEvent::SettingsPersistFailed { revision, error },
                        };
                        app.apply_event(event);
                    }
                    // No real firmware to flash in the desktop sim; the headless `--png` path stages
                    // synthetic DFU answers directly, so the interactive loop just drops the request.
                    HostCommand::Dfu(_) => {}
                    _ => {}
                },
            );
        }

        // Mirror the sim's route-retention sidecar into the app each frame (auto-expiry epic #638,
        // S3), pairwise with the just-fed catalog ids — cheap, and it keeps the sweep reading
        // device-truth retention even on frames the reconcile didn't re-feed the catalog.
        let metas = self.store.retention_metas();
        self.app.set_route_meta(&metas);

        // Open the active route's geometry from the resident session — no per-frame `RouteIndex`
        // reparse (reloads only when the active bytes change). It stays borrowed through `tick` +
        // `render_frame` below so the map-matcher gets it.
        let changed = self.store.sync_active(self.app.active_route_index());
        self.host.session.reparse(changed, &self.store);
        let route_src = self.store.active_source();
        let route = match (self.host.session.index(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };
        // The open Route overview's decimated shape preview — the shared once-per-entry fill.
        fill_nav_preview(&mut self.app, route.as_ref());

        // Drive the app from whichever location source is active. A loaded GPX replay takes over
        // from the manual panel fix (as the device's GPS would).
        if let Some(player) = self.gpx.as_mut() {
            // Advance + tick on the playback clock (shared with the headless replay).
            let dt = ctx.input(|i| i.stable_dt) as f64;
            // Feed the synthetic sensors on the same playback clock, from the *previous* frame's
            // speed (a ~1-frame lag is irrelevant at the 1 Hz emit cadence) — so a sample is stamped
            // onto the point this tick logs. Effort-follows-speed reads that speed; the sliders don't.
            let now_ms = (player.time() * 1000.0) as u32;
            let speed_mps = self.app.state.user_fix.and_then(|f| f.speed_mps).unwrap_or(0.0);
            self.sim_sensors.feed(now_ms, speed_mps);
            crate::replay_step(
                &mut self.app,
                player,
                &mut self.baro,
                Some(&mut self.compass),
                dt,
                route.as_ref(),
                self.tracks.sink(),
                crate::ReplaySensors {
                    hr: Some(&mut self.sim_sensors.hr),
                    power: Some(&mut self.sim_sensors.power),
                    cadence: Some(&mut self.sim_sensors.cadence),
                },
            );
            // Reflect the replayed fix in the panel mirrors, so manual control resumes from
            // here if the track is ejected.
            if let Some(f) = self.app.state.user_fix {
                self.panel.lat_deg = f.lat as f64 / 1e6;
                self.panel.lon_deg = f.lon as f64 / 1e6;
                if let Some(c) = f.course {
                    self.panel.heading_deg = c;
                }
            }
        } else {
            // Manual panel control: no barometer, wall-clock for any moving-time.
            self.baro.clear();
            let now_ms = self.input.now_ms();
            // The synthetic sensors run under manual control too (their sliders drive fixed values);
            // effort-follows-speed has no GPX speed here, so it reads whatever the last fix had (~0).
            let speed_mps = self.app.state.user_fix.and_then(|f| f.speed_mps).unwrap_or(0.0);
            self.sim_sensors.feed(now_ms, speed_mps);
            // The **GPS time** feed (auto-expiry epic #638, S3): with the control-panel toggle on
            // (the default), stamp the device clock from the host wall clock (plus the "+1 day"
            // offset) each tick — booting the sim into a trusted clock like a real fix would, the
            // precondition the deletion sweep gates on.
            let mut sim_clock = SimClock { enabled: self.panel.gps_time, offset_secs: self.panel.clock_offset_secs };
            let sensors = Sensors {
                loc: &mut self.loc,
                altimeter: None,
                // No thermometer in manual control — BMP581 temperature is device-only.
                temperature: None,
                clock: Some(&mut sim_clock),
                compass: Some(&mut self.compass),
                track: self.tracks.sink(),
                // Battery is set once from `--battery`; no live sim gauge.
                fuel: None,
                // The panel's "Sensors" section drives these (SE8); each source honours the ~1 Hz
                // fresh-mailbox contract, so a disabled quantity goes stale → `--` on its tile.
                hr: Some(&mut self.sim_sensors.hr),
                power: Some(&mut self.sim_sensors.power),
                cadence: Some(&mut self.sim_sensors.cadence),
            };
            self.app.tick(RideClock(now_ms), sensors, route.as_ref());
        }

        // Time the whole frame draw into `render_us` (`obc-render` is clockless, so the host
        // fills it; the device uses the DWT cycle counter). Render the whole frame straight into the
        // backend's resident device-64 plane — the device's own color path (`Rgb565` → device-64
        // pack), exactly as the firmware's map plane draws into its `FbDevice64`.
        let t0 = std::time::Instant::now();
        let (dev_w, dev_h) = (self.dev_w, self.dev_h);
        let mut fbdev = FbDevice64::new(&mut self.fb, dev_w, dev_h);
        let mut stats = self.app.render_frame(&mut fbdev, &reader, route.as_ref(), dev_w as f32, dev_h as f32, |c| {
            Rgb565::from(RawU16::new(c))
        });
        stats.render_us = t0.elapsed().as_micros() as u32;
        self.last_stats = stats;
        // Drain the shared dirty signal for the stats readout (the sim always redraws, so this
        // doesn't gate drawing).
        self.last_dirty = self.app.take_dirty();

        // Present: the presenter self-diffs the resident frame and pushes only the changed spans
        // into its reconstructed texture (under the exact-diff oracle). Uploading *that* — not a
        // whole-frame copy — means a diff bug shows as a stale row on glass, not just a failed
        // assert. `present_now` is the same engine the display-contract impls delegate to (the
        // contracts type geometry at compile time; the GUI's device size is the `--size` knob).
        self.present.present_now(&self.fb, None);
        let image = egui::ColorImage::from_rgb([dev_w as usize, dev_h as usize], self.present.texture());
        let opts = egui::TextureOptions::NEAREST;
        match &mut self.texture {
            Some(t) => t.set(image, opts),
            None => self.texture = Some(ctx.load_texture("screen", image, opts)),
        }
    }

    /// Apply mouse pan/scroll-zoom over the screen `rect`, switching to Free mode. `scale` is
    /// the *displayed* device-pixels-to-screen-points factor (the image is fit to the window,
    /// so it can differ from the requested `--scale`).
    fn handle_camera_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, scale: f32) {
        let (w, h) = (self.dev_w as f32, self.dev_h as f32);
        let st = &mut self.app.state;

        if resp.dragged() {
            let d = resp.drag_delta();
            let dpx = d.x / scale;
            let dpy = d.y / scale;
            let vp = st.viewport(w, h);
            // Convert the screen-space drag into a map delta through the inverse
            // projection (`to_map`), so panning follows the cursor even when the
            // view is rotated (heading-up) — a fixed `cam ± dx/zoom` would drift.
            let (lon0, lat0) = vp.to_map(w / 2.0, h / 2.0);
            let (lon1, lat1) = vp.to_map(w / 2.0 - dpx, h / 2.0 - dpy);
            st.cam_lon = st.cam_lon.wrapping_add(lon1.wrapping_sub(lon0));
            st.cam_lat = st.cam_lat.wrapping_add(lat1.wrapping_sub(lat0));
            st.mode = CameraMode::Free;
        }

        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(pos) = resp.hover_pos() {
                // Cursor in device pixels, the space `Viewport::to_map` expects.
                let local = pos - rect.min;
                let px = (local.x / scale).clamp(0.0, w);
                let py = (local.y / scale).clamp(0.0, h);
                let new_zoom = (st.zoom * (scroll * 0.005).exp()).clamp(units::MIN_ZOOM, units::MAX_ZOOM);

                // Keep the ground point under the cursor fixed across the zoom.
                let (olon, olat) = st.viewport(w, h).to_map(px, py);
                st.zoom = new_zoom;
                let (nlon, nlat) = st.viewport(w, h).to_map(px, py);
                st.cam_lon = st.cam_lon.wrapping_add(olon.wrapping_sub(nlon));
                st.cam_lat = st.cam_lat.wrapping_add(olat.wrapping_sub(nlat));
                st.mode = CameraMode::Free;
            }
        }
    }

    /// Draw the device — housing chrome plus the framebuffer blitted into its screen cutout —
    /// centred, at either the integer fit scale (default) or the panel's true physical size
    /// when 1:1 is on and calibrated. Reports this frame's device-control hit-test ([`DeviceHit`])
    /// so the caller can fold it into the shared input recognizer via [`apply_device_input`]; the
    /// drawing itself has no input side effects.
    #[must_use]
    fn show_device_image(&mut self, ctx: &egui::Context) -> DeviceHit {
        // Frame the device in a charcoal backdrop.
        let frame = egui::Frame::none().fill(housing::background());
        egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |ui| {
                let tex = self.texture.clone().expect("texture uploaded this frame");
                let style = housing::HousingStyle::default();
                let screen = egui::vec2(self.dev_w as f32, self.dev_h as f32);
                let disp_scale = match (self.physical, self.points_per_mm) {
                    // 1:1 — points per device pixel so 240 px spans the panel's real width.
                    // Fractional on purpose (the size isn't a whole multiple of the grid);
                    // NEAREST keeps it crisp at the cost of a slightly uneven pixel grid.
                    (true, Some(ppm)) => (crate::calib::PANEL_W_MM * ppm / self.dev_w as f32).max(0.05),
                    // Otherwise: largest integer scale at which the whole *device* fits,
                    // capped at `--scale`, ≥1 — keeps the screen at a crisp whole multiple.
                    _ => {
                        let avail = ui.available_size();
                        let dev = style.device_size_px(screen);
                        let fit = (avail.x / dev.x).min(avail.y / dev.y);
                        fit.floor().clamp(1.0, self.scale as f32)
                    }
                };
                let lo = style.layout(ui.available_rect_before_wrap(), disp_scale, screen);

                // The device controls live on the housing: click any of the four pads — UP / DOWN
                // on the left flank, SELECT / BACK on the right. Hit-test their rects here (drawing
                // only); the keyboard fold-in and shared recognizer run in `apply_device_input` from
                // the returned `DeviceHit`.
                let pad = |ui: &mut egui::Ui, rect, id| {
                    ui.interact(rect, egui::Id::new(id), egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                };
                let up = pad(ui, lo.up, "dev_up");
                let down = pad(ui, lo.down, "dev_down");
                let select = pad(ui, lo.select, "dev_select");
                let back = pad(ui, lo.back, "dev_back");
                // Wheel scroll over the pads stands in for tapping UP/DOWN (zero when not hovered);
                // the delta is applied in `apply_device_input`.
                let scroll_dy =
                    if up.hovered() || down.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
                // UP/DOWN are momentary: a *click* is one step (auto-repeat, which the firmware's
                // `ButtonInput` synthesizes from a held GPIO, isn't emulated — hold the arrow key
                // instead, which repeats through the OS).
                let steps = self.kbd_steps + i32::from(down.clicked()) - i32::from(up.clicked());
                let up_down = up.is_pointer_button_down_on() || self.kbd_up;
                let down_down = down.is_pointer_button_down_on() || self.kbd_down;
                let select_down = select.is_pointer_button_down_on() || self.kbd_select;
                let back_down = back.is_pointer_button_down_on() || self.kbd_back;

                // Mirror the live control state onto the housing.
                let ctrl = housing::ControlVisual { up_down, down_down, select_down, back_down };
                let palette = self.colorway.palette();

                // Paint the housing, then blit the framebuffer into its screen rect, corners rounded
                // to follow the bezel. Clone the painter so `ui`'s borrow is released before `ui.put`.
                let painter = ui.painter().clone();
                housing::draw(&painter, &lo, &style, &palette, &ctrl);
                let resp = ui.put(
                    lo.screen,
                    egui::Image::new(egui::load::SizedTexture::from_handle(&tex))
                        .fit_to_exact_size(lo.screen.size())
                        .texture_options(egui::TextureOptions::NEAREST)
                        .rounding(egui::Rounding::same(style.screen_radius_pts(disp_scale)))
                        .sense(egui::Sense::click_and_drag()),
                );
                // Mouse drag pans / scroll zooms over the screen.
                self.handle_camera_input(ui, &resp, resp.rect, disp_scale);

                DeviceHit { up_down, down_down, select_down, back_down, steps, scroll_dy }
            })
            .inner
    }

    /// Fold a frame's device-control hit-test ([`show_device_image`]'s [`DeviceHit`]) into the
    /// shared input recognizer — the same path the firmware runs with real GPIO — and persist
    /// settings on the dirty edge. Split out of the draw so drawing reports geometry only; the same
    /// events reach [`handle_input`](obc_app::App::handle_input) in the same order, with the same
    /// coordinates, they did inline.
    fn apply_device_input(&mut self, hit: DeviceHit) {
        // Mouse-wheel scroll → steps (non-zero only when a UP/DOWN pad was hovered this frame).
        if hit.scroll_dy != 0.0 {
            self.input.scroll(hit.scroll_dy);
        }
        self.input.step(hit.steps);
        self.input.set_button(Button::Select, hit.select_down);
        self.input.set_button(Button::Back, hit.back_down);
        // UP/DOWN held state is visual only (the housing pads sink) — the app already got the step.
        let _ = (hit.up_down, hit.down_down);
        let now = self.input.now_ms();
        self.app.handle_input(InputClock(now), &mut self.input);
        // Settings persistence rides the shared dispatcher now (the `PersistSettings` command, gated
        // on leaving the settings subtree), so `render_to_texture` picks up the dirty edge next frame
        // — the same debounced save-on-subtree-exit the device runs, one seam instead of two.
    }

    /// The 1:1 calibration screen: draw a reference bar of a known point-width; the user
    /// measures it and types the length → points-per-mm. `calib` is taken out of `self` so the
    /// egui closure borrows only locals.
    fn show_calibration(&mut self, ctx: &egui::Context) {
        let Some(mut calib) = self.calib.take() else { return };
        let mut save_ppm: Option<f32> = None;
        let mut cancel = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.heading("Actual-size calibration");
                ui.add_space(8.0);
                ui.label("Hold a ruler to the screen, measure the bar between the two ticks,");
                ui.label("then type its length. Saved once and reused on every launch.");
                ui.add_space(22.0);

                // Reference bar: a known width in points (clamped to the window). The user
                // measures its physical length, so points-per-mm = drawn width / mm.
                let bar_w = crate::calib::REF_BAR_POINTS.min(ui.available_width() - 48.0).max(60.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_w, 34.0), egui::Sense::hover());
                let p = ui.painter_at(rect);
                let col = ui.visuals().strong_text_color();
                let y = rect.center().y;
                p.line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(3.0_f32, col),
                );
                for x in [rect.left(), rect.right()] {
                    p.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(2.0_f32, col),
                    );
                }

                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    ui.label("Measured length:");
                    ui.add(egui::TextEdit::singleline(&mut calib.measured_mm).desired_width(70.0));
                    ui.label("mm");
                });
                let parsed = calib.measured_mm.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 1.0);

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(parsed.is_some(), egui::Button::new("Save")).clicked() {
                        save_ppm = parsed.map(|mm| bar_w / mm);
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                if parsed.is_none() && !calib.measured_mm.trim().is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "enter a length in mm (> 1)");
                }
            });
        });

        match save_ppm {
            Some(ppm) => match crate::calib::save(ppm) {
                Ok(()) => {
                    self.points_per_mm = Some(ppm);
                    self.physical = true;
                    self.physical_resize_pending = true;
                    self.calib_error = None;
                    // `calib` stays taken (None) → leave the calibration screen.
                }
                Err(e) => {
                    self.calib_error = Some(e);
                    self.calib = Some(calib); // keep the screen up to retry
                }
            },
            None if !cancel => self.calib = Some(calib), // still editing
            None => {}                                   // cancelled → leave the screen
        }
    }

    /// Snap the device window to match the current mode once, when 1:1 is toggled:
    /// the panel's true size in physical mode, the `--scale` default otherwise.
    fn apply_physical_resize(&mut self, ctx: &egui::Context) {
        if !std::mem::take(&mut self.physical_resize_pending) {
            return;
        }
        let dev = housing::HousingStyle::default().window_size_px(egui::vec2(self.dev_w as f32, self.dev_h as f32));
        let size = match (self.physical, self.points_per_mm) {
            (true, Some(ppm)) => {
                let s = crate::calib::PANEL_W_MM * ppm / self.dev_w as f32;
                egui::vec2(dev.x * s, dev.y * s)
            }
            // 1:1 off → back to the requested `--scale` window.
            _ => egui::vec2(dev.x * self.scale as f32, dev.y * self.scale as f32),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }
}

impl eframe::App for SimGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Read the device-control keyboard shortcuts *first*, before a widget can take focus and
        // swallow the keys. ↑/↓ are consumed (one step per press, repeating while held through the
        // OS key-repeat — the stand-in for the firmware's auto-repeat); Enter/Backspace carry the
        // live held state. Applied in `show_device_image`.
        let keys = ctx.input_mut(|i| {
            let mut steps = 0;
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Period)
            {
                steps += 1;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Comma)
            {
                steps -= 1;
            }
            (
                steps,
                i.key_down(egui::Key::ArrowUp),
                i.key_down(egui::Key::ArrowDown),
                i.key_down(egui::Key::Enter),
                i.key_down(egui::Key::Backspace),
            )
        });
        (self.kbd_steps, self.kbd_up, self.kbd_down, self.kbd_select, self.kbd_back) = keys;

        // Drag-and-drop a `.gpx` onto the window to import it (the device's USB-drop path).
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            let is_gpx = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("gpx"));
            if is_gpx {
                match self.store.import_gpx(&path) {
                    Ok(s) => {
                        self.gpx_error = None;
                        eprintln!(
                            "imported {} | {} km, +{} m",
                            path.display(),
                            (s.total_distance_m + 500) / 1000,
                            s.total_ascent_m
                        );
                    }
                    Err(e) => self.gpx_error = Some(e),
                }
                self.app.set_routes_with_ids(self.store.catalog(), self.store.ids());
            }
        }

        self.render_to_texture(ctx);

        // The device window shows either the live screen or the size-calibration UI. Drawing the
        // device only reports its hit-test; folding that into the input recognizer + saving
        // settings happens right after, out of the draw.
        if self.calib.is_some() {
            self.show_calibration(ctx);
        } else {
            let hit = self.show_device_image(ctx);
            self.apply_device_input(hit);
        }
        self.apply_physical_resize(ctx);

        // The Controls window (the development tool driving fix/sensors/BLE).
        self.show_control_panel(ctx);

        if self.screenshot.is_some() {
            self.run_screenshot(ctx);
        }

        // Closing the Controls window quits (otherwise a controls-less window lingers with no
        // way to drive the fix).
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Repaint continuously so control-panel / GPX changes show without a mouse event.
        ctx.request_repaint();
    }
}

/// Save an egui `ColorImage` (the captured frame) to a PNG.
/// A canned scan-hit set for the sim's fake sensor manager (SE7, epic #707): one HR strap, one power
/// meter, one unnamed cadence sensor — so any kind's scan list shows something (the unnamed one
/// exercises the address fallback). The scan-list screen filters to the row's quantity by `slot`.
fn fake_scan_hits() -> [obc_app::SensorScanHit; 3] {
    [
        obc_app::SensorScanHit::new(0, 1, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11], "HRM-Dual", -58),
        obc_app::SensorScanHit::new(1, 0, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F], "Stages LR", -67),
        obc_app::SensorScanHit::new(2, 0, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06], "", -80),
    ]
}

fn save_color_image(img: &egui::ColorImage, path: &str) -> Result<(), String> {
    let (w, h) = (img.size[0] as u32, img.size[1] as u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in &img.pixels {
        rgba.extend_from_slice(&[p.r(), p.g(), p.b(), p.a()]);
    }
    image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| "screenshot size mismatch".to_string())?
        .save(path)
        .map_err(|e| format!("screenshot save failed: {e}"))
}

impl SimGui {
    /// `--screenshot` flow: request a viewport capture on the first frame, save it when egui
    /// delivers it next frame, then close. Captures what the GPU actually displays (texture
    /// upload + draw), not just the framebuffer the headless `--png` path dumps.
    fn run_screenshot(&mut self, ctx: &egui::Context) {
        if !self.screenshot_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            self.screenshot_requested = true;
            return;
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            if let Some(path) = &self.screenshot {
                match save_color_image(&image, path) {
                    Ok(()) => eprintln!("wrote {path}"),
                    Err(e) => eprintln!("{e}"),
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
