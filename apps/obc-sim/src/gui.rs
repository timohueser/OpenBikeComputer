//! The eframe host window: the device screen.
//!
//! The desktop counterpart to the firmware's main loop. Each frame it polls the
//! [`SimLocationSource`], advances the shared [`obc_app::App`], renders the firmware-identical path
//! into the resident device-64 frame it owns beside its [`Present`] presenter, presents it through
//! the presenter's self-diffing engine, and blits the reconstructed texture to a GPU texture at
//! integer scale with nearest-neighbor sampling, so the pixel grid stays crisp.
//!
//! The second Controls viewport lives in [`panel`], and its zoom and formatting helpers in
//! [`units`].

use std::path::Path;

use eframe::egui;
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_app::device_core::PassClock;
use obc_app::settings::Settings;
use obc_app::{App, AppState, CameraMode, Dirty, Gesture};
use obc_display::FbDevice64;
use obc_host_core::{ActiveRouteSession, HostLoop, HostPlatform};
use obc_ports::{Button, Fix, InputClock, RideClock, Sensors, SettingsSaveError, SettingsStore};
use obc_route::RouteReader;

use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

/// How long the powering-off frame stays on the glass before the process ends: long enough to
/// read, short enough that it reads as an ending and not a hang.
const POWERING_OFF_HOLD: std::time::Duration = std::time::Duration::from_millis(700);

use crate::map_file::{LoadedMap, StdClock};
use crate::present::Present;
use crate::sim_compass::SimCompass;
use crate::sim_location::SimLocationSource;
use obc_host_core::frame;
use obc_host_core::{DeviceInput, FileSettingsStore, TrackStore};
use obc_host_core::{FlatRideStore as RideStore, RideRepository};
use obc_host_core::{FlatRouteStore as RouteStore, FlatTripStore as TripStore, RouteRepository};

use crate::Args;

mod housing;
mod panel;
mod units;

use housing::Colorway;

/// The control panel's editable mirrors. The [`SimLocationSource`] stores the fix as integer
/// microdegrees and a course, and egui widgets need `&mut` floats, so the panel edits these and
/// pushes them into the source each frame.
struct PanelState {
    lat_deg: f64,
    lon_deg: f64,
    heading_deg: f32,
    /// The Compass slider: the magnetometer heading that orients a heading-up map while the rider
    /// is stopped and the GPS course is `None`.
    compass_deg: f32,
    /// The injected BLE link state, pushed into the app each frame. It is the whole
    /// [`obc_app::BleStatus`] and not a `connected` flag, so passkey and upload injection extend
    /// this field rather than restructuring around it.
    ble: obc_app::BleStatus,
    /// The Inject upload combo's selected catalog row: which route the panel's upload buttons
    /// duplicate or rewrite by id.
    upload_sel: usize,
    /// The Delete trip combo's selected trip row: which trip the panel's delete button removes.
    /// The delete does not cascade.
    trip_sel: usize,
    gps_time: bool,
    clock_offset_secs: u32,
}

/// In-progress 1:1 size calibration: the user measures the on-screen reference bar and types the
/// millimetres here. `Some` while the calibration screen is up.
#[derive(Default)]
struct CalibState {
    measured_mm: String,
}

mod support;
pub(crate) use support::SIM_SUPPORT;

/// What only this host can do: the RRAM stand-in file, the injected panel bond, and the fixed
/// free-space figure, because the desktop sim has no FAT to scan.
struct SimPlatform<'a> {
    settings: &'a mut FileSettingsStore,
    panel: &'a mut PanelState,
}

impl HostPlatform for SimPlatform<'_> {
    /// Persist to the RRAM stand-in file. The answer clears the app's dirty state, or keeps the
    /// revision retryable on a failure.
    fn persist_settings(&mut self, settings: &Settings, _revision: u16) -> Result<(), SettingsSaveError> {
        self.settings.save(settings)
    }

    /// A fixed stand-in figure: the sim has no allocation table to walk.
    fn measure_free_space(&mut self) -> Result<u64, obc_app::device_core::StorageInfoError> {
        Ok(crate::SIM_CARD_FREE)
    }

    fn forget_bond(&mut self) -> Result<obc_app::ble::ControllerClearance, obc_app::ble::BondError> {
        self.panel.ble.paired = false;
        Ok(obc_app::ble::ControllerClearance::Confirmed)
    }
}

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
        // `DateTime` is minute-resolution, so the seconds ride separately and the wall clock
        // back-dates its epoch as a real fix does.
        Some(obc_ports::GpsTime { utc: obc_ports::DateTime::from_unix(unix), second: (unix % 60) as u8 })
    }
}

/// Launch the simulator window. `map` is opened once before this, for the process lifetime, as the
/// device parses its map once at boot, and the per-frame [`Reader`] is a cheap view over it.
pub fn run(
    map: LoadedMap,
    store: RouteStore,
    trip_store: TripStore,
    ride_store: RideStore,
    tracks: TrackStore,
    args: Args,
) -> Result<(), eframe::Error> {
    // The window wraps the whole device, housing and screen and a little backdrop, at `--scale`,
    // so the body has room around the framebuffer.
    let dev = housing::HousingStyle::default().window_size_px(egui::vec2(args.width as f32, args.height as f32));
    let win = [dev.x * args.scale as f32, dev.y * args.scale as f32];
    let title = "OBC Simulator";
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(title).with_inner_size(win),
        ..Default::default()
    };
    eframe::run_native(
        "OBC Simulator",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(SimGui::new(map, store, trip_store, ride_store, tracks, args)) as Box<dyn eframe::App>)
        }),
    )
}

/// One frame's device-control hit-test, produced while the housing is drawn and consumed by
/// [`SimGui::apply_device_input`]. The draw reports geometry only; the caller drives the
/// recognizer.
struct DeviceHit {
    /// UP pressed this frame, by the housing hit-test or the key alias.
    up_down: bool,
    /// DOWN pressed this frame, by the housing hit-test or the key alias.
    down_down: bool,
    /// SELECT pressed this frame, by the housing hit-test or the key alias.
    select_down: bool,
    /// BACK pressed this frame, by the housing hit-test or the key alias.
    back_down: bool,
    /// Selection steps injected directly this frame by the keyboard's one-shot aliases.
    steps: i32,
    /// Mouse-wheel delta over the screen, which stands in for tapping UP and DOWN.
    scroll_dy: f32,
}

struct SimGui {
    peak_view: crate::peak_view::Runtime,
    /// The opened map, held for the session as the device holds its own. Only the cheap `Reader`
    /// view is rebuilt per frame. It carries the immutable tables, parsed once at startup, and the
    /// session-long chunk cache, whose cross-frame reuse lets a panned-into view warm to a full hit
    /// rate so the stats track real device behaviour.
    map: LoadedMap,
    app: App,
    /// The render path's per-frame scratch, owned by the host: the app borrows it for a render
    /// call and keeps nothing across frames. Boxed so it never rides this struct's moves through
    /// the eframe setup.
    scratch: Box<obc_render::RenderScratch>,
    photo: obc_host_core::photo::Preparer,
    /// The card route projection and its retained active geometry.
    store: RouteStore,
    /// The trips beside the routes: the grouped-route folders. Rescanned and re-fed with the route
    /// catalog, so a rescan re-resolves the trips' stage ids.
    trip_store: TripStore,
    /// The tracks folder as the simulator's ride catalog: fixture files and process-local synced
    /// flags for the Rides screen. Rescanned when a ride is saved or deleted.
    ride_store: RideStore,
    /// The simulator's temporary sample log plus saved v3/GPX conveniences, reconciled to the app's
    /// tracking session each frame. The shipping device records directly into the flat journal.
    tracks: TrackStore,
    /// The persisted-settings store, standing in for the device's RRAM: it seeds the app at boot
    /// and is written on each settings change, so they survive a relaunch.
    settings_store: FileSettingsStore,
    loc: SimLocationSource,
    /// The resident device-64 frame the app renders into, owned by the GUI beside its presenter.
    /// Runtime-sized, because the device resolution is a CLI knob.
    fb: Vec<u8>,
    /// The simulator's presenter. [`Present::present_now`] self-diffs the resident frame and
    /// pushes only the changed spans into the uploaded texture. Its
    /// [`stats`](Present::stats) feed the panel.
    present: Present,
    dev_w: u32,
    dev_h: u32,
    scale: u32,
    /// Saved display calibration in egui points per millimetre, or `None` until the user
    /// calibrates.
    points_per_mm: Option<f32>,
    /// Render the device at the panel's true physical size. Needs `points_per_mm`.
    physical: bool,
    /// Snap the window to the device's 1:1 size next frame (set when 1:1 turns on).
    physical_resize_pending: bool,
    /// `Some` while the size-calibration screen is shown instead of the device image.
    calib: Option<CalibState>,
    /// Last calibration-save error, surfaced in the control panel.
    calib_error: Option<String>,
    /// Editable mirrors for the control-panel widgets.
    panel: PanelState,
    /// The four emulated device buttons, feeding the shared gesture recognizer.
    input: DeviceInput,
    /// The loaded GPX replay. When `Some` it drives the fix instead of the manual
    /// [`SimLocationSource`], as the device's GPS would. `None` is manual panel control.
    gpx: Option<GpxPlayer>,
    /// Simulated barometer, fed the replay's elevation on its own cadence, asynchronous to the GPS
    /// fix.
    baro: BaroSensor,
    /// Simulated compass, driven by the panel's Compass slider. It orients a heading-up map or
    /// Peak View while stopped, when the GPS has no course.
    compass: SimCompass,
    /// Synthetic BLE sensors, driven by the panel's Sensors section. Their three source fields feed
    /// `Sensors` each tick and honour the fresh-mailbox contract, so a disabled quantity reads as
    /// blank.
    sim_sensors: crate::sim_sensors::SimSensors,
    /// A short status line for the loaded track.
    gpx_label: Option<String>,
    /// The last GPX load error, shown in the panel until the next successful load.
    gpx_error: Option<String>,
    /// Set when the Controls window is closed; quits the whole app next frame.
    quit: bool,
    /// The panel's brightness, driven from [`App::backlight_level`] every frame: the simulator's
    /// [`Backlight`](obc_ports::Backlight) implementation.
    backlight: crate::panel_power::SimBacklight,
    /// The simulator's [`PowerOff`](obc_ports::PowerOff) port.
    power: crate::panel_power::SimPowerOff,
    /// Plays each pass's cue through the sound card.
    sounder: crate::sounder::SimSounder,
    /// The cue the last pass raised, for the Controls window's Sound line.
    last_cue: Option<obc_ports::Cue>,
    preview_cue: obc_ports::Cue,
    preview_volume: obc_ports::Volume,
    /// Milliseconds at which the rider completed the power-off hold, so the powering-off frame is
    /// looked at before the process ends. `None` until then.
    powering_off_at: Option<std::time::Instant>,
    texture: Option<egui::TextureHandle>,
    last_stats: obc_render::RenderStats,
    /// The render-on-demand signal the last pass planned, kept for the stats panel. The sim always
    /// redraws, so this is a readout and not a gate. Mouse pan and zoom bypass the app's input
    /// path, so they are not reflected.
    last_dirty: Dirty,
    /// The last pass's `next_wake_ms`, for the same panel. The simulator repaints continuously so
    /// its Controls window stays live, so the device's sleep schedule is shown and not obeyed.
    last_wake_ms: Option<u32>,
    /// The shared typed executor: the next pass's outcomes and facts, and the in-flight resumable
    /// planner, stepped once per frame as the board steps it once per pass. Every sequencing
    /// decision lives in a domain, not here.
    host: HostLoop,
    /// The resident active-route parse, opened once per frame and lent to both the pass and the
    /// render, so the Map opens without a per-frame `RouteIndex` reparse.
    session: ActiveRouteSession,
    /// The gestures the recognizer produced at the end of the previous frame, applied by the next
    /// pass's input stage. Recognition happens inside the egui draw, where the housing is
    /// hit-tested, and the pass applies them, so they wait one frame here.
    pending_gestures: Vec<Gesture>,
    /// Embedded terrain from the retained map, or the null source when unavailable.
    /// Planner emission and map-referenced altitude share this bounded cache.
    elevation: Box<dyn obc_route::ElevationSource>,
    /// The device body color drawn by the housing chrome. Switchable in the control panel.
    colorway: Colorway,
    /// This frame's device-control keyboard state, read at the top of `update` before a widget can
    /// take focus and swallow the keys, then folded into the on-housing controls. Steps are
    /// edge-counted; the four buttons carry held state.
    kbd_steps: i32,
    kbd_up: bool,
    kbd_down: bool,
    kbd_select: bool,
    kbd_back: bool,
}

impl SimGui {
    fn note_card_commit(&mut self) {
        if let Some(scope) = self.store.store_scope() {
            self.host.facts().note_store_revision(scope);
        }
    }

    fn new(
        map: LoadedMap,
        store: RouteStore,
        trip_store: TripStore,
        ride_store: RideStore,
        tracks: TrackStore,
        args: Args,
    ) -> Self {
        // The map's style table and LOD pyramid, parsed once: the tables every reader borrows.
        let map_tables = map.tables();
        let (cx, cy, zoom) = crate::initial_camera(&map.reader(), args.width);
        let mut state = AppState::new(cx, cy, zoom);
        let peak_view = crate::peak_view::Runtime::new(&map, args.peak_view);
        state.peak_view_profile = peak_view.profile();
        let peak_profile = args.peak_view.map(crate::peak_view::Preset::profile);
        if let Some(profile) = peak_profile {
            state.compass_deg = Some(profile.default_heading_q4 as f32 / 4.0);
        }
        if let Some(b) = args.battery {
            state.device.battery_pct = b;
        }
        // Start in Free so the mouse drives the camera. The fix is still seeded at the map center,
        // so the loop and the user marker have something to track.
        state.mode = CameraMode::Free;
        state.heading_up = args.heading.is_some();
        let (fix_lat, fix_lon) =
            peak_profile.map(|profile| (profile.observer_lat, profile.observer_lon)).unwrap_or((cy, cx));
        let loc = SimLocationSource::new(Some(Fix {
            lat: fix_lat,
            lon: fix_lon,
            course: args.heading,
            speed_mps: args.heading.map(|_| 5.0).or(Some(0.0)),
        }));

        // Seed the panel mirrors from the initial fix so the widgets open at the real position.
        let panel = match loc.current() {
            Some(f) => PanelState {
                lat_deg: f.lat as f64 / 1e6,
                lon_deg: f.lon as f64 / 1e6,
                heading_deg: f.course.unwrap_or_else(|| {
                    peak_profile.map(|profile| profile.default_heading_q4 as f32 / 4.0).unwrap_or(0.0)
                }),
                compass_deg: f.course.unwrap_or_else(|| {
                    peak_profile.map(|profile| profile.default_heading_q4 as f32 / 4.0).unwrap_or(0.0)
                }),
                ble: obc_app::BleStatus::DISCONNECTED,
                upload_sel: 0,
                trip_sel: 0,
                gps_time: args.clock.is_none(),
                clock_offset_secs: 0,
            },
            None => PanelState {
                lat_deg: 0.0,
                lon_deg: 0.0,
                heading_deg: 0.0,
                compass_deg: 0.0,
                ble: obc_app::BleStatus::DISCONNECTED,
                upload_sel: 0,
                trip_sel: 0,
                gps_time: args.clock.is_none(),
                clock_offset_secs: 0,
            },
        };

        // Boot at the device's real power-on state. The headless `--png` path opens straight on
        // the map instead.
        let mut app = App::new_idle(state);
        if app.state.user_fix.is_some() {
            let mut loc = crate::sim_location::SimLocationSource::new(app.state.user_fix);
            app.tick(obc_ports::RideClock(0), obc_ports::Sensors::new(&mut loc), None);
        }
        tracks.offer_recovery(&mut app);
        // Seed the live settings from the persisted store, falling back to defaults on a first run
        // or an unreadable file, as the device's boot path does.
        let mut settings_store = FileSettingsStore::open(args.settings_path());
        let boot_settings = settings_store.load().unwrap_or_default();
        app.set_settings(boot_settings);
        args.stamp_initial_clock(&mut app);
        app.set_map_nav_graph(map_tables.has_nav_graph());
        // Device-info built-ins for the System settings screen: the firmware version, standing in
        // as the sim's crate version, and the loaded map's name and version. The free-space scan is
        // answered per frame in `update`, when the screen posts its on-entry request.
        // The panel-light capability, straight from the port the window drives. The drawer's root
        // row is built from it, and `--no-backlight` builds a lightless platform instead, so the
        // window shows the three-control sheet and never scales the blit.
        let backlight = crate::panel_power::SimBacklight::new(!args.no_backlight);
        app.set_backlight_available(obc_ports::Backlight::available(&backlight));
        let sounder = crate::sounder::SimSounder::open(!args.no_sound);
        app.set_sound_available(obc_ports::Sounder::available(&sounder));
        // The window draws into one resident device-64 plane and presents it by self-diff, as the
        // board does, so the frozen base's rows survive between frames.
        app.set_resident_frame(true);
        if peak_profile.is_some() {
            app.show_peak_view();
        }
        app.set_fw_version(env!("CARGO_PKG_VERSION"));
        let map_name = map.display_name();
        app.set_map_info(map_name, map_tables.version);
        // `--physical` takes effect only with a saved calibration; the panel opens calibration.
        let points_per_mm = crate::calib::load();
        let physical = args.physical && points_per_mm.is_some();
        let colorway = Colorway::Forest;
        let mut gui = SimGui {
            peak_view,
            app,
            scratch: Box::new(obc_render::RenderScratch::new()),
            photo: obc_host_core::photo::Preparer::default(),
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
            calib: None,
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
            backlight,
            power: crate::panel_power::SimPowerOff,
            sounder,
            last_cue: None,
            preview_cue: obc_ports::Cue::KeyClick,
            preview_volume: obc_ports::Volume::Loud,
            powering_off_at: None,
            texture: None,
            elevation: map.elevation(),
            map,
            last_stats: obc_render::RenderStats::default(),
            last_dirty: Dirty::CLEAN,
            last_wake_ms: None,
            host: HostLoop::new(),
            session: ActiveRouteSession::new(),
            pending_gestures: Vec::new(),
            colorway,
            kbd_steps: 0,
            kbd_up: false,
            kbd_down: false,
            kbd_select: false,
            kbd_back: false,
        };
        gui.note_card_commit();
        gui.app.set_routes_with_ids(gui.store.catalog(), gui.store.ids());
        gui.app.set_trips(&gui.trip_store.inputs());
        gui.app.set_trip_progress(gui.trip_store.progress().iter().cloned());
        let join = obc_host_core::day_join(&gui.app, &gui.store, &gui.trip_store);
        gui.app.set_day_join(join);
        gui.app.set_rides(gui.ride_store.catalog(), gui.ride_store.trip_names());

        // `--gpx` opens with a track loaded, paused at the start.
        if let Some(path) = &args.gpx {
            gui.load_gpx(Path::new(path));
        }
        gui
    }

    /// Parse a GPX file and load it as the active replay, paused at the start, or record the error
    /// for the panel.
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

    fn render_to_texture(&mut self, ctx: &egui::Context) {
        // Reuse the session-long tables and chunk cache: the map is parsed once at startup, as the
        // device parses once at boot, so a frame costs one cheap `Reader` view. The map plane, nav,
        // POI, hours and routing all read it.
        let reader = self.map.reader();

        // Feed the BLE seam with the control panel's injected link state, every frame, as the
        // board's ride loop feeds its own snapshot. An unchanged status repaints nothing.
        self.host.facts().note_link(self.panel.ble);

        // Feed the BLE sensor seam from a fake central manager, so the Sensors screen is drivable
        // without a radio. While the scan list is up it publishes a canned hit set, one per kind,
        // and reports each saved slot as connected with a stand-in battery. Pairing a hit is a
        // settings write, so its row flips next frame, and Forget drops it back.
        if self.app.sensor_scan_active() {
            self.app.set_sensor_scan_hits(&crate::fake_scan_hits());
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

        // One DeviceCore pass. The active route is opened once from the resident session and lent
        // to the pass, so the map-matcher reads the geometry the frame draws. The render below
        // re-opens it, because the executor may commit new bytes under it.
        self.session.sync(&self.app, &mut self.store);
        let ui_now = self.input.now_ms();
        let gestures = core::mem::take(&mut self.pending_gestures);
        let mut plan = {
            let route_src = self.store.active_source();
            let route = match (self.session.index(), route_src) {
                (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
                _ => None,
            };
            // Drive the app from whichever location source is active. A loaded GPX replay takes
            // over from the manual panel fix, as the device's GPS would.
            if let Some(player) = self.gpx.as_mut() {
                let dt = ctx.input(|i| i.stable_dt) as f64;
                // Feed the synthetic sensors on the same playback clock, from the previous frame's
                // speed, so a sample is stamped onto the point this pass logs. The one-frame lag
                // does not matter at the emit cadence.
                let speed_mps = self.app.state.user_fix.and_then(|f| f.speed_mps).unwrap_or(0.0);
                self.sim_sensors.feed((player.time() * 1000.0) as u32, speed_mps);
                let (ride, sensors) = obc_host_core::replay_advance(
                    player,
                    &mut self.baro,
                    Some(&mut self.compass),
                    dt,
                    obc_host_core::ReplaySensors {
                        hr: Some(&mut self.sim_sensors.hr),
                        power: Some(&mut self.sim_sensors.power),
                        cadence: Some(&mut self.sim_sensors.cadence),
                    },
                );
                self.host.pass(
                    &mut self.app,
                    PassClock { ride, ui: InputClock(ui_now) },
                    &gestures,
                    sensors,
                    route.as_ref(),
                    SIM_SUPPORT,
                )
            } else {
                // Manual panel control: no barometer, and wall clock for any moving time.
                self.baro.clear();
                // The synthetic sensors run under manual control too, driven by their sliders.
                // Effort-follows-speed has no replay speed here, so it reads the last fix's.
                let speed_mps = self.app.state.user_fix.and_then(|f| f.speed_mps).unwrap_or(0.0);
                self.sim_sensors.feed(ui_now, speed_mps);
                let mut sim_clock =
                    SimClock { enabled: self.panel.gps_time, offset_secs: self.panel.clock_offset_secs };
                // Defaulted away: no thermometer in manual control, and no live fuel gauge, because
                // the battery is set once from `--battery`.
                let sensors = Sensors {
                    clock: Some(&mut sim_clock),
                    compass: Some(&mut self.compass),
                    // The panel's Sensors section drives these. Each source honours the
                    // fresh-mailbox contract, so a disabled quantity goes stale and reads blank.
                    hr: Some(&mut self.sim_sensors.hr),
                    power: Some(&mut self.sim_sensors.power),
                    cadence: Some(&mut self.sim_sensors.cadence),
                    ..Sensors::new(&mut self.loc)
                };
                self.host.pass(
                    &mut self.app,
                    PassClock { ride: RideClock(ui_now), ui: InputClock(ui_now) },
                    &gestures,
                    sensors,
                    route.as_ref(),
                    SIM_SUPPORT,
                )
            }
        };
        // A single-loop host has no second recognizer to cancel, so it consumes the hold-cancel
        // latch the pass may have armed rather than leaving it set for a plane that does not exist.
        let _ = self.app.take_hold_cancel();

        // Reflect the replayed fix in the panel mirrors, so manual control resumes from here if the
        // track is ejected.
        if self.gpx.is_some() {
            if let Some(f) = self.app.state.user_fix {
                self.panel.lat_deg = f.lat as f64 / 1e6;
                self.panel.lon_deg = f.lon as f64 / 1e6;
                if let Some(c) = f.course {
                    self.panel.heading_deg = c;
                }
            }
        }

        {
            let mut platform = SimPlatform { settings: &mut self.settings_store, panel: &mut self.panel };
            self.host.execute(
                &mut self.app,
                &mut plan,
                &mut self.session,
                &mut self.store,
                &mut self.ride_store,
                &mut self.tracks,
                &mut self.trip_store,
                self.map.planner_map(),
                &mut *self.elevation,
                &mut platform,
            );
        }
        if let Some(sound) = plan.sound {
            obc_ports::Sounder::play(&mut self.sounder, obc_platform::sound::pattern(sound.cue), sound.volume);
            self.last_cue = Some(sound.cue);
        }
        // The map-referenced altimeter's terrain read, drained once per frame behind the pass, as
        // the board's ride loop does. A fresh fix arms it, so it reads at most one tile per fix.
        self.app.sample_terrain(&mut *self.elevation);

        self.session.sync(&self.app, &mut self.store);
        let route = frame::active_route(&self.session, &self.store);

        self.peak_view.update(&mut self.app);
        let panorama = self.peak_view.panorama();

        // Time the whole frame draw into `render_us`: `obc-render` is clockless, so the host fills
        // it. The frame renders straight into the resident device-64 plane, which is the device's
        // own color path.
        let t0 = std::time::Instant::now();
        let (dev_w, dev_h) = (self.dev_w, self.dev_h);
        let mut fbdev = FbDevice64::new(&mut self.fb, dev_w, dev_h);
        let scene = frame::Scene { reader: &reader, route: route.as_ref() };
        // The frame renders the snapshot this pass decided over, sampled before it, so the card,
        // the step count and the raster are one decision.
        let (app, scratch) = (&mut self.app, &mut *self.scratch);
        let mut stats = frame::render(
            app,
            scratch,
            &mut fbdev,
            scene,
            panorama,
            (dev_w as f32, dev_h as f32),
            |c| Rgb565::from(RawU16::new(c)),
            &StdClock(t0),
            Some(self.photo.interactive(plan.render.map)),
        );
        stats.render_us = t0.elapsed().as_micros() as u32;
        self.last_stats = stats;
        // The plan's render decision, for the stats readout: the sim always redraws, so it gates
        // nothing. `plan.next_wake_ms` rides beside it for the same reason.
        self.last_dirty = plan.render;
        self.last_wake_ms = plan.next_wake_ms;

        // The presenter self-diffs the resident frame and pushes only the changed spans into its
        // reconstructed texture. Uploading that texture, rather than a whole-frame copy, means a
        // diff bug shows as a stale row on glass and not only as a failed assert.
        self.present.present_now(&self.fb, None);
        self.peak_view.note_frame_presented(&self.app);
        // The backlight, applied where a panel's brightness is visible: the pixels the window
        // blits. Driven every frame from the app's own answer, which is the drawer's staged preview
        // while its editor is open and the committed setting otherwise.
        let _ = obc_ports::Backlight::apply(&mut self.backlight, self.app.backlight_level());
        let size = [dev_w as usize, dev_h as usize];
        let image = match self.backlight.gain() {
            None => egui::ColorImage::from_rgb(size, self.present.texture()),
            Some(gain) => {
                let dim: Vec<u8> = self.present.texture().iter().map(|c| (*c as u16 * gain / 255) as u8).collect();
                egui::ColorImage::from_rgb(size, &dim)
            }
        };
        let opts = egui::TextureOptions::NEAREST;
        match &mut self.texture {
            Some(t) => t.set(image, opts),
            None => self.texture = Some(ctx.load_texture("screen", image, opts)),
        }
    }

    /// Apply mouse pan and scroll-zoom over the screen `rect`, switching to Free mode. `scale` is
    /// the displayed device-pixels-to-screen-points factor, which can differ from the requested
    /// `--scale` because the image is fit to the window.
    fn handle_camera_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect, scale: f32) {
        let (w, h) = (self.dev_w as f32, self.dev_h as f32);
        let st = &mut self.app.state;

        if resp.dragged() {
            let d = resp.drag_delta();
            let dpx = d.x / scale;
            let dpy = d.y / scale;
            let vp = st.viewport(w, h);
            // Convert the screen-space drag into a map delta through the inverse projection, so
            // panning follows the cursor even when the view is rotated. A fixed offset would drift.
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

    /// Draw the device, housing chrome plus the framebuffer blitted into its screen cutout,
    /// centred, at the integer fit scale or the panel's true physical size when 1:1 is on and
    /// calibrated. It reports this frame's [`DeviceHit`] and has no input side effects.
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
                    // 1:1: points per device pixel, so the frame spans the panel's real width.
                    // Fractional on purpose, and nearest sampling keeps it crisp at the cost of a
                    // slightly uneven pixel grid.
                    (true, Some(ppm)) => (crate::calib::PANEL_W_MM * ppm / self.dev_w as f32).max(0.05),
                    // Otherwise the largest integer scale at which the whole device fits, capped
                    // at `--scale`, which keeps the screen at a crisp whole multiple.
                    _ => {
                        let avail = ui.available_size();
                        let dev = style.device_size_px(screen);
                        let fit = (avail.x / dev.x).min(avail.y / dev.y);
                        fit.floor().clamp(1.0, self.scale as f32)
                    }
                };
                let lo = style.layout(ui.available_rect_before_wrap(), disp_scale, screen);

                // The device controls live on the housing: UP and DOWN on the left flank, SELECT
                // and BACK on the right. Their rects are hit-tested here; the keyboard fold-in and
                // the shared recognizer run in `apply_device_input`.
                let pad = |ui: &mut egui::Ui, rect, id| {
                    ui.interact(rect, egui::Id::new(id), egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                };
                let up = pad(ui, lo.up, "dev_up");
                let down = pad(ui, lo.down, "dev_down");
                let select = pad(ui, lo.select, "dev_select");
                let back = pad(ui, lo.back, "dev_back");
                // Wheel scroll over the pads stands in for tapping UP and DOWN, and is zero when
                // they are not hovered. The delta is applied in `apply_device_input`.
                let scroll_dy =
                    if up.hovered() || down.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
                // All four pads carry held state, so a held pad or arrow key auto-repeats through
                // the shared recognizer at the device's own cadence. Only the one-shot keyboard
                // aliases inject finished steps.
                //
                // `clicked()` is OR-ed in because held state is sampled once a frame: a press and
                // its release inside one long frame would otherwise show no transition and the tap
                // would vanish. The extra frame of held state still yields one press and release.
                let steps = self.kbd_steps;
                let up_down = up.is_pointer_button_down_on() || up.clicked() || self.kbd_up;
                let down_down = down.is_pointer_button_down_on() || down.clicked() || self.kbd_down;
                let select_down = select.is_pointer_button_down_on() || select.clicked() || self.kbd_select;
                let back_down = back.is_pointer_button_down_on() || back.clicked() || self.kbd_back;

                // Mirror the live control state onto the housing.
                let ctrl = housing::ControlVisual { up_down, down_down, select_down, back_down };
                let palette = self.colorway.palette();

                // Paint the housing, then blit the framebuffer into its screen rect with the
                // corners rounded to follow the bezel. The painter is cloned so `ui`'s borrow is
                // released before `ui.put`.
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
                // Mouse drag pans and scroll zooms over the screen.
                self.handle_camera_input(ui, &resp, resp.rect, disp_scale);

                DeviceHit { up_down, down_down, select_down, back_down, steps, scroll_dy }
            })
            .inner
    }

    /// Fold a frame's [`DeviceHit`] into the shared input recognizer, which is the path the
    /// firmware runs with real GPIO, and persist settings on the dirty edge. Split out of the draw
    /// so drawing reports geometry only.
    fn apply_device_input(&mut self, hit: DeviceHit) {
        // Mouse-wheel scroll becomes steps, and is non-zero only over a UP or DOWN pad.
        if hit.scroll_dy != 0.0 {
            self.input.scroll(hit.scroll_dy);
        }
        self.input.step(hit.steps);
        self.input.set_button(Button::Up, hit.up_down);
        self.input.set_button(Button::Down, hit.down_down);
        self.input.set_button(Button::Select, hit.select_down);
        self.input.set_button(Button::Back, hit.back_down);
        let now = self.input.now_ms();
        // Recognition only: the pass applies the batch at its input stage on the next frame,
        // where a gesture lands after what the executor finished and before the domains decide.
        self.pending_gestures.extend(self.app.recognize(InputClock(now), &mut self.input));
    }

    /// The 1:1 calibration screen: it draws a reference bar of a known point width, and the user
    /// measures it and types the length, which gives points per millimetre. `calib` is taken out of
    /// `self` so the egui closure borrows only locals.
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

                // Reference bar: a known width in points, clamped to the window. The user measures
                // its physical length, which gives points per millimetre.
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
                    // `calib` stays taken, which leaves the calibration screen.
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

    /// Snap the device window to the current mode once, when 1:1 is toggled: the panel's true size
    /// in physical mode, the `--scale` default otherwise.
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
            // 1:1 off goes back to the requested `--scale` window.
            _ => egui::vec2(dev.x * self.scale as f32, dev.y * self.scale as f32),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }
}

impl eframe::App for SimGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Read the device-control keyboard shortcuts first, before a widget can take focus and
        // swallow the keys. All four keys carry live held state into the same edge recognizer the
        // firmware feeds, so a held key auto-repeats at the device's cadence and not the OS key
        // repeat's. The bracket, comma and period aliases stay one-shot injected steps, because no
        // button models them. Applied in `show_device_image`.
        let keys = ctx.input_mut(|i| {
            let mut steps = 0;
            if i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Period)
            {
                steps += 1;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::Comma)
            {
                steps -= 1;
            }
            // The four device keys are read as held state, OR-ed with pressed-this-frame, so a tap
            // that also releases inside one long frame still produces an edge. The arrow keys then
            // have their press events eaten, so a focused slider or text field does not act on them
            // too. Consuming does not touch `key_down`, and a key repeat can queue more than one
            // press per frame, so they are drained. A modified arrow is an editing shortcut and is
            // left alone.
            let held = |i: &egui::InputState, k| i.modifiers.is_none() && (i.key_down(k) || i.key_pressed(k));
            let (left, right) = (held(i, egui::Key::ArrowLeft), held(i, egui::Key::ArrowRight));
            let (enter, back) = (held(i, egui::Key::Enter), held(i, egui::Key::Backspace));
            for key in [egui::Key::ArrowLeft, egui::Key::ArrowRight] {
                while i.consume_key(egui::Modifiers::NONE, key) {}
            }
            (steps, left, right, enter, back)
        });
        (self.kbd_steps, self.kbd_up, self.kbd_down, self.kbd_select, self.kbd_back) = keys;

        // Drag and drop a `.gpx` onto the window to import it, as the device's USB drop does.
        let dropped: Vec<std::path::PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            let is_gpx = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("gpx"));
            if is_gpx {
                match crate::routes::import_gpx(
                    &mut self.store,
                    &path,
                    self.app.settings().bike_type,
                    Some((&self.map.reader(), self.map.route_attribution_key())),
                ) {
                    Ok(s) => {
                        self.gpx_error = None;
                        self.note_card_commit();
                        eprintln!(
                            "imported {} | {} km, +{} m",
                            path.display(),
                            (s.total_distance_m + 500) / 1000,
                            s.total_ascent_m
                        );
                    }
                    Err(e) => self.gpx_error = Some(e),
                }
            }
        }

        self.render_to_texture(ctx);

        // The device window shows either the live screen or the size-calibration UI. Drawing the
        // device only reports its hit-test; folding that into the input recognizer and saving
        // settings happens right after, out of the draw.
        if self.calib.is_some() {
            self.show_calibration(ctx);
        } else {
            let hit = self.show_device_image(ctx);
            self.apply_device_input(hit);
        }
        self.apply_physical_resize(ctx);

        // The Controls window, which drives the fix, the sensors and BLE.
        self.show_control_panel(ctx);

        // Closing the Controls window quits, because otherwise a window with no controls lingers
        // and nothing can drive the fix.
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // The power-off port, once the frame the rider is looking at has been on the glass long
        // enough to read. `power_off` does not return.
        if self.app.power_off_requested() {
            let since = *self.powering_off_at.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= POWERING_OFF_HOLD {
                obc_ports::PowerOff::power_off(&mut self.power);
            }
        } else {
            self.powering_off_at = None;
        }

        // Repaint continuously, so control-panel and GPX changes show with no mouse event.
        ctx.request_repaint();
    }
}
