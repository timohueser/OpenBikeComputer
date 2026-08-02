//! [`UiRuntime`] — the UI-plane component behind the [`App`](crate::App) façade.
//!
//! Owns the screen stack and everything scheduled around it: the fused input plane, the map-plane
//! clock, the accumulated repaint demand (full-frame + region) and its render-clip/next-wake
//! bookkeeping, hold cancellation, the idle-return policy, and the **modal reconciliation** rules
//! for every host-pushed card (the BLE passkey card, the route-upload popups, the advisory warning
//! card, the post-update toast, the DFU answer landings) — the delivery/defer/replace discipline
//! those cards share (never land mid-hold, the passkey card outranks, timeout = dismiss).
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

use crate::catalog_state::CatalogState;
use crate::corridor::CorridorScratch;
use crate::dirty::Dirty;
use crate::input_plane::InputPlane;
use crate::next_ahead::NextAhead;
use crate::screen::{self, BaseContent, HomeScreen, MapScreen, PoiScratch, ReaderNeed, Screen, Stack, WarningFlags};
use crate::settings::{DateTime, Settings};

/// One committed route upload, as [`App::apply_event`](crate::App::apply_event)
/// queues it for prompt delivery (epic #447, P4).
#[derive(Debug, Clone, Copy)]
pub(crate) struct UploadEvent {
    /// The committed route's durable object id — resolved to a catalog index at *delivery* time.
    pub(crate) id: u16,
    /// The upload replaced the **actively-navigated** route (snapshotted at arrival): the
    /// info-only "ROUTE UPDATED" card instead of a choice prompt — adoption already happened.
    pub(crate) active_replace: bool,
    /// The route's mini elevation sparkline ([`obc_route::elevation_sparkline`]), built by the host
    /// from the just-committed OBCR at commit time (#682) — `None` when the route carries no
    /// elevation. Carried with the event so the idle "ROUTE RECEIVED" card can draw it; the
    /// mid-ride swap / active-replace variants ignore it.
    pub(crate) elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
}

/// What the single pending-upload slot holds: a committed **route** upload or a committed **trip**
/// upload. One slot for both kinds keeps the locked most-recent-wins rule across the whole popup
/// family — and since a trip object always arrives *after* its member routes (it references their
/// ids, so every client sends the routes first), a burst of route events capped by the trip event
/// naturally collapses to the one "TRIP RECEIVED" prompt.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingUpload {
    Route(UploadEvent),
    /// The committed trip's durable object id — validated against the (already re-fed) trip
    /// catalog at delivery time.
    Trip {
        id: u16,
    },
}

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
    /// Whether idle time is currently accumulating. A screen/circumstance for
    /// which no idle return is eligible suspends the clock; the first eligible
    /// pass after that suspension starts a fresh full window. This is what keeps
    /// a long modal operation from donating its elapsed time to the ordinary
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
    /// The live BLE pairing passkey ([`BleStatus::passkey`](crate::BleStatus)), fed by
    /// [`set_ble_status`](App::set_ble_status) and driving the passkey card (P2, #449) via
    /// [`reconcile_passkey_card`](App::reconcile_passkey_card). Held off `AppState` so feeding it
    /// never gates a map redraw; [`ble_passkey`](App::ble_passkey) exposes it for tests to observe
    /// the seam carrying it.
    pub(crate) ble_passkey: Option<u32>,
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
    /// The one **pending upload prompt** (epic #447, P4) — a route *or* a trip commit
    /// ([`PendingUpload`]), set by [`App::apply_event`](crate::App::apply_event) and delivered (or
    /// dropped) by [`reconcile_upload_prompt`](App::reconcile_upload_prompt). Deliberately a single
    /// slot: consecutive uploads replace it — most recent wins, the popup rule. Carried by
    /// **durable object id**, never a catalog index, so a rescan between arrival and a
    /// hold-deferred delivery can't retarget it.
    pub(crate) pending_upload: Option<PendingUpload>,
    /// Device warnings **discovered but not yet shown** on the advisory card (issue #504) — a
    /// missing-sensor probe result, or the map-slow flag. Accumulated by
    /// [`notify_warning`](App::apply_event) and delivered (or deferred behind a passkey card /
    /// hold) by [`reconcile_warning`](App::reconcile_warning), like [`pending_upload`].
    pub(crate) pending_warnings: WarningFlags,
    /// Warnings **already shown** on a card this session, so each flag is surfaced once and a
    /// dismissed notice doesn't nag — while a genuinely *new* flag (e.g. a late sensor timeout)
    /// still re-opens the card. Never cleared (the boot's warnings are the boot's).
    pub(crate) warned: WarningFlags,
    /// The firmware update **this boot just confirmed** (S4, #619): the running image's version
    /// string, set by the board once the trial confirm has written `Idle { installed }`. The
    /// one-time fact S5's "updated to vX" toast takes; `None` on a normal boot.
    pub(crate) update_confirmed: Option<heapless::String<32>>,
    /// The firmware update **this boot detected as failed** (the board's boot-outcome reconcile):
    /// the typed [`DfuFailure`](crate::dfu::DfuFailure) verdict + the staged version the arm
    /// marker recorded (if it survived). The one-time fact the "UPDATE FAILED" card takes; `None`
    /// on a normal boot.
    pub(crate) update_failed: Option<(crate::dfu::DfuFailure, Option<heapless::String<32>>)>,
}

impl UiRuntime {
    /// The boot state: the Home root on the stack, first frame dirty, nothing pending.
    pub(crate) fn new() -> Self {
        let mut stack = Stack::new();
        let _ = stack.push(Screen::Home(HomeScreen::new()));
        UiRuntime {
            stack,
            input: InputPlane::new(),
            now_ms: 0,
            // Force the host's first frame: nothing has been drawn yet, so the map is dirty.
            map_dirty: true,
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
            ble_passkey: None,
            sensor_status: [crate::sensors::SensorStatus::default(); crate::settings::SENSOR_SLOTS],
            sensor_scan_hits: crate::sensors::SensorScanHits::new(),
            pending_upload: None,
            pending_warnings: WarningFlags::NONE,
            warned: WarningFlags::NONE,
            update_confirmed: None,
            update_failed: None,
        }
    }

    /// Initialize `slot` **in place** to the [`new`](UiRuntime::new) state — the placement path
    /// (the screen stack and POI scratch are KB-scale; nothing here may form a by-value
    /// `UiRuntime` on the stack). Same field-by-field `addr_of_mut!` discipline as
    /// [`App::init_idle`](crate::App::init_idle), with the same trailing exhaustiveness guard.
    ///
    /// # Safety
    /// `slot` must be valid, aligned, exclusively owned, and writable for a full `UiRuntime`.
    pub(crate) unsafe fn init_in_place(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        // SAFETY: caller's contract; every field is written exactly once before any read.
        unsafe {
            // The screen stack: empty in place, then push the always-present Home root.
            // `heapless::Vec::push` isn't `const`, so the root can't be part of a literal.
            addr_of_mut!((*slot).stack).write(Stack::new());
            let _ = (*slot).stack.push(Screen::Home(HomeScreen::new()));
            addr_of_mut!((*slot).input).write(InputPlane::new());
            addr_of_mut!((*slot).now_ms).write(0);
            // Force the host's first frame: nothing has been drawn yet, so the map is dirty.
            addr_of_mut!((*slot).map_dirty).write(true);
            addr_of_mut!((*slot).region_dirty).write(None);
            addr_of_mut!((*slot).frame_size).write((0, 0));
            addr_of_mut!((*slot).render_clip).write(None);
            addr_of_mut!((*slot).next_wake_ms).write(None);
            addr_of_mut!((*slot).last_input_ms).write(0);
            addr_of_mut!((*slot).idle_return_timing).write(true);
            addr_of_mut!((*slot).hold_progress_override).write(None);
            addr_of_mut!((*slot).hold_cancel_pending).write(false);
            addr_of_mut!((*slot).poi_scratch).write(PoiScratch::new());
            addr_of_mut!((*slot).corridor_scratch).write(CorridorScratch::new());
            addr_of_mut!((*slot).next_ahead).write(NextAhead::new());
            addr_of_mut!((*slot).ble_passkey).write(None);
            addr_of_mut!((*slot).sensor_status)
                .write([crate::sensors::SensorStatus::default(); crate::settings::SENSOR_SLOTS]);
            addr_of_mut!((*slot).sensor_scan_hits).write(crate::sensors::SensorScanHits::new());
            addr_of_mut!((*slot).pending_upload).write(None);
            addr_of_mut!((*slot).pending_warnings).write(WarningFlags::NONE);
            addr_of_mut!((*slot).warned).write(WarningFlags::NONE);
            addr_of_mut!((*slot).update_confirmed).write(None);
            addr_of_mut!((*slot).update_failed).write(None);
            // Exhaustiveness guard: a field added to `UiRuntime` fails to compile here until its
            // `addr_of_mut!(...).write(...)` is added above (see `App::init_idle`).
            let UiRuntime {
                stack: _,
                input: _,
                now_ms: _,
                map_dirty: _,
                region_dirty: _,
                frame_size: _,
                render_clip: _,
                next_wake_ms: _,
                last_input_ms: _,
                idle_return_timing: _,
                hold_progress_override: _,
                hold_cancel_pending: _,
                poi_scratch: _,
                corridor_scratch: _,
                next_ahead: _,
                ble_passkey: _,
                sensor_status: _,
                sensor_scan_hits: _,
                pending_upload: _,
                pending_warnings: _,
                warned: _,
                update_confirmed: _,
                update_failed: _,
            } = &*slot;
        }
    }

    /// Advance the map-plane clock to `now_ms` and poll each visible screen's timers
    /// ([`Screen::tick_timers`]) in one pass: any time-driven repaint that fired dirties the map —
    /// so a screen surfaces its own timed-refresh rather than the host re-rendering on a blind
    /// heartbeat — and the soonest residual deadline is stored for
    /// [`App::ms_until_next_wake`](crate::App::ms_until_next_wake). Cheap: a clock comparison per
    /// drawn screen, over the same `base..` range the render draws. The per-pass modal sweeps
    /// (upload popups, warnings, toast, idle return) are sequenced by
    /// [`App::advance_animations`](crate::App::advance_animations) right after this.
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

    /// The stack index of the passkey card, or `None` when it isn't up. The card only ever sits as
    /// the top (it swallows input, and nothing navigates past it), but this searches the whole stack
    /// so a close removes it wherever it ended up.
    fn passkey_card_index(&self) -> Option<usize> {
        self.stack.iter().position(|s| matches!(s, Screen::Passkey(_)))
    }

    /// Whether the passkey card is currently up (epic #447). The P4 route-upload popups poll this to
    /// honour the priority rule — a popup is dropped, not queued, while the card shows.
    pub(crate) fn passkey_card_up(&self) -> bool {
        self.passkey_card_index().is_some()
    }

    /// Record the live BLE pairing passkey and reconcile the host-pushed card to it — the tail
    /// of [`App::set_ble_status`](crate::App::set_ble_status), owned here because the card's
    /// open/close discipline (defer mid-hold, outrank the upload popups) is modal-reconciliation
    /// policy.
    pub(crate) fn update_passkey_card(&mut self, passkey: Option<u32>) {
        self.ble_passkey = passkey;
        self.reconcile_passkey_card();
    }

    /// Open or close the host-pushed passkey card to match the seam's passkey ([`ble_passkey`](App::ble_passkey)):
    /// push a [`PasskeyScreen`](crate::screen::PasskeyScreen) when a passkey is present and no card is
    /// up, remove it when the passkey clears. Idempotent — the steady state (same passkey re-fed each
    /// pass) does nothing, so it never re-dirties. **Deferred while a hold charges** so a host-pushed
    /// screen never lands mid-hold (push *or* pop); the desired state is re-fed every pass, so the
    /// deferral is simply "try again next pass". Each transition dirties the map exactly once: opening
    /// covers the screen below (its own draw); closing repaints whatever the card covered.
    ///
    /// The card outranks the P4 route-upload popups: a popup consults
    /// [`passkey_card_up`](App::passkey_card_up) and drops its prompt while the card is showing.
    pub(crate) fn reconcile_passkey_card(&mut self) {
        // Never move a host-pushed screen onto/off the stack while a hold is charging.
        if self.hold_charging() {
            return;
        }
        match (self.ble_passkey, self.passkey_card_index()) {
            // A passkey to show and no card up → open it over the current top. The card outranks
            // the route-upload popups (P4) in *both* directions: a popup arriving under the card
            // is dropped (see `reconcile_upload_prompt`), and a passkey arriving while a popup is
            // up **replaces** it — remove the popup rather than stacking the card over it (it's
            // advisory; the route is in the menu either way). The manual, menu-opened Route-swap
            // prompt is not a popup and stays put under the card.
            (Some(passkey), None) => {
                self.remove_received_popups();
                let r = self.stack.push(Screen::Passkey(crate::screen::PasskeyScreen::new(passkey)));
                debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
                self.map_dirty = true;
            }
            // No passkey but a card is up → remove it wherever it sits (the rider may not have
            // touched anything), and repaint what it covered.
            (None, Some(i)) => {
                let _ = self.stack.remove(i);
                self.map_dirty = true;
            }
            // Card already matches the passkey (both present, or both absent): nothing to do.
            _ => {}
        }
    }

    /// The stack index of the map-transfer card, or `None` when it isn't up (issue #927). Searched
    /// across the whole stack for the same reason the passkey card's index is: a close must find it
    /// wherever it ended up.
    fn map_transfer_index(&self) -> Option<usize> {
        self.stack.iter().position(|s| matches!(s, Screen::MapTransfer(_)))
    }

    /// Whether the map-transfer card is up — the modal-priority query, and what
    /// [`App::map_transfer_card_up`](crate::App::map_transfer_card_up) exposes.
    pub(crate) fn map_transfer_card_up(&self) -> bool {
        self.map_transfer_index().is_some()
    }

    /// Reconcile the host-pushed map-transfer card to the board's live transfer state (issue #927) —
    /// the tail of [`App::set_map_transfer`](crate::App::set_map_transfer), and the direct analogue
    /// of [`reconcile_passkey_card`](Self::reconcile_passkey_card):
    ///
    /// - a state with no card up → push one;
    /// - a **changed** state with a card up → rewrite it in place and dirty (never stack a second);
    /// - an unchanged state → nothing, so the per-pass feed never re-dirties on the steady state
    ///   (progress is published in KiB, so even a fast card only changes this a few times a second);
    /// - no state with a card up → remove it and repaint what it covered.
    ///
    /// **Deferred while a hold charges**, like every host-pushed screen: the desired state is re-fed
    /// each pass, so the deferral is just "try again next pass". Unlike the passkey card this one
    /// does *not* clear the upload popups — the two cannot coexist in practice (a map transfer is
    /// USB-only and a route popup is BLE-driven), and stacking over one is harmless if they ever do.
    pub(crate) fn reconcile_map_transfer_card(&mut self, state: Option<crate::screen::MapTransfer>) {
        if self.hold_charging() {
            return;
        }
        match (state, self.map_transfer_index()) {
            (Some(state), Some(i)) => {
                let Screen::MapTransfer(card) = &mut self.stack[i] else { return };
                if card.state() != state {
                    card.set_state(state);
                    self.map_dirty = true;
                }
            }
            (Some(state), None) => {
                let r = self.stack.push(Screen::MapTransfer(crate::screen::MapTransferScreen::new(state)));
                debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
                self.map_dirty = true;
            }
            (None, Some(i)) => {
                let _ = self.stack.remove(i);
                self.map_dirty = true;
            }
            (None, None) => {}
        }
    }

    /// Queue a route-upload advisory event (single slot — most recent wins) and try to deliver
    /// it immediately — the tail of [`HostEvent::RouteUploaded`](crate::host::HostEvent) handling.
    pub(crate) fn post_upload_event(&mut self, ev: UploadEvent, catalogs: &CatalogState, tracking: bool) {
        self.pending_upload = Some(PendingUpload::Route(ev));
        self.reconcile_upload_prompt(catalogs, tracking);
    }

    /// Queue a trip-upload advisory event into the same single slot (most recent wins) and try to
    /// deliver it — the tail of [`HostEvent::TripUploaded`](crate::host::HostEvent) handling. The
    /// trip commit always follows its member routes' commits, so this is what collapses the
    /// per-route popup burst into the one "TRIP RECEIVED" card.
    pub(crate) fn post_trip_upload_event(&mut self, id: u16, catalogs: &CatalogState, tracking: bool) {
        self.pending_upload = Some(PendingUpload::Trip { id });
        self.reconcile_upload_prompt(catalogs, tracking);
    }

    /// Deliver (or drop) the pending route-upload prompt (epic #447, P4). Called on arrival and
    /// once per [`advance_animations`](App::advance_animations) pass, so a hold-deferred prompt
    /// lands on the next tick — the P2 host-pushed-screen precedent, adapted to a one-shot event
    /// (the pending slot *is* the re-fed desired state).
    ///
    /// The locked popup rules, in order:
    /// - **Passkey outranks**: while the card is up the prompt is dropped, not queued (advisory —
    ///   the route is in the Route menu regardless).
    /// - **Never lands mid-hold**: delivery waits a tick while either button's hold charges.
    /// - **Vanished id**: a route deleted between commit and delivery drops the prompt.
    /// - **Replace, don't stack**: an existing upload popup — or a manual
    ///   [`RouteSwapScreen`](crate::screen::RouteSwapScreen) opened from the menu — is replaced in
    ///   place by the new prompt (most recent wins; selection resets with the fresh screen).
    pub(crate) fn reconcile_upload_prompt(&mut self, catalogs: &CatalogState, tracking: bool) {
        let Some(ev) = self.pending_upload else { return };
        if self.passkey_card_up() {
            self.pending_upload = None; // dropped, not queued — the card outranks
            return;
        }
        if self.hold_charging() {
            return; // defer a tick; retried from `advance_animations`
        }
        self.pending_upload = None;
        let screen = match ev {
            PendingUpload::Route(ev) => {
                // Resolve the durable id in the (already rescanned) catalog; a vanished route
                // drops the advisory prompt entirely.
                let Some(idx) = catalogs.route_index_of(ev.id) else { return };
                if ev.active_replace {
                    Screen::RouteUpdated(crate::screen::RouteUpdatedScreen::new(idx, self.now_ms))
                } else if tracking {
                    Screen::RouteSwap(crate::screen::RouteSwapScreen::received(idx, self.now_ms))
                } else {
                    Screen::RouteReceived(crate::screen::RouteReceivedScreen::new(idx, self.now_ms, ev.elevation))
                }
            }
            // The trip card is the same whether idle or tracking (there is nothing to swap onto —
            // a trip is a folder, not a navigable route). Validate the id against the (already
            // re-fed) trip catalog; a vanished trip drops the advisory prompt entirely. The screen
            // keeps the durable id, so no remap is needed while it is up.
            PendingUpload::Trip { id } => {
                if !catalogs.trips().iter().any(|t| t.id == id) {
                    return;
                }
                Screen::TripReceived(crate::screen::TripReceivedScreen::new(id, self.now_ms))
            }
        };
        match self.upload_prompt_index() {
            Some(i) => self.stack[i] = screen,
            None => {
                let r = self.stack.push(screen);
                debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            }
        }
        self.map_dirty = true;
    }

    /// The stack index of the screen an incoming upload prompt **replaces**: any upload popup, or
    /// the manual Route-swap prompt (the locked "same rule when the manual swap is up"). `None`
    /// when the prompt should push fresh.
    fn upload_prompt_index(&self) -> Option<usize> {
        self.stack.iter().position(|s| {
            matches!(
                s,
                Screen::RouteReceived(_) | Screen::RouteUpdated(_) | Screen::TripReceived(_) | Screen::RouteSwap(_)
            )
        })
    }

    /// Remove every host-pushed upload popup from the stack (the passkey card just opened over
    /// them — card outranks). The **manual** Route-swap prompt is rider-opened, not a popup, and
    /// stays. Returns whether anything was removed.
    fn remove_received_popups(&mut self) -> bool {
        let mut removed = false;
        let mut i = 0;
        while i < self.stack.len() {
            let popup = match &self.stack[i] {
                Screen::RouteReceived(_) | Screen::RouteUpdated(_) | Screen::TripReceived(_) => true,
                Screen::RouteSwap(s) => s.is_received(),
                _ => false,
            };
            if popup {
                let _ = self.stack.remove(i);
                removed = true;
            } else {
                i += 1;
            }
        }
        removed
    }

    /// Auto-close any upload popup past its 30 s deadline — **timeout = dismiss** (epic #447,
    /// P4): the popup is removed exactly as Back would, nothing else changes. Deferred while a
    /// hold charges (the P2 rule: never move a host-pushed screen mid-hold); the popups'
    /// `tick_timers` keep a short residual wake armed until the sweep lands.
    pub(crate) fn close_expired_upload_popups(&mut self) {
        if self.hold_charging() {
            return;
        }
        let now = self.now_ms;
        let mut i = 0;
        while i < self.stack.len() {
            let expired = match &self.stack[i] {
                Screen::RouteReceived(s) => s.expired(now),
                Screen::RouteUpdated(s) => s.expired(now),
                Screen::TripReceived(s) => s.expired(now),
                Screen::RouteSwap(s) => s.expired(now),
                _ => false,
            };
            if expired {
                let _ = self.stack.remove(i);
                self.map_dirty = true; // repaint what the popup covered
            } else {
                i += 1;
            }
        }
    }

    /// Accumulate freshly-raised warning flags and try to deliver them — the
    /// [`HostEvent::Warning`](crate::host::HostEvent) landing rule.
    pub(crate) fn post_warning(&mut self, flags: WarningFlags) {
        if flags.is_empty() {
            return;
        }
        self.pending_warnings |= flags;
        self.reconcile_warning();
    }

    /// Deliver (or defer) the pending [warnings](App::apply_event). Called on arrival and once
    /// per [`advance_animations`](App::advance_animations) pass, so a warning deferred behind a
    /// passkey card or a live hold lands on a later tick — the [`reconcile_upload_prompt`] pattern.
    /// Only the not-yet-shown subset is surfaced (`pending & !warned`); it ORs into an open card or
    /// pushes a fresh one.
    pub(crate) fn reconcile_warning(&mut self) {
        let fresh = self.pending_warnings & !self.warned;
        if fresh.is_empty() {
            self.pending_warnings = WarningFlags::NONE; // nothing new — drop any stale re-raise
            return;
        }
        // Advisory: never cover the passkey card (it outranks) and never land mid-hold. Keep the
        // flags pending and retry from `advance_animations` once the card clears / the hold resolves.
        if self.passkey_card_up() || self.hold_charging() {
            return;
        }
        self.warned |= fresh;
        self.pending_warnings = WarningFlags::NONE;
        match self.warning_index() {
            Some(i) => {
                if let Screen::Warning(s) = &mut self.stack[i] {
                    s.add(fresh);
                }
            }
            None => {
                let r = self.stack.push(Screen::Warning(crate::screen::WarningScreen::new(fresh)));
                debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            }
        }
        self.map_dirty = true;
    }

    /// The stack index of a live [warning card](crate::screen::WarningScreen), so a newly-discovered
    /// fault ORs into it rather than stacking a second card. `None` when no card is open.
    fn warning_index(&self) -> Option<usize> {
        self.stack.iter().position(|s| matches!(s, Screen::Warning(_)))
    }

    /// Surface the one-time post-update verdict — the "Updated to vX" toast (epic #615 S5, #620)
    /// if this boot confirmed a freshly-installed update, or its failure twin, the "UPDATE FAILED"
    /// card, if the boot-outcome reconcile found the armed update is not what's running. The board
    /// calls [`notify_update_confirmed`](App::apply_event) at the health anchor (the
    /// first frame with the SD mounted) or [`notify_update_failed`](App::apply_event) at
    /// boot; the next [`advance_animations`](App::advance_animations) pass drains the fact and
    /// pushes the card once. Deferred behind a
    /// passkey card or a live hold like [`reconcile_warning`](App::reconcile_warning), so it never
    /// covers the pairing code or lands mid-hold; a normal boot has no fact and does nothing.
    pub(crate) fn reconcile_update_toast(&mut self) {
        if self.update_confirmed.is_none() && self.update_failed.is_none() {
            return;
        }
        if self.passkey_card_up() || self.hold_charging() {
            return; // retried next pass, once the card clears / the hold resolves
        }
        if let Some(version) = self.update_confirmed.take() {
            let r = self.stack.push(Screen::DfuUpdated(crate::screen::DfuUpdatedScreen::new(&version)));
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            self.map_dirty = true;
        }
        // The failure twin (the board's boot-outcome reconcile sets at most one of the two facts).
        if let Some((why, staged)) = self.update_failed.take() {
            let card = crate::screen::DfuFailedScreen::new(why, staged.as_deref());
            let r = self.stack.push(Screen::DfuFailed(card));
            debug_assert!(r.is_ok(), "screen stack overflow — raise MAX_DEPTH");
            self.map_dirty = true;
        }
    }

    /// [`HostEvent::DfuScanned`]: land the scan answer in the "Checking card..." wait, or drop it.
    pub(crate) fn on_dfu_scanned(&mut self, result: Result<crate::dfu::DfuScanReport, crate::dfu::DfuScanError>) {
        let Some(i) = self.stack.iter().position(|s| matches!(s, Screen::DfuCheck(_))) else {
            return;
        };
        self.stack[i] = match result {
            Ok(report) => Screen::DfuConfirm(crate::screen::DfuConfirmScreen::new(report)),
            Err(e) => Screen::DfuError(crate::screen::DfuErrorScreen::new(e)),
        };
        self.map_dirty = true;
    }

    /// [`HostEvent::DfuInstallBegan`]: swap the spinner for (or push) the terminal installing card.
    pub(crate) fn on_dfu_install_began(&mut self) {
        let card = Screen::DfuInstalling(crate::screen::DfuInstallingScreen::new());
        if let Some(i) = self.stack.iter().position(|s| matches!(s, Screen::DfuProgress(_))) {
            self.stack[i] = card;
        } else {
            let _ = self.stack.push(card);
        }
        self.map_dirty = true;
    }

    /// [`HostEvent::DfuInstallFailed`]: land the failure in the live install wait, or drop it.
    pub(crate) fn on_dfu_install_failed(&mut self, reason: crate::dfu::DfuInstallError) {
        let Some(i) = self.stack.iter().position(|s| matches!(s, Screen::DfuProgress(_) | Screen::DfuInstalling(_)))
        else {
            return;
        };
        self.stack[i] = Screen::DfuError(crate::screen::DfuErrorScreen::new_install(reason));
        self.map_dirty = true;
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
            overlay: self.input.take_overlay_dirty(),
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
