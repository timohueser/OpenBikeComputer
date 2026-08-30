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
//! ## A row is a door, a value, an act or a switch
//!
//! Four shapes, and the sheet has no fifth.
//!
//! A **door** replaces the sheet with what it opens — a screen, or (D4c) a shorter sheet.
//!
//! A **value** binds to a [`ContextValue`] and slides the sheet to a nested editor: Up/Down stages,
//! Select commits, Back discards, and the choice already committed stays marked while the rider
//! browses. The editor is generic — a binding says *where the value lives*, how many choices it has
//! and what each is called, and the drawer does the rest. That is what makes "the drawer is the only
//! home for a contextual setting" affordable: a context joins by naming a table, not by growing a
//! page.
//!
//! A binding's choices need not be a fixed list: [`ContextValue::BikeProfile`] (#1515 D4d) counts
//! and names the **loaded map's** §8.6 routing profiles, so how many choices it has and whether its
//! row is live at all are read off [`ContextFacts`] like everything else a row draws.
//!
//! An **act** tells a domain something and leaves the sheet. It has to leave: a sheet's own frame
//! shadows every base fact (see [`ContextDrawerScreen::key`]), so a cue raised from an open sheet
//! could not be seen until it closed anyway.
//!
//! A **switch** ([`ContextToggle`]) flips a `bool` in place and keeps the sheet up. It is the one
//! shape whose effect is *also* hidden behind the frozen base and that still stays — because the row
//! draws its own state, so the control is legible while the sheet is up, and the rider sets a whole
//! group of them for one repaint of the screen underneath.

use embedded_graphics::prelude::Point;
use obc_reader::{PoiCategory, PoiCategorySet};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::navigator::RouteState;
use crate::settings::{UpAheadSource, WeatherRefresh};
use crate::{AppState, Msg, Settings};

use super::vocab::sheet;
use super::{
    palette, Ctx, DetourScreen, PoiMenuScreen, Render, RouteMenuScreen, Screen, ScreenTick, Transition, UpAheadScreen,
};

/// How long the sheet takes to slide up from the bottom edge on open (ms).
///
/// **The one knob this sheet's open feel hangs on** (#1559), and deliberately its own rather than
/// the quick drawer's: the two sheets are tuned on glass one at a time. 220 ms landed in about four
/// visible steps and read as lag rather than as motion; this is the owner's "start at about twice
/// that", a default to iterate on, not a measurement.
const OPEN_MS: u32 = 440;
/// How long a nested editor takes to slide in, and the sheet to grow into its height (ms).
const SLIDE_MS: u32 = 180;
/// How long one step of the open costs the panel, and therefore the cadence the sheet asks to be
/// woken at (ms).
///
/// Measured on the LS021B7DD02, release build (#1559 bench rounds 1 and 2): **8.4 ms** of
/// whole-frame row hash per present plus **0.137 ms per pushed row**, and about **12 ms** to draw
/// the sheet. This is the taller of the two sheets — a four-row table is 200 px against the quick
/// drawer's 104 px root — so its deepest step costs 8.4 + 27.4 + 12 ≈ 48 ms. That is the step; the
/// 16 ms token it replaces asked for three steps in the time one takes. [`OPEN_MS`] divided by this
/// is 9 steps, which is why the two sheets keep their own numbers rather than sharing one.
const STEP_MS: u32 = 48;

/// One row's height, and the padding above the first row / below the last.
const ROW_H: i32 = 44;
const SHEET_PAD: i32 = 12;

/// The nested value editor's sheet height: one title line, the staged choice, and the notch strip
/// whose tick marks the committed one. Fixed, because every binding draws the same three things —
/// and tall enough that the tick sits *inside* the sheet rather than on its bottom lip.
const EDITOR_H: i32 = 148;

/// The tallest a sheet may grow before it stops being a sheet: 244 px of the 320 px panel, leaving
/// 76 px of the screen underneath. #1515 asks a drawer to stay attached to its edge and use only the
/// height its content needs, and to prefer a bounded scrolling sheet over quietly becoming a page.
const MAX_SHEET_H: i32 = 244;

/// The widest table a context may declare — **five rows**, derived from [`MAX_SHEET_H`] rather than
/// asserted beside it, so the two can never drift.
///
/// The render key's availability bitmask is one `u8` ([`ContextDrawerScreen::key`]), which would
/// allow eight; the panel is the tighter limit and therefore the real one.
///
/// **Five is where D4c stopped, and why.** The Map needs the ride's four actions *and* its three
/// display modifiers; seven flat rows are 332 px and fit no sheet at any height. So the Map declares
/// [`MAP`] — the four ride actions, unchanged, plus one door onto [`MAP_DISPLAY`] — which is the
/// row list #1515's own body enumerates. A sixth row would be 288 px, and at that point the sheet is
/// a page: #1515's remedy for real overflow is a **bounded scrolling sheet**, not a taller one, and
/// that is the slice a seventh row has to wait for.
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
    pub navigation: &'a RouteState,
    pub settings: &'a Settings,
    /// Whether a ride is open — the level [`RecorderMachine`](crate::RecorderMachine) reports, not
    /// a copy of it.
    pub recording: bool,
    /// A weather request is outstanding — in flight, or asked for and not yet sent. The *Refresh
    /// now* row's one predicate ([`WeatherDomain::request_outstanding`](crate::weather::WeatherDomain::request_outstanding)).
    pub weather_request_outstanding: bool,
    /// The loaded map's routing-profile names — how many choices the bike-type binding has, and
    /// therefore whether its row is live at all. Read, never written.
    pub nav_profiles: &'a crate::NavProfiles,
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
    /// How often the device asks the phone for a fresh bundle on its own —
    /// [`Settings::weather_refresh`], the persisted cadence whose whole settings *screen* this slice
    /// deletes (#1515 D4b). Off / 15 / 30 / 60 / 120 minutes, default 30; the WX8 due scheduler
    /// re-reads the field at its next evaluation, so a commit needs no wake edge of its own.
    WeatherInterval,
    /// The routing profile the on-device planner weights edges by —
    /// [`Settings::bike_profile_idx`](crate::Settings), whose whole settings *screen* this slice
    /// deletes (#1515 D4d). The **first binding whose choices are map data**: they are the loaded
    /// map's §8.6 profile names ([`NavProfiles`](crate::NavProfiles)), so a custom web-builder
    /// profile appears without a hardcoded list — and a map that offers no choice makes the row
    /// inert rather than a control that walks a ring of one.
    BikeProfile,
}

impl ContextValue {
    /// How many choices this binding offers. Takes the facts because a binding's choices may be
    /// map data rather than a compiled-in list.
    fn count(self, f: &ContextFacts) -> u8 {
        match self {
            // "Everything" plus the six categories.
            ContextValue::UpAheadFilter => 1 + PoiCategory::ALL.len() as u8,
            ContextValue::UpAheadSource => UpAheadSource::COUNT as u8,
            ContextValue::WeatherInterval => WeatherRefresh::COUNT as u8,
            // At most `NAV_MAX_PROFILES` (8), which is also the notch strip's own ceiling.
            ContextValue::BikeProfile => f.nav_profiles.len() as u8,
        }
    }

    /// Whether the row that binds this may be pressed. Stated as a predicate rather than left
    /// implicit so the one-predicate rule has something to hold: the row is live exactly when the
    /// binding accepts.
    ///
    /// Three of the four always accept — a filter is as meaningful over an empty list as over a
    /// full one (it is how the rider finds out the list is empty *of that kind*), and a source
    /// scope or a refresh cadence is a preference no ride state can invalidate. The bike profile
    /// needs a map that offers a choice: this is the deleted Bike-type screen's own `count > 1`
    /// guard, which it expressed as a silent no-op on an empty-state page.
    fn accepts(self, f: &ContextFacts) -> bool {
        match self {
            ContextValue::UpAheadFilter | ContextValue::UpAheadSource | ContextValue::WeatherInterval => true,
            ContextValue::BikeProfile => f.nav_profiles.len() > 1,
        }
    }

    /// The ordinal currently committed — where the editor opens, and the choice it keeps marked.
    fn committed(self, f: &ContextFacts) -> u8 {
        match self {
            ContextValue::UpAheadFilter => filter_choice(f.state.up_ahead_filter),
            ContextValue::UpAheadSource => f.settings.up_ahead_source as u8,
            ContextValue::WeatherInterval => f.settings.weather_refresh as u8,
            // The **effective** index, not the stored one: a stale index against a smaller map
            // opens on profile 0 and marks profile 0, which is the profile the router will actually
            // use (routing-v2 N3). The #538 truthful-label rule.
            ContextValue::BikeProfile => f.nav_profiles.effective(f.settings.bike_profile_idx),
        }
    }

    /// Write `ordinal` to wherever this binding's value lives.
    fn commit(self, cx: &mut Ctx, ordinal: u8) {
        match self {
            ContextValue::UpAheadFilter => cx.state.up_ahead_filter = choice_filter(ordinal),
            ContextValue::UpAheadSource => {
                cx.settings.up_ahead_source = UpAheadSource::ALL[(ordinal as usize).min(UpAheadSource::COUNT - 1)]
            }
            ContextValue::WeatherInterval => {
                cx.settings.weather_refresh = WeatherRefresh::ALL[(ordinal as usize).min(WeatherRefresh::COUNT - 1)]
            }
            // The ordinal came from the editor's ring, which is `count` long, so it is already an
            // index the loaded map has.
            ContextValue::BikeProfile => cx.settings.bike_profile_idx = ordinal,
        }
    }

    /// What `ordinal` is called, in the rider's language — or, for the bike profile, in the map's
    /// own words. The borrow is `rx`'s because those names live in
    /// [`NavProfiles`](crate::NavProfiles) rather than in `.rodata`; every catalog arm coerces.
    fn choice_label<'a>(self, ordinal: u8, rx: &'a Render) -> &'a str {
        match self {
            ContextValue::UpAheadFilter => match choice_category(ordinal) {
                Some(cat) => rx.t(super::poi_menu::category_msg(cat)),
                None => rx.t(Msg::UpAheadEverything),
            },
            ContextValue::UpAheadSource => {
                UpAheadSource::ALL[(ordinal as usize).min(UpAheadSource::COUNT - 1)].name(rx.settings.language)
            }
            ContextValue::WeatherInterval => {
                WeatherRefresh::ALL[(ordinal as usize).min(WeatherRefresh::COUNT - 1)].name(rx.settings.language)
            }
            // The deleted screen's own name resolution. `write_label`'s generic `Profile N`
            // fallback is deliberately not used: it exists for an empty table, and an empty table
            // makes this row inert, so it has no reachable case in the sheet.
            ContextValue::BikeProfile => rx.nav_profiles.name(ordinal).unwrap_or(""),
        }
    }

    /// The choice's own icon, for the bindings whose values already have one. `None` draws the
    /// label alone rather than inventing a glyph.
    fn choice_icon(self, ordinal: u8) -> Option<PoiCategory> {
        match self {
            ContextValue::UpAheadFilter => choice_category(ordinal),
            // The hero bike sprite does not follow the setting into the sheet: `bike_icons` draws a
            // 200 × 120 px art asset and the editor is 148 px tall. The names are the choice.
            ContextValue::UpAheadSource | ContextValue::WeatherInterval | ContextValue::BikeProfile => None,
        }
    }
}

/// A `bool` a context row flips **in place** — the switch shape. The binding owns *where the bit
/// lives*; the drawer owns the row, the slider it draws and what a press does.
///
/// All three are the Map's display modifiers, and all three are device-only —
/// `adopt_ble_fields` never pulls them. That is what makes the render key's one `committed` byte
/// sufficient here: under an open sheet only the rider can move one of these bits, and only the
/// selected row's.
///
/// The shared `Map` prefix is deliberate: each variant is named for the [`Settings`] field it binds
/// to, so `read`/`flip` can be checked by reading them side by side.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextToggle {
    /// [`Settings::map_clock`] — the map's `HH:MM` pill.
    MapClock,
    /// [`Settings::map_scale_bar`] — the map's bottom-left scale bar.
    MapScaleBar,
    /// [`Settings::map_contours`] — the map's terrain layer. **Provisional** (elevation EL10c,
    /// #1096): it exists so #1097's ride review can A/B contours on the same ride, and it goes with
    /// that review's verdict — the row migrated here, it was not retired.
    MapContours,
}

impl ContextToggle {
    /// Whether the bit is set — what the row's slider draws, and what the render key reports as the
    /// selected row's committed state.
    fn read(self, f: &ContextFacts) -> bool {
        match self {
            ContextToggle::MapClock => f.settings.map_clock,
            ContextToggle::MapScaleBar => f.settings.map_scale_bar,
            ContextToggle::MapContours => f.settings.map_contours,
        }
    }

    /// Flip it. [`App`](crate::App)'s one `==` diff over [`Settings`] is what turns the write into a
    /// save, exactly as it does for a settings screen's edit — and a later flip supersedes an
    /// in-flight older revision, so three of them cannot queue three competing writes.
    fn flip(self, cx: &mut Ctx) {
        match self {
            ContextToggle::MapClock => cx.settings.map_clock = !cx.settings.map_clock,
            ContextToggle::MapScaleBar => cx.settings.map_scale_bar = !cx.settings.map_scale_bar,
            ContextToggle::MapContours => cx.settings.map_contours = !cx.settings.map_contours,
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
    /// **Ask for fresh weather now** (#1515 D4b) — the only manual refresh the rider has. Neither a
    /// door nor a value: it raises one [`WeatherIntent::RefreshRequested`](crate::weather::WeatherIntent)
    /// and closes the sheet, because the cue and the data it asks for are base facts an open sheet
    /// shadows.
    RefreshWeather,
    /// **The map's display modifiers** (#1515 D4c) — a door onto [`MAP_DISPLAY`], which is a sheet
    /// rather than a screen. The only row that replaces a sheet with a sheet, and the shape the map
    /// forced: a nested *sliding* page over a map costs a map render per frame of the slide, while
    /// a swap costs exactly one. See [`ContextDrawerScreen::swapped_in`].
    MapDisplay,
    /// A `bool` the row flips **in place**: the sheet stays up, the row's own slider is the
    /// feedback, and the screen underneath is redrawn once, when the sheet closes — so setting all
    /// three map modifiers costs one map render, not three.
    Toggle(ContextToggle),
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
            ContextAction::Detour => super::detour::reachable(f.navigation, f.recording, f.state.has_nav_graph),
            ContextAction::Edit(v) => v.accepts(f),
            // There is a question to ask exactly when one is not already outstanding: the domain
            // coalesces a repeat anyway, so a live row here would be a control with no effect.
            ContextAction::RefreshWeather => !f.weather_request_outstanding,
            // A display modifier is a preference no ride state can invalidate, and the door onto
            // them is as live as they are. Stated rather than left implicit, so the one-predicate
            // rule has something to hold here too.
            ContextAction::MapDisplay | ContextAction::Toggle(_) => true,
        }
    }

    /// The screen this row opens. Every context row **replaces** the sheet, so Back out of the
    /// destination lands on the base screen the rider squeezed from rather than back inside a
    /// drawer they are finished with — the same rule the quick drawer's settings icon follows.
    ///
    /// `None` for a value row: it edits in place and never leaves the sheet.
    ///
    /// The *Refresh now* row is neither — it acts and **pops**, so the base it was squeezed from is
    /// still under it. It has to: a sheet's key shadows every weather fact and freezes the base's
    /// timers, so the UPDATING cue, the freshness line and the data itself are all invisible until
    /// the sheet closes. Closing is therefore the frame the press produced.
    ///
    /// A **switch** row is the other way round: it flips its bit and stays, because the row draws
    /// its own state and the screen underneath is worth exactly one repaint however many bits move.
    fn open(self, cx: &mut Ctx) -> Option<Transition> {
        Some(Transition::Replace(match self {
            ContextAction::UpAhead => {
                // The list always opens on **Everything** (epic #946, U3): the filter is selection
                // state, cleared on entry exactly as the rain map's step is, so a category the
                // rider chose one ride never silently empties the list on the next.
                cx.state.up_ahead_filter = PoiCategorySet::ALL;
                Screen::UpAhead(UpAheadScreen::new(cx.navigator.route_state().progress_m))
            }
            ContextAction::Detour => Screen::Detour(DetourScreen::new(cx.navigator.route_state())),
            ContextAction::Pois => Screen::PoiMenu(PoiMenuScreen::new()),
            ContextAction::Routes => Screen::RouteMenu(RouteMenuScreen::new()),
            ContextAction::RefreshWeather => {
                cx.weather.apply_intent(crate::weather::WeatherIntent::RefreshRequested);
                return Some(Transition::Pop);
            }
            // The shorter sheet takes the taller one's place, already landed.
            ContextAction::MapDisplay => {
                Screen::ContextDrawer(ContextDrawerScreen::swapped_in(&MAP_DISPLAY, cx.now_ms))
            }
            ContextAction::Toggle(t) => {
                t.flip(cx);
                return Some(Transition::None);
            }
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

/// The **map context** (#1515 D4c): the ride's four secondary actions, in the order every riding
/// view offers them, plus the one row only the Map has a referent for — its display modifiers.
/// Rows 0-3 are [`RIDE`]'s, unchanged and pinned equal by test, so a rider who squeezes on the Map
/// and a rider who squeezes on Statistics reach the same actions by the same steps.
pub static MAP: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::RideContextUpAhead, action: ContextAction::UpAhead },
        ContextRow { label: Msg::RideContextDetour, action: ContextAction::Detour },
        ContextRow { label: Msg::MenuPois, action: ContextAction::Pois },
        ContextRow { label: Msg::MenuRoutes, action: ContextAction::Routes },
        ContextRow { label: Msg::MapContextMapDisplay, action: ContextAction::MapDisplay },
    ],
};

/// The **map display sheet** (#1515 D4c): the three switches that change nothing but what the Map
/// draws. The only home any of them has — before this they were three rows two levels inside the
/// central Settings tree, on a page the rider could only reach by leaving the map.
pub static MAP_DISPLAY: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::MapContextClock, action: ContextAction::Toggle(ContextToggle::MapClock) },
        ContextRow { label: Msg::MapContextScaleBar, action: ContextAction::Toggle(ContextToggle::MapScaleBar) },
        ContextRow { label: Msg::MapContextContours, action: ContextAction::Toggle(ContextToggle::MapContours) },
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

/// The **weather context** (#1515 D4b): ask for a bundle now, and set how often the device asks on
/// its own. The only home either control has — before this the interval was a whole settings screen
/// and the manual refresh did not exist at all, so the only way to ask again was to leave the
/// weather screens and come back in through the Menu.
pub static WEATHER: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::WeatherContextRefreshNow, action: ContextAction::RefreshWeather },
        ContextRow { label: Msg::WeatherContextInterval, action: ContextAction::Edit(ContextValue::WeatherInterval) },
    ],
};

/// The **route-plan context** (#1515 D4d): the profile the on-device planner will weight edges by,
/// offered on the card that is about to ask for a plan — and the only home it has. One row, because
/// there is one thing to say: `NavPlanner::new` takes the profile and nothing else, so a second
/// "route options" row would be a label bound to nothing.
pub static ROUTE_PLAN: ContextMenu = ContextMenu {
    rows: &[ContextRow { label: Msg::RouteContextBikeType, action: ContextAction::Edit(ContextValue::BikeProfile) }],
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
    /// When the open slide started — the clock of the **first frame that could draw the sheet**,
    /// and `None` until one has.
    ///
    /// The sheet is not handed a clock when it is built, and that is the fix for #1569. A chord is
    /// resolved *above* the pass, before the pass sets its `now_ms`, so a sheet stamped at
    /// construction carries the clock of the pass **before** the squeeze. On a host whose frames
    /// gap — the board's Map sleeps until something happens — that is seconds old, the first frame
    /// computes an elapsed far past [`OPEN_MS`], and the sheet is drawn already landed: the open
    /// cuts. Starting the clock on the first tick makes the open begin where it can first be seen,
    /// on every host and with no host having to say anything.
    opened_ms: Option<u32>,
    menu: &'static ContextMenu,
    /// When the page transition in flight started, or `None` when none is.
    slide_ms: Option<u32>,
    selected: u8,
    page: Page,
    /// The choice the editor is previewing. Meaningful only on [`Page::Editor`]; off that page
    /// every reader falls back to the committed value, which is what makes Back-discards free.
    staged: u8,
    /// How much of the sheet the last reported tick put on the panel, in device pixels; `-1` before
    /// the first one.
    ///
    /// It is what makes the open **motion** rather than a cut (#1559). A step that would redraw the
    /// sheet where it already stands is not reported at all — the bench measured whole renders
    /// pushing zero rows — and the frame the sheet lands on is reported exactly when it moves the
    /// sheet, which is what the old `landed` edge was approximating.
    shown_h: i16,
    /// The draw of the screen below that this sheet **owes** — see [`needs_base`](Self::needs_base).
    needs_base: bool,
}

impl ContextDrawerScreen {
    /// A drawer over `menu` that has begun to open, with the first row selected. Its slide starts
    /// on the first frame that ticks it — see [`opened_ms`](Self::opened_ms).
    pub fn opening(menu: &'static ContextMenu) -> Self {
        debug_assert!(menu.rows.len() <= MAX_ROWS, "a context table is a sheet, not a page — see MAX_ROWS");
        ContextDrawerScreen {
            opened_ms: None,
            menu,
            slide_ms: None,
            selected: 0,
            page: Page::Root,
            staged: 0,
            shown_h: -1,
            needs_base: false,
        }
    }

    /// A drawer over `menu` that is **already landed** — the sheet a row of another sheet swapped in
    /// (#1515 D4c). A sheet that is on the panel does not make an entrance, and re-running a 440 ms
    /// open to change tables would read as a stutter, so the open's clock is stamped as spent: the
    /// frame the press produced draws the new table at full height.
    ///
    /// The sheet owes the screen below one draw from here: the incoming sheet is shorter than the
    /// one it replaced, so it gives back a band still holding the old sheet's ink. The debt is
    /// armed here rather than left to the first tick, which would be one frame late — the same
    /// reason [`slide_to`](Self::slide_to) arms it eagerly — and it stands until a frame pays it.
    pub fn swapped_in(menu: &'static ContextMenu, now_ms: u32) -> Self {
        ContextDrawerScreen {
            opened_ms: Some(now_ms.wrapping_sub(OPEN_MS)),
            needs_base: true,
            ..ContextDrawerScreen::opening(menu)
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
        // A value row reports its committed ordinal; a switch row reports its bit. Both are "what
        // the selected row is set to", which is the only per-row state either shape draws.
        let committed = match self.menu.rows.get(self.selected as usize).map(|r| r.action) {
            Some(ContextAction::Edit(v)) => v.committed(f),
            Some(ContextAction::Toggle(t)) => t.read(f) as u8,
            _ => 0,
        };
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
        // fast double-press land on a row the rider cannot see yet. Asked, never retired — see
        // [`slide_running`](Self::slide_running).
        if self.slide_running(cx.now_ms) {
            return Transition::None;
        }
        match self.page {
            Page::Root => self.handle_root(g, cx),
            Page::Editor => self.handle_editor(g, cx),
        }
    }

    /// Whether a page slide is still in flight at `now_ms` — the input gate's question, asked
    /// without answering the tick's (#1515 D5).
    ///
    /// Retiring a slide is [`settle`](Self::settle)'s edge, and that edge is what
    /// [`tick_timers`](Self::tick_timers) reads to arm the base draw the settling frame owes. Input
    /// runs first in a pass, so a gesture landing at or after the slide's end used to retire the
    /// slide silently and leave the tick nothing to read: the sheet kept its two pages' ink in the
    /// margin either side of it, or — with no render key moved — asked for no repaint at all and
    /// stayed half-slid. The gate is a pure read, so the gesture is accepted exactly as before.
    fn slide_running(&self, now_ms: u32) -> bool {
        self.slide_ms.is_some_and(|s| now_ms.wrapping_sub(s) < SLIDE_MS)
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
                let count = value.count(&cx.context_facts()) as usize;
                self.staged = super::vocab::list::step_selection(self.staged as usize, n, count) as u8;
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

    /// The sheet's animation: the open slide, then any page slide, at the panel's step cadence.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        // This frame is the open's origin if no frame has been one yet (#1569).
        let opened_ms = *self.opened_ms.get_or_insert(now_ms);
        let settled = self.settle(now_ms);
        let sheet_h = self.sheet_height(now_ms);
        let visible = self.visible_height(now_ms, sheet_h);
        // The open is over when the sheet has **arrived**, not when its clock runs out: the
        // ease-out's last few per cent move no pixel, and the steps they would ask for are whole
        // renders that push nothing.
        let opening =
            if sheet_h > 0 && visible >= sheet_h { 0 } else { OPEN_MS.saturating_sub(now_ms.wrapping_sub(opened_ms)) };
        let sliding = self.slide_ms.map_or(0, |s| SLIDE_MS.saturating_sub(now_ms.wrapping_sub(s)));
        let moved = visible != self.shown_h as i32;
        // The base draw this sheet owes is a **debt**, so this adds to it and never clears it: a
        // pass may tick and then draw no frame at all, and only a frame that drew the base ends the
        // obligation ([`needs_base`](Self::needs_base)).
        self.needs_base |= sliding > 0 || settled;
        self.shown_h = visible as i16;
        // The wake is the time to the **next step boundary**, not a whole step from wherever this
        // poll happened to land: the sheet advances on those boundaries, so asking for a full step
        // off one carries the offset to the end and finishes the open a step late.
        let to_step = STEP_MS - now_ms.wrapping_sub(opened_ms) % STEP_MS;
        match [opening, sliding].into_iter().filter(|r| *r > 0).min() {
            // A page slide moves its two pages across a sheet that may not change height at all, so
            // it is a change whether or not the sheet grew.
            Some(remaining) => {
                ScreenTick { changed: sliding > 0 || moved, next_wake_ms: Some(to_step.min(remaining)), region: None }
            }
            // The frame a slide ends on still differs from the one before it; the frame the open
            // ends on differs only if it moved the sheet, and reporting one that did not is a whole
            // render spent on nothing.
            None if settled || moved => ScreenTick { changed: true, next_wake_ms: None, region: None },
            None => ScreenTick::idle(),
        }
    }

    /// Whether this sheet still **owes** the screen below a draw (#1559, #1515 D5).
    ///
    /// A sheet owes one from the moment it stops purely covering the base. Two things do that. A
    /// **page slide**: its two pages travel through the inset margin either side of the sheet,
    /// where the base shows, so every frame of one — including the frame it settles on, which is
    /// the last that can leave ink there — needs the base under it, and a slide between pages of
    /// different heights also *shrinks* the sheet, whose given-back rows the same draw puts back.
    /// And a **swapped-in** sheet ([`swapped_in`](Self::swapped_in)), which is shorter than the one
    /// it replaced and gives a band back at once.
    ///
    /// It is a **debt, not a flag**: nothing but
    /// [`clear_base_debt`](Self::clear_base_debt) — called by the frame that actually drew the base
    /// — ends it. A tick that decided it per frame could have the obligation stolen from under it
    /// by a pass that ticked and drew nothing, or by input running first and retiring the slide
    /// before the tick could see the edge.
    pub(crate) fn needs_base(&self) -> bool {
        self.needs_base
    }

    /// Discharge the debt: the frame that drew the base has put back everything this sheet was not
    /// covering. Called at the frame boundary, which is the only place that answer exists.
    pub(crate) fn clear_base_debt(&mut self) {
        self.needs_base = false;
    }

    /// Begin a horizontal transition to `to`, which becomes the live page at once (so `handle` and
    /// the render key already speak about the destination) while the slide draws both.
    fn slide_to(&mut self, to: Page, now_ms: u32) {
        self.slide_ms = Some(now_ms);
        self.page = to;
        // From this frame on the two pages travel outside the sheet's own footprint, so the base
        // has to be under them — armed here rather than at the next tick, which would be one frame
        // late.
        self.needs_base = true;
    }

    /// Retire a finished slide. Returns whether this call is the one that retired it. The **tick**
    /// calls this and nothing else does: the edge it returns is what arms the settling frame's
    /// base draw (see [`slide_running`](Self::slide_running)).
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

    /// How much of the sheet has arrived from the bottom edge, on the open animation's ease-out —
    /// advanced in whole [`STEP_MS`] steps.
    ///
    /// The quantising is the pacing (#1559). A device wakes on more than its own timers, and a
    /// sheet that answered the raw clock would give a busy host a hundred one-pixel steps of a
    /// 104 px sheet, each one a whole frame the panel cannot finish. Reading the step boundary
    /// instead means the sheet moves exactly as often as it asked to be woken, and a wake between
    /// two steps draws the frame that is already there — which the tick then does not ask for.
    fn visible_height(&self, now_ms: u32, sheet_h: i32) -> i32 {
        // Before the first tick the open has not started, so a host that draws a sheet it has not
        // ticked draws no sheet — which is the frame the open begins from anyway.
        let Some(opened_ms) = self.opened_ms else { return 0 };
        let elapsed = now_ms.wrapping_sub(opened_ms);
        // The frame the sheet opens on is its **first step**, not a frame that draws nothing: the
        // chord costs the host a repaint whatever this returns, and #1559's rule is that no frame
        // of the open is spent on nothing. So a step boundary is counted from one step in.
        let stepped = (elapsed / STEP_MS + 1) * STEP_MS;
        (sheet_h as f32 * sheet::arrived(stepped, 0, OPEN_MS) + 0.5) as i32
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
    /// **A switch row draws its state instead of a chevron**, because it goes nowhere and because
    /// its state is the whole feedback it gets: the screen under the sheet is frozen, so the slider
    /// moving is the only thing a flip can change until the sheet closes. It is the settings tree's
    /// own 50 × 28 slider, from the shared row vocabulary, so the control the rider learned in one
    /// place looks the same in the other.
    ///
    /// **The row does not state its value, and that is measured rather than chosen.** A label plus
    /// the longest choice is 284 px of `Body` glyphs on a 204 px row (`de`'s *Campingplatz*, `es`'s
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
            match row.action {
                ContextAction::Toggle(t) => super::vocab::rows::toggle_slider(cv, area, t.read(&facts)),
                _ if live => {
                    let right = area.top_left.x + area.size.width as i32;
                    let (cx0, cy) = (right - 18, area.top_left.y + (ROW_H - 4) / 2);
                    cv.triangle(Point::new(cx0, cy - 8), Point::new(cx0, cy + 8), Point::new(cx0 + 9, cy), ink);
                }
                _ => {}
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
        let facts = rx.context_facts();
        let count = value.count(&facts);
        let committed = value.committed(&facts);
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
        navigator: crate::navigator::NavigatorMachine,
        settings: Settings,
        /// The ride the rider is on, so a row press can be held to leaving it alone (#1554 moved
        /// the session out of `Activity` and into this machine).
        recorder: RecorderMachine,
        /// The weather domain the sheet's Refresh row talks to — the real one, so "exactly one
        /// request" is asserted against the coalescing the domain actually does.
        weather: crate::weather::WeatherDomain,
        /// The loaded map's §8.6 profile names — the bike-type binding's whole choice list, and
        /// the predicate its row is live by. Four by default, the fixture maps' own set.
        nav_profiles: crate::NavProfiles,
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
            let activity = Activity::new(Mode::Riding);
            let mut navigator = crate::navigator::NavigatorMachine::new();
            navigator.set_active_route(Some(0));
            let mut recorder = RecorderMachine::new();
            recorder.test_open();
            World {
                state,
                activity,
                navigator,
                settings: Settings::default(),
                recorder,
                weather: crate::weather::WeatherDomain::new(),
                nav_profiles: crate::NavProfiles::from_names(&["Road", "Gravel", "MTB", "Touring"]),
                now_ms: 1_000,
            }
        }

        fn press(&mut self, d: &mut ContextDrawerScreen, g: Gesture) -> Transition {
            let now_ms = self.now_ms;
            let t = d.handle(
                g,
                &mut Ctx {
                    recorder: &mut self.recorder,
                    navigator: &mut self.navigator,
                    weather: &mut self.weather,
                    nav_profiles: &self.nav_profiles,
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
                navigation: self.navigator.route_state(),
                settings: &self.settings,
                recording: self.recorder.recording(),
                weather_request_outstanding: self.weather.request_outstanding(),
                nav_profiles: &self.nav_profiles,
            }
        }
    }

    fn drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::opening(&RIDE)
    }

    fn up_ahead_drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::opening(&UP_AHEAD)
    }

    fn weather_drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::opening(&WEATHER)
    }

    fn map_drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::opening(&MAP)
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
            (|w: &mut World| w.navigator.set_active_route(None)) as fn(&mut World),
            |w: &mut World| w.state.has_nav_graph = false,
            |w: &mut World| w.navigator.route_state_mut().off_route = true,
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
        assert!(
            w.navigator.route_state().active_route.is_some()
                && w.state.has_nav_graph
                && !w.navigator.route_state().off_route
        );

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1)); // → the Detour row
        assert_eq!(d.key(&w.facts()).4 & (1 << 1), 0, "the row draws recessed");
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "…and a press does nothing");

        // The predicate the row read is the one the chooser reads: identical inputs, identical
        // answer, so the row cannot become an enabled door onto a screen that opens inert.
        assert!(!super::super::detour::reachable(
            w.navigator.route_state(),
            w.recorder.recording(),
            w.state.has_nav_graph
        ));
        assert!(
            ContextAction::Detour.available(&w.facts())
                == super::super::detour::reachable(
                    w.navigator.route_state(),
                    w.recorder.recording(),
                    w.state.has_nav_graph
                ),
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
        w.navigator.route_state_mut().progress_m = 4_200;
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
        // The derivation itself, so a geometry change is read here rather than asserted twice.
        assert_eq!(MAX_ROWS, 5, "24 px of padding plus 44 px rows inside a {MAX_SHEET_H} px sheet");

        // Every table the tree declares; each D4 slice added its own to this list.
        let declared: &[&ContextMenu] = &[&RIDE, &MAP, &MAP_DISPLAY, &UP_AHEAD, &WEATHER, &ROUTE_PLAN];
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
        let mut d = ContextDrawerScreen::opening(&RIDE);
        d.tick_timers(1_000); // the frame the open starts on
        let target = Page::Root.height(&RIDE);
        let frames: heapless::Vec<i32, 8> =
            [0, 55, 110, 165, OPEN_MS].iter().map(|dt| d.visible_height(1_000 + dt, target)).collect();
        assert!(frames[0] > 0, "the sheet's first step is on the frame it opens on, not one step later");
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

        let facts = w.facts();
        assert_eq!(ContextValue::UpAheadFilter.count(&facts), 7);
        assert_eq!(ContextValue::UpAheadSource.count(&facts), UpAheadSource::COUNT as u8);
        for ordinal in 0..ContextValue::UpAheadFilter.count(&facts) {
            assert_eq!(filter_choice(choice_filter(ordinal)), ordinal, "ordinal {ordinal} round-trips");
        }
        assert_eq!(choice_filter(0), PoiCategorySet::ALL, "ordinal 0 is Everything");
    }

    /// **The one-predicate rule for value rows.** A binding that always accepts must give a row
    /// that is always live: a browse map with no route, no graph and no ride still lets the rider
    /// see and change what the timeline is scoped to. And a binding that *refuses* must give a row
    /// that is recessed — [`ContextValue::BikeProfile`] is the first that ever answers `false`, so
    /// this is where the rule is checked in both directions rather than in one.
    #[test]
    fn a_value_row_is_live_exactly_where_its_binding_accepts() {
        let mut w = World::riding();
        w.navigator.set_active_route(None);
        w.state.has_nav_graph = false;
        w.recorder.test_close();
        w.nav_profiles = crate::NavProfiles::EMPTY; // …and no map, so the bike binding refuses
        let d = up_ahead_drawer();
        let facts = w.facts();

        assert_eq!(d.key(&facts).4, 0b11, "both value rows stay live on a bare browse map");
        for menu in [&UP_AHEAD, &ROUTE_PLAN] {
            for row in menu.rows {
                let ContextAction::Edit(v) = row.action else { panic!("these tables are all value rows") };
                assert_eq!(row.action.available(&facts), v.accepts(&facts), "the row reads the binding's own answer");
            }
        }
        assert_eq!(
            ContextDrawerScreen::opening(&ROUTE_PLAN).key(&facts).4,
            0,
            "…and the one binding that refuses leaves its row out of the live mask"
        );

        // And a press really does open the editor there, rather than drawing live and doing nothing.
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor);
    }

    /// **The open is paced by the two constants and nothing else** (#1559): the sheet asks to be
    /// woken every [`STEP_MS`], it takes about [`OPEN_MS`] to arrive, and every step it reports
    /// moves it. The twin of the quick drawer's test, on this sheet's own pair of numbers — which
    /// is the point of the numbers being per sheet.
    #[test]
    fn the_open_takes_open_ms_in_steps_of_step_ms_and_every_step_moves_the_sheet() {
        let mut d = drawer();
        let target = Page::Root.height(&RIDE);
        let (mut ms, mut heights) = (0u32, std::vec::Vec::new());
        // Poll at 1 ms, the finest any host could: a wake between two steps must cost nothing.
        while ms < OPEN_MS * 2 {
            if d.tick_timers(ms).changed {
                heights.push(d.visible_height(ms, target));
            }
            ms += 1;
        }
        assert!(heights.windows(2).all(|p| p[0] < p[1]), "no step redraws the sheet where it stands: {heights:?}");
        assert_eq!(heights.last(), Some(&target), "the last step is the sheet landed");
        let steps = heights.len() as u32;
        assert!(
            (OPEN_MS / STEP_MS / 2..=OPEN_MS / STEP_MS + 1).contains(&steps),
            "{steps} steps for a {OPEN_MS} ms open at a {STEP_MS} ms cadence"
        );
        assert!(steps >= 7, "an open that reads as motion is many steps, not the four the panel used to show");

        // …and then it is silent, which is what the frozen base under it depends on.
        for ms in OPEN_MS * 2..OPEN_MS * 2 + 300 {
            assert_eq!(d.tick_timers(ms), ScreenTick::idle(), "a landed sheet is quiet at {ms} ms");
        }
        assert!(!d.needs_base(), "…and asks for nothing under it either");
    }

    /// The wake asks for the **next step boundary**, not a whole step from wherever the poll landed
    /// — otherwise a device that wakes off-boundary carries the offset to the end and finishes the
    /// open a step late. The mutant is `STEP_MS.min(remaining)`.
    #[test]
    fn the_wake_lands_on_the_next_step_boundary() {
        let mut d = drawer();
        d.tick_timers(0); // the frame the open starts on
        assert_eq!(d.tick_timers(STEP_MS + 5).next_wake_ms, Some(STEP_MS - 5), "five into a step, ask for the rest");
        assert_eq!(d.tick_timers(STEP_MS * 2).next_wake_ms, Some(STEP_MS), "on a boundary, ask for a whole step");
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

        let mut d = ContextDrawerScreen::opening(&UP_AHEAD);
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

    /// The sheet is all copy, so this is its overflow check: every row label clears its own
    /// right-hand control on a 240 px panel in every language, and every editor choice fits the
    /// editor's own line.
    ///
    /// **There are two row budgets, not one** (#1515 D4c). A door or a value row clears the 18 px
    /// chevron and has 164 px; a **switch** row clears the 50 px slider and its margin and has 128.
    /// That is where the map sheet's copy is decided — the settings rows these three switches came
    /// from had a second line to split `Courbes de niveau` / `Curvas de nivel` across, and a sheet
    /// row does not. If a column overruns, the copy shortens; the slider and the row do not.
    ///
    /// **Both are measured in the font the code actually draws.** `draw_root` writes a row label in
    /// [`Font::Body`] and `draw_editor` writes a choice in [`Font::Body`] too — an earlier version
    /// of this test measured the choice in `Font::Label` and believed 36 px of clearance where the
    /// panel has 12 (`de` "Campingplatz" is 168 px in Body, 144 in Label).
    ///
    /// It also **pins the measurement the row design rests on** — that a label plus its longest
    /// choice does not fit one row — so a future geometry pass sees the number rather than the
    /// conclusion.
    ///
    /// **[`ContextValue::BikeProfile`]'s choices are not catalog copy** (#1515 D4d): they are the
    /// loaded map's §8.6 names, so the check on them is the format's own cap rather than four
    /// columns of a string. That cap turns out to be exactly the widest catalog choice, which is
    /// why the two numbers below do not move for it.
    #[test]
    fn every_label_and_choice_fits_the_sheet_in_every_language() {
        use obc_formats::obcm::NAV_PROFILE_NAME_LEN;
        const W: i32 = 240;
        const MIN_CLEAR: i32 = 8;
        // The draw's own geometry: the row area starts 14 px into the sheet and is `w - 36` wide;
        // the label starts 14 px inside it, the chevron takes the last 18 and the slider the last
        // 54 (50 px wide, 4 px margin).
        let area_w = W - 36;
        let door_room = area_w - 14 - 18 - MIN_CLEAR;
        let switch_room = area_w - 14 - 54 - MIN_CLEAR;
        assert_eq!((door_room, switch_room), (164, 128), "the two row budgets, pinned");
        // `draw_editor` starts the choice at x + 48 with a category icon in the gutter, and the
        // sheet's own right inset is 12.
        let choice_room = W - 48 - 12;
        let w = World::riding();
        let facts = w.facts();
        let (mut worst_row, mut worst_choice) = (0, 0);
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            for menu in [&RIDE, &MAP, &MAP_DISPLAY, &UP_AHEAD, &WEATHER, &ROUTE_PLAN] {
                for row in menu.rows {
                    let label = t(row.label, lang);
                    let lw = text_width(label, Font::Body) as i32;
                    let room = match row.action {
                        ContextAction::Toggle(_) => switch_room,
                        _ => door_room,
                    };
                    assert!(lw <= room, "{lang:?}: row label {label:?} ({lw} px) overruns {room} px");
                    let ContextAction::Edit(v) = row.action else { continue };
                    // The map's names are not in the catalog; they are measured at their cap below.
                    if v == ContextValue::BikeProfile {
                        continue;
                    }
                    for ordinal in 0..v.count(&facts) {
                        let choice = choice_text(v, ordinal, lang);
                        let cw = text_width(choice, Font::Body) as i32;
                        assert!(cw <= choice_room, "{lang:?}: choice {choice:?} ({cw} px) overruns {choice_room} px");
                        worst_choice = worst_choice.max(cw);
                        worst_row = worst_row.max(14 + lw + MIN_CLEAR + cw + 10);
                    }
                }
            }
        }
        // The bike binding's worst case is the §8.6 name field filled: 12 monospace `Body`
        // characters. It fits the same line the catalog choices do — and it is *exactly* the widest
        // of them, so the `worst_choice` pin below does not move for it. The `worst_row` figure
        // below is catalog-only (the loop skips map-data choices): the bike row's own label+name
        // worst is 340 px, and it is governed by this editor-line check, not by the row figure.
        // If it ever exceeds `choice_room`, either a font tier or the format's name cap changed,
        // and that is a finding rather than a re-pin.
        let widest_profile_name = NAV_PROFILE_NAME_LEN as i32 * Font::Body.char_width() as i32;
        assert_eq!(widest_profile_name, 168, "12 §8.6 name bytes in Body, pinned");
        assert!(widest_profile_name <= choice_room, "a full-length profile name overruns the editor line");

        // The widest choice on the editor line, and how little room is left over it. This is the
        // on-glass question the PR names, so the number is here rather than in prose.
        assert_eq!(worst_choice, 168, "de \"Campingplatz\" / \"Fahrradladen\" in Body, pinned");
        assert_eq!(choice_room - worst_choice, 12, "…with 12 px to spare on the 240 px panel");

        assert!(
            worst_row > area_w,
            "a label and its longest choice now fit one {area_w} px row ({worst_row} px) — \
             the row could state its value again; see `draw_root`"
        );
        assert_eq!(worst_row, 284, "the widest catalog label+choice pair — the bike row's map-data worst is 340 px");
    }

    /// The catalog lookup [`ContextValue::choice_label`] makes, without a `Render` to hang it off.
    /// [`ContextValue::BikeProfile`] has none — its choices are map data — so it is not a case here.
    fn choice_text(v: ContextValue, ordinal: u8, lang: Language) -> &'static str {
        match v {
            ContextValue::UpAheadFilter => match choice_category(ordinal) {
                Some(cat) => t(super::super::poi_menu::category_msg(cat), lang),
                None => t(Msg::UpAheadEverything, lang),
            },
            ContextValue::UpAheadSource => UpAheadSource::ALL[ordinal as usize].name(lang),
            ContextValue::WeatherInterval => WeatherRefresh::ALL[ordinal as usize].name(lang),
            ContextValue::BikeProfile => unreachable!("the map's own names are measured at their §8.6 cap"),
        }
    }

    // ---- D4b: the weather context --------------------------------------------------------------

    /// **The Refresh row asks once and leaves.** A live press raises exactly one intent and pops
    /// the sheet; with a request already outstanding the row is out of the `enabled` mask, a press
    /// does nothing at all, and nothing further is asked.
    ///
    /// The mutants: a row that returns `Transition::None` (a control whose whole effect is hidden
    /// behind the frozen base), and an `available` that ignores the outstanding request (a live row
    /// whose press the domain silently drops).
    #[test]
    fn the_refresh_row_asks_once_and_leaves_the_sheet() {
        let mut w = World::riding();
        let mut d = weather_drawer();
        assert_eq!(d.key(&w.facts()).4 & 1, 1, "with nothing outstanding the row is live");
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Pop), "the row acts, then leaves the sheet");
        assert!(w.weather.refresh_pending(), "…having raised exactly one request");
        assert!(w.weather.request_outstanding());

        // The row is now inert, and a second press is not a second question.
        let mut d = weather_drawer();
        assert_eq!(d.key(&w.facts()).4 & 1, 0, "a request outstanding draws the row recessed");
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "…and a press does nothing at all");
        assert_eq!(d.page, Page::Root, "not even a page slide");

        // The Interval row beside it is unaffected: availability is per row, not per sheet.
        assert_eq!(d.key(&w.facts()).4 & 0b10, 0b10, "the value row stays live");
    }

    /// **The Interval editor is the D4a editor over the persisted field.** It opens on
    /// `Settings::weather_refresh`, staging writes nothing, Select writes the field, Back out of a
    /// re-opened editor discards, and the key reports the staged and the committed ordinal apart.
    #[test]
    fn the_interval_editor_opens_on_the_persisted_value_and_commits_it() {
        let mut w = World::riding();
        w.settings.weather_refresh = WeatherRefresh::Every60;
        let mut d = weather_drawer();
        w.press(&mut d, Gesture::Step(1)); // → the Interval row
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor);
        assert_eq!(d.staged, WeatherRefresh::Every60 as u8, "the editor opens on the persisted value");

        w.press(&mut d, Gesture::Step(-1)); // → 30 min
        assert_eq!(w.settings.weather_refresh, WeatherRefresh::Every60, "staging commits nothing");
        let (_, _, staged, committed, _) = d.key(&w.facts());
        assert_eq!((staged, committed), (WeatherRefresh::Every30 as u8, WeatherRefresh::Every60 as u8));

        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Root, "Select returns to the row table");
        assert_eq!(w.settings.weather_refresh, WeatherRefresh::Every30, "…having written the settings field");

        // Back out of a re-opened editor discards the staged choice.
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor, "the press re-opened the editor");
        assert_eq!(d.staged, WeatherRefresh::Every30 as u8, "re-opens on what is now committed");
        w.press(&mut d, Gesture::Step(2));
        w.press(&mut d, Gesture::Back);
        assert_eq!(d.page, Page::Root, "Back closes the editor, not the sheet");
        assert_eq!(w.settings.weather_refresh, WeatherRefresh::Every30, "…and the field is untouched");

        // The whole ring is exactly the five named intervals, and the editor never leaves it.
        assert_eq!(ContextValue::WeatherInterval.count(&w.facts()), WeatherRefresh::COUNT as u8);
        assert_eq!(WeatherRefresh::COUNT, 5);
    }

    // ---- D4c: the map context --------------------------------------------------------------

    /// **A switch row flips in place and keeps the sheet.** Each of the three flips its own field
    /// both ways, returns [`Transition::None`], leaves the page on the root and the other two fields
    /// alone — and the key follows the selected row's bit through `committed`, which is the one byte
    /// that makes a flip visible to the frame's identity.
    ///
    /// The mutants: a row that pops or replaces (the rider would set one switch per squeeze), and a
    /// `key` that reports 0 for a switch row (the slider would not move until the sheet closed).
    #[test]
    fn a_toggle_row_flips_in_place_and_keeps_the_sheet() {
        for (i, toggle) in MAP_DISPLAY.rows.iter().enumerate() {
            let ContextAction::Toggle(t) = toggle.action else { panic!("the display sheet is all switch rows") };
            let mut w = World::riding();
            let mut d = ContextDrawerScreen::opening(&MAP_DISPLAY);
            w.press(&mut d, Gesture::Step(i as i32));
            assert_eq!(d.key(&w.facts()).3, 1, "all three default on");

            assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "the sheet stays up");
            assert_eq!(d.page, Page::Root, "…on its root page: no editor, no slide");
            assert!(!t.read(&w.facts()), "on -> off");
            assert_eq!(d.key(&w.facts()).3, 0, "…and the key carries the selected row's new state");

            // The other two are untouched: a flip is one bit, not a sheet-wide act.
            let others = MAP_DISPLAY.rows.iter().enumerate().filter(|(j, _)| *j != i);
            for (_, row) in others {
                let ContextAction::Toggle(other) = row.action else { unreachable!() };
                assert!(other.read(&w.facts()), "the other switches are untouched");
            }

            w.press(&mut d, Gesture::Press);
            assert!(t.read(&w.facts()), "off -> on again, from the same row");
            assert_eq!(d.key(&w.facts()).4, 0b111, "every switch row is always live");
        }
    }

    /// **The Map display row swaps one sheet for another**, and the swap is free of an entrance:
    /// the shorter sheet is landed on the frame the press produced, that frame owes the screen
    /// below one draw (it uncovers a band the taller sheet held), and Back leaves the sheet family
    /// altogether rather than climbing to the table it came from.
    ///
    /// **What ends the obligation is the draw, not the next tick** (#1515 D5). Ticking again does
    /// not put the band back, so the debt survives every tick until
    /// [`clear_base_debt`](ContextDrawerScreen::clear_base_debt) — which is what the frame that
    /// drew the base calls — and no tick after that re-arms it: the swap still costs exactly one
    /// map draw.
    #[test]
    fn the_display_row_swaps_the_sheet_and_back_lands_on_the_map() {
        let mut w = World::riding();
        let mut d = map_drawer();
        w.press(&mut d, Gesture::Step(4)); // → the Map display row
        let Transition::Replace(Screen::ContextDrawer(mut swapped)) = w.press(&mut d, Gesture::Press) else {
            panic!("row 4 did not replace the sheet with the display sheet")
        };

        // Landed on its first frame: `visible_height` is already the whole table, and the tick
        // reports no further wake — a second open animation would show up as both.
        let target = Page::Root.height(&MAP_DISPLAY);
        let first = swapped.tick_timers(w.now_ms);
        assert_eq!(swapped.visible_height(w.now_ms, target), target, "the swapped-in sheet is already landed");
        assert_eq!(first.next_wake_ms, None, "…so it asks for no open steps");
        assert!(swapped.needs_base(), "its first frame uncovers the band the taller sheet held");
        swapped.tick_timers(w.now_ms + 16);
        assert!(swapped.needs_base(), "…and a tick that drew no frame does not put the band back");
        swapped.clear_base_debt();
        swapped.tick_timers(w.now_ms + 32);
        assert!(!swapped.needs_base(), "the draw ends it, and nothing re-arms it: the swap costs exactly one");

        assert!(matches!(w.press(&mut swapped, Gesture::Back), Transition::Pop), "Back closes onto the Map");
    }

    /// **A gesture landing as the slide lands does not take the base draw with it** (#1515 D5).
    ///
    /// Input runs before the tick in one pass, so a gesture at or after `slide start + SLIDE_MS` —
    /// well inside an ordinary double-tap — used to retire the slide itself, through the `settle`
    /// call `handle` opened with. The tick then found no edge and *assigned* `needs_base = false`.
    ///
    /// This drives the sharper of the two faces. The sheet is D4d's one-row `ROUTE_PLAN`, where
    /// `step_selection(0, n, 1) == 0`, so the stealing gesture moves no render key at all: the tick
    /// went on to return [`ScreenTick::idle`] and the two pages stayed **half-slid** until something
    /// else asked for a frame. The other face — a moved key, a repainted sheet, and the outgoing
    /// page's ink left in the 4 px margin either side — is the same lost `settled` edge.
    ///
    /// Every frame of the slide is modelled as it really runs: it draws the base, so it discharges
    /// the debt, and the next tick has to arm it again. The mutant is `self.settle(cx.now_ms)` back
    /// at the top of `handle`: both halves below fail.
    #[test]
    fn a_press_as_the_slide_lands_does_not_spend_the_base_draw_it_owes() {
        let mut w = World::riding();
        let mut d = route_plan_drawer();
        d.tick_timers(w.now_ms.saturating_sub(OPEN_MS)); // the open's origin, so the sheet is landed
        w.press(&mut d, Gesture::Press); // → the bike-type editor; `press` steps the clock past it
        assert_eq!(d.page, Page::Editor);

        // Back out: the sheet shrinks 148 -> 68, so this slide gives rows back as well as travelling
        // through the margin.
        let start = w.now_ms;
        d.handle(
            Gesture::Back,
            &mut Ctx {
                recorder: &mut w.recorder,
                navigator: &mut w.navigator,
                weather: &mut w.weather,
                nav_profiles: &w.nav_profiles,
                now_ms: start,
                ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings)
            },
        );
        for ms in start..start + SLIDE_MS {
            assert!(d.tick_timers(ms).changed, "a frame of the slide is a frame the host renders");
            assert!(d.needs_base(), "…and it is drawn over the base, at {ms} ms");
            d.clear_base_debt();
        }

        // The settling frame, with a gesture landing on exactly it.
        let landed = start + SLIDE_MS;
        d.handle(
            Gesture::Step(1),
            &mut Ctx {
                recorder: &mut w.recorder,
                navigator: &mut w.navigator,
                weather: &mut w.weather,
                nav_profiles: &w.nav_profiles,
                now_ms: landed,
                ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings)
            },
        );
        assert_eq!(d.selected, 0, "a one-row table: the gesture moves nothing the key can see");
        let tick = d.tick_timers(landed);
        assert!(d.needs_base(), "the settling frame still owes the margin the two pages travelled through");
        assert!(tick.changed, "…and is still asked for, so the pages do not stay half-slid");
    }

    /// **A pass that ticks and draws no frame keeps the draw it owes** (#1515 D5).
    ///
    /// The board can tick and then drop the frame — the render scratch arena is held, or a weather
    /// bind failed — and the retry pass must still draw the base the sheet uncovered. Expressed
    /// where it can be tested: only [`clear_base_debt`](ContextDrawerScreen::clear_base_debt), which
    /// the frame that drew the base calls, ends the obligation.
    ///
    /// The mutant is the tick *assigning* `needs_base` instead of adding to it: the first tick after
    /// the swap clears a debt no frame has paid.
    #[test]
    fn the_base_draw_a_sheet_owes_outlives_a_pass_that_drew_no_frame() {
        let mut swapped = ContextDrawerScreen::swapped_in(&MAP_DISPLAY, 1_000);
        assert!(swapped.needs_base(), "the shorter sheet owes the band the taller one held");

        swapped.tick_timers(1_000);
        swapped.tick_timers(1_016);
        assert!(swapped.needs_base(), "two passes that rendered nothing put no pixel back");

        swapped.clear_base_debt();
        assert!(!swapped.needs_base(), "the draw is what pays it");
        swapped.tick_timers(1_032);
        assert!(!swapped.needs_base(), "…and a settled sheet does not ask a second time");
    }

    /// The map's table is the ride's four actions **plus** one door, and the Map is the only screen
    /// that declares it. Pinned here as well as in `harness/screens.rs` because this is where the
    /// two tables live: rows 0-3 must stay label-for-label and action-for-action identical, or a
    /// rider's muscle memory differs between the Map and Statistics.
    #[test]
    fn the_map_table_is_the_ride_table_plus_one_door() {
        assert_eq!(MAP.rows.len(), RIDE.rows.len() + 1);
        for (m, r) in MAP.rows.iter().zip(RIDE.rows) {
            // `Msg` is a bare catalog index with no `Debug`, so the label is compared as the string
            // the rider reads — which is the thing that must not drift anyway.
            assert_eq!(t(m.label, Language::En), t(r.label, Language::En), "the ride labels must not drift per view");
            assert_eq!(m.action, r.action, "…nor what they do");
        }
        let last = MAP.rows[MAP.rows.len() - 1];
        assert_eq!(last.action, ContextAction::MapDisplay, "the fifth row is the door onto the display sheet");
    }

    // ---- D4d: the route-plan context ------------------------------------------------------------

    fn route_plan_drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::opening(&ROUTE_PLAN)
    }

    /// **The bike-type row is live exactly where the loaded map offers a choice.** This is the
    /// deleted Bike-type screen's own `count > 1` guard, restated as the binding's predicate: with
    /// no map (a fresh boot, or a router-less `ble` image) and with a single-profile map the row is
    /// out of the `enabled` mask and a press does nothing at all; from two profiles up it is live
    /// and a press opens the editor.
    ///
    /// The mutant is an `accepts` that returns `true`: the row would draw live on a device with no
    /// map and a press would open an editor over a ring of nothing.
    #[test]
    fn the_bike_type_row_is_live_exactly_where_a_map_offers_a_choice() {
        for names in [&[][..], &["Road"][..]] {
            let mut w = World::riding();
            w.nav_profiles = crate::NavProfiles::from_names(names);
            let mut d = route_plan_drawer();
            assert_eq!(d.key(&w.facts()).4, 0, "{} profile(s): the row draws recessed", names.len());
            assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "…and a press does nothing");
            assert_eq!(d.page, Page::Root, "not even a page slide");
        }

        let mut w = World::riding(); // the fixture maps' four §8.6 profiles
        let mut d = route_plan_drawer();
        assert_eq!(d.key(&w.facts()).4, 1, "two or more profiles: the row is live");
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor, "…and a press opens its editor");
        assert_eq!(ContextValue::BikeProfile.count(&w.facts()), 4, "the ring is the map's own name list");
    }

    /// **The editor opens on the *effective* profile and commits an index.** A stale stored index
    /// against a smaller map opens on profile 0 and marks profile 0 — the profile the router will
    /// actually route under (routing-v2 N3, the #538 truthful-label rule) — rather than on a
    /// profile the map does not have. Staging writes nothing, Select writes
    /// `Settings::bike_profile_idx`, Back out of a re-opened editor discards, and the key reports
    /// the staged and the committed ordinal apart.
    ///
    /// The mutant is a `committed` that returns the stored index: the first assertion below would
    /// open the editor on ordinal 7 of a four-profile ring.
    #[test]
    fn the_bike_type_editor_opens_on_the_effective_profile_and_commits_an_index() {
        let mut w = World::riding();
        w.settings.bike_profile_idx = 7; // stale: the map carries four
        let mut d = route_plan_drawer();
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor);
        assert_eq!(d.staged, 0, "a stale index opens on the profile the router falls back to");
        assert_eq!(d.key(&w.facts()).3, 0, "…and marks that one, not the one stored");

        w.press(&mut d, Gesture::Step(1)); // → Gravel
        assert_eq!(w.settings.bike_profile_idx, 7, "staging commits nothing");
        let (_, _, staged, committed, _) = d.key(&w.facts());
        assert_eq!((staged, committed), (1, 0), "the key carries the browsed and the set profile apart");

        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Root, "Select returns to the row table");
        assert_eq!(w.settings.bike_profile_idx, 1, "…having written the settings field");

        // Back out of a re-opened editor discards the staged choice.
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.staged, 1, "the editor re-opens on what is now committed");
        w.press(&mut d, Gesture::Step(2)); // → Touring
        w.press(&mut d, Gesture::Back);
        assert_eq!(d.page, Page::Root, "Back closes the editor, not the sheet");
        assert_eq!(w.settings.bike_profile_idx, 1, "…and the field is untouched");

        // The ring wraps over exactly the map's profiles, and every ordinal names one of them.
        w.press(&mut d, Gesture::Press);
        w.press(&mut d, Gesture::Step(-1));
        assert_eq!(d.staged, 0, "stepping back off Gravel lands on Road");
        w.press(&mut d, Gesture::Step(-1));
        assert_eq!(d.staged, 3, "…and off Road wraps to the last profile the map carries");
    }
}
