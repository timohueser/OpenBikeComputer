//! The demo core: one `Demo` owns the whole embedded device, and advances it one JS-driven frame
//! at a time.
//!
//! JS owns the rAF loop. Each [`tick`](Demo::tick) drains the queued [`Cmd`]s, advances the ride,
//! steps an in-flight [`NavPlan`] once, and renders into the RGBA frame only when the app says
//! something changed.
//!
//! Everything here is target-independent and tested natively; only the `#[wasm_bindgen]` surface in
//! `main.rs` is wasm-only.

use obc_app::device_core::{PassClock, PassPlan, PlatformSupport, RouteUpload};
use obc_app::recorder::RecorderOutcome;
use obc_app::{App, AppState, CameraMode, Gesture};
use obc_host_core::flat_map::FlatMap;
use obc_host_core::frame;
use obc_host_core::{
    initial_camera, replay_advance, ActiveRouteSession, FlatRideRecorder, FlatRideStore, FlatRouteStore, HostLoop,
    ReplaySensors, RgbaFrame,
};
use obc_host_core::{RideRepository, RouteRepository};
use obc_ports::InputClock;
use obc_reader::{MapTables, SliceSource};
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};
use obc_route::RouteReader;

/// The demo panel resolution, from the one [`obc_display`] frame authority.
pub const FRAME_W: u32 = obc_display::ls021::FRAME_W as u32;
pub const FRAME_H: u32 = obc_display::ls021::FRAME_H as u32;

// The wasm-only map stays app-owned. Shared authored route and replay sources live in the fixture
// registry, so other components never reach through this app's asset directory.
pub(crate) const DEMO_MAP: &[u8] = include_bytes!("../../obc-sim/assets/grimsel-demo.obcm");
const DEMO_ROUTE: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
const DEMO_RIDE_GPX: &str = include_str!("../../../fixtures/sources/sim-grimsel/tracks/grimsel-climb-demo.gpx");

/// The demo map's terrain region: the elevation the altimeter samples and the surface Peak View
/// builds its panorama from.
static TERRAIN: std::sync::LazyLock<SliceSource<'static>> = std::sync::LazyLock::new(|| {
    let tables = MapTables::parse(&SliceSource(DEMO_MAP)).expect("demo map parses");
    let region = tables.terrain().expect("demo map contains terrain");
    SliceSource(&DEMO_MAP[region.offset as usize..(region.offset + region.len) as usize])
});

/// Replay-speed multiplier: three times a normal climbing pace keeps the map moving without a
/// blur.
const DEMO_SPEED: f32 = 3.0;

/// Keep the opening map at riding scale even when a refreshed extract has distant boundary nodes.
const DEMO_MPP: f32 = 6.4;

/// The GPX playback time in seconds a guided-demo baseline seeks to: mid the ride's first climb,
/// so a climb is active and the map sits in the switchbacks. The map-matcher re-locks from this
/// teleport within a few frames. The ambient baseline starts from 0 instead.
const TOUR_BASELINE_S: f64 = 1500.0;

/// Ceiling on one frame's replay advance, in seconds of wall clock. A backgrounded tab stops the
/// rAF loop, and without the clamp the first frame back would teleport the ride by minutes.
const MAX_FRAME_DT_S: f64 = 0.25;

/// What this host implements: everything the shared screens can reach, because the page can walk
/// anywhere the device can. A capability withdrawn here would show the visitor a screen the real
/// device does not have. The bounded work behind DFU and free space is never answered.
const SUPPORT: PlatformSupport = PlatformSupport {
    detour: true,
    settings_persistence: true,
    dfu: true,

    bonding: true,
    storage_space_report: true,
    // The shared memory card holds metadata for this page session.
};

/// One queued page command, drained per [`Demo::tick`]. Gestures go through the app's deterministic
/// [`apply_gesture`](App::apply_gesture) seam, and only finished gestures: the long-press hold
/// timers live in JS.
pub enum Cmd {
    Gesture(Gesture),
    Play,
    Pause,
    Seek(f64),
    Heading(f32),
    /// Rebuild the idle device that waits underneath the phone-to-device handoff.
    StageUpload,
    /// Deliver the same typed upload event the BLE host posts after committing the embedded route.
    ReceiveRoute,
    /// Enter guided-demo mode: reset to the staged mid-climb baseline. The tour engine drives
    /// playback and gestures from here, and the ambient summit auto-restart is suspended.
    Enter,
    /// Leave guided-demo mode: hand the device to the visitor where the demo left it, and restore
    /// the ambient auto-restart.
    Exit,
    /// Reset to the state the page opens on: a clean live ride from the start, controls enabled.
    Ambient,
    /// One device-wide squeeze. Unlike a gesture it is applied straight to the app, because the
    /// recognizer that would produce it lives below the page's command vocabulary.
    Chord(obc_app::Chord),
}

impl Cmd {
    fn baseline(&self) -> Option<Baseline> {
        match self {
            Self::StageUpload => Some(Baseline::Upload),
            Self::Enter => Some(Baseline::Tour),
            Self::Ambient => Some(Baseline::Ambient),
            _ => None,
        }
    }
}

/// Parse one command string. The page-facing vocabulary is exactly: `press`, `back`, `hold`,
/// `backhold`, `quick`, `context`, `step:<n>`, `play`, `pause`, `seek:<secs>`, `enter`, `exit`,
/// `ambient`, `upload`, `receive` and `heading:<degrees>`. Unknown input is ignored.
pub fn parse_cmd(cmd: &str) -> Option<Cmd> {
    match cmd {
        "press" => Some(Cmd::Gesture(Gesture::Press)),
        "context" => Some(Cmd::Chord(obc_app::Chord::Context)),
        "quick" => Some(Cmd::Chord(obc_app::Chord::Quick)),
        "back" => Some(Cmd::Gesture(Gesture::Back)),
        "hold" => Some(Cmd::Gesture(Gesture::Hold)),
        "backhold" => Some(Cmd::Gesture(Gesture::BackHold)),
        "play" => Some(Cmd::Play),
        "pause" => Some(Cmd::Pause),
        "enter" => Some(Cmd::Enter),
        "exit" => Some(Cmd::Exit),
        "ambient" => Some(Cmd::Ambient),
        "upload" => Some(Cmd::StageUpload),
        "receive" => Some(Cmd::ReceiveRoute),
        other => {
            if let Some(n) = other.strip_prefix("step:") {
                n.trim().parse::<i32>().ok().map(|n| Cmd::Gesture(Gesture::Step(n)))
            } else if let Some(t) = other.strip_prefix("seek:") {
                t.trim().parse::<f64>().ok().map(Cmd::Seek)
            } else if let Some(heading) = other.strip_prefix("heading:") {
                heading.trim().parse::<f32>().ok().filter(|n| n.is_finite()).map(Cmd::Heading)
            } else {
                None
            }
        }
    }
}

/// The two app-rebuild baselines behind [`Cmd::Enter`] and [`Cmd::Ambient`]. Both rebuild the app
/// to a fresh `[Home, Map]` riding session on the demo route, so a previous demo cannot leak in.
///
/// Both run Manual climb mode. The demo ride is one long climb, so Auto would swap the opening Map
/// for the Climb profile within the first frames. Manual keeps the Climb screen reachable through
/// the conditional Back-cycle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// Guided-demo entry: the ride seeked mid-climb, page engine in control.
    Tour,
    /// The page-opening state: ride from the start, visitor in control.
    Ambient,
    /// The upload bookend: an idle device whose catalog already contains the committed route.
    /// `ReceiveRoute` posts the host event right after this reset, so the `ROUTE RECEIVED` card is
    /// the real one.
    Upload,
}

#[derive(Default)]
struct Compass(Option<f32>);
impl obc_ports::CompassSource for Compass {
    fn poll(&mut self) -> Option<f32> {
        self.0
    }
}

struct StoppedFix(Option<obc_ports::Fix>);
impl obc_ports::LocationSource for StoppedFix {
    fn poll(&mut self) -> Option<obc_ports::Fix> {
        self.0.take()
    }
}

/// Completion of the most recent queued baseline reset. Failure remains latched until retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetStatus {
    Ready,
    Pending,
    Failed,
}

pub struct Demo {
    map: FlatMap,
    /// The shared app, heap-allocated: a by-value `App` temporary of this size is a silent wasm
    /// stack trap.
    app: Box<App>,
    /// The render path's per-frame scratch, owned by the host and lent to each render call. Boxed
    /// for the same reason as the app.
    scratch: Box<obc_render::RenderScratch>,
    routes: FlatRouteStore,
    rides: FlatRideStore,
    tracks: FlatRideRecorder,
    player: GpxPlayer,
    baro: BaroSensor,
    compass: Compass,
    /// The shared typed executor: the next pass's outcomes and facts, and the in-flight route plan,
    /// stepped once per tick. Every sequencing decision lives in `obc-host-core`.
    host: HostLoop,
    /// The resident active-route parse, opened once per frame and lent to both the pass and the
    /// render, so the map opens without a per-frame `RouteIndex` reparse.
    session: ActiveRouteSession,
    frame: RgbaFrame,
    photo: obc_host_core::photo::Preparer,
    peaks: obc_host_core::peak_view::Runtime,
    elevation: obc_elevation::TerrainElevation<'static, 4>,
    /// Page commands queued since the last [`tick`](Demo::tick), drained in full and in order once
    /// per tick. A guided-tour step pushes several commands in one frame and relies on that.
    ///
    /// Every command drained in one tick applies with no draw between them, because the single
    /// render happens after the whole queue is drained. A gesture that consumes draw-time lazy
    /// state, such as the POI list's nearest-POI ordering, must therefore land in a separate tick
    /// from the gesture that opens that screen. The page's step engine gets this for free, because
    /// each step waits for its target screen before issuing the next step's commands.
    queue: Vec<Cmd>,
    /// The previous `tick` timestamp, for the replay `dt`.
    last_now_ms: Option<f64>,
    /// How far the device's UI clock runs ahead of the rAF timestamp: the time the guided-demo
    /// pre-roll's render-free passes consumed.
    ///
    /// The UI clock is `now_ms` plus this, so it is monotonic and it keeps running. Clamping to the
    /// larger of the two would freeze the device for as long as the pre-roll took.
    ui_offset_ms: u32,
    /// The baseline's ride, still to be asked for. A ride needs a mounted card, and a device learns
    /// it has one on its first pass, so the page asks on the first frame that can grant one and then
    /// stops. Asking at construction would be refused, and the visitor would see the refusal card.
    pending_ride: bool,
    /// Guided-demo mode: the page's tour engine owns playback and baseline resets, so the ambient
    /// summit auto-restart is suspended. A `start_session` mid-demo would reset progress under the
    /// script.
    tour_active: bool,
    /// First frame rendered: the page's readiness signal.
    ready: bool,
    reset_status: ResetStatus,
}

impl Demo {
    /// Build the whole embedded device and stage the ambient baseline. The app, map cache and
    /// render scratch stay on the heap.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Box<Self> {
        Self::on_card(obc_host_core::flat_store::HostStore::memory().expect("session card initializes"))
    }

    fn on_card(owner: obc_host_core::flat_store::HostStore) -> Box<Self> {
        let map = FlatMap::from_bytes_in(&owner, DEMO_MAP).expect("embedded demo map imports into the flat store");
        let routes =
            FlatRouteStore::new(owner.clone(), &[DEMO_ROUTE]).expect("embedded routes import into the flat store");
        let tracks = FlatRideRecorder::new(owner.clone()).expect("new demo card has no recovery debt");
        let rides = FlatRideStore::new(owner).expect("demo ride catalog loads");
        let track = Track::parse(DEMO_RIDE_GPX).expect("embedded demo GPX parses");
        let mut player = GpxPlayer::new(track);
        player.set_speed(DEMO_SPEED);

        let mut demo = Box::new(Demo {
            map,
            // Placeholder app; `reset(Ambient)` below builds the real baseline.
            app: Box::new(App::new(AppState::new(0, 0, 1.0))),
            scratch: Box::new(obc_render::RenderScratch::new()),
            routes,
            rides,
            tracks,
            player,
            baro: BaroSensor::new(),
            compass: Compass::default(),
            host: HostLoop::new(),
            session: ActiveRouteSession::new(),
            frame: RgbaFrame::new(FRAME_W, FRAME_H),
            photo: obc_host_core::photo::Preparer::default(),
            peaks: obc_host_core::peak_view::Runtime::new(Box::new(SliceSource(TERRAIN.0)))
                .expect("demo surface terrain parses"),
            elevation: obc_elevation::TerrainElevation::parse(&*TERRAIN).expect("demo elevation parses"),
            queue: Vec::new(),
            last_now_ms: None,
            ui_offset_ms: 0,
            pending_ride: false,
            tour_active: false,
            ready: false,
            reset_status: ResetStatus::Ready,
        });
        demo.install_baseline(Baseline::Ambient);
        demo
    }

    /// Queue one page command, drained on the next [`tick`](Demo::tick). Unknown input is ignored.
    pub fn cmd(&mut self, cmd: &str) {
        if let Some(c) = parse_cmd(cmd) {
            if c.baseline().is_some() {
                self.reset_status = ResetStatus::Pending;
            }
            self.queue.push(c);
        }
    }

    /// The current input-receiving screen's variant name. The page polls this to advance a demo
    /// step only once the app reached the target screen, so there are no fixed sleeps and it waits
    /// out the real planner.
    pub fn state(&self) -> &'static str {
        self.app.top_screen().name()
    }

    /// True once the first frame is rendered, so the page can swap its poster for the canvas.
    pub fn ready(&self) -> bool {
        self.ready
    }

    pub fn peak_active(&self) -> bool {
        self.app.peak_view_is_base()
    }

    pub fn heading(&self) -> u16 {
        self.app.peak_view_heading_q4() / 4
    }

    pub fn peak_ready(&self) -> bool {
        self.peaks.panorama().is_some_and(|panorama| {
            self.app.state.peak_view_profile.is_some_and(|profile| {
                panorama.view_ready(self.app.peak_view_heading_q4(), profile.horizontal_fov_q4())
            })
        })
    }

    pub fn find_ready(&self) -> bool {
        self.app.find_place_state() == obc_app::find_place::State::Ready
            && self.app.find_place_result_count() > 0
            && self.app.assistant_planner_released()
    }

    pub fn visit_status(&self) -> obc_app::navigator::ReviewStatus {
        self.app.assistant_review_status()
    }

    pub fn reset_status(&self) -> ResetStatus {
        self.reset_status
    }

    /// The rendered RGBA frame, for `putImageData`.
    pub fn frame(&self) -> &[u8] {
        self.frame.as_rgba()
    }

    /// Advance one JS-driven frame, and answer whether the frame buffer changed. `now_ms` is the
    /// rAF timestamp; any monotonic millisecond clock works.
    pub fn tick(&mut self, now_ms: f64) -> bool {
        // Replay dt from the rAF clock, clamped so a backgrounded tab cannot teleport the ride on
        // the first frame back.
        let dt = match self.last_now_ms.replace(now_ms) {
            Some(last) => ((now_ms - last) / 1000.0).clamp(0.0, MAX_FRAME_DT_S),
            None => 0.0,
        };

        // Drain the page's commands first, so a gesture's transition is visible in this frame's
        // render. Gestures go into this frame's pass rather than straight into the app, so a page
        // command and a rider's button land by the same path. See `queue` for the caveat that
        // constrains how tour steps are grouped.
        let mut gestures: Vec<Gesture> = Vec::new();
        let mut reset_failed = false;
        let mut commands = std::mem::take(&mut self.queue).into_iter();
        while let Some(cmd) = commands.next() {
            if let Some(baseline) = cmd.baseline() {
                // A baseline supersedes earlier deferred inputs. Later commands depend on it.
                gestures.clear();
                if !self.reset(baseline) {
                    self.reset_status = ResetStatus::Failed;
                    reset_failed = true;
                    break;
                }
                self.reset_status = if commands.as_slice().iter().any(|cmd| cmd.baseline().is_some()) {
                    ResetStatus::Pending
                } else {
                    ResetStatus::Ready
                };
            } else {
                self.apply(cmd, &mut gestures);
            }
        }

        if self.app.peak_view_is_base() {
            self.player.pause();
        }
        self.peaks.update(&mut self.app, &self.map.reader());
        if !reset_failed {
            self.arm_baseline_ride();
        }
        let was_playing = self.player.is_playing();
        let plan = self.device_frame(self.ui_now(), dt, &gestures);
        // A single-loop host has no second recognizer to cancel, so it consumes the hold-cancel
        // latch the pass may have armed rather than leaving it set for a plane that does not exist.
        let _ = self.app.take_hold_cancel();

        // At the summit, start the next ambient lap through the same acknowledged cleanup. A pause
        // or a completed Save is not a new lap.
        if self.reset_status == ResetStatus::Ready
            && !self.tour_active
            && was_playing
            && !self.player.is_playing()
            && self.player.time() >= self.player.duration()
            && self.app.recording()
            && !self.app.recorder.closing()
        {
            self.cmd("ambient");
        }

        // `plan.next_wake_ms` and `plan.immediate` are ignored: the page is rAF-paced, so the
        // browser decides when the next frame happens.
        //
        // `plan.render` is the same signal the firmware gates its repaints on. The first frame
        // always renders, because `ready` is also the page's poster-swap signal.
        if plan.render.map || plan.render.overlay || !self.ready || self.app.photo_pending() {
            self.session.sync(&self.app, &mut self.routes);
            let route = frame::active_route(&self.session, &self.routes);
            let reader = self.map.reader();
            frame::render(
                &mut self.app,
                &mut self.scratch,
                &mut self.frame,
                frame::Scene { reader: &reader, route: route.as_ref() },
                self.peaks.panorama(),
                (FRAME_W as f32, FRAME_H as f32),
                frame::device_rgb888,
                &obc_render::NoopClock,
                Some(self.photo.interactive(plan.render.map || !self.ready)),
            );
            self.ready = true;
            return true;
        }
        false
    }

    /// The device's UI clock: the rAF timestamp plus whatever a guided pre-roll ran through.
    /// Monotonic, because the rAF clock is and the offset only grows.
    fn ui_now(&self) -> u32 {
        (self.last_now_ms.unwrap_or(0.0).max(0.0) as u32).wrapping_add(self.ui_offset_ms)
    }

    /// One device frame: the active route opened once, one [`App::run_pass`], and the typed
    /// executor behind it. The page's tick and the guided pre-roll share it.
    fn device_frame(&mut self, ui_ms: u32, dt: f64, gestures: &[Gesture]) -> PassPlan {
        // Open the active route's geometry from the resident session. The index is kept until the
        // active bytes change, so there is no per-frame `RouteIndex` reparse.
        self.session.sync(&self.app, &mut self.routes);
        // Pausing replay freezes its clock, so publish one stopped GPS fix and let the compass
        // path take over from the last moving course.
        let mut stopped = StoppedFix(
            self.app
                .state
                .user_fix
                .filter(|fix| self.compass.0.is_some() && !self.player.is_playing() && fix.course.is_some())
                .map(|fix| obc_ports::Fix { course: None, speed_mps: Some(0.0), ..fix }),
        );
        let stopped_fix = stopped.0.is_some();
        let mut plan = {
            let route_src = self.routes.active_source();
            let route = match (self.session.index(), route_src) {
                (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
                _ => None,
            };
            // GPS course orients the moving map; the compass lets a stopped rider look around.
            let (ride, mut sensors) =
                replay_advance(&mut self.player, &mut self.baro, Some(&mut self.compass), dt, ReplaySensors::default());
            if stopped_fix {
                sensors.loc = &mut stopped;
            }
            self.host.pass(
                &mut self.app,
                PassClock { ride, ui: InputClock(ui_ms) },
                gestures,
                sensors,
                route.as_ref(),
                SUPPORT,
            )
        };
        // The typed executor: the plan's bounded effects against the in-memory stores, and
        // token-carrying outcomes for the next pass. The demo has no trips and no platform work of
        // its own, so the whole loop is repository sequencing from `obc-host-core`.
        self.host.execute(
            &mut self.app,
            &mut plan,
            &mut self.session,
            &mut self.routes,
            &mut self.rides,
            &mut self.tracks,
            &mut (),
            &self.map,
            &mut self.elevation,
            &mut (),
        );
        plan
    }

    /// Apply one drained command. A gesture joins this frame's batch; everything else drives the
    /// replay or a baseline reset.
    fn apply(&mut self, cmd: Cmd, gestures: &mut Vec<Gesture>) {
        match cmd {
            Cmd::Gesture(g) => gestures.push(g),
            // A chord is applied now, not deferred into this frame's gesture batch, which is the
            // device's own order: the recogniser swallows a chord's constituents whole, and the app
            // resolves the chord above the screen stack before it applies the frame's gestures.
            Cmd::Chord(c) => {
                self.app.apply_chord(c);
            }
            Cmd::Play => {
                if !self.app.peak_view_is_base() {
                    self.player.play();
                }
            }
            Cmd::Pause => self.player.pause(),
            Cmd::Seek(t) => self.player.seek(t),
            Cmd::Heading(degrees) => {
                self.compass.0 = Some(degrees.rem_euclid(360.0));
                self.player.pause();
            }
            Cmd::StageUpload | Cmd::Enter | Cmd::Ambient => unreachable!("resets are queue barriers"),
            Cmd::ReceiveRoute => {
                if let Some(&id) = self.routes.ids().first() {
                    self.host.facts().note_route_upload(RouteUpload { id, replaced: false, elevation: None });
                }
            }
            Cmd::Exit => {
                // Take control: leave the device where the demo parked it, controls live.
                self.tour_active = false;
                if !self.app.peak_view_is_base() {
                    self.player.play();
                }
            }
        }
    }

    /// Retire the old recorder through its exact acknowledgment before recycling App tokens.
    fn reset(&mut self, baseline: Baseline) -> bool {
        if self.host.owns_navigation() {
            return false;
        }
        if !self.tracks.is_idle() || self.app.recording() || !self.host.outcomes().recorder.is_empty() {
            self.app.recorder.request(obc_app::RecorderIntent::Discard);
            // First consume any old reply, then issue Discard. The memory executor answers in this
            // pass, and the second pass must consume that exact answer before replacing App.
            self.device_frame(self.ui_now(), 0.0, &[]);
            if let Some(reply) = self.host.outcomes().recorder.take() {
                // Put the unchanged token back for RecorderMachine to validate and consume.
                self.host.outcomes().recorder.try_put(reply).expect("the inspected slot is empty");
                if !matches!(reply, RecorderOutcome::Discarded { .. }) {
                    return false;
                }
                self.device_frame(self.ui_now(), 0.0, &[]);
            }
            if !self.tracks.is_idle() || self.app.recording() || self.host.owns_navigation() {
                return false;
            }
        }
        self.install_baseline(baseline);
        true
    }

    /// Rebuild only the device baseline. The same card keeps maps, routes and saved rides.
    fn install_baseline(&mut self, baseline: Baseline) {
        use obc_app::settings::{ClimbMode, Settings};

        // Both bookend baselines are page-driven. Upload must stay idle instead of being caught by
        // the ambient restart loop.
        self.tour_active = baseline != Baseline::Ambient;

        let (cx, cy, _) = {
            let reader = self.map.reader();
            initial_camera(&reader, FRAME_W)
        };
        let mut state = AppState::new(cx, cy, obc_render::zoom_for_mpp(DEMO_MPP));
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        state.peak_view_profile = Some(obc_app::PeakViewProfile::at(0, 0, 0));
        self.peaks.reset();
        self.compass.0 = None;
        let mut app = if baseline == Baseline::Upload { App::new_idle(state) } else { App::new(state) };
        // The page keeps one RGBA frame and repaints it on demand, so every render is a render over
        // the last one. That is what lets a drawer's sheet grow over a base the frame no longer
        // draws.
        app.set_resident_frame(true);
        app.set_map_nav_graph(self.map.tables().has_nav_graph());
        app.set_routes_with_ids(self.routes.catalog(), self.routes.ids());
        app.set_rides(self.rides.catalog());
        // Manual climb mode for both baselines: the whole demo ride is a climb, so Auto would swap
        // the opening Map for the Climb profile within the first frames.
        //
        // `IdleReturn::Never`, because the page has no rider whose idleness means anything. A guided
        // step that dwells on a menu while the visitor reads it would otherwise be swept back to the
        // Map thirty seconds in.
        app.set_settings(Settings {
            climb_mode: ClimbMode::Manual,
            idle_return: obc_app::settings::IdleReturn::Never,
            ..Settings::default()
        });
        // Select the embedded demo route. `arm_baseline_ride` asks for the ride itself on the first
        // frame that can grant one.
        self.pending_ride = baseline != Baseline::Upload && !self.routes.catalog().is_empty();
        if self.pending_ride {
            app.activate_route(0);
        }
        // Overwrite in the existing heap slot, so there is no fresh allocation and no lingering old
        // app. The executor is rebuilt with it, because its inbox holds outcomes and tokens minted
        // by the app that is being replaced.
        *self.app = app;
        self.host = HostLoop::new();
        // The resident parse goes with it, and the store's active binding must be dropped too:
        // `sync_active` only reparses on a change, so a store still bound to route 0 would answer
        // unchanged and the fresh session would never open the route.
        self.session = ActiveRouteSession::new();
        self.routes.invalidate_active();

        self.player.seek(0.0);
        if baseline == Baseline::Upload {
            self.player.pause();
        } else {
            self.player.play();
        }

        if baseline == Baseline::Tour {
            // Arrive at the mid-climb camera through real replay ticks instead of a teleport.
            // Nothing renders during this deterministic pre-roll, but Activity sees the genuine
            // one-Hz fixes and barometric samples, so the bookend saves believable totals. About
            // 500 render-free ticks, paid only when a guided chapter starts.
            self.baro = BaroSensor::new();
            while self.player.time() < TOUR_BASELINE_S {
                let wall_dt = ((TOUR_BASELINE_S - self.player.time()) / self.player.speed() as f64).min(1.0);
                // Full frames, not bare ticks: a pass whose effects nobody serves leaves its
                // domains in flight, and the device would then refuse the first delete or stamp it
                // is asked for. The pre-roll's own time joins the offset, so the clock the page
                // resumes on carries it.
                self.ui_offset_ms = self.ui_offset_ms.wrapping_add((wall_dt * 1000.0) as u32);
                self.arm_baseline_ride();
                let _ = self.device_frame(self.ui_now(), wall_dt, &[]);
            }
        }
    }
}

impl Demo {
    /// Ask Recorder for the baseline's ride, once, on the first frame the device can grant one.
    ///
    /// The request is spent here, so the ride the visitor finishes stays finished. A page that
    /// re-asked every frame would reopen it two frames later.
    fn arm_baseline_ride(&mut self) {
        if self.pending_ride && self.app.can_record() {
            self.pending_ride = false;
            self.app.recorder.request(obc_app::RecorderIntent::Start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(demo: &mut Demo, now_ms: &mut f64, command: &str, expected: &str) {
        demo.cmd(command);
        *now_ms += 16.0;
        demo.tick(*now_ms);
        assert_eq!(demo.state(), expected, "`{command}` should reach {expected}");
    }

    #[test]
    fn reset_drains_the_old_reply_before_reusing_session_tokens() {
        let mut d = Demo::new();
        let map = d.map.source();
        let route = d.routes.ids()[0];
        d.tick(0.0);
        d.tick(250.0);
        d.tick(500.0); // cross the replay's one-second fix boundary
        assert!(!d.tracks.is_idle());
        assert!(!d.host.outcomes().recorder.is_empty(), "the old App still owes its reply");
        d.cmd("back");
        d.cmd("ambient");
        d.cmd("upload");
        d.cmd("receive");
        assert_eq!(d.reset_status(), ResetStatus::Pending);
        assert_eq!(d.state(), "Map", "an old matching screen cannot acknowledge reset");
        d.tick(750.0);
        assert_eq!(d.reset_status(), ResetStatus::Ready);
        assert_eq!(d.state(), "RouteReceived", "earlier gestures are superseded; later inputs follow the baseline");
        assert!(d.tracks.is_idle());
        assert!(!d.app.recording());
        assert!(d.map.source().same_revision(&map));
        assert_eq!(d.routes.ids(), &[route]);
        assert!(d.rides.catalog().is_empty());
        // Repeated cleanup must return the reservation and leave no old recording at Start.
        for i in 0..4 {
            d.cmd("ambient");
            d.tick(1000.0 + i as f64 * 500.0);
            d.tick(1250.0 + i as f64 * 500.0);
            assert_eq!(d.reset_status(), ResetStatus::Ready);
            assert!(d.app.recording());
            assert!(!d.tracks.is_idle());
        }
        assert_ne!(Demo::new().map.source().store_id(), map.store_id(), "a new page owns a new volatile card");
    }

    #[test]
    fn playback_end_cannot_discard_a_save_waiting_for_its_append_reply() {
        let mut d = Demo::new();
        d.tick(0.0);
        d.tick(250.0);
        d.tick(500.0);
        let reply = d.host.outcomes().recorder.take().expect("the real append awaits delivery");
        assert!(matches!(reply, RecorderOutcome::Appended { .. }));
        assert!(!d.app.recorder.staged().is_empty());
        d.app.recorder.request(obc_app::RecorderIntent::Save);
        d.player.seek(d.player.duration() - 0.5);
        d.tick(750.0);
        assert!(!d.player.is_playing());
        assert!(d.app.recording() && d.app.recorder.closing());
        assert_eq!(d.reset_status(), ResetStatus::Ready);
        assert!(d.queue.is_empty(), "playback end cannot supersede Save with automatic Discard");
        d.host.outcomes().recorder.try_put(reply).unwrap();
        for i in 4..12 {
            d.tick(i as f64 * 250.0);
        }
        assert!(!d.app.recording());
        assert!(d.tracks.is_idle());
        assert_eq!(d.rides.catalog().len(), 1, "the original sample becomes a saved object");
        assert!(!d.rides.catalog()[0].summary.synced);
    }

    #[test]
    #[cfg(unix)]
    fn refused_discard_preserves_session_and_latches_reset_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.obc");
        let owner = obc_host_core::flat_store::HostStore::create_file(&path).unwrap();
        let mut d = Demo::on_card(owner);
        d.tick(0.0);
        d.tick(250.0);
        let session = d.app.recorder.session();
        let map = d.map.source();
        let offset = d.ui_offset_ms;
        // An unavailable catalog: the cached owner cannot read the truncated backing file.
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(0).unwrap();
        d.cmd("upload");
        d.cmd("receive");
        d.tick(250.0);
        assert_eq!(d.reset_status(), ResetStatus::Failed);
        assert_ne!(d.state(), "RouteReceived");
        assert_eq!(d.app.recorder.session(), session);
        assert!(!d.tracks.is_idle());
        assert_eq!(d.ui_offset_ms, offset, "no Tour pre-roll ran");
        assert!(d.map.source().same_revision(&map));
        d.cmd("back");
        d.tick(250.0);
        assert_eq!(d.reset_status(), ResetStatus::Failed, "an unrelated input cannot erase failure");
        d.cmd("enter");
        assert_eq!(d.reset_status(), ResetStatus::Pending, "an explicit retry remains possible");
        d.tick(250.0);
        assert_eq!(d.reset_status(), ResetStatus::Failed);
        assert_eq!(d.ui_offset_ms, offset);
    }

    /// The shipped payload is a map this build's reader accepts.
    ///
    /// `include_bytes!` accepts any bytes at all, and everything else in this module mounts the map
    /// behind layers of app state, so a payload the reader refuses would show up as a blank canvas
    /// in a browser. `MapTables::parse` is the version gate, so a format bump fails here instead.
    #[test]
    fn the_shipped_demo_map_parses_at_this_builds_obcm_version() {
        let src = SliceSource(DEMO_MAP);
        let tables = MapTables::parse(&src).expect("the shipped demo payload parses at this build's OBCM version");
        assert!(tables.bbox.max_lat > tables.bbox.min_lat, "and it carries a real bbox, not a stub");
    }

    #[test]
    fn drawers_and_live_peaks_use_the_shipped_map() {
        let mut d = Demo::new();
        let mut now = 0.0;
        d.tick(now);
        drive(&mut d, &mut now, "quick", "QuickDrawer");
        drive(&mut d, &mut now, "quick", "Map");
        drive(&mut d, &mut now, "context", "ContextDrawer");
        drive(&mut d, &mut now, "back", "Map");
        drive(&mut d, &mut now, "enter", "Map");
        drive(&mut d, &mut now, "seek:3000", "Map");
        drive(&mut d, &mut now, "heading:250", "Map");
        drive(&mut d, &mut now, "backhold", "Menu");
        drive(&mut d, &mut now, "step:3", "Menu");
        drive(&mut d, &mut now, "press", "PeakView");
        assert!(!d.peak_ready(), "opening yields before terrain work");
        for _ in 0..1200 {
            now += 16.0;
            d.tick(now);
            if d.peak_ready() {
                break;
            }
        }
        assert!(d.peak_ready(), "the real DEM builds a visible panorama in bounded frames");
        assert!(d.app.state.peak_view_peak_count > 0, "named summits come from the map");
        for _ in 0..1200 {
            now += 16.0;
            d.tick(now);
        }
        let visible: Vec<_> = d.app.state.peak_view_peaks[..d.app.state.peak_view_peak_count as usize]
            .iter()
            .filter(|p| p.visible)
            .map(|p| (p.name.as_str(), p.azimuth_q4))
            .collect();
        assert!(!visible.is_empty(), "the skyline has named visible summits");
        let observer = d.app.state.peak_view_profile.unwrap();
        assert!(observer.observer_elevation_m > 1000, "ground height comes from the DEM");
        drive(&mut d, &mut now, "exit", "PeakView");
        assert!(!d.player.is_playing(), "looking around keeps the rider stopped");
        drive(&mut d, &mut now, "quick", "QuickDrawer");
        drive(&mut d, &mut now, "quick", "PeakView");
        assert!(d.peak_ready(), "a drawer retains the built view");
        drive(&mut d, &mut now, "play", "PeakView");
        assert!(!d.player.is_playing(), "resuming the page keeps the lookout stopped");
        drive(&mut d, &mut now, "heading:90", "PeakView");
        for _ in 0..1200 {
            now += 16.0;
            d.tick(now);
            if d.peak_ready() {
                break;
            }
        }
        assert!(d.peak_ready());
        assert_eq!(d.app.peak_view_heading_q4(), 360, "the compass turns the actual view");
        drive(&mut d, &mut now, "back", "Menu");
        drive(&mut d, &mut now, "ambient", "Map");
        assert!(d.peaks.panorama().is_none());
    }

    /// The page-opening contract: the first tick renders, the demo opens on the live Map, and the
    /// frame is exactly the putImageData layout.
    #[test]
    fn boots_ready_on_the_map() {
        let mut d = Demo::new();
        assert!(!d.ready());
        assert!(d.tick(0.0), "the first tick always renders");
        assert!(d.ready());
        assert_eq!(d.state(), "Map");
        assert_eq!(d.frame().len(), (FRAME_W * FRAME_H * 4) as usize);
        assert!(d.frame().iter().skip(3).step_by(4).all(|&a| a == 0xFF), "opaque alpha for putImageData");
    }

    /// A queued gesture lands on the app on the same tick, so the closed-loop tour never waits an
    /// extra frame, and junk commands are ignored.
    #[test]
    fn commands_drain_on_the_next_tick_and_junk_is_ignored() {
        let mut d = Demo::new();
        d.tick(0.0);
        // Back on the Map cycles the riding views.
        d.cmd("back");
        d.tick(16.0);
        assert_ne!(d.state(), "Map", "the queued gesture applied this tick");
        d.cmd("ambient");
        d.tick(32.0);
        assert_eq!(d.state(), "Map", "ambient resets to the Map baseline");
        for junk in ["", "prss", "step:", "step:x", "seek:", "seek:x", "hold ", "STEP:1"] {
            d.cmd(junk);
        }
        d.tick(48.0);
        assert_eq!(d.state(), "Map", "malformed input is dropped, never applied");
    }

    /// With playback paused and no input, the app settles and ticks stop reporting frame changes,
    /// so the page's rAF loop skips the putImageData.
    #[test]
    fn settles_clean_when_paused() {
        let mut d = Demo::new();
        d.tick(0.0);
        d.cmd("pause");
        // A few frames drain the pause and any in-flight fix or animation edges.
        let mut changed = true;
        for i in 1..=20 {
            changed = d.tick(i as f64 * 16.0);
        }
        assert!(!changed, "a parked, paused demo stops redrawing");
        // Playback then resumes movement, at about one fix per second of playback time.
        d.cmd("play");
        let mut any = false;
        for i in 21..=100 {
            any |= d.tick(i as f64 * 16.0);
        }
        assert!(any, "resuming playback dirties the map again");
    }

    /// `enter` stages a mid-climb Map with a live session, `exit` hands control back without a
    /// reset, and the ride keeps playing throughout.
    #[test]
    fn enter_and_exit_stage_the_tour_baseline() {
        let mut d = Demo::new();
        d.tick(0.0);
        d.cmd("back"); // walk off the Map…
        d.tick(16.0);
        d.cmd("enter"); // …and a demo entry resets to the staged baseline
        d.tick(32.0);
        assert_eq!(d.state(), "Map");
        assert!(d.tour_active);
        assert!((d.player.time() - TOUR_BASELINE_S).abs() < 60.0, "staged mid-climb (plus a frame of playback)");
        d.cmd("exit");
        d.tick(48.0);
        assert!(!d.tour_active);
        assert_eq!(d.state(), "Map", "take-control keeps the device where the demo left it");
    }

    #[test]
    fn upload_bookend_uses_the_real_route_received_event() {
        let mut d = Demo::new();
        d.tick(0.0);
        d.cmd("upload");
        d.tick(8.0);
        assert_eq!(d.state(), "Home");
        d.cmd("receive");
        d.tick(16.0);

        assert_eq!(d.state(), "RouteReceived");
        assert!(d.tour_active, "the idle upload card must not be restarted as an ambient ride");
        assert!(!d.app.recording(), "a phone upload lands before the ride starts");

        let mut now = 16.0;
        drive(&mut d, &mut now, "press", "RouteOverview");
        // The replay stands mid-climb, far from the route's first point, so the ride asks first.
        drive(&mut d, &mut now, "press", "StartAway");
        drive(&mut d, &mut now, "step:1", "StartAway");
        drive(&mut d, &mut now, "press", "Map");
        assert!(d.app.recording(), "Join nearest begins the session before the next chapter");
        assert!(d.app.progress_m() > 1_000, "the ride joins the route mid-climb");
    }

    #[test]
    fn ride_log_bookend_pauses_selects_finish_and_saves() {
        use obc_formats::io::ByteSource;
        use obc_storage::flat::{ObjectId, Revision};
        let owner = obc_host_core::flat_store::HostStore::memory().unwrap();
        let mut d = Demo::on_card(owner.clone());
        let mut now = 0.0;
        d.tick(now);
        assert!(d.rides.catalog().is_empty(), "no fabricated ride or archive proof");

        drive(&mut d, &mut now, "enter", "Map");
        let stats = d.app.recorder.ride_stats();
        assert!(stats.distance_m > 1_000, "the visible Finish flow should contain a real partial ride");
        assert!(stats.moving_time_s > 60);
        assert!(stats.climb_m > 50);
        drive(&mut d, &mut now, "press", "RideControl");
        drive(&mut d, &mut now, "step:1", "RideControl");
        drive(&mut d, &mut now, "hold", "Home");
        assert!(d.app.recording(), "the ride is open until the store answers for the close");
        for _ in 0..8 {
            now += 16.0;
            d.tick(now);
            if !d.app.recording() {
                break;
            }
        }
        assert!(!d.app.recording(), "the final staged samples drain before the finalize verdict closes it");
        // It stays ended. The baseline's Start is a one-shot; a page that re-asked every frame
        // would reopen a ride the rider just finished.
        for _ in 0..8 {
            now += 16.0;
            d.tick(now);
            assert!(!d.app.recording(), "the finished ride must not reopen itself");
        }
        let saved = d.rides.catalog()[0].clone();
        let source = owner.open(ObjectId(saved.id), Revision(1)).unwrap();
        assert_eq!(source.store_id(), d.map.source().store_id());
        let info = obc_route::RideInfo::read(&source).unwrap();
        assert!(info.point_count > 100);
        assert_eq!(saved.summary, obc_app::RideSummary::from_info(&info, false, 0));
        assert!(!saved.summary.synced);
        let mut samples = vec![0; info.point_count as usize * obc_formats::track::RECORD_LEN];
        source.read_at(0, &mut samples).unwrap();
        let points: Vec<_> = samples
            .as_chunks::<{ obc_formats::track::RECORD_LEN }>()
            .0
            .iter()
            .map(obc_formats::track::decode_record)
            .collect();
        assert!(points.windows(2).all(|pair| pair[0].t_ms < pair[1].t_ms));
        assert!(points.iter().any(|point| point.lat != points[0].lat));
        let mut profile = obc_route::Profile::EMPTY;
        assert!(d.rides.fill_track(saved.id, &mut profile).unwrap().len() > 1);
        d.cmd("exit");
        d.cmd("pause");
        now += 16.0;
        d.tick(now);
        assert!(!d.player.is_playing());
        assert!(!d.app.recording(), "leaving the tour cannot reopen a saved ride");
        d.cmd("upload");
        now += 16.0;
        d.tick(now);
        assert_eq!(d.reset_status(), ResetStatus::Ready);
        assert_eq!(d.rides.catalog(), &[saved]);
        assert!(source.is_current(), "reset preserves the committed revision and its held reader");
    }

    /// `Screen::NAMES` contains every state this host can report, so a rename in the screens table
    /// breaks this before it breaks the page.
    #[test]
    fn state_is_always_a_known_screen_name() {
        let mut d = Demo::new();
        d.tick(0.0);
        assert!(obc_app::Screen::NAMES.contains(&d.state()));
    }

    #[test]
    fn climb_tour_stays_on_the_climb_view() {
        let mut d = Demo::new();
        let mut now = 0.0;
        d.tick(now);

        drive(&mut d, &mut now, "enter", "Map");
        drive(&mut d, &mut now, "back", "Statistics");
        drive(&mut d, &mut now, "back", "Climb");
    }

    #[test]
    fn up_ahead_tour_opens_a_populated_route_timeline() {
        let mut d = Demo::new();
        let mut now = 0.0;
        d.tick(now);

        drive(&mut d, &mut now, "enter", "Map");
        drive(&mut d, &mut now, "context", "ContextDrawer");
        drive(&mut d, &mut now, "press", "Assistant");
        drive(&mut d, &mut now, "step:1", "Assistant");
        drive(&mut d, &mut now, "press", "WhatsNext");
        drive(&mut d, &mut now, "press", "WhatsNext");
        assert!(d.app.corridor_snapshot_len() > 0, "the demo route should showcase map POIs ahead");
        drive(&mut d, &mut now, "back", "WhatsNext");
        drive(&mut d, &mut now, "back", "Assistant");
        drive(&mut d, &mut now, "back", "Map");
    }

    #[test]
    fn reroute_tour_reviews_and_accepts_a_real_visit_without_restarting_recording() {
        use obc_app::navigator::ReviewStatus;
        fn dwell(d: &mut Demo, now: &mut f64, ms: u32) {
            for _ in 0..ms.div_ceil(16) {
                *now += 16.0;
                d.tick(*now);
            }
        }
        let mut d = Demo::new();
        let mut now = 0.0;
        d.tick(now);
        drive(&mut d, &mut now, "enter", "Map");
        drive(&mut d, &mut now, "pause", "Map");
        let original = d.app.route_ids()[d.app.active_route_index().unwrap()];
        assert!(d.app.recording());
        dwell(&mut d, &mut now, 2600);
        drive(&mut d, &mut now, "context", "ContextDrawer");
        drive(&mut d, &mut now, "press", "Assistant");
        drive(&mut d, &mut now, "press", "FindPlace");
        dwell(&mut d, &mut now, 2600);
        for _ in 0..6 {
            drive(&mut d, &mut now, "step:1", "FindPlace");
            dwell(&mut d, &mut now, 420);
        }
        drive(&mut d, &mut now, "press", "FindPlace");
        for _ in 0..750 {
            if d.find_ready() {
                break;
            }
            now += 16.0;
            d.tick(now);
        }
        assert!(d.find_ready(), "real station search: {:?}, {:?}", d.app.find_place_state(), d.visit_status());
        dwell(&mut d, &mut now, 2600);
        drive(&mut d, &mut now, "press", "VisitReview");
        now += 16.0;
        d.tick(now);
        assert!(d.host.owns_navigation());
        let offset = d.ui_offset_ms;
        d.cmd("enter");
        d.cmd("receive");
        now += 16.0;
        d.tick(now);
        assert_eq!(d.reset_status(), ResetStatus::Failed);
        assert_eq!(d.ui_offset_ms, offset);
        assert!(d.host.owns_navigation());
        assert!(!d.app.recorder.closing());
        for _ in 0..750 {
            if d.visit_status() == ReviewStatus::Preview {
                break;
            }
            now += 16.0;
            d.tick(now);
        }
        assert_eq!(d.visit_status(), ReviewStatus::Preview);
        let preview = d.app.assistant_preview().unwrap();
        assert!(preview.visit_costs.is_some() && preview.distance_m > 0);
        assert_eq!(d.app.route_ids()[d.app.active_route_index().unwrap()], original);
        dwell(&mut d, &mut now, 2600);
        assert_eq!(d.visit_status(), ReviewStatus::Preview);
        drive(&mut d, &mut now, "press", "VisitReview");
        for _ in 0..200 {
            if d.visit_status() == ReviewStatus::Accepted {
                break;
            }
            now += 16.0;
            d.tick(now);
        }
        assert_eq!(d.visit_status(), ReviewStatus::Accepted);
        assert_eq!(d.routes.read_checkpoint().unwrap().unwrap().route, preview.source);
        assert_eq!(d.app.route_ids()[d.app.active_route_index().unwrap()], preview.source.object);
        assert!(d.app.recording() && !d.app.recorder.closing());
        assert_eq!(d.state(), "Map", "accepted Visit returns to the riding map");
        let session = d.app.recorder.session();
        d.cmd("exit");
        d.cmd(&format!("seek:{}", d.player.duration() - 0.5));
        now += 250.0;
        d.tick(now);
        assert!(!d.player.is_playing());
        assert_eq!(d.reset_status(), ResetStatus::Failed);
        assert!(d.queue.is_empty(), "playback end cannot clear a refused navigation reset");
        assert_eq!(d.app.recorder.session(), session);
    }

    /// The landing page's guided scenarios wait on screen-name strings, so a rename in the
    /// `screens!` table would turn a tour into a timeout march. This reads `docs/index.html` and
    /// asserts every screen name the scenarios target is a real [`obc_app::Screen::NAMES`] entry.
    ///
    /// The convention, documented identically in `docs/index.html`: every guided-step target is a
    /// double-quoted string inside a `until:` array literal, and screen names appear nowhere else in
    /// a parseable position.
    #[test]
    fn tour_targets_are_real_screens() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/index.html");
        let html = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        let targets = extract_until_targets(&html);
        // Guard against a silent parser break: a convention change that found nothing would let a
        // real rename through. The reroute and climb scenarios give many targets.
        assert!(
            targets.len() >= 8 && targets.contains(&"Map".to_string()),
            "drift-guard parsed too few `until:` targets ({}) — the parseable convention in \
             docs/index.html likely changed; keep every target a quoted string in a `until: [..]` \
             array. Found: {targets:?}",
            targets.len()
        );
        for name in &targets {
            assert!(
                obc_app::Screen::NAMES.contains(&name.as_str()),
                "docs/index.html guided-tour targets screen {name:?}, which is not in \
                 Screen::NAMES — rename the tour target or the screen. Known: {:?}",
                obc_app::Screen::NAMES
            );
        }
    }

    /// Pull every screen name out of the page source's array literals. Only a `until:` glued to an
    /// opening bracket counts, so prose never opens a spurious match.
    fn extract_until_targets(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = html.as_bytes();
        let mut search = html;
        let mut base = 0usize;
        while let Some(rel) = search.find("until:") {
            let after = base + rel + "until:".len();
            base = after;
            search = &html[after..];
            // Require the next non-whitespace byte to be an opening bracket.
            let mut i = after;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] != b'[' {
                continue;
            }
            let Some(close_rel) = html[i..].find(']') else { continue };
            let arr = &html[i..i + close_rel];
            // Collect the double-quoted strings in this array.
            let mut rest = arr;
            while let Some(q0) = rest.find('"') {
                let tail = &rest[q0 + 1..];
                let Some(q1) = tail.find('"') else { break };
                out.push(tail[..q1].to_string());
                rest = &tail[q1 + 1..];
            }
        }
        out
    }
}
