//! [`UiRuntime`] — the UI-plane component behind the [`App`](crate::App) façade.
//!
//! It owns the screen stack and everything scheduled around it: the fused input plane, the
//! map-plane clock, the accumulated repaint demand, hold cancellation and the idle-return policy.
//! `App` stays the orchestrator, but every stack, dirty and timer mutation lands here. Facts the
//! other components hold arrive as parameters; this component never reaches back into them.

use embedded_graphics::primitives::Rectangle;

use obc_ports::Fix;
use obc_reader::Reader;

use crate::card_scheduler::{CardCtx, CardScheduler};
use crate::catalog_state::CatalogState;
use crate::corridor::CorridorScratch;
use crate::dirty::Dirty;
use crate::input_plane::InputPlane;
use crate::next_ahead::NextAhead;
use crate::placement::define_placement_constructors;
use crate::screen::vocab::marquee::Marquee;
use crate::screen::{self, BaseContent, HomeScreen, MapScreen, PoiScratch, ReaderNeed, Screen, Stack};
use crate::settings::{DateTime, Settings};

/// One frame's hold charge on the two hold buttons, as the host's own input plane sees them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HoldSample {
    pub(crate) select: f32,
    pub(crate) back: f32,
}

impl HoldSample {
    fn charging(&self) -> bool {
        self.select > 0.0 || self.back > 0.0
    }
}

pub(crate) struct UiRuntime {
    /// The screen stack (root = Home). The top screen receives input; drawing starts from the
    /// topmost opaque screen so overlays composite over the map.
    pub(crate) stack: Stack,

    /// The input + overlay plane: gesture recognizer, long-press hint overlay, live hold-progress.
    /// The firmware runs its own on a separate high-priority executor that preempts the map render
    /// and feeds gestures back through [`apply_gesture`](App::apply_gesture); `App` keeps this one
    /// for the [`handle_input`](App::handle_input) path.
    pub(crate) input: InputPlane,
    /// Millis at the last input or animation pass: the map plane's clock, which is distinct from
    /// the input plane's own.
    pub(crate) now_ms: u32,
    /// Accumulated map-plane repaint demand since the last drain. It starts `true`, so the host's
    /// first frame paints.
    pub(crate) map_dirty: bool,
    /// Accumulated region-scoped repaint demand: the union of every region-carrying screen-tick
    /// change since the last drain. It stays apart from [`map_dirty`](App::map_dirty), which
    /// overrides any region at drain time.
    pub(crate) region_dirty: Option<Rectangle>,
    /// Panel size (device px) of the last rendered frame, so a reported
    /// [`ScreenTick::region`](screen::ScreenTick::region) is sized to the real panel. `(0, 0)` until
    /// the first frame; region reporting abstains until then.
    pub(crate) frame_size: (i16, i16),
    /// One-shot clip for the next render: the frame's `Canvas` rejects whole primitives outside
    /// the region, which is draw-call work a pixel-level framebuffer clip cannot skip. The render
    /// takes it, so a host that never sets it always draws full frames.
    pub(crate) render_clip: Option<Rectangle>,
    /// Whether the host's render target keeps the last frame between renders.
    ///
    /// A resident target is the precondition for a partial repaint: it lets the frozen base's
    /// pixels stand while a sheet grows over them. `false` until a host says otherwise, so a host
    /// that composes each frame from nothing gets every screen drawn every time.
    pub(crate) resident_frame: bool,
    /// The soonest timed-redraw deadline across the visible stack, in millis from the last
    /// animation pass. `None` when nothing is time-animating.
    pub(crate) next_wake_ms: Option<u32>,
    /// The one scrolling name of the frame, adopted from each frame's draw and stepped here.
    pub(crate) marquee: Marquee,
    /// Map-plane millis of the last user input, which drives the idle-return timeout. It advances
    /// only on input: a GPS fix, a BLE event or a timed repaint must not reset it. Seeded to `0`,
    /// so the idle clock runs from power-on until the first touch.
    pub(crate) last_input_ms: u32,
    /// Whether idle time is accumulating. A screen for which no idle return is eligible suspends
    /// the clock, so a long modal operation cannot donate its elapsed time to the ordinary screen
    /// that replaces it.
    pub(crate) idle_return_timing: bool,
    /// Host-supplied hold progress (0.0-1.0) for both hold buttons. `None` on the single-loop
    /// hosts; the two-plane firmware feeds it each frame, because its holds live on a plane
    /// `App`'s own never sees. Select drives the in-screen confirm fills; Back is here because a
    /// Back hold defers a card too, and a sample that carried Select alone deferred nothing on the
    /// board.
    pub(crate) hold_progress_override: Option<HoldSample>,
    /// Set whenever a gesture changed the screen stack: a hold charging at that moment was aimed
    /// at a screen that is no longer the top, so it must be cancelled and never delivered to
    /// whatever replaced it.
    pub(crate) hold_cancel_pending: bool,
    /// The single POI-list snapshot buffer, held once here so the ~800 B does not multiply across
    /// the screen-stack union. It is filled lazily by the POI list screen's first draw, and
    /// invalidated when a POI list opens so re-entering a category re-queries.
    pub(crate) poi_scratch: screen::PoiScratch,
    pub(crate) find: crate::find_place::FindState,
    pub(crate) landmarks: crate::landmarks::Landmarks,
    pub(crate) map_icons: crate::map_icons::MapIcons,
    /// The settlement-name candidates the map overlay draws. App-owned for the same reason
    /// as [`map_icons`](Self::map_icons): a `Screen` variant is a slot in a `.bss` union.
    pub(crate) settlements: crate::settlements::SettlementCache,
    pub(crate) ahead: crate::whats_next::AheadState,
    /// The single route-corridor snapshot buffer: the map POIs near the route ahead, frozen on
    /// take. Held once here for the same reason as [`poi_scratch`](UiRuntime::poi_scratch), and
    /// disarmed until a screen asks for it.
    pub(crate) corridor_scratch: CorridorScratch,
    /// The per-category "next ahead" cache for the six `Next: <category>` stat tiles, harvested
    /// out of [`corridor_scratch`](Self::corridor_scratch). App-owned for the same reason as the
    /// two snapshots above, and it asks for nothing unless such a tile is on the drawn grid.
    pub(crate) next_ahead: NextAhead,
    /// Every host-pushed modal card. It is held here because the stack is here: the scheduler
    /// borrows the stack for the length of [`run_card_sweep`](UiRuntime::run_card_sweep) and never
    /// longer.
    pub(crate) cards: CardScheduler,
    /// The per-slot BLE sensor status, fed each pass by the host and drawn only by the Sensors
    /// settings screen. It is held off [`AppState`], so feeding it never gates a map redraw on a
    /// non-sensor screen.
    pub(crate) sensor_status: [crate::sensors::SensorStatus; crate::settings::SENSOR_SLOTS],
    /// The sensors discovered while the scan-list screen runs a scan. Empty outside a scan, and
    /// replaced wholesale each pass while one runs.
    pub(crate) sensor_scan_hits: crate::sensors::SensorScanHits,
}

impl UiRuntime {
    define_placement_constructors!(
        /// The boot state: the Home root on the stack, first frame dirty, nothing pending.
        pub(crate) fn new();
        /// Initialize `slot` in place to the [`new`](UiRuntime::new) state. The screen stack and
        /// the POI scratch are KB-scale, so nothing here may form a by-value `UiRuntime` on the
        /// stack.
        pub(crate) unsafe fn init_in_place;
        fields {
            stack: Stack::new(),
            input: InputPlane::new(),
            now_ms: 0,
            map_dirty: true,
            region_dirty: None,
            resident_frame: false,
            frame_size: (0, 0),
            render_clip: None,
            next_wake_ms: None,
            marquee: Marquee::default(),
            last_input_ms: 0,
            idle_return_timing: true,
            hold_progress_override: None,
            hold_cancel_pending: false,
            poi_scratch: PoiScratch::new(),
            find: crate::find_place::FindState::new(),
            landmarks: crate::landmarks::Landmarks::new(),
            map_icons: crate::map_icons::MapIcons::new(),
            settlements: crate::settlements::SettlementCache::new(),
            ahead: crate::whats_next::AheadState::new(),
            corridor_scratch: CorridorScratch::new(),
            next_ahead: NextAhead::new(),
            cards: CardScheduler::new(),
            sensor_status: [crate::sensors::SensorStatus::default(); crate::settings::SENSOR_SLOTS],
            sensor_scan_hits: crate::sensors::SensorScanHits::new(),
        }
        // The always-present Home root. It can't be part of the field plan above:
        // `heapless::Vec::push` isn't `const`, so an empty stack is all a field expression can say.
        post |ui| {
            let _ = ui.stack.push(Screen::Home(HomeScreen::new()));
        }
    );

    /// Advance the map-plane clock to `now_ms` and poll each visible screen's timers in one pass.
    /// A time-driven repaint that fired dirties the map, and the soonest residual deadline is
    /// stored for [`App::ms_until_next_wake`](crate::App::ms_until_next_wake). Polling starts at
    /// the base, except while the base is frozen under a sheet.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn advance_timers(
        &mut self,
        now_ms: u32,
        now: DateTime,
        ms_to_next_minute: u32,
        settings: &Settings,
        pan_active: bool,
        tracking: bool,
    ) {
        self.now_ms = now_ms;
        let base = screen::base_index(&self.stack);
        let first = base + usize::from(self.base_frozen());
        let (w, h) = (self.frame_size.0 as i32, self.frame_size.1 as i32);
        let mut next_wake = None;
        let screens = self.stack.iter_mut().skip(first);
        let ticks = screens
            .map(|scr| scr.tick_timers(self.now_ms, now, ms_to_next_minute, settings, w, h, pan_active, tracking))
            .chain(core::iter::once(self.marquee.tick(self.now_ms)));
        for tick in ticks {
            // A change that promises a containing region accumulates apart from the full-frame
            // demand; `take_dirty` folds the two.
            if tick.changed {
                match tick.region {
                    Some(r) => self.region_dirty = Some(self.region_dirty.map_or(r, |acc| union_rect(acc, r))),
                    None => self.map_dirty = true,
                }
            }
            next_wake = next_wake.into_iter().chain(tick.next_wake_ms).min();
        }
        self.next_wake_ms = next_wake;
    }

    /// The [`BaseContent`] of the lowest opaque screen: the declared fact the live-data, map-I/O
    /// and indicator gates read instead of open-coding a `matches!` on the enum. An overlay over a
    /// riding view still reports the riding view's content.
    fn base_content(&self) -> BaseContent {
        let base = screen::base_index(&self.stack);
        self.stack.get(base).map(|s| s.caps().base).unwrap_or(BaseContent::Chrome)
    }

    /// Whether the base screen draws the map. A render-on-demand host polls this to skip the whole
    /// map pipeline on a non-map frame, including the `Reader` build and its stack spike.
    pub(crate) fn base_draws_map(&self) -> bool {
        self.base_content() == BaseContent::Map
    }

    /// Whether an overlay sheet covers the base screen. A frozen base does not tick, and its rows
    /// on the panel stand, so its draw is skipped.
    pub(crate) fn base_frozen(&self) -> bool {
        let base = screen::base_index(&self.stack);
        self.stack.iter().skip(base + 1).any(|s| s.is_overlay())
    }

    /// Whether this frame draws the sheet and nothing else: the frozen base's rows are already
    /// right, so an open step costs the sheet alone. Three things exclude it, and each is declared
    /// by the thing it is about. A host that composes every frame from nothing never claims a
    /// resident frame. A base that recesses takes its second draw, because the recess is that draw.
    /// A sheet that is not purely covering asks for the base itself ([`Screen::needs_base`]).
    pub(crate) fn sheet_only(&self) -> bool {
        let base = screen::base_index(&self.stack);
        self.resident_frame
            && self.base_frozen()
            && self.stack.get(base).is_some_and(|s| !s.caps().recess)
            && !self.stack.iter().skip(base + 1).any(|s| s.needs_base())
    }

    pub(crate) fn spend_base_draw(&mut self) {
        let base = screen::base_index(&self.stack);
        for scr in self.stack.iter_mut().skip(base + 1) {
            scr.clear_base_debt();
        }
    }

    /// Whether the frame needs the streamed-map [`Reader`] built and passed to the render: a
    /// superset of [`base_draws_map`](App::base_draws_map). The POI list and POI detail screens
    /// need it only until their one-shot read lands in [`prepare`](crate::screen::Screen::prepare),
    /// after which the host skips the build again.
    pub(crate) fn base_needs_reader(&self) -> bool {
        // The route-corridor snapshot is armed by a screen but owned by the App, so its need is a
        // request and not a `ReaderNeed` row. Disarmed, which is the normal state, this is free.
        if self.corridor_scratch.pending() {
            return true;
        }
        // A frame that does not draw the base needs nothing the base reads. On the board that
        // build is an SD style-table parse the frame then throws away: about 45 ms of an 80 ms
        // open step.
        let base = screen::base_index(&self.stack);
        let rebuilding_photo =
            self.stack.get(base).is_some_and(|s| matches!(s, Screen::LandmarkPhoto(page) if page.covered_rebuild));
        if self.sheet_only() && !rebuilding_photo {
            return false;
        }
        let Some(scr) = self.stack.get(base) else { return false };
        match scr.caps().reader {
            ReaderNeed::Always => true,
            ReaderNeed::PoiSnapshot => matches!(scr, Screen::PoiList(s) if self.poi_snapshot_pending(s)),
            // The detail's hours read runs in `prepare` off the `Reader`; keep it built until it lands.
            ReaderNeed::PoiHours => matches!(scr, Screen::PoiDetail(s) if s.hours_pending(&self.poi_scratch)),
            ReaderNeed::Photo | ReaderNeed::Articles => true,
            ReaderNeed::Never => false,
        }
    }

    /// Run the base screen's pre-draw acquisition: hand it the frame's `Reader`, streamed route
    /// and fix, so it resolves reader-backed state into immutable prepared state before the draw
    /// loop. `Render` then carries the POI scratch read-only and draw stays side-effect-free.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_base(
        &mut self,
        reader: Option<&Reader>,
        route: Option<&obc_route::RouteReader>,
        user_fix: Option<Fix>,
        active_route: Option<usize>,
        progress_m: u32,
        route_total_m: u32,
        detour_preview: &[(i32, i32)],
        place_local: Option<(u8, u16)>,
    ) {
        // The corridor snapshot belongs to no single screen, so it resolves at this boundary and
        // not inside one screen's `prepare`. A no-op unless a screen armed it.
        if !self.find.owns_pages() && !self.stack.iter().any(|s| matches!(s, Screen::WhatsNext(_))) {
            self.corridor_scratch.prepare(reader, route, place_local);
        }
        // If the snapshot that just landed is the one the `Next: <category>` cache asked for,
        // distil it here, the one place a fresh snapshot is sure to exist. This runs ahead of the
        // draw, so the tile this frame draws already names the entry the landing wrote. That is
        // why no render key names the cached entries, only the request.
        if let Some(key) = self.next_ahead.request() {
            if self.corridor_scratch.holds(key) {
                self.next_ahead.harvest(key, self.corridor_scratch.entries());
            }
        }
        let base = screen::base_index(&self.stack);
        if let Some(scr) = self.stack.get_mut(base) {
            let mut px = screen::Prepare {
                place_local,
                reader,
                route,
                poi_scratch: &mut self.poi_scratch,
                user_fix,
                active_route,
                progress_m,
                route_total_m,
                detour_preview,
            };
            scr.prepare(&mut px);
            if reader.is_some()
                && ((user_fix.is_some() && matches!(scr, Screen::PoiList(s) if s.pending(&self.poi_scratch)))
                    || (route.is_some() && self.corridor_scratch.pending()))
            {
                self.map_dirty = true;
                self.next_wake_ms = Some(1);
            }
        }
    }

    /// The visible Assistant query owns the shared corridor scratch. Statistics use it only
    /// when no Assistant query is active. Rows are still read in prepare, never in draw.
    pub(crate) fn reconcile_corridor(&mut self, scope: crate::corridor::UpAheadScope) {
        if self.stack.iter().any(|s| matches!(s, Screen::WhatsNext(_))) {
            if let Some(key) = self.ahead.request(scope) {
                self.corridor_scratch.arm(key);
            } else {
                self.corridor_scratch.disarm();
            }
            return;
        }
        if self.find.owns_pages() {
            return;
        }
        match self.next_ahead.request() {
            Some(key) => self.corridor_scratch.arm(key),
            None => self.corridor_scratch.disarm(),
        }
    }

    /// Re-decide what the `Next: <category>` tiles need and re-point the corridor scratch at it.
    /// Called once per pass with the facts the policy needs; the policy itself lives in
    /// [`NextAhead::reconcile`](crate::next_ahead::NextAhead). The request is scoped to the
    /// Statistics screen being drawn, so elsewhere the reader seam stays quiet.
    pub(crate) fn reconcile_next_ahead(
        &mut self,
        settings: &Settings,
        scope: crate::corridor::UpAheadScope,
        active_route: Option<usize>,
        progress_m: u32,
    ) {
        let mut placed = obc_reader::PoiCategorySet::EMPTY;
        for f in settings.stat_fields.as_slice() {
            if let Some(cat) = f.category() {
                placed = placed.with(cat);
            }
        }
        self.next_ahead.reconcile(placed, self.stats_grid_shown(), active_route, progress_m);
        self.reconcile_corridor(scope);
    }

    /// Whether the Statistics screen, the only place a `Next: <category>` tile draws, is the base
    /// screen this pass. Deliberately not "anywhere on the stack": a tile behind a menu is not
    /// being read, and the query it would keep warm costs a card spin-up.
    fn stats_grid_shown(&self) -> bool {
        let base = screen::base_index(&self.stack);
        matches!(self.stack.get(base), Some(Screen::Statistics(_)))
    }

    /// Whether the given POI list screen still needs a `Reader` at draw.
    pub(crate) fn poi_snapshot_pending(&self, screen: &crate::screen::PoiListScreen) -> bool {
        screen.pending(&self.poi_scratch)
    }

    /// Feed the host's per-slot sensor status, pushed each pass and stored app-side, so no radio
    /// type crosses the seam. A change while the Sensors screen is up dirties the map; on any other
    /// screen the status is not drawn, so an update repaints nothing.
    pub(crate) fn set_sensor_status(&mut self, status: &[crate::sensors::SensorStatus]) {
        let mut next = self.sensor_status;
        for (dst, src) in next.iter_mut().zip(status) {
            *dst = *src;
        }
        if next != self.sensor_status {
            self.sensor_status = next;
            if self.sensors_screen_up() {
                self.map_dirty = true;
            }
        }
    }

    /// Feed the live sensor scan hits, replacing the resident list wholesale; an empty slice
    /// clears it. A change while the scan screen is up dirties the map, so a freshly-found sensor
    /// appears without waiting for another input.
    pub(crate) fn set_sensor_scan_hits(&mut self, hits: &[crate::sensors::SensorScanHit]) {
        let changed =
            self.sensor_scan_hits.len() != hits.len() || self.sensor_scan_hits.iter().zip(hits).any(|(a, b)| a != b);
        if !changed {
            return;
        }
        self.sensor_scan_hits.clear();
        for h in hits.iter().take(crate::sensors::SCAN_HITS_MAX) {
            let _ = self.sensor_scan_hits.push(h.clone());
        }
        if self.sensors_screen_up() {
            self.map_dirty = true;
        }
    }

    fn sensors_screen_up(&self) -> bool {
        matches!(self.stack.last(), Some(Screen::Sensors(_) | Screen::SensorScan(_)))
    }

    /// Whether the base screen draws the connected indicator: everything whose base is
    /// [`BaseContent::Chrome`]. It gates the BLE-status repaint, so a link change never re-renders
    /// the map on the Map, Statistics or Climb screens, which omit the glyph.
    pub(crate) fn indicator_visible(&self) -> bool {
        self.base_content() == BaseContent::Chrome
    }

    /// Whether a hold gesture is charging right now. It reads the host-fed sample and `App`'s own
    /// input plane, and gates the passkey card so it never lands mid-hold.
    pub(crate) fn hold_charging(&self) -> bool {
        self.hold_progress_override.is_some_and(|p| p.charging())
            || self.input.select_hold_progress() > 0.0
            || self.input.back_hold_progress() > 0.0
    }

    /// Whether the passkey card is on the stack, which is distinct from the desired passkey level
    /// the scheduler holds: while a hold charges the level can be set with no card up yet.
    pub(crate) fn passkey_card_up(&self) -> bool {
        self.stack.iter().any(|s| matches!(s, Screen::Passkey(_)))
    }

    pub(crate) fn map_transfer_card_up(&self) -> bool {
        self.stack.iter().any(|s| matches!(s, Screen::MapTransfer(_)))
    }

    /// The scheduler's one door onto the stack. It runs a sweep with the cross-component facts the
    /// scheduler needs, once per animation pass and again whenever a host fact is posted, so an
    /// arriving card lands in the same frame unless a rule defers it.
    ///
    /// It is deliberately not covered by a render key. The scheduler already answers "did anything
    /// visible move" at this one door, including for host seams that run between two passes, where
    /// a stack-local key comparison sees nothing.
    pub(crate) fn run_card_sweep(&mut self, catalogs: &CatalogState, tracking: bool) {
        let ctx = CardCtx { now_ms: self.now_ms, hold_charging: self.hold_charging(), catalogs, tracking };
        if self.cards.sweep(&mut self.stack, &ctx) {
            self.map_dirty = true;
        }
    }

    /// Whether the top screen is one of the settings screens: the gate `SettingsMachine` uses to
    /// hold a pending save until exit. It reads the kind each screen declares in its `screens!`
    /// row, so a new settings screen cannot be forgotten here.
    pub(crate) fn top_is_settings(&self) -> bool {
        self.stack.last().is_some_and(|s| s.kind().is_settings())
    }

    /// Millis until the idle-return timeout expires, or `None` when no return is pending. At least
    /// `1` while pending, so a due return has already fired this pass and the wake is strictly in
    /// the future.
    pub(crate) fn idle_return_remaining_ms(&self, settings: &Settings, tracking: bool) -> Option<u32> {
        let timeout = settings.idle_return.timeout_ms()?;
        if !self.idle_return_pending(tracking) {
            return None;
        }
        if !self.idle_return_timing {
            return Some(timeout);
        }
        let elapsed = self.now_ms.wrapping_sub(self.last_input_ms);
        Some(timeout.saturating_sub(elapsed).max(1))
    }

    /// Whether an idle return would actually move somewhere. It gates both the idle wake and the
    /// sweep, so an already-arrived device arms no needless wake.
    fn idle_return_pending(&self, tracking: bool) -> bool {
        if self.idle_return_exempt() {
            return false;
        }
        if tracking {
            !self.is_ride_view()
        } else {
            // Not tracking: any overlay above the Home root returns to Home, except a
            // browse-exempt view. Riding with the map open without recording is a deliberate view,
            // not idleness.
            self.stack.len() > 1 && !self.stack.last().is_some_and(|s| s.caps().browse_exempt)
        }
    }

    /// Whether the top screen is exempt from the idle-return timeout: the modal cards that stay
    /// put until dismissed, the route-planning spinner, and the SD-sideload update flow. It reads
    /// the declared [`idle_exempt`](crate::screen::Caps::idle_exempt) capability, so a new modal
    /// card cannot be forgotten here.
    fn idle_return_exempt(&self) -> bool {
        self.stack.last().is_some_and(|s| s.caps().idle_exempt)
    }

    /// Whether the top screen is a deliberate ride view that must never time out while a ride is
    /// tracked. Every other screen returns to the Map on the idle timeout when tracking. It reads
    /// the declared [`ride_view`](crate::screen::Caps::ride_view) capability.
    fn is_ride_view(&self) -> bool {
        self.stack.last().is_some_and(|s| s.caps().ride_view)
    }

    /// Navigate back to where it belongs once the idle-return timeout has elapsed with no user
    /// input, once per animation pass. Not tracking, it clears every overlay back to the Home root
    /// and reseeds the screensaver. Tracking, a menu, list, settings or overview screen returns to
    /// the Map, while the deliberate ride views stay put.
    pub(crate) fn apply_idle_return(&mut self, settings: &Settings, tracking: bool) {
        let Some(timeout) = settings.idle_return.timeout_ms() else {
            self.idle_return_timing = false;
            return;
        };
        // Suspend the clock rather than ignore an expired deadline: when a long plan, upload or
        // update wait later reveals an ordinary screen, that screen gets a fresh full window.
        if !self.idle_return_pending(tracking) {
            self.idle_return_timing = false;
            return;
        }
        // A charging hold is live activity even before it resolves into a gesture.
        if self.hold_charging() {
            self.last_input_ms = self.now_ms;
            self.idle_return_timing = true;
            return;
        }
        if !self.idle_return_timing {
            self.last_input_ms = self.now_ms;
            self.idle_return_timing = true;
            return;
        }
        if self.now_ms.wrapping_sub(self.last_input_ms) < timeout {
            return;
        }
        // Past the deadline: consume it so the return fires once, not every pass hereafter. The
        // repaint needs no request, because the return moves the visible stack.
        self.last_input_ms = self.now_ms;
        if tracking {
            self.stack.truncate(1);
            let _ = self.stack.push(Screen::Map(MapScreen::new()));
        } else {
            self.stack.truncate(1);
            if let Some(Screen::Home(home)) = self.stack.first_mut() {
                home.reseed(self.now_ms);
            }
        }
    }

    /// Drain the repaint demand accumulated since the last call, resetting to [`Dirty::CLEAN`].
    /// The host calls this once per frame and then renders each plane only when its flag is set.
    ///
    /// [`overlay`](Dirty::overlay) is left `false` here: it is a level comparison, not an
    /// accumulator, and [`App::take_dirty`](crate::App::take_dirty) owns the one converter that
    /// makes it. [`region`](Dirty::region) survives only when no full-frame demand joined it, since
    /// over-redraw is safe and under-redraw is a bug.
    pub(crate) fn take_dirty(&mut self) -> Dirty {
        let full = core::mem::take(&mut self.map_dirty);
        let region = self.region_dirty.take();
        Dirty { map: full || region.is_some(), overlay: false, region: if full { None } else { region } }
    }

    /// Cancel every hold in flight, because the stack moved under it: `App`'s own recogniser now,
    /// and the host's own plane when it next drains the edge. A long press aimed at the screen that
    /// has just been replaced must not complete onto whatever replaced it.
    pub(crate) fn cancel_holds(&mut self) {
        self.input.cancel_holds();
        self.hold_cancel_pending = true;
    }

    /// Drain the pending hold-cancel edge: `true` when a gesture changed the screen stack, so any
    /// hold charging on the host's input plane is aimed at a vanished target and must be cancelled.
    /// [`handle_input`](App::handle_input) consumes it itself, so single-loop hosts never see it.
    pub(crate) fn take_hold_cancel(&mut self) -> bool {
        core::mem::take(&mut self.hold_cancel_pending)
    }
}

/// The bounding union of two rects. Both operands are screen regions, so non-empty by
/// construction (embedded-graphics 0.8 has `intersection` but no union).
fn union_rect(a: Rectangle, b: Rectangle) -> Rectangle {
    use embedded_graphics::prelude::{Point, Size};
    let x0 = a.top_left.x.min(b.top_left.x);
    let y0 = a.top_left.y.min(b.top_left.y);
    let x1 = (a.top_left.x + a.size.width as i32).max(b.top_left.x + b.size.width as i32);
    let y1 = (a.top_left.y + a.size.height as i32).max(b.top_left.y + b.size.height as i32);
    Rectangle::new(Point::new(x0, y0), Size::new((x1 - x0) as u32, (y1 - y0) as u32))
}

#[cfg(test)]
impl UiRuntime {
    /// Assert the [`new`](UiRuntime::new) boot state, field by field. The destructure is
    /// exhaustive, so a field added to the plan must state its boot value here too.
    pub(crate) fn assert_boot_state(&self) {
        let UiRuntime {
            stack,

            input,
            now_ms,
            map_dirty,
            region_dirty,
            frame_size,
            render_clip,
            resident_frame,
            next_wake_ms,
            marquee,
            last_input_ms,
            idle_return_timing,
            hold_progress_override,
            hold_cancel_pending,
            poi_scratch,
            find,
            landmarks,
            map_icons,
            settlements,
            ahead,
            corridor_scratch,
            next_ahead,
            cards,
            sensor_status,
            sensor_scan_hits,
        } = self;
        assert!(ahead.window.is_none());
        assert!(map_icons.is_empty());
        assert!(settlements.is_empty(), "no settlement candidates are held");
        assert_eq!(stack.len(), 1, "Home is the only screen");
        assert!(matches!(stack[0], Screen::Home(_)), "Home is the stack root");
        assert!(!input.overlay_active() && input.last_gesture().is_none(), "no gesture in flight");
        assert_eq!(*now_ms, 0, "the map plane's clock starts at the boot origin");
        assert!(*map_dirty, "the host's first frame must paint");
        assert!(region_dirty.is_none(), "no accumulated region demand");
        assert_eq!(*frame_size, (0, 0), "no frame rendered yet");
        assert!(render_clip.is_none() && next_wake_ms.is_none(), "no clip armed, nothing time-animating");
        assert_eq!(*marquee, Marquee::default(), "no name scrolling");
        assert!(!*resident_frame, "no host has claimed a resident frame yet");
        assert_eq!(*last_input_ms, 0, "the idle clock runs from power-on");
        assert!(*idle_return_timing, "idle time accumulates from the first pass");
        assert!(hold_progress_override.is_none() && !*hold_cancel_pending, "no hold charging or cancelled");
        assert_eq!(find.state, crate::find_place::State::Idle);
        assert_eq!(landmarks.status, crate::landmarks::Status::Idle);
        assert_eq!(poi_scratch.len(), 0, "the POI snapshot is empty");
        assert!(corridor_scratch.armed().is_none() && corridor_scratch.is_empty(), "the corridor is disarmed");
        assert!(next_ahead.request().is_none(), "the next-ahead cache asks for nothing");
        assert!(cards.is_empty(), "no card pending, no warning raised or shown");
        assert!(
            sensor_status.iter().all(|s| *s == crate::sensors::SensorStatus::default()),
            "no sensor slot has a status yet"
        );
        assert!(sensor_scan_hits.is_empty(), "no scan hits");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_in_place_matches_new() {
        UiRuntime::new().assert_boot_state();

        let mut slot = core::mem::MaybeUninit::<UiRuntime>::uninit();
        // SAFETY: `slot` is a valid, aligned, exclusively-owned region for one `UiRuntime`.
        let placed = unsafe {
            UiRuntime::init_in_place(slot.as_mut_ptr());
            slot.assume_init_ref()
        };
        placed.assert_boot_state();
    }
}
