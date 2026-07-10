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
use obc_app::{App, AppState, CameraMode, Gesture};
use obc_host_core::{
    finish_nav_plan, initial_camera, replay_step, MemRideStore, MemRouteStore, MemTrackStore, NavPlan,
};
use obc_reader::{rgb565_to_device64, MapCache, MapTables, Reader, SliceSource};
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};
use obc_route::{RouteIndex, RouteReader};

use crate::frame::RgbaFrame;

/// The demo panel resolution — the one [`obc_platform`] frame authority, not re-declared literals.
pub const FRAME_W: u32 = obc_platform::FRAME_W as u32;
pub const FRAME_H: u32 = obc_platform::FRAME_H as u32;

// The embedded demo payload (epic #624 S4, #637). The binaries live with the other committed
// fixtures in `obc-sim/assets/` — one provenance-controlled home (`repack.sh` + its README rules)
// for every packed asset — but only this crate ships them.
const DEMO_MAP: &[u8] = include_bytes!("../../obc-sim/assets/grimsel-demo.obcm");
const DEMO_ROUTE: &[u8] = include_bytes!("../../obc-sim/assets/grimsel-climb.obcr");
const DEMO_RIDE_GPX: &str = include_str!("../../obc-sim/assets/grimsel-climb-demo.gpx");

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

/// One queued page command, drained per [`Demo::tick`]. Gestures are injected through the app's
/// deterministic [`apply_gesture`](App::apply_gesture) seam (finished gestures only — the
/// long-press hold timers live in JS); the rest drive the replay / demo baselines.
pub enum Cmd {
    Gesture(Gesture),
    Play,
    Pause,
    Seek(f64),
    /// Enter guided-demo mode: reset to the staged mid-climb baseline (the tour engine drives
    /// playback + gestures from here; the ambient summit auto-restart is suspended).
    Enter,
    /// Leave guided-demo mode ("take control"): hand the device to the visitor where the demo
    /// left it and restore the ambient auto-restart.
    Exit,
    /// Reset to the ambient "just riding" state the page opens on — clean live ride from the
    /// start, visitor's controls enabled.
    Ambient,
}

/// Parse one command string — the page-facing vocabulary (exact strings): `press`, `back`,
/// `hold`, `backhold`, `turn:<n>` (signed detents), `play`, `pause`, `seek:<secs>`, `enter`,
/// `exit`, `ambient`. `None` for unknown or malformed input — the page can't crash the demo
/// with a typo.
pub fn parse_cmd(cmd: &str) -> Option<Cmd> {
    match cmd {
        "press" => Some(Cmd::Gesture(Gesture::Press)),
        "back" => Some(Cmd::Gesture(Gesture::Back)),
        "hold" => Some(Cmd::Gesture(Gesture::Hold)),
        "backhold" => Some(Cmd::Gesture(Gesture::BackHold)),
        "play" => Some(Cmd::Play),
        "pause" => Some(Cmd::Pause),
        "enter" => Some(Cmd::Enter),
        "exit" => Some(Cmd::Exit),
        "ambient" => Some(Cmd::Ambient),
        other => {
            if let Some(n) = other.strip_prefix("turn:") {
                n.trim().parse::<i32>().ok().map(|n| Cmd::Gesture(Gesture::Turn(n)))
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
    routes: MemRouteStore,
    rides: MemRideStore,
    tracks: MemTrackStore,
    player: GpxPlayer,
    baro: BaroSensor,
    /// An in-flight route plan (#499), stepped **once per tick** so the page stays live while a
    /// route computes. `None` when nothing is planning.
    nav_plan: Option<NavPlan>,
    frame: RgbaFrame,
    /// Page commands queued since the last [`tick`](Demo::tick), drained **in full, in order,
    /// once per tick** (not one-per-tick — a guided-tour step deliberately pushes several cmds in
    /// one frame, e.g. `["turn:2", "press"]`, and relies on the app draining them in that order
    /// within the single frame; one-per-tick would stall every multi-cmd step across extra frames
    /// and break that contract).
    ///
    /// **Gesture-batch caveat for tour authors:** every cmd drained in one tick applies with **no
    /// draw between them** (the single [`render_frame`](App::render_frame) happens after the whole
    /// queue is drained). A gesture that consumes *draw-time lazy state* — the canonical case is
    /// the POI list, whose first draw snapshots the nearest-POI ordering that a following `Press`
    /// consumes (the `d` "draw a throwaway frame" token in `obc-sim`'s `apply_script` exists for
    /// exactly this) — must therefore land in a **separate tour step / separate tick** from the
    /// gesture that opens that screen, so a real render happens in between. Batching them in one
    /// step presses against un-filled lazy state. The page's step engine gets this for free: each
    /// step waits (polls [`state`](Demo::state)) for its target screen — i.e. for a render — before
    /// issuing the next step's cmds.
    queue: Vec<Cmd>,
    /// The previous `tick` timestamp (rAF `now_ms`), for the replay `dt`.
    last_now_ms: Option<f64>,
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
            routes,
            rides,
            tracks: MemTrackStore::new(),
            player,
            baro: BaroSensor::new(),
            nav_plan: None,
            frame: RgbaFrame::new(FRAME_W, FRAME_H),
            queue: Vec::new(),
            last_now_ms: None,
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
        // frame's render (and the closed-loop tour never waits an extra frame). The whole queue
        // drains before the render below — see [`queue`](Self::queue) for the no-draw-between-cmds
        // caveat that constrains how tour steps are grouped.
        for cmd in std::mem::take(&mut self.queue) {
            self.apply(cmd);
        }

        // Host reconciliation, in the exact order the desktop sim runs it:
        // hold-to-delete drains (route, then ride) …
        if let Some(id) = self.app.take_route_delete() {
            if self.routes.delete_by_id(id) {
                self.app.set_routes_with_ids(self.routes.catalog(), self.routes.ids());
            }
        }
        if let Some(id) = self.app.take_ride_delete() {
            if self.rides.delete_by_id(id) {
                self.app.set_rides(self.rides.catalog(), self.rides.ids());
            }
        }

        // … then the resumable planner (#499): a drained create-route request starts a plan, a
        // drained cancel drops it, otherwise the in-flight plan runs one bounded step. A terminal
        // outcome commits + answers *before* `sync_active` below, so a successful plan's activated
        // route streams open this same frame.
        if let Some(req) = self.app.take_nav_request() {
            self.nav_plan = Some(NavPlan::start(&req, self.app.settings().bike_profile_idx));
        }
        if self.app.take_nav_cancel() {
            self.nav_plan = None;
        }
        let step = self.nav_plan.as_mut().map(|plan| {
            let src = SliceSource(self.bytes);
            let reader = Reader::new(&src, &self.tables, &self.cache);
            plan.step(&reader)
        });
        match step {
            None | Some(obc_route::Step::Running) => {}
            Some(obc_route::Step::Done(stats)) => {
                let plan = self.nav_plan.take().expect("just stepped it");
                finish_nav_plan(&mut self.app, &mut self.routes, Ok(stats), plan.bytes(), plan.tile_stats());
            }
            Some(obc_route::Step::Failed(e)) => {
                let plan = self.nav_plan.take().expect("just stepped it");
                finish_nav_plan(&mut self.app, &mut self.routes, Err(e), plan.bytes(), plan.tile_stats());
            }
        }

        // Open the active route's geometry before ticking so the map-matcher gets it.
        self.routes.sync_active(self.app.activity.active_route);
        let route_src = self.routes.active_source();
        let route_index = route_src.as_ref().and_then(|s| RouteIndex::read(s).ok());
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };

        // Reconcile the (memory-only) ride log to the app's tracking intent — this *drains* the
        // one-shot TrackAction, the host contract.
        let action = self.app.activity.take_track_action();
        self.tracks.reconcile(action, self.app.activity.session);

        // Advance the ride and tick the app on the playback clock (no compass on the web — the
        // replay's GPS course orients the heading-up map).
        replay_step(&mut self.app, &mut self.player, &mut self.baro, None, dt, route.as_ref(), self.tracks.sink());

        // Ambient: restart the climb at the summit so the page stays alive. Point-to-point, so
        // bump the tracking session to clear the breadcrumb + totals (a fresh lap instead of
        // dragging a trail across the map). Suspended while a guided demo owns playback.
        if !self.tour_active && !self.player.is_playing() {
            self.player.play();
            self.app.activity.start_session();
        }

        // The Bluetooth screen's Forget-phone drain: there is no bond on the web, so the request
        // just needs consuming (an undrained one-shot would linger).
        let _ = self.app.take_ble_forget();

        // Render on demand — the same dirty signal the firmware gates its repaints on. The first
        // frame always renders (`ready` doubles as the page's poster-swap signal).
        let dirty = self.app.take_dirty();
        if dirty.map || dirty.overlay || !self.ready {
            let src = SliceSource(self.bytes);
            let reader = Reader::new(&src, &self.tables, &self.cache);
            self.app.render_frame(&mut self.frame, &reader, route.as_ref(), FRAME_W as f32, FRAME_H as f32, |c| {
                let (r, g, b) = rgb565_to_device64(c);
                Rgb888::new(r, g, b)
            });
            self.ready = true;
            return true;
        }
        false
    }

    /// Apply one drained command.
    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Gesture(g) => self.app.apply_gesture(g),
            Cmd::Play => self.player.play(),
            Cmd::Pause => self.player.pause(),
            Cmd::Seek(t) => self.player.seek(t),
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

        self.tour_active = baseline == Baseline::Tour;

        let (cx, cy, zoom) = {
            let src = SliceSource(self.bytes);
            let reader = Reader::new(&src, &self.tables, &self.cache);
            initial_camera(&reader, FRAME_W)
        };
        let mut state = AppState::new(cx, cy, zoom * DEMO_ZOOM);
        state.mode = CameraMode::Follow;
        state.heading_up = true;
        let mut app = App::new(state);
        // Mirror the map's §8.6 routing-profile names for the Bike-type screen + overview label.
        app.set_nav_profiles(self.tables.nav_profiles());
        app.set_routes_with_ids(self.routes.catalog(), self.routes.ids());
        app.set_rides(self.rides.catalog(), self.rides.ids());
        // Manual climb mode for *both* baselines — see [`Baseline`]: the whole demo ride is a
        // climb, so Auto would swap the opening Map for the Climb profile within the first frames.
        app.set_settings(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() });
        // Select the embedded demo route and open a session so its line + ride stats show.
        if !self.routes.catalog().is_empty() {
            app.activity.active_route = Some(0);
            app.activity.start_session();
        }
        // Overwrite in the existing heap slot (no fresh allocation, no lingering old app).
        *self.app = app;

        self.player.seek(match baseline {
            Baseline::Tour => TOUR_BASELINE_S,
            Baseline::Ambient => 0.0,
        });
        self.player.play();
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
    };
    vec![
        mk("Grimsel Climb", 1_720_100_000, 48_200, 3 * 3600 + 40 * 60, 1620, true),
        mk("Evening Loop", 1_719_900_000, 22_500, 3600 + 12 * 60, 340, false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for junk in ["", "prss", "turn:", "turn:x", "seek:", "seek:x", "hold ", "TURN:1"] {
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

    /// `Screen::NAMES` (the drift-guard export) contains every state this host can report — a
    /// rename in the screens! table breaks this before it breaks the page.
    #[test]
    fn state_is_always_a_known_screen_name() {
        let mut d = Demo::new();
        d.tick(0.0);
        assert!(obc_app::Screen::NAMES.contains(&d.state()));
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
