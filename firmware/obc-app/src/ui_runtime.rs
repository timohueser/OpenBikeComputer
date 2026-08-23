//! [`UiRuntime`] — the UI-plane component behind the [`App`](crate::App) façade.
//!
//! Owns the screen stack and everything scheduled around it: the fused input plane, the map-plane
//! clock, the accumulated repaint demand (full-frame + region) and its render-clip/next-wake
//! bookkeeping, hold cancellation, and the idle-return policy. Every **host-pushed card** is the
//! [`CardScheduler`]'s: this component keeps one, feeds it the cross-component facts a sweep needs,
//! and lends it the stack through the single [`run_card_sweep`](UiRuntime::run_card_sweep) door.
//!
//! `App` stays the orchestrator: gestures still apply through [`App::apply_gesture`] (they need
//! the full [`Ctx`](crate::screen::Ctx) over settings/activity/catalogs) and
//! [`App::advance_animations`] still sequences the per-pass sweeps, but every stack/dirty/timer
//! mutation lands in this component. Cross-component facts a rule needs (is a ride tracking? does
//! this durable id still resolve?) arrive as parameters — this component never reaches back into
//! the others.
//!
//! [`App::apply_gesture`]: crate::App::apply_gesture
//! [`App::advance_animations`]: crate::App::advance_animations

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
use crate::screen::{self, BaseContent, HomeScreen, MapScreen, PoiScratch, ReaderNeed, Screen, Stack};
use crate::settings::{DateTime, Settings};

/// The UI-plane state + policy component. See the module docs; field-level invariants are on
/// each field (they are `App`'s former fields, moved verbatim).
pub(crate) struct UiRuntime {
    /// The screen stack (root = Home). The top screen receives input; drawing starts from the
    /// topmost opaque screen so overlays composite over the map.
    pub(crate) stack: Stack,
    /// The input + overlay plane: gesture recognizer, long-press hint overlay, live hold-progress.
    /// Split off `App` so the firmware can run it on a *separate, high-priority* executor that
    /// preempts the map render. `App` keeps this one for the [`handle_input`](App::handle_input)
    /// path; the two-plane firmware drives its own and feeds gestures back through
    /// [`apply_gesture`](App::apply_gesture).
    pub(crate) input: InputPlane,
    /// Millis at the last [`handle_input`](App::handle_input) /
    /// [`advance_animations`](App::advance_animations) — the **map plane's** clock, distinct from
    /// the input plane's own clock.
    pub(crate) now_ms: u32,
    /// Accumulated **map-plane** repaint demand since the last [`take_dirty`](App::take_dirty),
    /// drained once per frame. Starts `true` so the host's first frame paints. (The overlay flag
    /// isn't accumulated here — it's derived from the live hold-bulge state at drain time.)
    pub(crate) map_dirty: bool,
    /// A one-shot **overlay** repaint demand from something other than the hold bulge — today only
    /// the Recalculating freeze flipping (issue #1146, P2), whose banner appears and clears on the
    /// overlay plane. The bulge's own demand is derived from its live state at drain time
    /// ([`InputPlane::take_overlay_dirty`]); a freeze edge has no such continuous state to derive
    /// from, so it is latched here and OR'd in at [`take_dirty`](UiRuntime::take_dirty).
    pub(crate) overlay_edge: bool,
    /// Accumulated **region-scoped** repaint demand (#500 follow-up): the union of every
    /// region-carrying screen-tick change since the last drain — the nav-planning spinner's
    /// needle disc. Kept apart from [`map_dirty`](App::map_dirty) so the two can't blur: any
    /// full-frame demand (every other `map_dirty = true` site) overrides this at
    /// [`take_dirty`](App::take_dirty), and region ticks never set `map_dirty` — see the drain
    /// for the fold.
    pub(crate) region_dirty: Option<Rectangle>,
    /// Panel size (device px) of the last rendered frame, recorded by
    /// [`render_map_timed`](App::render_map_timed) — what
    /// [`advance_animations`](App::advance_animations) hands the screen ticks so a reported
    /// [`ScreenTick::region`](screen::ScreenTick::region) is sized to the real panel. `(0, 0)`
    /// until the first frame; region reporting abstains (full repaint) until then. Narrowed to
    /// `i16` per dimension (#802's resident-RAM offset, the #810 u16-repack precedent): a panel
    /// dimension is a few hundred pixels, bounded far below `i16::MAX`, and this pairs the four
    /// bytes saved against the component boundaries' new tail padding.
    pub(crate) frame_size: (i16, i16),
    /// One-shot clip for the **next** [`render_map_timed`](App::render_map_timed): the host that
    /// drained a region-scoped [`Dirty`](crate::Dirty) sets it via
    /// [`set_render_clip`](App::set_render_clip) so the frame's `Canvas` rejects whole primitives
    /// outside the region — the draw-call machinery (glyph decode, scanline iterators) a
    /// pixel-level framebuffer clip can't skip. Taken (cleared) by the render, so a host that
    /// never sets it — the sim, the tests — always draws full frames.
    pub(crate) render_clip: Option<Rectangle>,
    /// The soonest timed-redraw deadline across the visible stack, in millis from the last
    /// [`advance_animations`](App::advance_animations) — the min-fold of each screen's
    /// [`ScreenTick::next_wake_ms`](screen::ScreenTick::next_wake_ms), stored there and read back by
    /// [`ms_until_next_wake`](App::ms_until_next_wake). `None` when nothing is time-animating.
    pub(crate) next_wake_ms: Option<u32>,
    /// Map-plane millis of the last **user input** — any recognised gesture (see
    /// [`apply_gesture`](App::apply_gesture)), plus a per-tick refresh while a hold charges (a
    /// gesture in progress counts as activity). Drives the **idle-return** timeout
    /// ([`apply_idle_return`](App::apply_idle_return)): after
    /// [`idle_return`](crate::settings::Settings::idle_return) millis of silence the UI navigates
    /// itself back to where it belongs. Deliberately advanced **only** on input — a GPS fix, a BLE
    /// event, or a timed repaint must not reset it. Seeded to `0` (the boot origin), so the idle
    /// clock runs from power-on until the first touch.
    pub(crate) last_input_ms: u32,
    /// Whether idle time is currently accumulating. A screen/circumstance for which no idle return
    /// is eligible suspends the clock; the first eligible pass after that suspension starts a fresh
    /// full window. This keeps a long modal operation from donating its elapsed time to the ordinary
    /// screen that replaces it.
    pub(crate) idle_return_timing: bool,
    /// Host-supplied Select hold-progress (0.0–1.0) for the in-screen confirm fills (the factory
    /// Reset bar; [`RideControl`](crate::screen::RideControl) confirm rows). `None` on the
    /// single-loop hosts (the render reads `App`'s own [`InputPlane`]); the **two-plane firmware**
    /// feeds live progress in each frame via [`set_hold_progress`](App::set_hold_progress), since
    /// its holds live on a separate plane `App`'s own never sees.
    pub(crate) hold_progress_override: Option<f32>,
    /// Set by [`apply_gesture`](App::apply_gesture) whenever a gesture **changed the screen
    /// stack**: any hold charging at that moment was aimed at a screen that is no longer the
    /// top, so it must be cancelled rather than delivered to whatever replaced it (a hold aimed
    /// at a popup's "Finish & new" must never land on the Route menu's hold-to-delete footer —
    /// issue #480). [`handle_input`](App::handle_input) drains it inline (cancelling `input`'s
    /// holds and dropping stray `Hold`/`BackHold`s later in the same batch); the two-plane
    /// firmware drains it via [`take_hold_cancel`](App::take_hold_cancel) and cancels its own
    /// input plane's recogniser.
    pub(crate) hold_cancel_pending: bool,
    /// The single POI-list snapshot buffer (issue #425), threaded into the draw context as
    /// [`Render::poi_scratch`]. Held once here rather than per-screen so the ~800 B doesn't multiply
    /// across the screen-stack union (see [`PoiScratch`](crate::screen::PoiScratch)). Filled lazily
    /// by the POI list screen's first draw; invalidated in [`apply_gesture`](App::apply_gesture)
    /// when a POI list opens, so re-entering a category re-queries.
    pub(crate) poi_scratch: screen::PoiScratch,
    /// The single route-corridor snapshot buffer (epic #946, U2) — the map POIs near the route
    /// ahead, frozen on take. Held once here for the same reason as
    /// [`poi_scratch`](UiRuntime::poi_scratch): it must not multiply across the screen-stack union
    /// (see [`CorridorScratch`](crate::corridor::CorridorScratch)). Disarmed until a screen asks
    /// for it, so a device that never opens the Up-ahead list never runs the query.
    pub(crate) corridor_scratch: CorridorScratch,
    /// The per-category **"next ahead" cache** (epic #946, U5) — the distilled map-POI half of the
    /// six `Next: <category>` stat tiles, harvested out of [`corridor_scratch`](Self::corridor_scratch)
    /// on its own progress-keyed refresh policy. App-owned for the same #425 reason as the two
    /// snapshots above (a `Screen` variant is a slot in a `.bss` union), and quiet — asking for
    /// nothing — unless such a tile is on the grid while the Statistics screen is up.
    pub(crate) next_ahead: NextAhead,
    /// Every host-pushed modal card: the named pending slots, the policy table, and the one sweep
    /// that lands them (see [`CardScheduler`]). Held here because the stack is here — the scheduler
    /// borrows it for the length of [`run_card_sweep`](UiRuntime::run_card_sweep) and never longer.
    pub(crate) cards: CardScheduler,
    /// The per-slot BLE **sensor status** (BLE sensors epic #707, SE7): HR / power / cadence
    /// connection phase + battery + live tick, fed each pass by the host through
    /// [`set_sensor_status`](App::set_sensor_status) and drawn only by the Sensors settings screen.
    /// Held off [`AppState`] like [`ble_passkey`](App::ble_passkey) so feeding it never gates a map
    /// redraw on a non-sensor screen; the Sensors screen's repaint is gated on an actual change to a
    /// slot while it is up.
    pub(crate) sensor_status: [crate::sensors::SensorStatus; crate::settings::SENSOR_SLOTS],
    /// The live **sensor scan hits** (SE7): the sensors discovered while the scan-list screen runs a
    /// scan, fed by the host through [`set_sensor_scan_hits`](App::set_sensor_scan_hits). Empty
    /// outside a scan; replaced wholesale each pass while one runs.
    pub(crate) sensor_scan_hits: crate::sensors::SensorScanHits,
}

impl UiRuntime {
    define_placement_constructors!(
        /// The boot state: the Home root on the stack, first frame dirty, nothing pending.
        pub(crate) fn new();
        /// Initialize `slot` **in place** to the [`new`](UiRuntime::new) state — the placement path
        /// the firmware boots through (the screen stack and POI scratch are KB-scale; nothing here
        /// may form a by-value `UiRuntime` on the stack).
        pub(crate) unsafe fn init_in_place;
        fields {
            stack: Stack::new(),
            input: InputPlane::new(),
            now_ms: 0,
            // Force the host's first frame: nothing has been drawn yet, so the map is dirty.
            map_dirty: true,
            overlay_edge: false,
            region_dirty: None,
            frame_size: (0, 0),
            render_clip: None,
            next_wake_ms: None,
            last_input_ms: 0,
            idle_return_timing: true,
            hold_progress_override: None,
            hold_cancel_pending: false,
            poi_scratch: PoiScratch::new(),
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

    /// Advance the map-plane clock to `now_ms` and poll each visible screen's timers
    /// ([`Screen::tick_timers`]) in one pass: any time-driven repaint that fired dirties the map —
    /// so a screen surfaces its own timed-refresh rather than the host re-rendering on a blind
    /// heartbeat — and the soonest residual deadline is stored for
    /// [`App::ms_until_next_wake`](crate::App::ms_until_next_wake). Cheap: a clock comparison per
    /// drawn screen, over the same `base..` range the render draws. The card sweep and the idle-return
    /// sweep are sequenced by [`App::advance_animations`](crate::App::advance_animations) right
    /// after this.
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
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let (w, h) = (self.frame_size.0 as i32, self.frame_size.1 as i32);
        let mut next_wake = None;
        for scr in self.stack.iter_mut().skip(base) {
            let tick = scr.tick_timers(self.now_ms, now, ms_to_next_minute, settings, w, h, pan_active, tracking);
            // A change that promises a containing region accumulates apart from the full-frame
            // demand (#500 follow-up): `take_dirty` folds the two — any `map_dirty` overrides
            // every region, so a region-clipped repaint happens only when region ticks were the
            // *sole* dirt since the last drain.
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

    /// The [`BaseContent`] of the base (lowest *opaque*) screen — the single declared fact the
    /// live-data / map-I/O / indicator gates read instead of open-coding a `matches!` on the enum.
    /// The base is the lowest opaque drawn screen, so an overlay over a riding view still reports the
    /// riding view's content.
    fn base_content(&self) -> BaseContent {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        self.stack.get(base).map(|s| s.caps().base).unwrap_or(BaseContent::Chrome)
    }

    /// Whether the base screen shows live sensor data (user fix / ride accumulators) — the Map and
    /// the live-riding views ([`BaseContent::Map`] / [`LiveRiding`](BaseContent::LiveRiding)) do, so
    /// a fresh fix must redraw them; Home and the menus (chrome) don't.
    pub(crate) fn shows_live_data(&self) -> bool {
        self.base_content() != BaseContent::Chrome
    }

    /// Whether the base (lowest opaque) screen draws the **map** — any [`BaseContent::Map`] screen.
    /// A render-on-demand host polls this to skip the whole map pipeline on a non-map frame: don't
    /// build the `Reader` (an SD style-table parse + its stack spike), pass `None` to
    /// [`render_map_timed`](App::render_map_timed), and a menu / Home redraw draws only its own
    /// chrome with zero map I/O.
    pub(crate) fn base_draws_map(&self) -> bool {
        self.base_content() == BaseContent::Map
    }

    /// Whether the base (lowest opaque) screen **wants the rain overlay** — its declared
    /// [`Caps::rain_overlay`](crate::screen::Caps::rain_overlay), true only for the WX11 rain map.
    /// The frame's rain lease is dropped when this is false, so the precipitation raster is a
    /// property of the screen the rider is on rather than of the host's weather mount: leaving the
    /// rain map leaves the Map clean with nothing to reset, and no rain *tile* is decoded on a
    /// frame no screen would draw it on (a property that starts paying storage reads once the board
    /// renders rain — today `obc-sim` is the only host that leases one).
    pub(crate) fn base_wants_rain(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        self.stack.get(base).is_some_and(|s| s.caps().rain_overlay)
    }

    /// Whether the frame needs the streamed-map [`Reader`] built and passed to
    /// [`render_map_timed`](App::render_map_timed) — a superset of [`base_draws_map`](App::base_draws_map).
    /// Chosen from the base screen's declared [`ReaderNeed`]: map-base screens always need it; the
    /// **POI list** screen (issue #425) does too, but only until it has taken its one-shot snapshot; and
    /// the **POI detail** screen (issue #444) does until it has resolved its one hours read. Both
    /// take their one-shot read in the pre-draw [`prepare`](crate::screen::Screen::prepare) pass off
    /// the `Reader`, so a render-on-demand host (the board's two-plane loop) must build the `Reader`
    /// on the frame each one-shot read is taken. Once the list's
    /// [`poi_snapshot_pending`](App::poi_snapshot_pending) is false — or the detail's schedule cache
    /// has resolved — the screen draws from its frozen state with no `Reader`, so the host skips the
    /// build again.
    ///
    /// The sim's `render_frame` always passes `Some(reader)`, so it never consults this — only the
    /// board host does, keeping its per-frame `Reader` build (and stack spike) off every non-map,
    /// already-resolved frame.
    pub(crate) fn base_needs_reader(&self) -> bool {
        // The route-corridor snapshot (epic #946, U2) is armed by a screen but owned by the App, so
        // its need is a **request**, not a `ReaderNeed` row: a screen that wants an Up-ahead list
        // keeps the `Reader` built until the one query lands, then stops asking — the same one-shot
        // energy pattern as the two POI rows below. Disarmed (the normal state) this is free.
        if self.corridor_scratch.pending() {
            return true;
        }
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let Some(scr) = self.stack.get(base) else { return false };
        match scr.caps().reader {
            ReaderNeed::Always => true,
            ReaderNeed::PoiSnapshot => matches!(scr, Screen::PoiList(s) if self.poi_snapshot_pending(s)),
            // The detail's hours read runs in `prepare` off the `Reader`; keep it built until it lands.
            ReaderNeed::PoiHours => matches!(scr, Screen::PoiDetail(s) if s.hours_pending()),
            ReaderNeed::Never => false,
        }
    }

    /// Run the base (lowest-opaque) screen's pre-draw acquisition (#803): hand it the frame's
    /// `Reader`, streamed route, and fix so it resolves reader-backed state (POI snapshot / hours,
    /// or Skip-ahead geometry) into immutable prepared state before the draw loop. Called by
    /// [`render_map_timed`](App::render_map_timed) ahead of building the draw context, so `Render`
    /// carries the POI scratch read-only and draw stays side-effect-free.
    #[allow(clippy::too_many_arguments)] // the per-frame prepare snapshot, one value per field
    pub(crate) fn prepare_base(
        &mut self,
        reader: Option<&Reader>,
        route: Option<&obc_route::RouteReader>,
        user_fix: Option<Fix>,
        active_route: Option<usize>,
        progress_m: u32,
        route_total_m: u32,
        detour_preview: &[(i32, i32)],
    ) {
        // The App-owned corridor snapshot (epic #946, U2) resolves first: it belongs to no single
        // screen (U3's list and U5's stat fields both read it), so it runs at the boundary rather
        // than inside one screen's `prepare`. A no-op unless a screen armed it.
        self.corridor_scratch.prepare(reader, route);
        // …and if the snapshot that just landed is the one the `Next: <category>` cache asked for
        // (U5), distil it here — the one place a fresh snapshot is guaranteed to exist. A no-op
        // whenever the scratch is serving a screen instead: `harvest` only takes its own key.
        if let Some(key) = self.next_ahead.request() {
            if self.corridor_scratch.holds(key) {
                self.next_ahead.harvest(key, self.corridor_scratch.entries());
            }
        }
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        if let Some(scr) = self.stack.get_mut(base) {
            let mut px = screen::Prepare {
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
        }
    }

    /// Point the App-owned corridor snapshot at whatever the **stack** currently wants (epic #946,
    /// U3). The Up-ahead screen never queries anything itself: it declares a
    /// [`CorridorKey`](crate::corridor::CorridorKey) (its filter + the progress anchor frozen at
    /// entry) through [`Screen::corridor_request`], and this arms it. Everything the lifecycle needs
    /// falls out of that one declaration:
    ///
    /// * **entry** — a screen appears that wants a key ⇒ armed (and, on a *fresh* open,
    ///   [`invalidate`](crate::corridor::CorridorScratch::invalidate)d, so re-entering re-takes the
    ///   identical key: the "re-enter refreshes" half of the #115 contract);
    /// * **a filter change** — the key changes ⇒ the stale rows drop and the query re-runs;
    /// * **riding on** — the key does *not* change (the anchor is frozen) ⇒ nothing re-runs;
    /// * **exit** (Back, or the idle return — both *pop* the list off the stack) — nobody wants a
    ///   key ⇒ disarmed, and the reader-build seam goes quiet;
    /// * **buried** — a host-pushed card (a passkey, a warning) on top is *not* an exit: the scan
    ///   covers the whole stack, so the list's request is still found and the scratch stays armed
    ///   for the uncover.
    ///
    /// Cheap enough to call whenever the stack may have moved: a scan of ≤ [`MAX_DEPTH`] slots and
    /// an idempotent `arm`. The **query** still runs only in the pre-draw `prepare` boundary.
    ///
    /// [`MAX_DEPTH`]: crate::screen::MAX_DEPTH
    /// U5 adds a **second, lower-priority** requester: with no screen asking, the
    /// [`NextAhead`](crate::next_ahead::NextAhead) cache may want one single-category snapshot to
    /// refresh a `Next: <category>` tile. A screen always wins — the Up-ahead list is a thing the
    /// rider is *looking at*, a stat tile's refresh can wait a screen visit — and a cache request
    /// never counts as a "fresh open" (there is no screen entry to re-take for).
    ///
    /// The two can never fight over the buffer's *contents*: the cache only asks while the
    /// Statistics screen is the base one (so never while the Up-ahead list is up, including U4's
    /// `Waypoints only` scope where that screen deliberately asks for nothing), and
    /// [`NextAhead::harvest`](crate::next_ahead::NextAhead) only accepts a snapshot taken for its own
    /// key — so a foreign snapshot can no more land in a tile than a tile's can land in the list.
    pub(crate) fn reconcile_corridor(&mut self, fresh_open: bool) {
        match self.stack.iter().rev().find_map(|s| s.corridor_request()) {
            Some(key) => {
                self.corridor_scratch.arm(key);
                if fresh_open {
                    self.corridor_scratch.invalidate();
                }
            }
            None => match self.next_ahead.request() {
                Some(key) => self.corridor_scratch.arm(key),
                None => self.corridor_scratch.disarm(),
            },
        }
    }

    /// Re-decide what the `Next: <category>` tiles need (epic #946, U5) and re-point the corridor
    /// scratch at it. Called once per pass from
    /// [`advance_animations`](crate::App::advance_animations) — the one hook every host runs — with
    /// the facts the policy needs: the rider's field selection, the active route, and matched
    /// progress. Everything else (the triggers, the round-robin, the one-category-per-query rule)
    /// lives in [`NextAhead::reconcile`](crate::next_ahead::NextAhead).
    ///
    /// The tiles only exist on the Statistics grid, so the request is scoped to that screen being
    /// the one drawn: elsewhere the cache asks for nothing, the scratch disarms, and the reader seam
    /// is as quiet as before this feature existed.
    pub(crate) fn reconcile_next_ahead(&mut self, settings: &Settings, active_route: Option<usize>, progress_m: u32) {
        let mut placed = obc_reader::PoiCategorySet::EMPTY;
        for f in settings.stat_fields.as_slice() {
            if let Some(cat) = f.category() {
                placed = placed.with(cat);
            }
        }
        self.next_ahead.reconcile(placed, self.stats_grid_shown(), active_route, progress_m);
        self.reconcile_corridor(false);
    }

    /// Whether the **Statistics** screen — the only place a `Next: <category>` tile draws — is the
    /// base (lowest opaque) screen this pass. Deliberately not "is anywhere on the stack": a tile
    /// behind a menu isn't being read, and the query it would keep warm costs a card spin-up.
    fn stats_grid_shown(&self) -> bool {
        let base = self.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        matches!(self.stack.get(base), Some(Screen::Statistics(_)))
    }

    /// Whether the given POI list screen still needs a `Reader` at draw — its category's snapshot
    /// hasn't been taken into the shared scratch yet. Drives [`base_needs_reader`](App::base_needs_reader).
    pub(crate) fn poi_snapshot_pending(&self, screen: &crate::screen::PoiListScreen) -> bool {
        !self.poi_scratch.holds(screen.category())
    }

    /// Feed the host's per-slot **sensor status** ([`SensorStatus`](crate::sensors::SensorStatus)) —
    /// the central manager's HR / power / cadence connection phase + battery + live tick, distilled to
    /// app vocabulary and pushed each pass (the board's `ble::sensors` snapshot, or the sim's fake
    /// manager). Stored app-side like [`set_ble_status`](App::set_ble_status); no radio type crosses
    /// the seam. Up to [`SENSOR_SLOTS`](crate::settings::SENSOR_SLOTS) slots are copied (extra ignored).
    ///
    /// A change **while the Sensors screen is up** dirties the map so the status lines repaint; on any
    /// other screen the status isn't drawn, so an update — fed every pass — repaints nothing.
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

    /// Feed the host's live **sensor scan hits** ([`SensorScanHit`](crate::sensors::SensorScanHit)) —
    /// the sensors discovered while the scan-list screen runs a scan. Replaces the resident list
    /// wholesale (up to [`SCAN_HITS_MAX`](crate::sensors::SCAN_HITS_MAX)); an empty slice clears it
    /// (the host feeds `&[]` when no scan is active). A change while the scan screen is up dirties the
    /// map so a freshly-found sensor appears without waiting for another input.
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

    /// Whether the Sensors settings screen (its row list or a scan list) is the top screen — gates the
    /// sensor-seam repaint so a status/scan-hit update dirties the map only where it's drawn.
    fn sensors_screen_up(&self) -> bool {
        matches!(self.stack.last(), Some(Screen::Sensors(_) | Screen::SensorScan(_)))
    }

    /// Whether the base (lowest opaque) screen draws the connected indicator — Home, or any framed
    /// screen with a title bar (a menu / list / prompt): everything whose base is
    /// [`BaseContent::Chrome`] rather than a full-screen riding view. Gates
    /// [`set_ble_status`](App::set_ble_status)'s repaint so a link change never re-renders the map
    /// on the Map / Statistics / Climb screens, which deliberately omit the glyph.
    pub(crate) fn indicator_visible(&self) -> bool {
        self.base_content() == BaseContent::Chrome
    }

    /// Whether a hold gesture is charging right now — either button down, its long-press not yet
    /// fired. Reads the host-fed Select progress ([`set_hold_progress`](App::set_hold_progress), the
    /// two-plane firmware) and `App`'s own input plane (the single-loop hosts). Gates the host-pushed
    /// passkey card's open/close so it never lands mid-hold.
    pub(crate) fn hold_charging(&self) -> bool {
        self.hold_progress_override.is_some_and(|p| p > 0.0)
            || self.input.select_hold_progress() > 0.0
            || self.input.back_hold_progress() > 0.0
    }

    /// Whether the **passkey card** is on the stack (epic #447) — the modal-priority query, distinct
    /// from the desired passkey *level* the scheduler holds: while a hold charges the level can be
    /// set with no card up yet. A stack read, never a mutation.
    pub(crate) fn passkey_card_up(&self) -> bool {
        self.stack.iter().any(|s| matches!(s, Screen::Passkey(_)))
    }

    /// Whether the **map-transfer card** is on the stack (issue #927) — what
    /// [`App::map_transfer_card_up`](crate::App::map_transfer_card_up) exposes to the board's
    /// transfer gate.
    pub(crate) fn map_transfer_card_up(&self) -> bool {
        self.stack.iter().any(|s| matches!(s, Screen::MapTransfer(_)))
    }

    /// **The scheduler's one door onto the stack.** Runs a
    /// [`CardScheduler::sweep`](crate::card_scheduler::CardScheduler::sweep) with the
    /// cross-component facts it needs, and folds its single "something visible moved" answer into
    /// the map's repaint demand. Called once per
    /// [`advance_animations`](crate::App::advance_animations) pass and again whenever a host fact is
    /// posted, so an arriving card lands in the same frame unless a rule defers it.
    pub(crate) fn run_card_sweep(&mut self, catalogs: &CatalogState, tracking: bool) {
        let ctx = CardCtx { now_ms: self.now_ms, hold_charging: self.hold_charging(), catalogs, tracking };
        if self.cards.sweep(&mut self.stack, &ctx) {
            self.map_dirty = true;
        }
    }

    /// Whether the top (input-receiving) screen is one of the settings screens — the gate
    /// [`take_settings_dirty`](App::drain_host_commands) uses to hold a pending save until exit.
    /// Reads the [`ScreenKind`](crate::screen::ScreenKind) each screen declares in its `screens!`
    /// table row, so a new settings screen can't be forgotten here.
    pub(crate) fn top_is_settings(&self) -> bool {
        self.stack.last().is_some_and(|s| s.kind().is_settings())
    }

    /// Millis until the idle-return timeout expires, or `None` when no return is pending — the
    /// mechanism is off ([`Never`](crate::settings::IdleReturn::Never)), a modal exemption is up, or
    /// we're already at the target screen (Home when idle, a ride view while tracking), so no idle
    /// wake is owed. At least `1` while pending, so a due return has already fired this pass and the
    /// wake is strictly future.
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

    /// Whether an idle return would actually *move* somewhere — false when a modal exemption is up,
    /// or we're already where the timeout would land (the Home root when not tracking, a deliberate
    /// ride view while tracking). Gates both the idle wake and the sweep so an already-arrived
    /// device arms no needless wake and re-checks nothing each tick.
    fn idle_return_pending(&self, tracking: bool) -> bool {
        if self.idle_return_exempt() {
            return false;
        }
        if tracking {
            !self.is_ride_view()
        } else {
            // Not tracking: any overlay above the Home root would return to Home — **except** a
            // browse-exempt view (the route-less browse Map, Menu → Map). Riding with the map open
            // without recording is a deliberate view, not idleness, so it's exempt (the declared
            // `browse_exempt` capability) just like a ride view is mid-ride.
            self.stack.len() > 1 && !self.stack.last().is_some_and(|s| s.caps().browse_exempt)
        }
    }

    /// Whether the current top screen is **exempt** from the idle-return timeout — the modal cards
    /// that must stay put until dismissed (the BLE passkey card, the route-received / -updated /
    /// -swap / trip-received popups, the #504 sensor/storage warning card), the route-planning spinner (a
    /// multi-second wait that isn't idleness), and the whole SD-sideload update flow (a card/wait the
    /// rider is acting on — never yank it Home mid-flow). Reads the top screen's declared
    /// [`idle_exempt`](crate::screen::Caps::idle_exempt) capability, so a new modal card can't be
    /// forgotten here. While one is up, no idle return fires and no idle wake is armed.
    fn idle_return_exempt(&self) -> bool {
        self.stack.last().is_some_and(|s| s.caps().idle_exempt)
    }

    /// Whether the current top screen is one of the **deliberate ride views** that must never time
    /// out while a ride is being tracked — the Map (the ride base), Statistics, Climb, and the
    /// Paused / Ride-control page. A rider sitting on any of these is watching live ride data, not
    /// lost in a menu. Every *other* screen (menus, lists, settings, route overview) returns to the
    /// Map on the idle timeout when tracking. Reads the top screen's declared
    /// [`ride_view`](crate::screen::Caps::ride_view) capability.
    fn is_ride_view(&self) -> bool {
        self.stack.last().is_some_and(|s| s.caps().ride_view)
    }

    /// Navigate "back to where it belongs" once the idle-return timeout ([`idle_return`]) has
    /// elapsed with no user input — the app-level counterpart to the popups' timeout-dismiss sweep,
    /// run once per [`advance_animations`](App::advance_animations) pass.
    ///
    /// - **Not tracking a ride:** from any screen *except* the route-less browse Map (Menu → Map, a
    ///   deliberate view — see [`idle_return_pending`](App::idle_return_pending)), clear every
    ///   overlay back to the Home root and reseed the screensaver backdrop (as a manual return does).
    /// - **Tracking a ride:** a menu / list / settings / overview screen returns to the Map (the
    ///   ride base). The deliberate ride views ([`is_ride_view`](App::is_ride_view)) stay put.
    ///
    /// Never fires while the timeout is disabled ([`Never`]), a modal exemption is up
    /// ([`idle_return_exempt`](App::idle_return_exempt)), a hold is charging (a gesture in progress
    /// is activity — deferred a tick, like the popup sweeps), or we're already at the target screen.
    ///
    /// [`idle_return`]: crate::settings::Settings::idle_return
    /// [`Never`]: crate::settings::IdleReturn::Never
    pub(crate) fn apply_idle_return(&mut self, settings: &Settings, tracking: bool) {
        let Some(timeout) = settings.idle_return.timeout_ms() else {
            self.idle_return_timing = false;
            return;
        };
        // No return is eligible while already at the destination/deliberate view or while an
        // idle-exempt modal is up. Suspend rather than merely ignoring the expired absolute
        // deadline: when a long plan/upload/update wait later reveals an ordinary screen, that
        // screen receives a fresh full window instead of being swept away immediately.
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
        // Past the deadline: consume it so the return fires once, not every pass hereafter.
        self.last_input_ms = self.now_ms;
        self.map_dirty = true;
        if tracking {
            // Mid-ride, on a non-ride screen: return to the Map (the ride base).
            self.stack.truncate(1); // drop back toward the root…
            let _ = self.stack.push(Screen::Map(MapScreen::new())); // …then land on the Map
        } else {
            // Not tracking: clear to the Home root and reseed the screensaver (as a manual return does).
            self.stack.truncate(1);
            if let Some(Screen::Home(home)) = self.stack.first_mut() {
                home.reseed(self.now_ms);
            }
        }
    }

    /// Drain the repaint demand accumulated since the last call, resetting to [`Dirty::CLEAN`]. The
    /// host calls this **once per frame** after [`tick`](App::tick) +
    /// [`handle_input`](App::handle_input), then renders each plane only when its flag is set — the
    /// render-on-demand loop.
    ///
    /// [`map`](Dirty::map) accumulates every map-affecting mutation since the last drain.
    /// [`overlay`](Dirty::overlay) is *derived* from the live hold-bulge state: set while the bulge
    /// is live, plus one trailing frame after it goes quiet so the host can clear it off Layer 2.
    /// That trailing edge is tracked across calls, so draining twice in one frame swallows it — call
    /// exactly once per frame.
    ///
    /// [`region`](Dirty::region) carries the accumulated region-scoped tick demand — but only when
    /// no full-frame demand joined it since the last drain: a set `map_dirty` covers any region, so
    /// the region folds away and the host full-repaints (over-redraw is safe; under-redraw is a bug).
    pub(crate) fn take_dirty(&mut self) -> Dirty {
        let full = core::mem::take(&mut self.map_dirty);
        let region = self.region_dirty.take();
        Dirty {
            map: full || region.is_some(),
            // Drain the bulge's trailing edge unconditionally (it must be called exactly once per
            // frame, whatever else is on the overlay), then fold in a freeze flip.
            overlay: self.input.take_overlay_dirty() | core::mem::take(&mut self.overlay_edge),
            region: if full { None } else { region },
        }
    }

    /// Drain the pending hold-cancel edge (see `hold_cancel_pending`): `true` when a gesture
    /// changed the screen stack since the last drain, i.e. any hold charging on the host's input
    /// plane is aimed at a vanished target and must be cancelled
    /// ([`InputPlane::cancel_holds`](crate::InputPlane::cancel_holds)). The two-plane firmware
    /// checks this after each drained gesture; [`handle_input`](App::handle_input) consumes it
    /// itself, so single-loop hosts never see it.
    pub(crate) fn take_hold_cancel(&mut self) -> bool {
        core::mem::take(&mut self.hold_cancel_pending)
    }
}

/// The bounding union of two rects — how `advance_animations` folds multiple region-scoped tick
/// changes into one containing dirty region (embedded-graphics 0.8 has `intersection` but no
/// union). Both operands are screen regions, so non-empty by construction.
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
    /// Assert the [`new`](UiRuntime::new) boot state, field by field — including the Home root the
    /// shared post block seeds. The destructure is exhaustive, so a field added to the plan must
    /// state its boot value here too.
    pub(crate) fn assert_boot_state(&self) {
        let UiRuntime {
            stack,
            input,
            now_ms,
            map_dirty,
            overlay_edge,
            region_dirty,
            frame_size,
            render_clip,
            next_wake_ms,
            last_input_ms,
            idle_return_timing,
            hold_progress_override,
            hold_cancel_pending,
            poi_scratch,
            corridor_scratch,
            next_ahead,
            cards,
            sensor_status,
            sensor_scan_hits,
        } = self;
        assert_eq!(stack.len(), 1, "Home is the only screen");
        assert!(matches!(stack[0], Screen::Home(_)), "Home is the stack root");
        assert!(!input.overlay_active() && input.last_gesture().is_none(), "no gesture in flight");
        assert_eq!(*now_ms, 0, "the map plane's clock starts at the boot origin");
        assert!(*map_dirty, "the host's first frame must paint");
        assert!(!*overlay_edge && region_dirty.is_none(), "no accumulated overlay or region demand");
        assert_eq!(*frame_size, (0, 0), "no frame rendered yet");
        assert!(render_clip.is_none() && next_wake_ms.is_none(), "no clip armed, nothing time-animating");
        assert_eq!(*last_input_ms, 0, "the idle clock runs from power-on");
        assert!(*idle_return_timing, "idle time accumulates from the first pass");
        assert!(hold_progress_override.is_none() && !*hold_cancel_pending, "no hold charging or cancelled");
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

    /// The placement path must land exactly the state the by-value path builds — Home seeded by
    /// the one shared post block included.
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
