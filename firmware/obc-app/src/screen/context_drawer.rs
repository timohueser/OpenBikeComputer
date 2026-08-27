//! The **contextual drawer** (#1515 D3/D4): the bottom sheet the Down+Back chord opens on a screen
//! that declares secondary actions, and the declarative model those screens declare them with.
//!
//! ## Data, not behaviour
//!
//! A screen does not implement a drawer. It names one — a `&'static` [`ContextMenu`], returned from
//! [`Screen::context`](super::Screen::context) in the same partial-match idiom as
//! [`corridor_request`](super::Screen::corridor_request). Everything else lives here: the cursor,
//! which rows are inert, what a press resolves to, how the sheet is drawn and how it animates. A
//! screen that declares nothing gets no drawer, and the chord does nothing on it.
//!
//! That is what keeps the grammar one grammar. The compass ride menu this replaced was a *screen*,
//! so every station it offered was a line of navigation code on a page nothing else shared; here the
//! four ride actions are four rows of a table and the sheet that draws them is the same sheet every
//! later context gets.
//!
//! ## A row is a door or a value
//!
//! D3 shipped doors only. D4a adds the other half of the grammar: a row may instead bind to a
//! [`ContextValue`], and pressing it slides the sheet to a nested editor — Up/Down stages, Select
//! commits, Back discards, and the choice already committed stays marked while the rider browses.
//! The editor is generic: a binding says *where the value lives*, how many choices it has and what
//! each is called, and the drawer does the rest. That is what makes "the drawer is the only home
//! for a contextual setting" affordable — a context joins by naming a table, not by growing a page.

use embedded_graphics::prelude::Point;
use obc_reader::{PoiCategory, PoiCategorySet};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::Activity;
use crate::input::Gesture;
use crate::settings::UpAheadSource;
use crate::{AppState, Msg, Settings};

use super::vocab::sheet;
use super::{
    palette, Ctx, DetourScreen, PoiMenuScreen, Render, RouteMenuScreen, Screen, ScreenTick, Transition, UpAheadScreen,
};

/// How long the sheet takes to slide up from the bottom edge on open (ms).
const OPEN_MS: u32 = 220;
/// How long a nested editor takes to slide in, and the sheet to grow into its height (ms).
const SLIDE_MS: u32 = 180;
/// Repaint cadence while the sheet animates (ms) — the wake the event-driven host arms.
const FRAME_MS: u32 = 16;

/// One row's height, and the padding above the first row / below the last.
const ROW_H: i32 = 44;
const SHEET_PAD: i32 = 12;

/// The nested value editor's sheet height: one title line, the staged choice, and the notch strip
/// whose tick marks the committed one. Fixed, because every binding draws the same three things —
/// and tall enough that the tick sits *inside* the sheet rather than on its bottom lip.
const EDITOR_H: i32 = 148;

/// The tallest a sheet may grow before it stops being a sheet: three quarters of the 320 px panel.
/// #1515 asks a drawer to stay attached to its edge and use only the height its content needs, and
/// to prefer a bounded scrolling sheet over quietly becoming a page.
const MAX_SHEET_H: i32 = 240;

/// The widest table a context may declare — **four rows**, derived from [`MAX_SHEET_H`] rather than
/// asserted beside it, so the two can never drift.
///
/// The render key's availability bitmask is one `u8` ([`ContextDrawerScreen::key`]), which would
/// allow eight; the panel is the tighter limit and therefore the real one. A fifth row is 244 px,
/// leaving 76 px of map.
///
/// **This is a live constraint for D4c, not a theoretical one.** Decision 3 gives the *map's*
/// context the display modifiers, and D3's four riding views share one table — so D4c has to pick
/// one of three, and none of them needs a change here:
///
/// 1. **Give the Map its own table** ([`Screen::context`](super::Screen::context) is already
///    per-screen), with the display rows in place of the ones the Map does not need. That is also
///    the answer to *where* a scale-bar toggle belongs: on the Map, not on Statistics or the paused
///    page, where it has no referent.
/// 2. **Fold the display modifiers into one row** that opens a nested editor — the shape D4a built.
/// 3. **Bound and scroll the sheet**, which is what #1515 prescribes for real overflow and what
///    raising this constant would then mean.
const MAX_ROWS: usize = ((MAX_SHEET_H - SHEET_PAD * 2) / ROW_H) as usize;

/// The bitmask in [`ContextDrawerScreen::key`] is one `u8`, so the panel had better be the tighter
/// limit. If a geometry pass ever makes it not so, this is where that is caught.
const _: () = assert!(MAX_ROWS <= 8, "the render key carries row availability in one byte");

// ---- The declarative model ------------------------------------------------------------------

/// What a context row reads about the **base under the sheet**: whether it may be pressed, and what
/// the value it binds to currently is.
///
/// Gathered once by [`App::render_key`](crate::App) and once per press, from [`Ctx`] and
/// [`Render`] alike — so the row the rider sees, the row a press resolves and the row the render
/// key reports can never read three different worlds. This is the one definition of a row's inputs;
/// D3 threaded three loose arguments and the Detour row's fourth condition went missing between two
/// of them.
pub(crate) struct ContextFacts<'a> {
    pub state: &'a AppState,
    pub activity: &'a Activity,
    pub settings: &'a Settings,
    /// Whether a ride is open — the level [`RecorderMachine`](crate::RecorderMachine) reports, not
    /// a copy of it.
    pub recording: bool,
}

/// A typed value a context row edits in place of opening a screen. The binding owns **where the
/// value lives**, how many choices it has and what each one is called; the drawer owns the page,
/// the staging, the commit and the drawing.
///
/// Choices are addressed by ordinal, which is also what the render key carries — so "the staged
/// value" is one `u8` for every binding there will ever be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextValue {
    /// The Up-ahead timeline's **category filter**: Everything, then the six §7.4 categories.
    ///
    /// Rider *selection* state, so it lives in [`AppState`] beside the rain map's step rather than
    /// in [`Settings`]: the list opens on Everything every time (epic #946, U3 — predictable beats
    /// sticky), and a value that is reset on entry is not a preference. It sits in the app plane
    /// rather than on [`UpAheadScreen`] because the sheet that edits it is *above* that screen on
    /// the stack, so it belongs to neither of the two and to the pair.
    UpAheadFilter,
    /// Which sources feed the Up-ahead timeline — [`Settings::up_ahead_source`], the persisted
    /// preference whose second home in central Ride settings this slice deletes. The commit writes
    /// the field; [`App`](crate::App)'s one `==` diff over `Settings` is what arms the save, exactly
    /// as it does for a settings screen's edit.
    UpAheadSource,
}

impl ContextValue {
    /// How many choices this binding offers.
    fn count(self) -> u8 {
        match self {
            // "Everything" plus the six categories.
            ContextValue::UpAheadFilter => 1 + PoiCategory::ALL.len() as u8,
            ContextValue::UpAheadSource => UpAheadSource::COUNT as u8,
        }
    }

    /// Whether the row that binds this may be pressed. **Both bindings always accept**: a filter is
    /// as meaningful over an empty list as over a full one (it is how the rider finds out the list
    /// is empty *of that kind*), and the source scope is a preference no ride state can invalidate.
    /// Stated as a predicate rather than left implicit so the one-predicate rule has something to
    /// hold: the row is live exactly when the binding accepts.
    fn accepts(self, _f: &ContextFacts) -> bool {
        true
    }

    /// The ordinal currently committed — where the editor opens, and the choice it keeps marked.
    fn committed(self, f: &ContextFacts) -> u8 {
        match self {
            ContextValue::UpAheadFilter => filter_choice(f.state.up_ahead_filter),
            ContextValue::UpAheadSource => f.settings.up_ahead_source as u8,
        }
    }

    /// Write `ordinal` to wherever this binding's value lives.
    fn commit(self, cx: &mut Ctx, ordinal: u8) {
        match self {
            ContextValue::UpAheadFilter => cx.state.up_ahead_filter = choice_filter(ordinal),
            ContextValue::UpAheadSource => {
                cx.settings.up_ahead_source = UpAheadSource::ALL[(ordinal as usize).min(UpAheadSource::COUNT - 1)]
            }
        }
    }

    /// What `ordinal` is called, in the rider's language.
    fn choice_label(self, ordinal: u8, rx: &Render) -> &'static str {
        match self {
            ContextValue::UpAheadFilter => match choice_category(ordinal) {
                Some(cat) => rx.t(super::poi_menu::category_msg(cat)),
                None => rx.t(Msg::UpAheadEverything),
            },
            ContextValue::UpAheadSource => {
                UpAheadSource::ALL[(ordinal as usize).min(UpAheadSource::COUNT - 1)].name(rx.settings.language)
            }
        }
    }

    /// The choice's own icon, for the bindings whose values already have one. `None` draws the
    /// label alone rather than inventing a glyph.
    fn choice_icon(self, ordinal: u8) -> Option<PoiCategory> {
        match self {
            ContextValue::UpAheadFilter => choice_category(ordinal),
            ContextValue::UpAheadSource => None,
        }
    }
}

/// The category a filter ordinal names, or `None` for ordinal 0 ("Everything").
fn choice_category(ordinal: u8) -> Option<PoiCategory> {
    (ordinal > 0).then(|| PoiCategory::ALL[(ordinal as usize - 1).min(PoiCategory::ALL.len() - 1)])
}

/// The category set a filter ordinal selects.
fn choice_filter(ordinal: u8) -> PoiCategorySet {
    match choice_category(ordinal) {
        Some(cat) => PoiCategorySet::only(cat),
        None => PoiCategorySet::ALL,
    }
}

/// The ordinal a live filter shows as — the inverse of [`choice_filter`], so the editor opens on
/// what is already on. Any set the editor cannot produce reads as "Everything".
fn filter_choice(filter: PoiCategorySet) -> u8 {
    PoiCategory::ALL.iter().position(|c| filter == PoiCategorySet::only(*c)).map_or(0, |i| i as u8 + 1)
}

/// What pressing a context row does — a **destination or a binding**, not a closure. The drawer
/// resolves it against the live [`Ctx`] at press time, which is what lets the table be `&'static`
/// and lets one table serve four screens whose activity state differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextAction {
    /// The merged waypoint + corridor-POI timeline, anchored on live progress at entry.
    UpAhead,
    /// The rejoin chooser (#882). Inert without a route, a nav graph and an on-route rider.
    Detour,
    /// The POIs browser's category list.
    Pois,
    /// The stored-route / trip menu.
    Routes,
    /// A value the sheet edits on a nested page instead of a screen it opens.
    Edit(ContextValue),
}

impl ContextAction {
    /// Whether the row can be pressed right now. An unavailable row draws recessed and does
    /// nothing — the drawer's dim means *inert*, unlike the compass dial's, which dimmed stations
    /// a press still opened.
    /// **The row and its destination read one predicate.** For Detour that is
    /// [`detour::reachable`](super::detour::reachable), which the chooser's own availability check
    /// is built from — so a row can never be an enabled door onto an inert screen. For a value row
    /// it is [`ContextValue::accepts`], the same answer the commit obeys.
    fn available(self, f: &ContextFacts) -> bool {
        match self {
            // The timeline opens on its own empty state without a route, which is informative
            // rather than dead, so it is always live.
            ContextAction::UpAhead | ContextAction::Pois | ContextAction::Routes => true,
            // #882: a detour needs a recorded ride to re-route, a route to leave, a graph to route
            // on, and a rider on the route (the corridor anchors on live progress, which off-route
            // freezes).
            ContextAction::Detour => super::detour::reachable(f.activity, f.recording, f.state.has_nav_graph),
            ContextAction::Edit(v) => v.accepts(f),
        }
    }

    /// The screen this row opens. Every context row **replaces** the sheet, so Back out of the
    /// destination lands on the base screen the rider squeezed from rather than back inside a
    /// drawer they are finished with — the same rule the quick drawer's settings icon follows.
    ///
    /// `None` for a value row: it edits in place and never leaves the sheet.
    fn open(self, cx: &mut Ctx) -> Option<Transition> {
        Some(Transition::Replace(match self {
            ContextAction::UpAhead => {
                // The list always opens on **Everything** (epic #946, U3): the filter is selection
                // state, cleared on entry exactly as the rain map's step is, so a category the
                // rider chose one ride never silently empties the list on the next.
                cx.state.up_ahead_filter = PoiCategorySet::ALL;
                Screen::UpAhead(UpAheadScreen::new(cx.activity.progress_m))
            }
            ContextAction::Detour => Screen::Detour(DetourScreen::new(cx.activity)),
            ContextAction::Pois => Screen::PoiMenu(PoiMenuScreen::new()),
            ContextAction::Routes => Screen::RouteMenu(RouteMenuScreen::new()),
            ContextAction::Edit(_) => return None,
        }))
    }
}

/// One row of a declared context: its catalog label and what pressing it does.
#[derive(Clone, Copy)]
pub struct ContextRow {
    pub label: Msg,
    pub action: ContextAction,
}

/// A screen's declared contextual content — the rows the bottom sheet offers, in sheet order.
pub struct ContextMenu {
    pub rows: &'static [ContextRow],
}

/// The **ride context**: the secondary actions the four riding views share (Map, Statistics,
/// Climb, Ride control). It is the compass ride menu's row inventory minus its *Main menu* station,
/// which the global Back-hold escape now serves from everywhere instead of from one screen.
pub static RIDE: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::RideContextUpAhead, action: ContextAction::UpAhead },
        ContextRow { label: Msg::RideContextDetour, action: ContextAction::Detour },
        ContextRow { label: Msg::MenuPois, action: ContextAction::Pois },
        ContextRow { label: Msg::MenuRoutes, action: ContextAction::Routes },
    ],
};

/// The **Up-ahead context** (#1515 D4a): the two controls that scope the timeline, and the only
/// home either of them has. *Filter* is the category picker the list's Select-hold used to open —
/// a hold is a local action on a focused object, never the generic way into a menu — and *Sources*
/// is the scope the Ride settings screen used to cycle.
pub static UP_AHEAD: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::RideContextFilter, action: ContextAction::Edit(ContextValue::UpAheadFilter) },
        ContextRow { label: Msg::RideContextSources, action: ContextAction::Edit(ContextValue::UpAheadSource) },
    ],
};

// ---- The generic drawer ------------------------------------------------------------------------

/// The sheet's two pages. The value being edited is the selected row's, so the page needs no
/// payload — which is also why the render key's `page` byte says everything about where the rider is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Root,
    Editor,
}

impl Page {
    /// This page's sheet height over `menu`. The root grows with its table; every editor is the
    /// same three lines.
    fn height(self, menu: &ContextMenu) -> i32 {
        match self {
            Page::Root => SHEET_PAD * 2 + ROW_H * menu.rows.len() as i32,
            Page::Editor => EDITOR_H,
        }
    }
}

/// The contextual drawer's whole state: when it opened, the table it was opened over, the cursor,
/// the page, and the ordinal the editor has staged but not committed.
pub struct ContextDrawerScreen {
    opened_ms: u32,
    menu: &'static ContextMenu,
    /// When the page transition in flight started, or `None` when none is.
    slide_ms: Option<u32>,
    selected: u8,
    page: Page,
    /// The choice the editor is previewing. Meaningful only on [`Page::Editor`]; off that page
    /// every reader falls back to the committed value, which is what makes Back-discards free.
    staged: u8,
    /// Whether the open slide's **landing frame** has been reported. The frame the sheet lands on
    /// still differs from the one before it, and a host that only renders when asked would
    /// otherwise keep the last mid-slide frame on the panel for as long as the sheet is up.
    landed: bool,
}

impl ContextDrawerScreen {
    /// A freshly opened drawer over `menu`, sliding up from `now_ms` with the first row selected.
    pub fn new(now_ms: u32, menu: &'static ContextMenu) -> Self {
        debug_assert!(menu.rows.len() <= MAX_ROWS, "a context table is a sheet, not a page — see MAX_ROWS");
        ContextDrawerScreen {
            opened_ms: now_ms,
            menu,
            slide_ms: None,
            selected: 0,
            page: Page::Root,
            staged: 0,
            landed: false,
        }
    }

    /// The exact facts this drawer draws, for the pass's render key: the page, the selected row,
    /// the staged and committed values of the row's binding, and which rows are live. The identity
    /// of the sheet itself is the stack shape, which the key already carries.
    ///
    /// Availability and the committed value are derived from the *base* — the open ride, the route,
    /// the graph, the settings row — and are therefore the only things under an open sheet that may
    /// still move a pixel, exactly as the quick drawer's committed brightness is. They are the cue,
    /// not the values behind them, so a rider drifting off route re-draws the sheet once and a
    /// moving map still costs nothing.
    pub(crate) fn key(&self, f: &ContextFacts) -> (u8, u8, u8, u8, u8) {
        let mut live = 0u8;
        for (i, row) in self.menu.rows.iter().enumerate().take(MAX_ROWS) {
            if row.action.available(f) {
                live |= 1 << i;
            }
        }
        let committed = self.value().map_or(0, |v| v.committed(f));
        (self.page as u8, self.selected, self.staged, committed, live)
    }

    /// The binding the selected row carries, if it is a value row.
    fn value(&self) -> Option<ContextValue> {
        match self.menu.rows.get(self.selected as usize).map(|r| r.action) {
            Some(ContextAction::Edit(v)) => Some(v),
            _ => None,
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        // A page transition owns the input while it runs: acting on a half-drawn page would let a
        // fast double-press land on a row the rider cannot see yet.
        self.settle(cx.now_ms);
        if self.slide_ms.is_some() {
            return Transition::None;
        }
        match self.page {
            Page::Root => self.handle_root(g, cx),
            Page::Editor => self.handle_editor(g, cx),
        }
    }

    fn handle_root(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let rows = self.menu.rows;
        match g {
            Gesture::Step(n) => {
                self.selected = super::vocab::list::step_selection(self.selected as usize, n, rows.len()) as u8;
                Transition::None
            }
            Gesture::Press => {
                // An empty table (which the chord refuses to open a sheet for).
                let Some(row) = rows.get(self.selected as usize) else { return Transition::None };
                // Both answers are read before the row acts, so an `open` that edits app state
                // cannot change the world the availability check was made against.
                let facts = cx.context_facts();
                let (available, committed) =
                    (row.action.available(&facts), self.value().map_or(0, |v| v.committed(&facts)));
                if !available {
                    return Transition::None; // an inert row does nothing at all
                }
                match row.action.open(cx) {
                    Some(t) => t,
                    // A value row: open its editor on what is already committed.
                    None => {
                        self.staged = committed;
                        self.slide_to(Page::Editor, cx.now_ms);
                        Transition::None
                    }
                }
            }
            Gesture::Back => Transition::Pop,
            // Select-hold stays local and object-scoped; a context row has no held action.
            // Back-hold never arrives — `App` resolves the global escape above screen dispatch.
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    fn handle_editor(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let Some(value) = self.value() else {
            // Unreachable: the editor is only ever opened from a value row. Falling back to the
            // root is still better than editing nothing on a page with no title.
            self.slide_to(Page::Root, cx.now_ms);
            return Transition::None;
        };
        match g {
            // A **ring**, not an axis: these are named alternatives in no particular order, so the
            // cursor wraps exactly as every other named picker in the app does. (The quick drawer's
            // brightness clamps instead — a value axis has ends, and holding Up must not wrap the
            // panel from brightest to dimmest.)
            Gesture::Step(n) => {
                self.staged = super::vocab::list::step_selection(self.staged as usize, n, value.count() as usize) as u8;
                Transition::None
            }
            Gesture::Press => {
                value.commit(cx, self.staged);
                self.slide_to(Page::Root, cx.now_ms);
                Transition::None
            }
            // Discard: the staged ordinal is simply abandoned, so the row's value reverts to the
            // committed one on the very next frame.
            Gesture::Back => {
                self.slide_to(Page::Root, cx.now_ms);
                Transition::None
            }
            // Back-hold is the global escape, resolved above screen dispatch; it never arrives.
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    /// The sheet's animation: the open slide, then any page slide, at frame cadence.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let settled = self.settle(now_ms);
        let opening = OPEN_MS.saturating_sub(now_ms.wrapping_sub(self.opened_ms));
        let sliding = self.slide_ms.map_or(0, |s| SLIDE_MS.saturating_sub(now_ms.wrapping_sub(s)));
        let landing = !self.landed && opening == 0;
        self.landed |= opening == 0;
        match [opening, sliding].into_iter().filter(|r| *r > 0).min() {
            Some(remaining) => ScreenTick { changed: true, next_wake_ms: Some(FRAME_MS.min(remaining)), region: None },
            // The frame a slide (or the open) ends on still differs from the one before it.
            None if settled || landing => ScreenTick { changed: true, next_wake_ms: None, region: None },
            None => ScreenTick::idle(),
        }
    }

    /// Begin a horizontal transition to `to`, which becomes the live page at once (so `handle` and
    /// the render key already speak about the destination) while the slide draws both.
    fn slide_to(&mut self, to: Page, now_ms: u32) {
        self.slide_ms = Some(now_ms);
        self.page = to;
    }

    /// Retire a finished slide. Returns whether this call is the one that retired it.
    fn settle(&mut self, now_ms: u32) -> bool {
        let done = self.slide_ms.is_some_and(|s| now_ms.wrapping_sub(s) >= SLIDE_MS);
        if done {
            self.slide_ms = None;
        }
        done
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let sheet_h = self.sheet_height(rx.now_ms);
        let visible = self.visible_height(rx.now_ms, sheet_h);
        if visible == 0 {
            return;
        }
        // The sheet stays attached to the bottom edge: it slides up by drawing its full height with
        // its bottom off-screen, so the rounded top lip is what the rider sees arriving.
        let top = rx.h - visible;
        cv.round(rect(4, top, rx.w - 8, sheet_h + 8), 10, palette::PARCHMENT);
        cv.round_outline(rect(4, top, rx.w - 8, sheet_h + 8), 10, palette::WOOD_LIGHT);
        // The grab lip, so the sheet reads as pulled up from the bottom rather than as a card.
        cv.round(rect(rx.w / 2 - 18, top + 7, 36, 4), 2, palette::WOOD_LIGHT);

        match self.slide_ms {
            Some(started) => {
                let t = sheet::slid(rx.now_ms, started, SLIDE_MS);
                // Going deeper pushes the old page left; returning to the root pulls it right.
                let back = self.page == Page::Root;
                let (out, incoming) = if back {
                    ((t * rx.w as f32) as i32, -((1.0 - t) * rx.w as f32) as i32)
                } else {
                    (-((t * rx.w as f32) as i32), ((1.0 - t) * rx.w as f32) as i32)
                };
                self.draw_page(cv, rx, self.other_page(), top, out);
                self.draw_page(cv, rx, self.page, top, incoming);
            }
            None => self.draw_page(cv, rx, self.page, top, 0),
        }
    }

    /// The page a slide is coming *from* — the other one, since the sheet has exactly two.
    fn other_page(&self) -> Page {
        match self.page {
            Page::Root => Page::Editor,
            Page::Editor => Page::Root,
        }
    }

    /// The sheet height this frame: the page's own, or the interpolation between two pages' while a
    /// slide runs — which is how the sheet grows and shrinks with its content.
    fn sheet_height(&self, now_ms: u32) -> i32 {
        let Some(started) = self.slide_ms else { return self.page.height(self.menu) };
        let t = sheet::slid(now_ms, started, SLIDE_MS);
        let (from, to) = (self.other_page().height(self.menu) as f32, self.page.height(self.menu) as f32);
        (from + (to - from) * t + 0.5) as i32
    }

    /// How much of the sheet has arrived from the bottom edge, on the open animation's ease-out.
    fn visible_height(&self, now_ms: u32, sheet_h: i32) -> i32 {
        (sheet_h as f32 * sheet::arrived(now_ms, self.opened_ms, OPEN_MS) + 0.5) as i32
    }

    fn draw_page(&self, cv: &mut impl Surface, rx: &Render, page: Page, top: i32, x: i32) {
        match page {
            Page::Root => self.draw_root(cv, rx, top, x),
            Page::Editor => self.draw_editor(cv, rx, top, x),
        }
    }

    /// The row table: a label, and — on every live row — the chevron that says pressing it goes
    /// somewhere. A value row's chevron leads to its editor rather than to a screen.
    ///
    /// **The row does not state its value, and that is measured rather than chosen.** A label plus
    /// the longest choice is 260 px of `Label` glyphs on a 204 px row (`de`'s *Campingplatz*, `es`'s
    /// *Alojamiento*), so a one-line row cannot carry both, and a two-line row does not fit the
    /// 44 px pitch the ride sheet already ships. The editor one press away is where the value is
    /// spelled out and where the committed one is marked — the same division the quick drawer's
    /// brightness control makes. A taller value row is a question for D5's geometry pass.
    fn draw_root(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        let facts = rx.context_facts();
        for (i, row) in self.menu.rows.iter().enumerate() {
            let area = rect(x + 14, top + SHEET_PAD + i as i32 * ROW_H, rx.w - 36, ROW_H - 4);
            let live = row.action.available(&facts);
            super::vocab::rows::row_cursor(cv, area, i as u8 == self.selected, false);
            let ink = if live { palette::INK } else { palette::CONTOUR };
            cv.text_vcentered(
                rx.t(row.label),
                area.top_left.x + 14,
                (area.top_left.y, ROW_H - 4),
                Font::Body,
                TextAlign::Left,
                ink,
            );
            if live {
                let right = area.top_left.x + area.size.width as i32;
                let (cx0, cy) = (right - 18, area.top_left.y + (ROW_H - 4) / 2);
                cv.triangle(Point::new(cx0, cy - 8), Point::new(cx0, cy + 8), Point::new(cx0 + 9, cy), ink);
            }
        }
    }

    /// The nested value editor: the row's own label as the title, the staged choice spelled out
    /// (with its icon where the value has one), and a notch strip whose tick marks what is already
    /// committed.
    fn draw_editor(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        let Some(value) = self.value() else { return };
        let label = self.menu.rows.get(self.selected as usize).map_or("", |r| rx.t(r.label));
        cv.text(label, Point::new(x + 14, top + 18), Font::Label, TextAlign::Left, palette::WOOD);
        cv.hline(x + 12, top + 47, rx.w - 24, palette::RULE);

        let choice = value.choice_label(self.staged, rx);
        let cap = Font::Body.cap_height() as i32;
        let name_x = match value.choice_icon(self.staged) {
            Some(cat) => {
                let c = Point::new(x + 26, top + 64 + cap / 2);
                super::poi_menu::draw_category_icon(cv, cat, c, palette::INK, palette::PARCHMENT);
                x + 48
            }
            None => x + 20,
        };
        cv.text(choice, Point::new(name_x, top + 64), Font::Body, TextAlign::Left, palette::INK);

        let (x0, x1, y) = (x + 24, x + rx.w - 24, top + 112);
        let count = value.count();
        let committed = value.committed(&rx.context_facts());
        cv.round(rect(x0, y - 2, x1 - x0, 5), 2, palette::PARCHMENT_SHADE);
        for i in 0..count {
            let px = sheet::notch_x(x0, x1, i, count);
            cv.vline(px, y - 6, 13, 1, palette::SUBTEXT);
            if i == committed {
                sheet::committed_tick(cv, px, y + 22, palette::WOOD);
            }
        }
        let knob = sheet::notch_x(x0, x1, self.staged, count);
        cv.disc(Point::new(knob, y), 8, palette::AMBER);
        cv.disc(Point::new(knob, y), 3, palette::INK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::i18n::t;
    use crate::recorder::RecorderMachine;
    use crate::screen::test_ctx;
    use crate::settings::Language;
    use crate::{Activity, AppState, Settings};
    use obc_render::text::text_width;

    struct World {
        state: AppState,
        activity: Activity,
        settings: Settings,
        /// The ride the rider is on, so a row press can be held to leaving it alone (#1554 moved
        /// the session out of `Activity` and into this machine).
        recorder: RecorderMachine,
        now_ms: u32,
    }

    impl World {
        /// A routed, nav-graph, on-route **recorded** ride — every row of the ride context live.
        /// All four conditions matter: the Detour row reads the same predicate its chooser does
        /// ([`detour::reachable`](super::super::detour::reachable)), and that one names the open
        /// ride too.
        fn riding() -> Self {
            let mut state = AppState::new(0, 0, 1.0);
            state.has_nav_graph = true;
            let mut activity = Activity::new(Mode::Riding);
            activity.active_route = Some(0);
            let mut recorder = RecorderMachine::new();
            recorder.test_open();
            World { state, activity, settings: Settings::default(), recorder, now_ms: 1_000 }
        }

        fn press(&mut self, d: &mut ContextDrawerScreen, g: Gesture) -> Transition {
            let now_ms = self.now_ms;
            let t = d.handle(
                g,
                &mut Ctx {
                    recorder: &mut self.recorder,
                    now_ms,
                    ..test_ctx(&mut self.state, &mut self.activity, &mut self.settings)
                },
            );
            // Step the clock past whatever slide the gesture started, so the next one acts.
            self.now_ms += SLIDE_MS;
            t
        }

        fn facts(&self) -> ContextFacts<'_> {
            ContextFacts {
                state: &self.state,
                activity: &self.activity,
                settings: &self.settings,
                recording: self.recorder.recording(),
            }
        }
    }

    fn drawer() -> ContextDrawerScreen {
        // Opened far enough in the past that the slide has landed.
        ContextDrawerScreen::new(0, &RIDE)
    }

    fn up_ahead_drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::new(0, &UP_AHEAD)
    }

    /// The ride context is the compass menu's inventory minus its Main-menu station, and each row
    /// **replaces** the sheet so Back from the destination lands on the base.
    #[test]
    fn every_ride_row_replaces_the_sheet_with_its_destination() {
        let mut w = World::riding();
        let mut d = drawer();
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::UpAhead(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::Detour(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(2));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::PoiMenu(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(3));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::RouteMenu(_))));
    }

    /// The cursor wraps both ways over the declared rows, and Back closes the sheet.
    #[test]
    fn the_cursor_wraps_and_back_closes_the_sheet() {
        let mut w = World::riding();
        let mut d = drawer();
        w.press(&mut d, Gesture::Step(-1));
        assert!(
            matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::RouteMenu(_))),
            "wrapped to last"
        );

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(4));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::UpAhead(_))), "wrapped to first");
        assert!(matches!(w.press(&mut d, Gesture::Back), Transition::Pop));
    }

    /// An inert row is inert: pressing Detour without a route, without a graph, or off route does
    /// nothing at all — and the key says so, so the sheet redraws when the answer changes.
    #[test]
    fn an_unavailable_row_is_inert_and_shows_in_the_key() {
        let live_bit = 1 << 1; // the Detour row

        let mut w = World::riding();
        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1));
        assert_eq!(d.key(&w.facts()).4 & live_bit, live_bit, "on route, with a graph: live");

        for break_it in [
            (|w: &mut World| w.activity.active_route = None) as fn(&mut World),
            |w: &mut World| w.state.has_nav_graph = false,
            |w: &mut World| w.activity.off_route = true,
            // The fourth condition, and the one the row used to be missing: a *browse* map with a
            // route loaded and a graph under it still has nothing to re-route, because there is no
            // ride. Without this the row was an enabled door onto a chooser that opens inert.
            |w: &mut World| w.recorder.test_close(),
        ] {
            let mut w = World::riding();
            break_it(&mut w);
            let mut d = drawer();
            w.press(&mut d, Gesture::Step(1));
            assert_eq!(d.key(&w.facts()).4 & live_bit, 0, "the row went inert");
            assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "and a press does nothing");
        }
    }

    /// **A row is never an enabled door onto an inert screen.** The browse Map is the case that
    /// makes it checkable: a route is loaded and the map has a nav graph, so three of the Detour
    /// row's four conditions hold — but nothing is being recorded, and a detour re-routes *a ride*.
    ///
    /// The row and the chooser read the **same** predicate, so this is proved by identity rather
    /// than by two assertions that happen to agree: `detour::reachable` is what
    /// `DetourScreen::available` is built from.
    #[test]
    fn a_route_without_a_ride_leaves_the_detour_row_inert() {
        let mut w = World::riding();
        w.recorder.test_close(); // …a browse map: the route and the graph stay
        assert!(w.activity.active_route.is_some() && w.state.has_nav_graph && !w.activity.off_route);

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1)); // → the Detour row
        assert_eq!(d.key(&w.facts()).4 & (1 << 1), 0, "the row draws recessed");
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "…and a press does nothing");

        // The predicate the row read is the one the chooser reads: identical inputs, identical
        // answer, so the row cannot become an enabled door onto a screen that opens inert.
        assert!(!super::super::detour::reachable(&w.activity, w.recorder.recording(), w.state.has_nav_graph));
        assert!(
            ContextAction::Detour.available(&w.facts())
                == super::super::detour::reachable(&w.activity, w.recorder.recording(), w.state.has_nav_graph),
            "the row's availability *is* the chooser's entry condition"
        );
    }

    /// The timeline anchors its corridor snapshot on **live progress at entry** (epic #946 U3),
    /// opens on Everything, and leaves the recording session exactly as it found it — the property
    /// the compass menu's north station carried, now carried by the row that replaced it.
    #[test]
    fn up_ahead_anchors_on_live_progress_and_preserves_the_session() {
        let mut w = World::riding();
        w.recorder.test_open();
        w.activity.mode = Mode::Paused;
        w.activity.progress_m = 4_200;
        // A category the rider left on from an earlier list must not survive into this one.
        w.state.up_ahead_filter = PoiCategorySet::only(PoiCategory::Water);
        let session = w.recorder.session();
        let mut d = drawer();
        match w.press(&mut d, Gesture::Press) {
            Transition::Replace(Screen::UpAhead(screen)) => {
                let scope = crate::corridor::UpAheadScope {
                    filter: w.state.up_ahead_filter,
                    source: w.settings.up_ahead_source,
                };
                let key = screen.corridor_key(scope).expect("the default source scope wants a snapshot");
                assert_eq!(key.anchor_m, 4_200, "the snapshot anchors where the rider is");
                assert_eq!(key.filter, PoiCategorySet::ALL, "the list opens on Everything, every time");
            }
            _ => panic!("the Up ahead row did not open its timeline"),
        }
        assert_eq!(w.activity.mode, Mode::Paused, "opening ride chrome never resumes/pauses the session");
        assert!(w.recorder.recording(), "…and never closes the open ride");
        assert_eq!(w.recorder.session(), session, "…nor starts a new one");
        assert_eq!(w.recorder.test_take_intent(), None, "a row press names no recorder intent at all");
    }

    /// **Every declared table is a sheet, not a page**, and fits the key's availability mask. The
    /// D4 slices each add a table here; this is where one that outgrew the sheet is caught.
    #[test]
    fn pinned_by_the_row_tables() {
        // The derivation itself, so a geometry change is read here rather than discovered in D4c.
        assert_eq!(MAX_ROWS, 4, "24 px of padding plus 44 px rows inside a {MAX_SHEET_H} px sheet");

        // Two tables today; each remaining D4 slice adds its own to this list.
        let declared: &[&ContextMenu] = &[&RIDE, &UP_AHEAD];
        for menu in declared {
            assert!(menu.rows.len() <= MAX_ROWS, "{} rows outgrow the sheet", menu.rows.len());
            for page in [Page::Root, Page::Editor] {
                let h = page.height(menu);
                assert!(h <= MAX_SHEET_H, "a {h} px sheet is a page — bound it or scroll it first");
            }
            assert!(!menu.rows.is_empty(), "an empty table must not be declared: the chord shows no empty sheet");
        }
    }

    /// The sheet arrives monotonically from the bottom and lands exactly on its content height.
    #[test]
    fn the_sheet_slides_up_monotonically_and_lands_exactly() {
        let d = ContextDrawerScreen::new(1_000, &RIDE);
        let target = Page::Root.height(&RIDE);
        let frames: heapless::Vec<i32, 8> =
            [0, 55, 110, 165, OPEN_MS].iter().map(|dt| d.visible_height(1_000 + dt, target)).collect();
        assert_eq!(frames[0], 0, "nothing is visible on the opening frame");
        assert_eq!(frames[4], target, "and the sheet lands exactly on its height");
        assert!(frames.windows(2).all(|p| p[0] < p[1]), "monotonic: {frames:?}");
    }

    // ---- D4a: the nested value editor ----------------------------------------------------------

    /// The whole editor contract in one pass: a value row slides to its editor on the committed
    /// choice, Up/Down stages without committing, **Select commits and returns**, and the committed
    /// choice is what the row then reads.
    #[test]
    fn a_value_row_stages_on_up_down_and_commits_on_select() {
        let mut w = World::riding();
        let mut d = up_ahead_drawer();
        assert_eq!(d.page, Page::Root);

        w.press(&mut d, Gesture::Press); // → the Filter editor
        assert_eq!(d.page, Page::Editor, "a value row opens its editor rather than a screen");
        assert_eq!(d.staged, 0, "…on the committed choice, which is Everything");

        w.press(&mut d, Gesture::Step(1)); // → Water
        assert_eq!(d.staged, 1, "Up/Down stages");
        assert_eq!(w.state.up_ahead_filter, PoiCategorySet::ALL, "…and commits nothing yet");

        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Root, "Select returns to the row table");
        assert_eq!(w.state.up_ahead_filter, PoiCategorySet::only(PoiCategory::Water), "…having committed the choice");

        // Re-opening the editor opens on what is now committed.
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.staged, 1, "the editor opens on the committed choice, every time");
    }

    /// **Back discards.** The staged choice is abandoned, the value underneath is untouched, and
    /// the editor closes — so nothing has to be remembered in order to be undone.
    #[test]
    fn back_out_of_the_editor_discards_the_staged_choice() {
        let mut w = World::riding();
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Press);
        w.press(&mut d, Gesture::Step(3));
        assert_eq!(d.staged, 3);
        w.press(&mut d, Gesture::Back);
        assert_eq!(d.page, Page::Root, "Back closes the editor, not the sheet");
        assert_eq!(w.state.up_ahead_filter, PoiCategorySet::ALL, "…and the value is untouched");

        // …and the sheet is still there: a second Back is what closes it.
        assert!(matches!(w.press(&mut d, Gesture::Back), Transition::Pop));
    }

    /// The **committed choice stays marked** while the rider browses alternatives — the render key
    /// carries the staged and the committed value as two separate facts, which is what lets the
    /// editor draw both.
    #[test]
    fn the_committed_choice_stays_marked_while_browsing() {
        let mut w = World::riding();
        w.state.up_ahead_filter = PoiCategorySet::only(PoiCategory::Water);
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Press);
        w.press(&mut d, Gesture::Step(2)); // browse two on from Water

        let (page, _, staged, committed, _) = d.key(&w.facts());
        assert_eq!(page, Page::Editor as u8);
        assert_eq!(staged, 3, "the key carries what the rider is looking at");
        assert_eq!(committed, 1, "…and, separately, what the device is set to");
    }

    /// The **Sources** row is the migrated Ride-settings row: its commit writes
    /// `Settings::up_ahead_source`, which is the field the App's `==` diff turns into a save. The
    /// row draws the value it wrote, so the sheet alone answers "what is this set to".
    #[test]
    fn the_sources_row_commits_the_persisted_settings_field() {
        let mut w = World::riding();
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Step(1)); // → Sources
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.staged, UpAheadSource::Both as u8, "the editor opens on the persisted value");

        w.press(&mut d, Gesture::Step(2)); // → Map POIs only
        assert_eq!(w.settings.up_ahead_source, UpAheadSource::Both, "still nothing committed");
        w.press(&mut d, Gesture::Press);
        assert_eq!(w.settings.up_ahead_source, UpAheadSource::MapPoisOnly, "Select wrote the settings field");
        assert_eq!(
            d.key(&w.facts()).3,
            UpAheadSource::MapPoisOnly as u8,
            "…and the row now reads the value it committed"
        );
    }

    /// The editor is a **ring of named alternatives**: it wraps at both ends over exactly the
    /// choices the binding declares, and every ordinal round-trips to a value and back.
    #[test]
    fn the_editor_wraps_over_exactly_the_declared_choices() {
        let mut w = World::riding();
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Press); // the Filter editor: Everything + six categories
        w.press(&mut d, Gesture::Step(-1));
        assert_eq!(d.staged, 6, "stepping back off Everything wraps to the last category");
        w.press(&mut d, Gesture::Step(1));
        assert_eq!(d.staged, 0, "…and forward off the last wraps home");

        assert_eq!(ContextValue::UpAheadFilter.count(), 7);
        assert_eq!(ContextValue::UpAheadSource.count(), UpAheadSource::COUNT as u8);
        for ordinal in 0..ContextValue::UpAheadFilter.count() {
            assert_eq!(filter_choice(choice_filter(ordinal)), ordinal, "ordinal {ordinal} round-trips");
        }
        assert_eq!(choice_filter(0), PoiCategorySet::ALL, "ordinal 0 is Everything");
    }

    /// **The one-predicate rule for value rows.** A binding that always accepts must give a row
    /// that is always live: a browse map with no route, no graph and no ride still lets the rider
    /// see and change what the timeline is scoped to.
    #[test]
    fn a_value_row_is_live_exactly_where_its_binding_accepts() {
        let mut w = World::riding();
        w.activity.active_route = None;
        w.state.has_nav_graph = false;
        w.recorder.test_close();
        let d = up_ahead_drawer();
        let facts = w.facts();

        assert_eq!(d.key(&facts).4, 0b11, "both value rows stay live on a bare browse map");
        for row in UP_AHEAD.rows {
            let ContextAction::Edit(v) = row.action else { panic!("the Up-ahead table is all value rows") };
            assert_eq!(row.action.available(&facts), v.accepts(&facts), "the row reads the binding's own answer");
        }

        // And a press really does open the editor there, rather than drawing live and doing nothing.
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor);
    }

    /// A page transition owns the input while it runs: a gesture landing mid-slide changes nothing.
    #[test]
    fn a_gesture_during_a_slide_is_ignored() {
        let mut w = World::riding();
        let mut d = up_ahead_drawer();
        let now_ms = w.now_ms;
        d.handle(
            Gesture::Press,
            &mut Ctx { recorder: &mut w.recorder, now_ms, ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings) },
        );
        let staged = d.staged;
        d.handle(
            Gesture::Step(1),
            &mut Ctx {
                recorder: &mut w.recorder,
                now_ms: now_ms + SLIDE_MS / 2,
                ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings)
            },
        );
        assert_eq!(d.staged, staged, "a mid-slide step acts on nothing");
    }

    /// The sheet grows into the editor and shrinks back, monotonically, landing exactly on each
    /// page's own height — the adaptive-height rule #1515 asks for.
    #[test]
    fn the_sheet_grows_into_the_editor_and_back() {
        let root_h = Page::Root.height(&UP_AHEAD);
        assert!(root_h < EDITOR_H, "the two-row table is shorter than the editor, so the sheet must grow");

        let mut d = ContextDrawerScreen::new(0, &UP_AHEAD);
        d.slide_to(Page::Editor, 1_000);
        let grow: heapless::Vec<i32, 8> =
            [0, 45, 90, 135, SLIDE_MS].iter().map(|dt| d.sheet_height(1_000 + dt)).collect();
        assert_eq!((grow[0], grow[4]), (root_h, EDITOR_H));
        assert!(grow.windows(2).all(|p| p[0] <= p[1]), "monotonic growth: {grow:?}");

        d.settle(1_000 + SLIDE_MS);
        d.slide_to(Page::Root, 2_000);
        let shrink: heapless::Vec<i32, 8> =
            [0, 45, 90, 135, SLIDE_MS].iter().map(|dt| d.sheet_height(2_000 + dt)).collect();
        assert_eq!((shrink[0], shrink[4]), (EDITOR_H, root_h));
        assert!(shrink.windows(2).all(|p| p[0] >= p[1]), "…and back: {shrink:?}");
    }

    /// The sheet is all copy, so this is its overflow check: every row label clears the chevron on a
    /// 240 px panel in every language, and every editor choice fits the editor's own line.
    ///
    /// It also **pins the measurement the row design rests on** — that a label plus its longest
    /// choice does not fit one row — so a future geometry pass sees the number rather than the
    /// conclusion.
    #[test]
    fn every_label_and_choice_fits_the_sheet_in_every_language() {
        const W: i32 = 240;
        const MIN_CLEAR: i32 = 8;
        // The draw's own geometry: the row area starts 14 px into the sheet and is `w - 36` wide;
        // the label starts 14 px inside it and the chevron takes the last 18.
        let area_w = W - 36;
        let label_room = area_w - 14 - 18 - MIN_CLEAR;
        // The editor draws its choice from x + 48 (past the widest icon gutter) to the sheet edge.
        let choice_room = W - 48 - 12;
        let mut worst_row = 0;
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            for menu in [&RIDE, &UP_AHEAD] {
                for row in menu.rows {
                    let label = t(row.label, lang);
                    let lw = text_width(label, Font::Body) as i32;
                    assert!(lw <= label_room, "{lang:?}: row label {label:?} ({lw} px) overruns {label_room} px");
                    let ContextAction::Edit(v) = row.action else { continue };
                    for ordinal in 0..v.count() {
                        let choice = choice_text(v, ordinal, lang);
                        let cw = text_width(choice, Font::Label) as i32;
                        assert!(cw <= choice_room, "{lang:?}: choice {choice:?} ({cw} px) overruns {choice_room} px");
                        worst_row = worst_row.max(14 + lw + MIN_CLEAR + cw + 10);
                    }
                }
            }
        }
        assert!(
            worst_row > area_w,
            "a label and its longest choice now fit one {area_w} px row ({worst_row} px) — \
             the row could state its value again; see `draw_root`"
        );
        assert_eq!(worst_row, 260, "the measurement the row design rests on, pinned");
    }

    /// The catalog lookup [`ContextValue::choice_label`] makes, without a `Render` to hang it off.
    fn choice_text(v: ContextValue, ordinal: u8, lang: Language) -> &'static str {
        match v {
            ContextValue::UpAheadFilter => match choice_category(ordinal) {
                Some(cat) => t(super::super::poi_menu::category_msg(cat), lang),
                None => t(Msg::UpAheadEverything, lang),
            },
            ContextValue::UpAheadSource => UpAheadSource::ALL[ordinal as usize].name(lang),
        }
    }
}
