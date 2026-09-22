//! The contextual drawer: the bottom sheet the Down+Back chord opens on a screen that declares
//! secondary actions, and the declarative model those screens declare them with.
//!
//! A screen does not implement a drawer. It names one: a `&'static` [`ContextMenu`] returned from
//! [`Screen::context`](super::Screen::context). Everything else lives here — the cursor, which rows
//! are inert, what a press resolves to, how the sheet is drawn and how it animates. A screen that
//! declares nothing gets no drawer, and the chord does nothing on it.
//!
//! A row is a door, a value, an act or a switch, and the sheet has no fifth shape. A door replaces
//! the sheet with what it opens: a screen, or a shorter sheet. A value binds to a [`ContextValue`]
//! and slides the sheet to a nested editor, where Up/Down stages, Select commits and Back discards;
//! a binding's choices can be map data rather than a fixed list. An act tells a domain something
//! and leaves the sheet — it has to leave, because a sheet's own frame shadows every base fact (see
//! [`ContextDrawerScreen::key`]), so a cue raised from an open sheet could not be seen until it
//! closed. A switch ([`ContextToggle`]) flips a `bool` in place and keeps the sheet up, because the
//! row draws its own state, so the control stays legible and the rider sets a whole group of them
//! for one repaint of the screen underneath.

use embedded_graphics::prelude::Point;
use obc_reader::{PoiCategory, PoiCategorySet};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::navigator::RouteState;
use crate::settings::UpAheadSource;
use crate::{AppState, Msg, Settings};

use super::vocab::rows;
use super::vocab::sheet::{self, Edge, SheetMotion, SheetTiming};
use super::{palette, Ctx, DetourScreen, Render, RouteMenuScreen, Screen, ScreenTick, Transition};

/// How long the sheet takes to slide up from the bottom edge on open (ms). Deliberately its own
/// constant rather than the quick drawer's: the two sheets are tuned on glass one at a time.
const OPEN_MS: u32 = 440;
/// How long a nested editor takes to slide in, and the sheet to grow into its height (ms).
const SLIDE_MS: u32 = 180;
/// How long one step of the open costs the panel, and therefore the cadence the sheet asks to be
/// woken at (ms). This is the taller of the two sheets, so its deepest step costs more than the
/// quick drawer's and the two keep their own numbers.
const STEP_MS: u32 = 48;
/// The motion the shared sheet engine runs this sheet on.
pub(crate) const MOTION: SheetTiming = SheetTiming { open_ms: OPEN_MS, slide_ms: SLIDE_MS, step_ms: STEP_MS };

/// One row's height, and the padding above the first row / below the last.
const ROW_H: i32 = 44;
const SHEET_PAD: i32 = 12;

/// The nested value editor's sheet height: one title line, the staged choice, and the notch strip
/// whose tick marks the committed one. Fixed, because every binding draws the same three things,
/// and tall enough that the tick sits inside the sheet rather than on its bottom lip.
const EDITOR_H: i32 = 148;

/// The tallest a sheet may grow before it stops being a sheet: 244 px of the 320 px panel. A drawer
/// stays attached to its edge, uses only the height its content needs, and scrolls within a bounded
/// sheet rather than becoming a page.
const MAX_SHEET_H: i32 = 244;

/// At most five rows are visible; longer category menus scroll within the sheet.
const VISIBLE_ROWS: usize = ((MAX_SHEET_H - SHEET_PAD * 2) / ROW_H) as usize;
const MAX_ROWS: usize = 8;

/// What a context row reads about the base under the sheet: whether it may be pressed, and what
/// the value it binds to currently is.
///
/// Gathered once by [`App::render_key`](crate::App) and once per press, from [`Ctx`] and
/// [`Render`] alike, so the row the rider sees, the row a press resolves and the row the render key
/// reports cannot read three different worlds.
pub(crate) struct ContextFacts<'a> {
    pub state: &'a AppState,
    pub navigation: &'a RouteState,
    pub settings: &'a Settings,
    /// Whether a ride is open, at the level [`RecorderMachine`](crate::RecorderMachine) reports.
    pub recording: bool,

    /// The loaded map's routing-profile names: how many choices the bike-type binding has, and
    /// therefore whether its row is live at all.
    pub nav_profiles: &'a crate::NavProfiles,
}

/// A typed value a context row edits in place of opening a screen. The binding owns where the
/// value lives, how many choices it has and what each one is called; the drawer owns the page, the
/// staging, the commit and the drawing.
///
/// Choices are addressed by ordinal, which is also what the render key carries — so "the staged
/// value" is one `u8` for every binding there will ever be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextValue {
    /// The Up-ahead timeline's category filter: Everything, then the six categories. Rider
    /// selection state, so it lives in [`AppState`] rather than in [`Settings`]: the list opens on
    /// Everything every time, and a value that is reset on entry is not a preference. It sits in
    /// the app plane because the sheet that edits it is above the screen that reads it.
    UpAheadFilter,
    /// Which sources feed the Up-ahead timeline. The commit writes
    /// [`Settings::up_ahead_source`](crate::Settings), and [`App`](crate::App)'s one `==` diff over
    /// `Settings` arms the save.
    UpAheadSource,
    FindResults,

    /// The routing profile the on-device planner weights edges by. Its choices are the loaded map's
    /// own profile names ([`NavProfiles`](crate::NavProfiles)), so a custom web-builder profile
    /// appears without a hardcoded list, and a map that offers no choice makes the row inert rather
    /// than a control that walks a ring of one.
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
            ContextValue::FindResults => crate::settings::FindResults::COUNT as u8,

            // At most `NAV_MAX_PROFILES`, which is also the notch strip's own ceiling.
            ContextValue::BikeProfile => f.nav_profiles.len() as u8,
        }
    }

    /// Whether the row that binds this may be pressed: the row is live exactly when the binding
    /// accepts. A filter is as meaningful over an empty list as over a full one, and a source scope
    /// is a preference no ride state can invalidate, so only the bike profile ever refuses — it
    /// needs a map that offers more than one profile.
    fn accepts(self, f: &ContextFacts) -> bool {
        match self {
            ContextValue::UpAheadFilter | ContextValue::UpAheadSource | ContextValue::FindResults => true,

            ContextValue::BikeProfile => f.nav_profiles.len() > 1,
        }
    }

    /// The ordinal currently committed — where the editor opens, and the choice it keeps marked.
    fn committed(self, f: &ContextFacts) -> u8 {
        match self {
            ContextValue::UpAheadFilter => filter_choice(f.state.up_ahead_filter),
            ContextValue::UpAheadSource => f.settings.up_ahead_source as u8,
            ContextValue::FindResults => f.settings.find_results as u8,

            // The effective index, not the stored one: a stale index against a smaller map opens
            // on profile 0 and marks profile 0, which is the profile the router will use.
            ContextValue::BikeProfile => f.nav_profiles.effective(f.settings.bike_profile_idx),
        }
    }

    /// Write `ordinal` to wherever this binding's value lives.
    fn commit(self, cx: &mut Ctx, ordinal: u8) {
        match self {
            ContextValue::UpAheadFilter => cx.state.up_ahead_filter = choice_filter(ordinal),
            ContextValue::FindResults => cx.settings.find_results = crate::settings::FindResults::from_byte(ordinal),
            ContextValue::UpAheadSource => {
                cx.settings.up_ahead_source = UpAheadSource::ALL[(ordinal as usize).min(UpAheadSource::COUNT - 1)]
            }

            // The ordinal came from the editor's ring, which is `count` long, so the loaded map
            // already has it.
            ContextValue::BikeProfile => cx.settings.bike_profile_idx = ordinal,
        }
    }

    /// What `ordinal` is called, in the rider's language, or, for the bike profile, in the map's
    /// own words. The borrow is `rx`'s because those names live in
    /// [`NavProfiles`](crate::NavProfiles) rather than in `.rodata`.
    fn choice_label<'a>(self, ordinal: u8, rx: &'a Render) -> &'a str {
        match self {
            ContextValue::UpAheadFilter => match choice_category(ordinal) {
                Some(cat) => rx.t(super::poi_menu::category_msg(cat)),
                None => rx.t(Msg::UpAheadEverything),
            },
            ContextValue::UpAheadSource => {
                UpAheadSource::ALL[(ordinal as usize).min(UpAheadSource::COUNT - 1)].name(rx.settings.language)
            }

            // The generic `Profile N` fallback is deliberately not used: it exists for an empty
            // table, and an empty table makes this row inert, so it has no reachable case here.
            ContextValue::BikeProfile => rx.nav_profiles.name(ordinal).unwrap_or(""),
            ContextValue::FindResults => crate::settings::FindResults::from_byte(ordinal).name(),
        }
    }

    /// The choice's own icon, for the bindings whose values already have one. `None` draws the
    /// label alone rather than inventing a glyph.
    fn choice_icon(self, ordinal: u8) -> Option<PoiCategory> {
        match self {
            ContextValue::UpAheadSource | ContextValue::BikeProfile | ContextValue::FindResults => None,

            ContextValue::UpAheadFilter => choice_category(ordinal),
        }
    }
}

/// A `bool` a context row flips in place. The binding owns where the bit lives; the drawer owns the
/// row, the slider it draws and what a press does.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextToggle {
    /// The map's `HH:MM` pill.
    MapClock,
    /// The map's bottom-left scale bar.
    MapScaleBar,
    /// The map's terrain layer.
    MapContours,
    FindHideClosed,
    MapPeaks,
    MapLandmarks,
    MapPois,
    MapPoiCategory(PoiCategory),
}

impl ContextToggle {
    /// Whether the bit is set — what the row's slider draws, and what the render key reports as the
    /// selected row's committed state.
    fn read(self, f: &ContextFacts) -> bool {
        match self {
            ContextToggle::MapPeaks => f.settings.map_peaks,
            ContextToggle::MapLandmarks => f.settings.map_landmarks,
            ContextToggle::MapPois => f.settings.map_pois,
            ContextToggle::MapPoiCategory(cat) => f.settings.map_poi_categories & category_bit(cat) != 0,
            ContextToggle::MapClock => f.settings.map_clock,
            ContextToggle::MapScaleBar => f.settings.map_scale_bar,
            ContextToggle::MapContours => f.settings.map_contours,
            ContextToggle::FindHideClosed => f.settings.find_hide_closed,
        }
    }

    /// Flip it. [`App`](crate::App)'s one `==` diff over [`Settings`] turns the write into a save,
    /// and a later flip supersedes an in-flight older revision, so three of them cannot queue three
    /// competing writes.
    fn flip(self, cx: &mut Ctx) {
        match self {
            ContextToggle::MapPeaks => cx.settings.map_peaks = !cx.settings.map_peaks,
            ContextToggle::MapLandmarks => cx.settings.map_landmarks = !cx.settings.map_landmarks,
            ContextToggle::MapPois => cx.settings.map_pois = !cx.settings.map_pois,
            ContextToggle::MapPoiCategory(cat) => cx.settings.map_poi_categories ^= category_bit(cat),
            ContextToggle::MapClock => cx.settings.map_clock = !cx.settings.map_clock,
            ContextToggle::MapScaleBar => cx.settings.map_scale_bar = !cx.settings.map_scale_bar,
            ContextToggle::MapContours => cx.settings.map_contours = !cx.settings.map_contours,
            ContextToggle::FindHideClosed => cx.settings.find_hide_closed = !cx.settings.find_hide_closed,
        }
    }
}

fn category_bit(cat: PoiCategory) -> u8 {
    1 << PoiCategory::ALL.iter().position(|item| *item == cat).expect("service category")
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

/// What pressing a context row does: a destination or a binding, not a closure. The drawer
/// resolves it against the live [`Ctx`] at press time, which is what lets the table be `&'static`
/// and lets one table serve several screens whose activity state differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextAction {
    LandmarkSources,
    CurrentVisit,
    ResumeJourney,
    /// The merged waypoint + corridor-POI timeline, anchored on live progress at entry.
    Assistant,
    /// The rejoin chooser. Inert without a route, a nav graph and an on-route rider.
    Detour,
    /// The stored-route and trip menu.
    Routes,
    /// A value the sheet edits on a nested page instead of a screen it opens.
    Edit(ContextValue),

    /// A door onto [`MAP_DISPLAY`], which is a sheet rather than a screen. The only row that
    /// replaces a sheet with a sheet, and the shape the map forced: a nested sliding page over a
    /// map costs a map render per frame of the slide, while a swap costs exactly one.
    MapDisplay,
    MapIcons,
    MapPoiCategories,
    /// A `bool` the row flips in place: the sheet stays up, the row's own slider is the feedback,
    /// and the screen underneath is redrawn once, when the sheet closes, so setting every map
    /// modifier costs one map render.
    Toggle(ContextToggle),
}

impl ContextAction {
    /// Whether the row can be pressed right now. An unavailable row draws recessed and does
    /// nothing.
    ///
    /// The row and its destination read one predicate. For Detour that is
    /// [`detour::reachable`](super::detour::reachable), which the chooser's own availability check
    /// is built from, so a row can never be an enabled door onto an inert screen. For a value row
    /// it is [`ContextValue::accepts`], the same answer the commit obeys.
    fn available(self, f: &ContextFacts) -> bool {
        match self {
            ContextAction::LandmarkSources | ContextAction::ResumeJourney => true,
            ContextAction::CurrentVisit => f.navigation.active_route.is_some(),
            // The timeline opens on its own empty state without a route, which is informative
            // rather than dead, so it is always live.
            ContextAction::Assistant | ContextAction::Routes => true,
            // A detour needs a recorded ride to re-route, a route to leave, a graph to route on,
            // and a rider on the route, because the corridor anchors on live progress.
            ContextAction::Detour => super::detour::reachable(f.navigation, f.recording, f.state.has_nav_graph),
            ContextAction::Edit(v) => v.accepts(f),

            // A display modifier is a preference no ride state can invalidate, and the door onto
            // them is as live as they are.
            ContextAction::MapDisplay
            | ContextAction::MapIcons
            | ContextAction::MapPoiCategories
            | ContextAction::Toggle(_) => true,
        }
    }

    fn open(self, cx: &mut Ctx) -> Option<Transition> {
        Some(Transition::Replace(match self {
            ContextAction::LandmarkSources => {
                cx.landmarks.source_page = 0;
                Screen::LandmarkSources(super::LandmarkSourcesScreen)
            }
            ContextAction::ResumeJourney => Screen::Journey(super::JourneyScreen::new(true)),
            ContextAction::CurrentVisit => {
                cx.find.action = crate::find_place::Action::OpenAccepted;
                return Some(Transition::Pop);
            }
            ContextAction::Assistant => Screen::Assistant(super::AssistantScreen::new()),
            ContextAction::Detour => Screen::Detour(DetourScreen::new(cx.navigator.route_state())),
            ContextAction::Routes => Screen::RouteMenu(RouteMenuScreen::new()),

            // The shorter sheet takes the taller one's place, already landed.
            ContextAction::MapIcons => Screen::ContextDrawer(ContextDrawerScreen::swapped_in(&MAP_ICONS, cx.now_ms)),
            ContextAction::MapPoiCategories => {
                Screen::ContextDrawer(ContextDrawerScreen::swapped_in(&MAP_POI_CATEGORIES, cx.now_ms))
            }
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

fn row_font(row: &ContextRow, label: &str) -> Font {
    let room = if matches!(row.action, ContextAction::Toggle(_)) { 136 } else { 172 };
    if label.contains('\n') || obc_render::text::text_width(label, Font::Body) > room {
        Font::Label
    } else {
        Font::Body
    }
}

/// A screen's declared contextual content — the rows the bottom sheet offers, in sheet order.
pub struct ContextMenu {
    pub rows: &'static [ContextRow],
}

/// The ride context: the secondary actions the four riding views share.
pub static RIDE: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::AssistantTitle, action: ContextAction::Assistant },
        ContextRow { label: Msg::RideContextDetour, action: ContextAction::Detour },
        ContextRow { label: Msg::MenuRoutes, action: ContextAction::Routes },
    ],
};

/// The map context: the ride's secondary actions, in the order every riding view offers them, plus
/// the one row only the Map has a referent for. Its first rows are [`RIDE`]'s, pinned equal by
/// test, so a rider reaches the same actions by the same steps from either screen.
pub static MAP: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::AssistantTitle, action: ContextAction::Assistant },
        ContextRow { label: Msg::RideContextDetour, action: ContextAction::Detour },
        ContextRow { label: Msg::MenuRoutes, action: ContextAction::Routes },
        ContextRow { label: Msg::MapContextMapDisplay, action: ContextAction::MapDisplay },
    ],
};

pub static FIND_PLACE: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::FindContextHideClosed, action: ContextAction::Toggle(ContextToggle::FindHideClosed) },
        ContextRow { label: Msg::FindContextResults, action: ContextAction::Edit(ContextValue::FindResults) },
    ],
};

/// The map display sheet: the switches that change nothing but what the Map draws, and the only
/// home any of them has.
pub static MAP_DISPLAY: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::MapContextClock, action: ContextAction::Toggle(ContextToggle::MapClock) },
        ContextRow { label: Msg::MapContextScaleBar, action: ContextAction::Toggle(ContextToggle::MapScaleBar) },
        ContextRow { label: Msg::MapContextContours, action: ContextAction::Toggle(ContextToggle::MapContours) },
        ContextRow { label: Msg::MapContextIcons, action: ContextAction::MapIcons },
    ],
};

pub static MAP_ICONS: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::MenuPeaks, action: ContextAction::Toggle(ContextToggle::MapPeaks) },
        ContextRow { label: Msg::MapContextLandmarks, action: ContextAction::Toggle(ContextToggle::MapLandmarks) },
        ContextRow { label: Msg::MapContextPois, action: ContextAction::Toggle(ContextToggle::MapPois) },
        ContextRow { label: Msg::MapContextCategories, action: ContextAction::MapPoiCategories },
    ],
};

pub static MAP_POI_CATEGORIES: ContextMenu = ContextMenu {
    rows: &[
        ContextRow {
            label: Msg::PoiCatWater,
            action: ContextAction::Toggle(ContextToggle::MapPoiCategory(PoiCategory::Water)),
        },
        ContextRow {
            label: Msg::MapContextCampsites,
            action: ContextAction::Toggle(ContextToggle::MapPoiCategory(PoiCategory::Campsite)),
        },
        ContextRow {
            label: Msg::PoiCatAccommodation,
            action: ContextAction::Toggle(ContextToggle::MapPoiCategory(PoiCategory::Accommodation)),
        },
        ContextRow {
            label: Msg::PoiCatResupply,
            action: ContextAction::Toggle(ContextToggle::MapPoiCategory(PoiCategory::Resupply)),
        },
        ContextRow {
            label: Msg::PoiCatPharmacy,
            action: ContextAction::Toggle(ContextToggle::MapPoiCategory(PoiCategory::Pharmacy)),
        },
        ContextRow {
            label: Msg::MapContextBikeShops,
            action: ContextAction::Toggle(ContextToggle::MapPoiCategory(PoiCategory::BikeShop)),
        },
        ContextRow {
            label: Msg::MapContextTrains,
            action: ContextAction::Toggle(ContextToggle::MapPoiCategory(PoiCategory::Train)),
        },
    ],
};

pub(crate) static ASSISTANT_RESUME: ContextMenu =
    ContextMenu { rows: &[ContextRow { label: Msg::AssistantResumeRoute, action: ContextAction::ResumeJourney }] };

pub static ASSISTANT_VISIT: ContextMenu =
    ContextMenu { rows: &[ContextRow { label: Msg::AssistantCurrentVisit, action: ContextAction::CurrentVisit }] };

/// The Up-ahead context: the two controls that scope the timeline, and the only home either of
/// them has.
pub static UP_AHEAD: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::RideContextFilter, action: ContextAction::Edit(ContextValue::UpAheadFilter) },
        ContextRow { label: Msg::RideContextSources, action: ContextAction::Edit(ContextValue::UpAheadSource) },
    ],
};

/// The route-plan context: the profile the on-device planner will weight edges by, offered on the
/// card that is about to ask for a plan. One row, because `NavPlanner::new` takes the profile and
/// nothing else, so a second "route options" row would be a label bound to nothing.
pub static ROUTE_PLAN: ContextMenu = ContextMenu {
    rows: &[ContextRow { label: Msg::RouteContextBikeType, action: ContextAction::Edit(ContextValue::BikeProfile) }],
};

/// The sheet's two pages. The value being edited is the selected row's, so the page needs no
/// payload, which is why the render key's `page` byte says everything about where the rider is.
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
            Page::Root => SHEET_PAD * 2 + ROW_H * menu.rows.len().min(VISIBLE_ROWS) as i32,
            Page::Editor => EDITOR_H,
        }
    }
}

/// The contextual drawer's whole state: when it opened, the table it was opened over, the cursor,
/// the page, and the ordinal the editor has staged but not committed.
pub struct ContextDrawerScreen {
    /// The open, the page slide and the base-draw debt, on this sheet's own [`MOTION`].
    pub(crate) motion: SheetMotion,
    menu: &'static ContextMenu,
    selected: u8,
    page: Page,
    /// The choice the editor is previewing. Meaningful only on [`Page::Editor`]; off that page
    /// every reader falls back to the committed value, which is what makes Back-discards free.
    staged: u8,
}

impl ContextDrawerScreen {
    /// A drawer over `menu` that has begun to open, with the first row selected. Its slide starts
    /// on the first frame that ticks it, not on the pass the chord was resolved in.
    pub fn opening(menu: &'static ContextMenu) -> Self {
        debug_assert!(menu.rows.len() <= MAX_ROWS, "a context table is a sheet, not a page — see MAX_ROWS");
        ContextDrawerScreen { motion: SheetMotion::opening(), menu, selected: 0, page: Page::Root, staged: 0 }
    }

    /// A drawer over `menu` that is already landed: the sheet a row of another sheet swapped in.
    /// It makes no entrance, and it owes the screen below the band it gives back.
    pub fn swapped_in(menu: &'static ContextMenu, now_ms: u32) -> Self {
        ContextDrawerScreen { motion: SheetMotion::landed(now_ms, MOTION), ..ContextDrawerScreen::opening(menu) }
    }

    /// The exact facts this drawer draws, for the pass's render key: the page, the selected row,
    /// the staged and committed values of the row's binding, and which rows are live. The sheet's
    /// own identity is the stack shape, which the key already carries.
    ///
    /// Availability and the committed value are derived from the base, so they are the only things
    /// under an open sheet that may still move a pixel. They are the cue, not the values behind
    /// them, so a rider drifting off route re-draws the sheet once and a moving map costs nothing.
    pub(crate) fn key(&self, f: &ContextFacts) -> (u8, u8, u8, u8, u8) {
        let mut live = 0u8;
        for (i, row) in self.menu.rows.iter().enumerate().take(MAX_ROWS) {
            if row.action.available(f) {
                live |= 1 << i;
            }
        }
        // Both answers are "what the selected row is set to", the only per-row state either shape
        // draws.
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
        // fast double-press land on a row the rider cannot see yet.
        if self.motion.sliding(cx.now_ms, MOTION) {
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
                    return Transition::None;
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
            Gesture::Back if core::ptr::eq(self.menu, &MAP_POI_CATEGORIES) => {
                Transition::Replace(Screen::ContextDrawer(Self::swapped_in(&MAP_ICONS, cx.now_ms)))
            }
            Gesture::Back if core::ptr::eq(self.menu, &MAP_ICONS) => {
                Transition::Replace(Screen::ContextDrawer(Self::swapped_in(&MAP_DISPLAY, cx.now_ms)))
            }
            Gesture::Back => Transition::Pop,
            // A context row has no held action, and Back-hold is resolved above screen dispatch.
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
            // A ring, not an axis: these are named alternatives, so the cursor wraps. The quick
            // drawer's brightness clamps instead, because a value axis has ends.
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
            // Discard: the staged ordinal is abandoned, so the row's value reverts to the
            // committed one on the next frame.
            Gesture::Back => {
                self.slide_to(Page::Root, cx.now_ms);
                Transition::None
            }
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    /// The sheet's animation: the open slide, then any page slide, at the panel's step cadence.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let sheet_h = self.sheet_height(now_ms);
        self.motion.tick(now_ms, MOTION, sheet_h)
    }

    /// Begin a horizontal transition to `to`, which becomes the live page at once (so `handle` and
    /// the render key already speak about the destination) while the slide draws both.
    fn slide_to(&mut self, to: Page, now_ms: u32) {
        self.page = to;
        self.motion.slide_to(now_ms);
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let sheet_h = self.sheet_height(rx.now_ms);
        let visible = self.motion.visible_height(rx.now_ms, MOTION, sheet_h);
        if visible == 0 {
            return;
        }
        // The sheet hangs from the bottom edge: it slides up by drawing its full height with its
        // bottom off-screen.
        let top = rx.h - visible;
        sheet::frame(cv, rx.w, top, sheet_h, Edge::Bottom);
        match self.motion.page_offsets(rx.now_ms, MOTION, rx.w, self.page == Page::Root) {
            Some((out, incoming)) => {
                self.draw_page(cv, rx, self.other_page(), top, out);
                self.draw_page(cv, rx, self.page, top, incoming);
            }
            None => self.draw_page(cv, rx, self.page, top, 0),
        }
    }

    /// The page a slide is coming from, since the sheet has exactly two.
    fn other_page(&self) -> Page {
        match self.page {
            Page::Root => Page::Editor,
            Page::Editor => Page::Root,
        }
    }

    /// The sheet height this frame: the page's own, or the interpolation between two pages' while a
    /// slide runs — which is how the sheet grows and shrinks with its content.
    fn sheet_height(&self, now_ms: u32) -> i32 {
        self.motion.height(now_ms, MOTION, self.other_page().height(self.menu), self.page.height(self.menu))
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
    /// A switch row draws its state instead of a chevron, because it goes nowhere and because the
    /// screen under the sheet is frozen, so the slider moving is the only feedback a flip gets
    /// until the sheet closes. It is the settings tree's own slider, from the shared row
    /// vocabulary.
    ///
    /// The row does not state its value, and that is measured rather than chosen: a label plus the
    /// longest choice does not fit one row, and a two-line row does not fit the row pitch. The
    /// editor one press away is where the value is spelled out and the committed one is marked.
    fn draw_root(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        let facts = rx.context_facts();
        let first = (self.selected as usize).saturating_sub(VISIBLE_ROWS - 1);
        for (i, row) in self.menu.rows.iter().enumerate().skip(first).take(VISIBLE_ROWS) {
            let area =
                rect(x + rows::ROW_X, top + SHEET_PAD + (i - first) as i32 * ROW_H, rx.w - 2 * rows::ROW_X, ROW_H - 4);
            let live = row.action.available(&facts);
            rows::row_cursor(cv, area, i as u8 == self.selected, false);
            let ink = if live { palette::INK } else { palette::CONTOUR };
            let label = rx.t(row.label);
            if row.action == ContextAction::MapPoiCategories {
                use core::fmt::Write as _;
                let mut summary = heapless::String::<16>::new();
                let _ = write!(summary, "{} / {}", rx.settings.map_poi_categories.count_ones(), PoiCategory::ALL.len());
                cv.text_vcentered(
                    label,
                    area.top_left.x + 14,
                    (area.top_left.y, 20),
                    Font::Label,
                    TextAlign::Left,
                    ink,
                );
                cv.text_vcentered(
                    &summary,
                    area.top_left.x + 14,
                    (area.top_left.y + 20, 20),
                    Font::Label,
                    TextAlign::Left,
                    ink,
                );
            } else if let Some((first, second)) = label.split_once('\n') {
                for (line, text) in [first, second].into_iter().enumerate() {
                    cv.text_vcentered(
                        text,
                        area.top_left.x + 14,
                        (area.top_left.y + line as i32 * 20, 20),
                        Font::Label,
                        TextAlign::Left,
                        ink,
                    );
                }
            } else {
                cv.text_vcentered(
                    label,
                    area.top_left.x + 14,
                    (area.top_left.y, ROW_H - 4),
                    row_font(row, label),
                    TextAlign::Left,
                    ink,
                );
            }
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
        super::vocab::list::scrollbar(
            cv,
            x + rx.w - 7,
            top + SHEET_PAD,
            ROW_H * VISIBLE_ROWS as i32,
            self.menu.rows.len(),
            first,
            VISIBLE_ROWS,
        );
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
        let cap_mid = Font::Body.cap_mid() as i32;
        let name_x = match value.choice_icon(self.staged) {
            Some(cat) => {
                let c = Point::new(x + 26, top + 64 + cap_mid);
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

pub static LANDMARK_CONTENT: ContextMenu =
    ContextMenu { rows: &[ContextRow { label: Msg::RideContextSources, action: ContextAction::LandmarkSources }] };

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::i18n::t;
    use crate::recorder::RecorderMachine;
    use crate::screen::test_ctx;
    use crate::settings::Language;
    use crate::{Activity, AppState, Settings};

    struct World {
        state: AppState,
        activity: Activity,
        navigator: crate::navigator::NavigatorMachine,
        settings: Settings,
        recorder: RecorderMachine,

        /// The loaded map's profile names: the bike-type binding's choice list, and the predicate
        /// its row is live by. Four by default, the fixture maps' own set.
        nav_profiles: crate::NavProfiles,
        now_ms: u32,
    }

    impl World {
        /// A routed, nav-graph, on-route recorded ride: every row of the ride context live. All
        /// four conditions matter, because the Detour row reads the same predicate its chooser
        /// does, and that one names the open ride too.
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

    fn map_drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::opening(&MAP)
    }

    /// Each row replaces the sheet, so Back from the destination lands on the base.
    #[test]
    fn every_ride_row_replaces_the_sheet_with_its_destination() {
        let mut w = World::riding();
        let mut d = drawer();
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::Assistant(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::Detour(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(2));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::RouteMenu(_))));
    }

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
        w.press(&mut d, Gesture::Step(3));
        assert!(
            matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::Assistant(_))),
            "wrapped to first"
        );
        assert!(matches!(w.press(&mut d, Gesture::Back), Transition::Pop));
    }

    /// Pressing Detour without a route, without a graph, or off route does nothing at all, and the
    /// key says so, so the sheet redraws when the answer changes.
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
            // The fourth condition: a browse map with a route loaded and a graph under it still
            // has nothing to re-route, because there is no ride.
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

    /// A row is never an enabled door onto an inert screen. The browse Map makes it checkable:
    /// three of the Detour row's four conditions hold, but nothing is being recorded. The row and
    /// the chooser read the same predicate, so this is proved by identity.
    #[test]
    fn a_route_without_a_ride_leaves_the_detour_row_inert() {
        let mut w = World::riding();
        w.recorder.test_close(); // a browse map: the route and the graph stay
        assert!(
            w.navigator.route_state().active_route.is_some()
                && w.state.has_nav_graph
                && !w.navigator.route_state().off_route
        );

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1)); // → the Detour row
        assert_eq!(d.key(&w.facts()).4 & (1 << 1), 0, "the row draws recessed");
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "…and a press does nothing");

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

    #[test]
    fn assistant_preserves_the_session_and_selection_scope() {
        let mut w = World::riding();
        w.recorder.test_open();
        w.activity.mode = Mode::Paused;
        w.navigator.route_state_mut().progress_m = 4_200;
        w.state.up_ahead_filter = PoiCategorySet::only(PoiCategory::Water);
        let session = w.recorder.session();
        let mut d = drawer();
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::Assistant(_))));
        assert_eq!(w.state.up_ahead_filter, PoiCategorySet::only(PoiCategory::Water));
        assert_eq!(w.navigator.route_state().progress_m, 4_200);
        assert_eq!(w.activity.mode, Mode::Paused);
        assert!(w.recorder.recording());
        assert_eq!(w.recorder.session(), session);
        assert_eq!(w.recorder.test_take_intent(), None);
    }

    /// Every declared table is a sheet, not a page, and fits the key's availability mask.
    #[test]
    fn pinned_by_the_row_tables() {
        // The derivation itself, so a geometry change is read here rather than asserted twice.
        assert_eq!(VISIBLE_ROWS, 5, "24 px of padding plus 44 px rows inside a {MAX_SHEET_H} px sheet");

        let declared: &[&ContextMenu] =
            &[&RIDE, &MAP, &MAP_DISPLAY, &MAP_ICONS, &MAP_POI_CATEGORIES, &UP_AHEAD, &ROUTE_PLAN, &FIND_PLACE];
        for menu in declared {
            assert!(menu.rows.len() <= MAX_ROWS, "{} rows outgrow the sheet", menu.rows.len());
            for page in [Page::Root, Page::Editor] {
                let h = page.height(menu);
                assert!(h <= MAX_SHEET_H, "a {h} px sheet is a page — bound it or scroll it first");
            }
            assert!(!menu.rows.is_empty(), "an empty table must not be declared: the chord shows no empty sheet");
        }
    }

    /// The whole editor contract in one pass: a value row slides to its editor on the committed
    /// choice, Up/Down stages without committing, Select commits and returns, and the committed
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

    /// Back discards: the staged choice is abandoned and the value underneath is untouched, so
    /// nothing has to be remembered in order to be undone.
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

    /// The render key carries the staged and the committed value as two separate facts, which is
    /// what lets the editor keep the committed choice marked while the rider browses.
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

    /// The Sources row commits `Settings::up_ahead_source`, the field the App's `==` diff turns
    /// into a save, and then draws the value it wrote.
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

    /// The editor is a ring of named alternatives: it wraps at both ends over exactly the choices
    /// the binding declares, and every ordinal round-trips to a value and back.
    #[test]
    fn the_editor_wraps_over_exactly_the_declared_choices() {
        let mut w = World::riding();
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Press); // the Filter editor: Everything + six categories
        w.press(&mut d, Gesture::Step(-1));
        assert_eq!(d.staged, 7, "stepping back off Everything wraps to the last category");
        w.press(&mut d, Gesture::Step(1));
        assert_eq!(d.staged, 0, "…and forward off the last wraps home");

        let facts = w.facts();
        assert_eq!(ContextValue::UpAheadFilter.count(&facts), 8);
        assert_eq!(ContextValue::UpAheadSource.count(&facts), UpAheadSource::COUNT as u8);
        for ordinal in 0..ContextValue::UpAheadFilter.count(&facts) {
            assert_eq!(filter_choice(choice_filter(ordinal)), ordinal, "ordinal {ordinal} round-trips");
        }
        assert_eq!(choice_filter(0), PoiCategorySet::ALL, "ordinal 0 is Everything");
    }

    /// The one-predicate rule for value rows, checked in both directions: a binding that always
    /// accepts gives a row that is always live, and a binding that refuses gives a recessed row.
    #[test]
    fn a_value_row_is_live_exactly_where_its_binding_accepts() {
        let mut w = World::riding();
        w.navigator.set_active_route(None);
        w.state.has_nav_graph = false;
        w.recorder.test_close();
        w.nav_profiles = crate::NavProfiles::EMPTY; // no map, so the bike binding refuses
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

        // And a press really opens the editor, rather than drawing live and doing nothing.
        let mut d = up_ahead_drawer();
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor);
    }

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
    /// page's own height.
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

        d.tick_timers(1_000 + SLIDE_MS); // the frame the grow settles on
        d.slide_to(Page::Root, 2_000);
        let shrink: heapless::Vec<i32, 8> =
            [0, 45, 90, 135, SLIDE_MS].iter().map(|dt| d.sheet_height(2_000 + dt)).collect();
        assert_eq!((shrink[0], shrink[4]), (EDITOR_H, root_h));
        assert!(shrink.windows(2).all(|p| p[0] >= p[1]), "…and back: {shrink:?}");
    }

    /// A switch row flips in place and keeps the sheet: each one flips its own field both ways,
    /// leaves the page on the root and the other fields alone, and the key follows the selected
    /// row's bit through `committed`, which is what makes a flip visible to the frame's identity.
    #[test]
    fn a_toggle_row_flips_in_place_and_keeps_the_sheet() {
        for (i, toggle) in
            MAP_DISPLAY.rows.iter().enumerate().filter(|(_, row)| matches!(row.action, ContextAction::Toggle(_)))
        {
            let ContextAction::Toggle(t) = toggle.action else { panic!("the display sheet is all switch rows") };
            let mut w = World::riding();
            let mut d = ContextDrawerScreen::opening(&MAP_DISPLAY);
            w.press(&mut d, Gesture::Step(i as i32));
            assert_eq!(d.key(&w.facts()).3, 1, "all three default on");

            assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "the sheet stays up");
            assert_eq!(d.page, Page::Root, "…on its root page: no editor, no slide");
            assert!(!t.read(&w.facts()), "on -> off");
            assert_eq!(d.key(&w.facts()).3, 0, "…and the key carries the selected row's new state");

            // The others are untouched: a flip is one bit, not a sheet-wide act.
            let others = MAP_DISPLAY.rows.iter().enumerate().filter(|(j, _)| *j != i);
            for (_, row) in others {
                let ContextAction::Toggle(other) = row.action else { continue };
                assert!(other.read(&w.facts()), "the other switches are untouched");
            }

            w.press(&mut d, Gesture::Press);
            assert!(t.read(&w.facts()), "off -> on again, from the same row");
            assert_eq!(d.key(&w.facts()).4, 0b1111, "every switch row is always live");
        }
    }

    /// The Map display row swaps one sheet for another with no entrance: the shorter sheet is
    /// landed on the frame the press produced, and that frame owes the screen below one draw
    /// because it uncovers a band the taller sheet held.
    ///
    /// What ends the obligation is the draw, not the next tick. The debt survives every tick until
    /// [`clear_base_debt`](SheetMotion::clear_base_debt), and no tick after that re-arms it, so the
    /// swap costs exactly one map draw.
    #[test]
    fn the_display_row_swaps_the_sheet_and_back_lands_on_the_map() {
        let mut w = World::riding();
        let mut d = map_drawer();
        w.press(&mut d, Gesture::Step(3)); // → the Map display row
        let Transition::Replace(Screen::ContextDrawer(mut swapped)) = w.press(&mut d, Gesture::Press) else {
            panic!("row 4 did not replace the sheet with the display sheet")
        };

        // Landed on its first frame: `visible_height` is already the whole table, and the tick
        // reports no further wake — a second open animation would show up as both.
        let target = Page::Root.height(&MAP_DISPLAY);
        let first = swapped.tick_timers(w.now_ms);
        let visible = swapped.motion.visible_height(w.now_ms, MOTION, target);
        assert_eq!(visible, target, "the swapped-in sheet is already landed");
        assert_eq!(first.next_wake_ms, None, "…so it asks for no open steps");
        assert!(swapped.motion.needs_base(), "its first frame uncovers the band the taller sheet held");
        swapped.tick_timers(w.now_ms + 16);
        assert!(swapped.motion.needs_base(), "…and a tick that drew no frame does not put the band back");
        swapped.motion.clear_base_debt();
        swapped.tick_timers(w.now_ms + 32);
        assert!(!swapped.motion.needs_base(), "the draw ends it, and nothing re-arms it: the swap costs exactly one");

        assert!(matches!(w.press(&mut swapped, Gesture::Back), Transition::Pop), "Back closes onto the Map");
    }

    /// The map's table is the ride's actions plus one door. The shared rows must stay
    /// label-for-label and action-for-action identical, or a rider's muscle memory differs between
    /// the Map and Statistics.
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

    fn route_plan_drawer() -> ContextDrawerScreen {
        ContextDrawerScreen::opening(&ROUTE_PLAN)
    }

    /// The bike-type row is live exactly where the loaded map offers a choice: with no map and
    /// with a single-profile map the row is out of the live mask and a press does nothing at all;
    /// from two profiles up it is live and a press opens the editor.
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

        let mut w = World::riding(); // the fixture maps' four profiles
        let mut d = route_plan_drawer();
        assert_eq!(d.key(&w.facts()).4, 1, "two or more profiles: the row is live");
        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Editor, "…and a press opens its editor");
        assert_eq!(ContextValue::BikeProfile.count(&w.facts()), 4, "the ring is the map's own name list");
    }

    /// The editor opens on the effective profile and commits an index. A stale stored index
    /// against a smaller map opens on profile 0 and marks profile 0, the profile the router will
    /// route under, rather than on a profile the map does not have. Staging writes nothing, Select
    /// writes `Settings::bike_profile_idx`, Back out of a re-opened editor discards, and the key
    /// reports the staged and the committed ordinal apart.
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
    #[test]
    fn map_categories_scroll_and_survive_master_switch_and_restart() {
        let mut world = World::riding();
        let mut categories = ContextDrawerScreen::opening(&MAP_POI_CATEGORIES);
        assert_eq!(Page::Root.height(&MAP_POI_CATEGORIES), MAX_SHEET_H);
        world.press(&mut categories, Gesture::Step(2));
        for _ in 2..PoiCategory::ALL.len() {
            world.press(&mut categories, Gesture::Press);
            world.press(&mut categories, Gesture::Step(1));
        }
        assert_eq!(world.settings.map_poi_categories, 3);
        let Transition::Replace(Screen::ContextDrawer(mut icons)) = world.press(&mut categories, Gesture::Back) else {
            panic!("back returns to icon controls")
        };
        world.press(&mut icons, Gesture::Step(2));
        world.press(&mut icons, Gesture::Press);
        assert!(!world.settings.map_pois);
        assert!(world.settings.map_peaks && world.settings.map_landmarks);
        let restored = crate::settings::decode(&crate::settings::encode(&world.settings)).unwrap();
        assert_eq!(restored.map_poi_categories, 3);
        assert!(!restored.map_pois);
        world.settings = restored;
        world.press(&mut icons, Gesture::Press);
        assert!(world.settings.map_pois);
        assert_eq!(world.settings.map_poi_categories, 3);
        world.press(&mut icons, Gesture::Step(-2));
        world.press(&mut icons, Gesture::Press);
        assert!(!world.settings.map_peaks && world.settings.map_landmarks && world.settings.map_pois);
    }
}
