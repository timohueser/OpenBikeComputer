//! The demo core: one `Demo` owns the whole embedded device — map, app, replay, stores, planner,
//! RGBA frame — and advances it one JS-driven frame at a time.
//!
//! **JS owns the rAF loop; Rust owns the truth.** Per [`tick`](Demo::tick): drain the queued
//! [`Cmd`]s → [`replay_step`] (advance the ride, tick the app on the playback clock) → step an
//! in-flight [`NavPlan`] **once** (the board's one-step-per-pass shape, so the spinner animates
//! while the real A* runs) → render into the RGBA frame **only when the app says something
//! changed** ([`App::take_dirty`] — the same render-on-demand signal the firmware gates its
//! repaints on).
//!
//! Target-independent on purpose: everything here compiles and is tested natively (`cargo test`
//! from `firmware/`); only the thin `#[wasm_bindgen]` surface in `main.rs` is wasm-only.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::device_core::{PassClock, PassPlan, PlatformSupport, RouteUpload};
use obc_app::{App, AppState, CameraMode, Gesture};
use obc_host_core::{
    initial_camera, replay_advance, ActiveRouteSession, HostLoop, MemRideStore, MemRouteStore, MemTrackStore,
    ReplaySensors, RgbaFrame, TrackRepository,
};
use obc_ports::InputClock;
use obc_reader::{rgb565_to_device64, MapCache, MapTables, Reader, SliceSource};
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};
use obc_route::RouteReader;

/// The demo panel resolution — the one [`obc_display`] frame authority, not re-declared literals.
pub const FRAME_W: u32 = obc_display::ls021::FRAME_W as u32;
pub const FRAME_H: u32 = obc_display::ls021::FRAME_H as u32;

// The embedded demo payload (epic #624 S4, #637). The wasm-only map stays app-owned; shared
// authored route/replay sources live in the fixture registry so other components never reach
// through this app's asset directory.
const DEMO_MAP: &[u8] = include_bytes!("../../obc-sim/assets/grimsel-demo.obcm");
const DEMO_ROUTE: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
const DEMO_RIDE_GPX: &str = include_str!("../../../fixtures/sources/sim-grimsel/tracks/grimsel-climb-demo.gpx");

/// Replay-speed multiplier: 3× a normal climbing pace keeps the map moving without a blur.
const DEMO_SPEED: f32 = 3.0;

/// Zoom multiplier over the fit-the-bbox camera: tightens the opening view to a riding scale so
/// the switchbacks are visible.
const DEMO_ZOOM: f32 = 12.0;

/// The GPX playback time (seconds) a guided-demo baseline (`enter`) seeks to: mid the ride's
/// first climb, so a climb is active for "see the climb ahead" and the map sits in the
/// switchbacks. The map-matcher re-locks from this teleport within a few frames. (The ambient
/// baseline starts from 0 instead — the clean live ride the page opens on.)
const TOUR_BASELINE_S: f64 = 1500.0;

/// Ceiling on one frame's replay advance (seconds of wall clock). A backgrounded tab stops the
/// rAF loop; without the clamp the first frame back would teleport the ride by minutes and the
/// map-matcher/breadcrumb would see one giant jump.
const MAX_FRAME_DT_S: f64 = 0.25;

/// What this host implements. Everything the shared screens can reach, because the page can walk
/// anywhere the device can: a capability withdrawn here would show the visitor a screen the real
/// device does not have. The bounded work behind DFU and free-space is simply never answered (the
/// [`HostPlatform`](obc_host_core::HostPlatform) defaults), exactly as the page's old `|_, _| {}`
/// command sink dropped those requests.
const SUPPORT: PlatformSupport = PlatformSupport {
    detour: true,
    settings_persistence: true,
    dfu: true,
    weather: true,
    bonding: true,
    storage_space_report: true,
    // The in-memory repositories hold the route-use and ride-sync stamps for the session.
    retention_metadata: true,
};

/// One queued page command, drained per [`Demo::tick`]. Gestures are injected through the app's
/// deterministic [`apply_gesture`](App::apply_gesture) seam (finished gestures only — the
/// long-press hold timers live in JS); the rest drive the replay / demo baselines.
pub enum Cmd {
    Gesture(Gesture),
    Play,
    Pause,
    Seek(f64),
    /// Rebuild the idle device that waits underneath the phone-to-device handoff.
    StageUpload,
    /// Deliver the same typed upload event the BLE host posts after committing the embedded route.
    ReceiveRoute,
    /// Enter guided-demo mode: reset to the staged mid-climb baseline (the tour engine drives
    /// playback + gestures from here; the ambient summit auto-restart is suspended).
    Enter,
    /// Leave guided-demo mode ("take control"): hand the device to the visitor where the demo
    /// left it and restore the ambient auto-restart.
    Exit,
    /// Reset to the ambient "just riding" state the page opens on — clean live ride from the
    /// start, visitor's controls enabled.
    Ambient,
    /// One device-wide squeeze (#1515). Unlike a gesture it is applied straight to the app: the
    /// recognizer that would produce it lives below the page's command vocabulary.
    Chord(obc_app::Chord),
}

/// Parse one command string — the page-facing vocabulary (exact strings): `press`, `back`,
/// `hold`, `backhold`, `context` (the Down+Back squeeze that opens a screen's contextual drawer),
/// `step:<n>` (signed Up/Down steps), `play`, `pause`, `seek:<secs>`, `enter`, `exit`, `ambient`,
/// `upload`, `receive`. `None` for unknown or malformed input — the page can't crash the demo with
/// a typo.
pub fn parse_cmd(cmd: &str) -> Option<Cmd> {
    match cmd {
        "press" => Some(Cmd::Gesture(Gesture::Press)),
        "context" => Some(Cmd::Chord(obc_app::Chord::Context)),
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
            } else {
                None
            }
        }
    }
}

/// The two app-rebuild baselines behind [`Cmd::Enter`] / [`Cmd::Ambient`] — **the** demo-reset
/// seam (epic #624; S2 builds on exactly this). Both rebuild the app to a fresh `[Home, Map]`
/// riding session on the demo route so a previous demo can't leak in (a created reroute activates
/// a new route; an interrupted run can leave the stack deep in a menu).
/// Both baselines run **Manual** climb mode: the demo ride is one long climb, so Auto's
/// auto-switch would yank the opening view off the Map onto the Climb profile within the first
/// frames (the egui-era page's "opens on the Climb screen" flaw). Manual keeps the Climb screen
/// reachable through the conditional Back-cycle while a climb is active — which on this ride is
/// essentially always.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// Guided-demo entry: the ride seeked mid-climb ([`TOUR_BASELINE_S`]), page engine in control.
    Tour,
    /// The page-opening state: ride from the start, visitor in control.
    Ambient,
    /// The upload bookend: an idle device whose catalog already contains the committed route.
    /// `ReceiveRoute` posts the host event immediately after this reset, producing the real
    /// `ROUTE RECEIVED` card instead of a page-authored imitation.
    Upload,
}

pub struct Demo {
    /// Map file bytes; `Reader` is a cheap view rebuilt over them per use.
    bytes: &'static [u8],
    /// The immutable map tables (style table + LOD pyramid), parsed once at startup — mirroring
    /// the device, which parses them once at boot.
    tables: MapTables,
    /// The streamed-map cache, kept for the whole session (as the device holds one in its
    /// reserved region), so a settled view warms to full hit rate. **Boxed**: ~278 KB inline —
    /// like the app below, far too big for wasm's default stack to carry as a temporary.
    cache: Box<MapCache>,
    /// The shared app (~136 KB — heap-allocated: a by-value `App` temporary is exactly the kind
    /// of silent wasm stack trap the NavScratch gotcha is about).
    app: Box<App>,
    /// The render path's per-frame scratch (~90 KB), owned by the host since #1146 and lent to each
    /// render call. **Boxed** for the same reason as the app: a by-value temporary of this size is
    /// the wasm stack trap.
    scratch: Box<obc_render::RenderScratch>,
    routes: MemRouteStore,
    rides: MemRideStore,
    tracks: MemTrackStore,
    player: GpxPlayer,
    baro: BaroSensor,
    /// The shared typed executor: the next pass's outcomes and facts, and the in-flight route plan
    /// (stepped once per tick). Every sequencing decision lives in `obc-host-core`, not here.
    host: HostLoop,
    /// The resident active-route parse, opened once per frame and lent to both the pass and the
    /// render (so the map opens without a per-frame `RouteIndex` reparse).
    session: ActiveRouteSession,
    frame: RgbaFrame,
    /// Page commands queued since the last [`tick`](Demo::tick), drained **in full, in order,
    /// once per tick** (not one-per-tick — a guided-tour step deliberately pushes several cmds in
    /// one frame, e.g. `["step:2", "press"]`, and relies on the app draining them in that order
    /// within the single frame; one-per-tick would stall every multi-cmd step across extra frames
    /// and break that contract).
    ///
    /// **Gesture-batch caveat for tour authors:** every cmd drained in one tick applies with **no
    /// draw between them** (the single [`render_frame`](App::render_frame) happens after the whole
    /// queue is drained). A gesture that consumes *draw-time lazy state* — the canonical case is
    /// the POI list, whose first draw snapshots the nearest-POI ordering that a following `Press`
    /// consumes (the `f` "draw a throwaway frame" token in `obc-sim`'s `apply_script` exists for
    /// exactly this) — must therefore land in a **separate tour step / separate tick** from the
    /// gesture that opens that screen, so a real render happens in between. Batching them in one
    /// step presses against un-filled lazy state. The page's step engine gets this for free: each
    /// step waits (polls [`state`](Demo::state)) for its target screen — i.e. for a render — before
    /// issuing the next step's cmds.
    queue: Vec<Cmd>,
    /// The previous `tick` timestamp (rAF `now_ms`), for the replay `dt`.
    last_now_ms: Option<f64>,
    /// How far the device's UI clock runs **ahead of** the rAF timestamp: the time the guided-demo
    /// pre-roll's own render-free passes consumed.
    ///
    /// The UI clock (holds, animations, card dwell, the next wake) is `now_ms + this`, so it is
    /// monotonic *and* it keeps running. Clamping it to the larger of the two instead would freeze
    /// the device for as long as the pre-roll took — about eight minutes of wall clock after every
    /// `enter`, which is the whole guided tour.
    ui_offset_ms: u32,
    /// The baseline's ride, still to be asked for. A ride needs a mounted card, and a device
    /// learns it has one on its **first pass** — so the page asks on the first frame that can grant
    /// one, and then stops asking. Asking at construction would be refused, and the refusal is a
    /// recording-error card the visitor would see on a page that is about to record perfectly well.
    pending_ride: bool,
    /// Guided-demo mode: the page's tour engine owns playback + baseline resets, so the ambient
    /// summit auto-restart is suspended (a `start_session` mid-demo would reset progress under
    /// the script).
    tour_active: bool,
    /// First frame rendered — the page's readiness signal.
    ready: bool,
}

impl Demo {
    /// Build the whole embedded device and stage the ambient baseline (the state the page opens
    /// on: live ride from the start, controls enabled). **Boxed**: `Demo` embeds two six-figure
    /// structs (map cache + app) — returning it by value would put ~430 KB temporaries on wasm's
    /// default stack.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Box<Self> {
        let bytes: &'static [u8] = DEMO_MAP;
        let tables = MapTables::parse(&SliceSource(bytes)).expect("embedded demo map is a valid OBCM");
        let routes = MemRouteStore::new(&[DEMO_ROUTE]);
        let rides = MemRideStore::new(demo_rides());
        let track = Track::parse(DEMO_RIDE_GPX).expect("embedded demo GPX parses");
        let mut player = GpxPlayer::new(track);
        player.set_speed(DEMO_SPEED);

        let mut demo = Box::new(Demo {
            bytes,
            tables,
            cache: MapCache::new_boxed(),
            // Placeholder app; `reset(Ambient)` below builds the real baseline (the one seam).
            app: Box::new(App::new(AppState::new(0, 0, 1.0))),
            scratch: Box::new(obc_render::RenderScratch::new()),
            routes,
            rides,
            tracks: MemTrackStore::new(),
            player,
            baro: BaroSensor::new(),
            host: HostLoop::new(),
            session: ActiveRouteSession::new(),
            frame: RgbaFrame::new(FRAME_W, FRAME_H),
            queue: Vec::new(),
            last_now_ms: None,
            ui_offset_ms: 0,
            pending_ride: false,
            tour_active: false,
            ready: false,
        });
        demo.reset(Baseline::Ambient);
        demo
    }

    /// Queue one page command (drained on the next [`tick`](Demo::tick)). Unknown or malformed
    /// input is ignored.
    pub fn cmd(&mut self, cmd: &str) {
        if let Some(c) = parse_cmd(cmd) {
            self.queue.push(c);
        }
    }

    /// The current (input-receiving) screen's variant name, e.g. `"Map"`, `"PoiList"`,
    /// `"NavPlanning"`, `"RouteOverview"`, `"Climb"`. The page polls this to advance a demo step
    /// only once the app actually reached the target screen — no fixed sleeps, and it waits out
    /// the real planner (the name becomes `RouteOverview` / `NavFail` when it finishes).
    pub fn state(&self) -> &'static str {
        self.app.top_screen().name()
    }

    /// True once the first frame has been rendered (the page can swap its poster for the canvas).
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// The rendered RGBA frame ([`FRAME_W`]`×`[`FRAME_H`]`×4` bytes), for `putImageData`.
    pub fn frame(&self) -> &[u8] {
        self.frame.as_rgba()
    }

    /// Advance one JS-driven frame; returns `true` if the frame buffer changed (the page only
    /// blits then). `now_ms` is the rAF timestamp (any monotonic ms clock works).
    pub fn tick(&mut self, now_ms: f64) -> bool {
        // Replay dt from the rAF clock, clamped so a throttled/backgrounded tab can't teleport
        // the ride on the first frame back.
        let dt = match self.last_now_ms.replace(now_ms) {
            Some(last) => ((now_ms - last) / 1000.0).clamp(0.0, MAX_FRAME_DT_S),
            None => 0.0,
        };

        // Drain the page's commands first, so a gesture's transition is visible in this same
        // frame's render (and the closed-loop tour never waits an extra frame). Gestures go into
        // *this* frame's pass rather than straight into the app: the pass applies them at its own
        // stage, after what the executor finished and after the facts, so a page command and a
        // rider's button land by exactly the same path. See [`queue`](Self::queue) for the
        // no-draw-between-cmds caveat that constrains how tour steps are grouped.
        let mut gestures: Vec<Gesture> = Vec::new();
        for cmd in std::mem::take(&mut self.queue) {
            self.apply(cmd, &mut gestures);
        }

        self.arm_baseline_ride();
        let plan = self.device_frame(self.ui_now(), dt, &gestures);
        // A single-loop host has no second recognizer to cancel, so it consumes the hold-cancel
        // latch the pass may have armed rather than leaving it set for a plane that does not exist
        // — the same rule `App::handle_input` applies for the hosts that still go through it.
        let _ = self.app.take_hold_cancel();

        // Ambient: restart the climb at the summit so the page stays alive. Point-to-point, so
        // bump the tracking session to clear the breadcrumb + totals (a fresh lap instead of
        // dragging a trail across the map). Suspended while a guided demo owns playback.
        if !self.tour_active && !self.player.is_playing() {
            self.player.play();
            self.app.recorder.request(obc_app::RecorderIntent::Start);
        }

        // `plan.next_wake_ms` and `plan.immediate` are deliberately **ignored**: the page is
        // rAF-paced, so the browser decides when the next frame happens and the next rAF frame is
        // already the "come straight back" an immediate wake asks for.
        //
        // Render on demand — `plan.render` is the same signal the firmware gates its repaints on.
        // The first frame always renders (`ready` doubles as the page's poster-swap signal).
        if plan.render.map || plan.render.overlay || !self.ready {
            // Re-open the active route: the executor may have committed new geometry under it (a
            // planned route, a spliced detour), and the frame must draw what is there now.
            self.session.sync(&self.app, &mut self.routes);
            let route_src = self.routes.active_source();
            let route = match (self.session.index(), route_src.as_ref()) {
                (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
                _ => None,
            };
            let src = SliceSource(self.bytes);
            let reader = Reader::new(&src, &self.tables, &self.cache);
            self.app.render_frame(
                Some(&mut self.scratch),
                &mut self.frame,
                &reader,
                route.as_ref(),
                FRAME_W as f32,
                FRAME_H as f32,
                |c| {
                    let (r, g, b) = rgb565_to_device64(c);
                    Rgb888::new(r, g, b)
                },
            );
            self.ready = true;
            return true;
        }
        false
    }

    /// The device's UI clock: the rAF timestamp plus whatever a guided pre-roll ran through on its
    /// own. Monotonic, because the rAF clock is and the offset only ever grows.
    fn ui_now(&self) -> u32 {
        (self.last_now_ms.unwrap_or(0.0).max(0.0) as u32).wrapping_add(self.ui_offset_ms)
    }

    /// One device frame: the active route opened once, one [`App::run_pass`], and the typed
    /// executor behind it. The shape the page's tick and the guided pre-roll share.
    ///
    /// Named `device_frame` and not `frame` because [`frame`](Self::frame) is the page's RGBA buffer.
    fn device_frame(&mut self, ui_ms: u32, dt: f64, gestures: &[Gesture]) -> PassPlan {
        // Open the active route's geometry from the resident session — no per-frame `RouteIndex`
        // reparse (the acceptance-criterion fix): the index is kept until the active bytes change.
        self.session.sync(&self.app, &mut self.routes);
        let mut plan = {
            let route_src = self.routes.active_source();
            let route = match (self.session.index(), route_src.as_ref()) {
                (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
                _ => None,
            };
            // Advance the ride and hand the pass the playback clock (no compass on the web — the
            // replay's GPS course orients the heading-up map).
            let (ride, sensors) = replay_advance(
                &mut self.player,
                &mut self.baro,
                None,
                dt,
                self.tracks.sink(),
                ReplaySensors::default(),
            );
            self.host.pass(
                &mut self.app,
                PassClock { ride, ui: InputClock(ui_ms) },
                gestures,
                sensors,
                route.as_ref(),
                None, // the landing demo mounts no weather store
                SUPPORT,
            )
        };
        // The typed executor: the plan's bounded effects against the in-memory stores, and
        // token-carrying outcomes for the next pass. The demo has no trips (`&mut ()`) and no
        // platform work of its own (`&mut ()` — no card scan, bond, settings store or DFU on the
        // page), so the whole loop is repository sequencing that lives once in `obc-host-core`.
        let src = SliceSource(self.bytes);
        let reader = Reader::new(&src, &self.tables, &self.cache);
        self.host.execute(
            &mut self.app,
            &mut plan,
            &mut self.session,
            &mut self.routes,
            &mut self.rides,
            &mut self.tracks,
            &mut (),
            &reader,
            // The demo page ships one embedded `.obcm` and no terrain beside it (EL7): the null
            // source keeps a planned route exactly as flat as it has always been here.
            &mut obc_route::NullElevation,
            &mut (),
        );
        plan
    }

    /// Apply one drained command. A gesture joins this frame's batch; everything else drives the
    /// replay or a baseline reset.
    fn apply(&mut self, cmd: Cmd, gestures: &mut Vec<Gesture>) {
        match cmd {
            Cmd::Gesture(g) => gestures.push(g),
            // A chord is applied **now**, not deferred into this frame's gesture batch, and that is
            // the device's own order rather than a shortcut. On hardware a chord and a gesture
            // recognised in the same frame are independent by construction — the recogniser
            // swallows the chord's constituents whole — and both `App::handle_input` and
            // `App::recognize` resolve the chord first, above the screen stack, before applying the
            // frame's gestures. A page batch of `["press", "context"]` therefore lands here exactly
            // as the same two inputs would on a device.
            Cmd::Chord(c) => {
                self.app.apply_chord(c);
            }
            Cmd::Play => self.player.play(),
            Cmd::Pause => self.player.pause(),
            Cmd::Seek(t) => self.player.seek(t),
            Cmd::StageUpload => self.reset(Baseline::Upload),
            Cmd::ReceiveRoute => {
                if let Some(&id) = self.routes.ids().first() {
                    self.host.facts().note_route_upload(RouteUpload { id, replaced: false, elevation: None });
                }
            }
            Cmd::Enter => self.reset(Baseline::Tour),
            Cmd::Exit => {
                // "Take control": leave the device where the demo parked it, controls live.
                self.tour_active = false;
                self.player.play();
            }
            Cmd::Ambient => self.reset(Baseline::Ambient),
        }
    }

    /// **The demo-reset seam** (epic #624 S2): the single path behind boot, `ambient`, and
    /// `enter`. Rebuild the app to a clean `[Home, Map]` riding session on the demo route and stage
    /// `baseline`. Rebuilding — rather than unwinding — guarantees a previous demo can't leak state
    /// in. The three axes the seam is parameterized on:
    /// - **climb_mode** — `Manual` for *both* baselines (constant, not a knob): the demo ride is
    ///   one long climb, so `Auto` would yank the opening Map onto the Climb profile (req 2).
    /// - **seek_time** — the only per-baseline construction difference: [`Baseline::Tour`] seeks
    ///   mid-climb ([`TOUR_BASELINE_S`]), [`Baseline::Ambient`] starts from `0.0`.
    /// - **controls_enabled** — captured by `tour_active` (`= baseline == Tour`): a guided tour
    ///   owns playback (visitor controls paused on the page), ambient hands the visitor the wheel.
    fn reset(&mut self, baseline: Baseline) {
        use obc_app::settings::{ClimbMode, Settings};

        // Both bookend baselines are page-driven. In particular, Upload must stay idle instead of
        // being caught by the ambient "replay ended → start a fresh session" loop.
        self.tour_active = baseline != Baseline::Ambient;

        let (cx, cy, zoom) = {
            let src = SliceSource(self.bytes);
            let reader = Reader::new(&src, &self.tables, &self.cache);
            initial_camera(&reader, FRAME_W)
        };
        let mut state = AppState::new(cx, cy, zoom * DEMO_ZOOM);
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        let mut app = if baseline == Baseline::Upload { App::new_idle(state) } else { App::new(state) };
        // The page keeps one RGBA frame and repaints it on demand, so every render is a render over
        // the last one — which is what lets a drawer's sheet grow over a base the frame no longer
        // draws (#1559).
        app.set_resident_frame(true);
        // Mirror the map's §8.6 routing-profile names for the Bike-type screen + overview label.
        app.set_nav_profiles(self.tables.nav_profiles());
        app.set_map_nav_graph(self.tables.has_nav_graph());
        app.set_routes_with_ids(self.routes.catalog(), self.routes.ids());
        app.set_rides(self.rides.catalog(), self.rides.ids());
        // Manual climb mode for *both* baselines — see [`Baseline`]: the whole demo ride is a
        // climb, so Auto would swap the opening Map for the Climb profile within the first frames.
        //
        // `IdleReturn::Never`, because the page has no rider whose idleness means anything: the
        // pass runs the device's animation clock (the legacy frame here never did), and a guided
        // step that dwells on a menu while the visitor reads it would otherwise be swept back to
        // the Map thirty seconds in.
        app.set_settings(Settings {
            climb_mode: ClimbMode::Manual,
            idle_return: obc_app::settings::IdleReturn::Never,
            ..Settings::default()
        });
        // Select the embedded demo route; the ride itself is asked for by `arm_baseline_ride` on
        // the first frame that can grant one.
        self.pending_ride = baseline != Baseline::Upload && !self.routes.catalog().is_empty();
        if self.pending_ride {
            app.activate_route(0);
        }
        // Overwrite in the existing heap slot (no fresh allocation, no lingering old app). The
        // executor is rebuilt with it: its inbox holds outcomes and operation tokens minted by the
        // app that is being replaced, and none of them answer anything in the new one.
        *self.app = app;
        self.host = HostLoop::new();
        // The resident parse goes with it, and the store's active binding has to be dropped in the
        // same breath: `sync_active` only reparses on a *change*, so a store still bound to route 0
        // would answer "unchanged" and the fresh session would never open the route at all.
        self.session = ActiveRouteSession::new();
        self.routes.invalidate_active();

        self.player.seek(0.0);
        if baseline == Baseline::Upload {
            self.player.pause();
        } else {
            self.player.play();
        }

        if baseline == Baseline::Tour {
            // Arrive at the same mid-climb camera through real replay ticks instead of a teleport.
            // Nothing renders during this deterministic pre-roll, but Activity sees the genuine
            // one-Hz fixes and barometric samples, so the Pause → Finish bookend saves believable
            // distance/time/climb totals from the exact GPX it later shows in the phone capture.
            // About 500 cheap, render-free ticks at 3×; paid only when a guided chapter starts.
            self.baro = BaroSensor::new();
            while self.player.time() < TOUR_BASELINE_S {
                let wall_dt = ((TOUR_BASELINE_S - self.player.time()) / self.player.speed() as f64).min(1.0);
                // Full frames, not bare ticks: a pass whose effects nobody serves leaves its
                // domains in flight, and the device the guided demo hands over would then refuse
                // the first delete or stamp it is asked for.
                // The pre-roll's own time joins the offset, so the clock the page resumes on
                // carries it and keeps ticking from there.
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
    /// A **one-shot**, and that is the whole of it: the request is spent here, so the ride the
    /// visitor finishes stays finished. A page that re-asked every frame would reopen it two
    /// frames later.
    fn arm_baseline_ride(&mut self) {
        if self.pending_ride && self.app.can_record() {
            self.pending_ride = false;
            self.app.recorder.request(obc_app::RecorderIntent::Start);
        }
    }
}

/// The seeded demo ride catalog — two rides, one synced and one not, so the Rides screen's
/// red/plain footers both show.
fn demo_rides() -> Vec<obc_app::RideSummary> {
    let mk = |name: &str, start: u32, dist: u32, mv: u32, climb: u16, synced: bool| obc_app::RideSummary {
        name: heapless::String::try_from(name).unwrap_or_default(),
        start_time: start,
        distance_m: dist,
        moving_time_s: mv,
        climb_m: climb,
        synced,
        synced_at_utc: 0,
    };
    vec![
        mk("Grimsel Climb", 1_720_100_000, 48_200, 3 * 3600 + 40 * 60, 1620, true),
        mk("Evening Loop", 1_719_900_000, 22_500, 3600 + 12 * 60, 340, false),
    ]
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

    /// The shipped payload is a map **this build's reader accepts**.
    ///
    /// Everything else in this module drives `Demo`, which mounts the map behind several layers of
    /// app state — so a payload the reader refuses shows up as a blank canvas in a browser rather
    /// than as a failing assertion with the reason in it. That is exactly what a format bump does:
    /// `grimsel-demo.obcm` was repacked to v14 in #1420's FS7.5b because a v14 reader refuses a v13
    /// file outright, and nothing in the tree would have caught it if the repack had been missed
    /// (`include_bytes!` is happy with any bytes at all).
    ///
    /// `MapTables::parse` *is* the version gate — it refuses any version but this build's — so
    /// three lines here fail at the payload on the next bump instead of on the landing page.
    #[test]
    fn the_shipped_demo_map_parses_at_this_builds_obcm_version() {
        let src = SliceSource(DEMO_MAP);
        let tables = MapTables::parse(&src).expect("the shipped demo payload parses at this build's OBCM version");
        assert!(tables.bbox.max_lat > tables.bbox.min_lat, "and it carries a real bbox, not a stub");
    }

    /// The page-opening contract: the first tick renders (ready), the demo opens on the live Map,
    /// and the frame is exactly the putImageData layout.
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

    /// The one input path: a queued gesture lands on the app on the *same* tick (no extra-frame
    /// lag for the closed-loop tour), and junk commands are ignored rather than trusted.
    #[test]
    fn commands_drain_on_the_next_tick_and_junk_is_ignored() {
        let mut d = Demo::new();
        d.tick(0.0);
        // Back on the Map cycles the riding views (Map → Statistics on develop's Back-cycle).
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

    /// Render-on-demand: with playback paused and no input, the app settles and ticks stop
    /// reporting frame changes — the page's rAF loop then skips the putImageData entirely.
    #[test]
    fn settles_clean_when_paused() {
        let mut d = Demo::new();
        d.tick(0.0);
        d.cmd("pause");
        // A few frames drain the pause + any in-flight fix/animation edges…
        let mut changed = true;
        for i in 1..=20 {
            changed = d.tick(i as f64 * 16.0);
        }
        assert!(!changed, "a parked, paused demo stops redrawing");
        // …and playback resumes movement (fix cadence ≈ 1 s of playback time).
        d.cmd("play");
        let mut any = false;
        for i in 21..=100 {
            any |= d.tick(i as f64 * 16.0);
        }
        assert!(any, "resuming playback dirties the map again");
    }

    /// The guided-demo baseline seam: `enter` stages a mid-climb Map with a live session, `exit`
    /// hands control back without a reset, and the ride keeps playing throughout.
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
        drive(&mut d, &mut now, "press", "Map");
        assert!(d.app.recording(), "Start ride begins the session before the next chapter");
    }

    #[test]
    fn ride_log_bookend_pauses_selects_finish_and_saves() {
        let mut d = Demo::new();
        let mut now = 0.0;
        d.tick(now);

        drive(&mut d, &mut now, "enter", "Map");
        let stats = d.app.ride_stats();
        assert!(stats.distance_m > 1_000, "the visible Finish flow should contain a real partial ride");
        assert!(stats.moving_time_s > 60);
        assert!(stats.climb_m > 50);
        drive(&mut d, &mut now, "press", "RideControl");
        drive(&mut d, &mut now, "step:1", "RideControl");
        drive(&mut d, &mut now, "hold", "Home");
        assert!(d.app.recording(), "the ride is open until the store answers for the close");
        now += 16.0;
        d.tick(now);
        assert!(!d.app.recording(), "and the finalize's verdict is what closes it");
        // …and it stays ended. The baseline's Start is a one-shot; a page that re-asked for it
        // every frame would reopen a ride the rider just finished, about two frames later.
        for _ in 0..8 {
            now += 16.0;
            d.tick(now);
            assert!(!d.app.recording(), "the finished ride must not reopen itself");
        }
    }

    /// `Screen::NAMES` (the drift-guard export) contains every state this host can report — a
    /// rename in the screens! table breaks this before it breaks the page.
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
        drive(&mut d, &mut now, "press", "UpAhead");
        assert!(d.app.corridor_snapshot_len() > 0, "the demo route should showcase map POIs ahead");
        // One Back: the row replaced the sheet rather than stacking over it.
        drive(&mut d, &mut now, "back", "Map");
    }

    #[test]
    fn reroute_tour_reaches_pois_directly_from_the_ride_context() {
        let mut d = Demo::new();
        let mut now = 0.0;
        d.tick(now);

        drive(&mut d, &mut now, "enter", "Map");
        drive(&mut d, &mut now, "context", "ContextDrawer");
        drive(&mut d, &mut now, "step:1", "ContextDrawer");
        drive(&mut d, &mut now, "step:1", "ContextDrawer");
        drive(&mut d, &mut now, "press", "PoiMenu");
        drive(&mut d, &mut now, "step:1", "PoiMenu");
        drive(&mut d, &mut now, "step:1", "PoiMenu");
        drive(&mut d, &mut now, "press", "PoiList");
        assert!(d.app.poi_snapshot_len() > 0, "the scripted category should contain a demo POI");
        drive(&mut d, &mut now, "press", "PoiDetail");
        drive(&mut d, &mut now, "press", "NavConfirm");

        d.cmd("press");
        now += 16.0;
        d.tick(now);
        for _ in 0..2_000 {
            if d.state() != "NavPlanning" {
                break;
            }
            now += 16.0;
            d.tick(now);
        }
        assert_eq!(d.state(), "RouteOverview", "the embedded map should route to the scripted POI");
        drive(&mut d, &mut now, "press", "RouteSwap");
        drive(&mut d, &mut now, "press", "Map");
    }

    /// **The tour drift-guard** (epic #624 S3 / #628). The landing page's guided scenarios wait on
    /// screen-name strings; if the `screens!` table renames one, the page would silently turn that
    /// tour into a timeout march. This test reads `docs/index.html`, extracts every screen name the
    /// scenarios target, and asserts each is a real [`obc_app::Screen::NAMES`] entry — so a rename
    /// fails `cargo test` in the `test` job instead.
    ///
    /// **Parseable convention** (documented identically in `docs/index.html`): every guided-step
    /// target is a double-quoted string inside a `until: [ ... ]` array literal, and screen names
    /// appear *nowhere else* in a parseable position. We find each `until:` immediately followed by
    /// `[`, take up to the first `]`, and collect the quoted strings. (Doc-comment mentions of
    /// `until: [ ... ]` carry no quoted strings, so they contribute nothing.)
    #[test]
    fn tour_targets_are_real_screens() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/index.html");
        let html = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        let targets = extract_until_targets(&html);
        // Guard against a silent parser break (a convention change that finds nothing would let a
        // real rename slip through vacuously): the reroute + climb scenarios give us many targets.
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

    /// Pull every screen name out of `until: [ "A", "B" ]` array literals in the page source. Only
    /// a `until:` glued (modulo whitespace) to a `[` counts, so prose like "the `until:` array"
    /// never opens a spurious match.
    fn extract_until_targets(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = html.as_bytes();
        let mut search = html;
        let mut base = 0usize;
        while let Some(rel) = search.find("until:") {
            let after = base + rel + "until:".len();
            base = after;
            search = &html[after..];
            // Require the next non-whitespace byte to be `[`.
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
