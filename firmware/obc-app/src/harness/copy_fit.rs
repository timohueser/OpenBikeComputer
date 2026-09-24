//! The copy-fit gate: every screen, in every language, drawn through [`App::render_frame`], with
//! every string it asks for measured where it lands.
//!
//! `obc-render`'s `text-tap` feature records each text draw as (box, font, string). The screens
//! therefore state their own geometry instead of a test re-deriving it, which is what the
//! per-screen width tests this replaces had to do. The snapshot sweep stays the layout oracle;
//! this gate answers one question only, and answers it for the whole catalog: does the copy fit?

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use obc_ports::InputClock;
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, PoiCategory, Reader, SliceSource};

use crate::device_core::StoreIdentity;
use crate::dfu::{DfuFailure, DfuScanError, DfuScanReport};
use crate::host::DetourPreview;
use crate::navigator::RouteState;
use crate::screen::context_drawer::{self as ctx_menu, ContextAction};
use crate::screen::settings::page;
use crate::screen::*;
use crate::settings::Language;
use crate::{App, AppState, Gesture, Settings, WarningFlags};

use super::support::{build_min_obcm, selected_place, Buf};

/// The panel, in pixels. Every screen lays out against the size the frame hands it, so the gate
/// renders at the size the board has.
const PANEL: Size = Size::new(240, 320);

/// The panel's copy width, which is the frame width less the rounded outline and the clearance
/// either side. A line the screen centres on the panel midline is laid out across the whole panel,
/// so it is the budget that line is wrapped to.
const COPY_W: i32 = crate::screen::vocab::chrome::copy_w(PANEL.width as i32);

/// The screen no seed can build: its state holds an `obc_reader::peaks::Selection`, which only a
/// mounted map with a peak-article section hands out, and this harness has no such map. Its draw
/// is `landmarks::reading(.., sources = false)` and reads none of its own fields, so the page it
/// draws is the one [`the_article_page_fits_the_panel_in_every_language`] renders through
/// `Landmarks`. What is unmeasured is only the article a peak's own section carries.
const NO_SEED: [&str; 1] = ["PeakArticle"];

/// A long day's ride, for the day-done card's ledger.
const DAY: RideTotals = RideTotals { distance_m: 184_300, moving_s: 11 * 3600 + 52 * 60, climb_m: 3_080 };

/// A seed: a screen, and a walk over it. Every frame of the walk is measured, not only the last
/// one, so a page a gesture opens is measured as well as the page it opened from.
type Seed = (Screen, Vec<Gesture>);

/// More steps than any sheet editor holds choices, so a walk over a value row reaches every one.
const CHOICES: usize = 10;

/// A walk that presses row `row` of a sheet and then takes `steps` steps on the page that press
/// opened, which is how an editor's choices are reached.
fn press_row(row: usize, steps: usize) -> Vec<Gesture> {
    let mut walk = vec![Gesture::Step(1); row];
    walk.push(Gesture::Press);
    walk.extend(core::iter::repeat_n(Gesture::Step(1), steps));
    walk
}

/// One seed per reachable screen state. A screen whose copy changes with its state is seeded once
/// per state, because the gate measures what a screen draws, not what it could draw.
fn seeds(language: Language) -> Vec<Seed> {
    let mut v: Vec<Seed> = plain(vec![
        Screen::Home(HomeScreen::new()),
        Screen::Map(MapScreen::new()),
        Screen::Assistant(AssistantScreen::new()),
        Screen::Journey(JourneyScreen::new(false)),
        Screen::Journey(JourneyScreen::new(true)),
        Screen::Landmarks(LandmarksScreen),
        Screen::LandmarkSources(LandmarkSourcesScreen),
        Screen::LandmarkPhoto(LandmarkPhotoScreen::new(
            crate::photo::Selection { qid: 1, map_generation: 0, record_index: 0 },
            "Cabane du Mont Fort",
        )),
        Screen::Statistics(StatisticsScreen::new()),
        Screen::Climb(ClimbScreen::new()),
        Screen::RideControl(RideControl::new()),
        Screen::RideStart(RideStartScreen::new()),
        Screen::DayDone(DayDoneScreen::new(DAY, 1, 7, 11, Some(12))),
        Screen::DayDone(DayDoneScreen::new(DAY, 1, 7, 11, None)),
        Screen::Menu(MenuScreen::new()),
        Screen::PeakView(PeakViewScreen::new(None)),
        Screen::Detour(DetourScreen::new(&RouteState::default())),
        Screen::DetourPreview(DetourPreviewScreen::new(
            &DetourScreen::new(&RouteState::default()),
            DetourPreview { cost_delta_m: -1_500, total_distance_m: 8_000, rejoin_m: 9_000, ascent_m: Some(120) },
        )),
        Screen::WhatsNext(WhatsNextScreen::new()),
        Screen::FindPlace(FindPlaceScreen::new()),
        Screen::VisitReview(VisitReviewScreen::new("Fontaine du Mont Ventoux")),
        Screen::PoiList(PoiListScreen::new(PoiCategory::Water)),
        Screen::Easier(EasierScreen::new()),
        Screen::Easier(EasierScreen::sample(crate::easier::Phase::Failed, true)),
        Screen::Easier(EasierScreen::sample(crate::easier::Phase::NoBetter, true)),
        Screen::Easier(EasierScreen::sample(crate::easier::Phase::NoBetter, false)),
        Screen::Easier(EasierScreen::sample(crate::easier::Phase::Stale, true)),
        selected_place(),
        Screen::NavPlanning(NavPlanningScreen::new("Fontaine du Mont Ventoux")),
        Screen::NavPlanning(NavPlanningScreen::detour()),
        Screen::NavFail(NavFailScreen::too_far()),
        Screen::NavFail(NavFailScreen::not_found()),
        Screen::NavFail(NavFailScreen::detour_too_far()),
        Screen::NavFail(NavFailScreen::detour_not_found()),
        Screen::RouteMenu(RouteMenuScreen::new()),
        Screen::RouteCleanup(RouteCleanupScreen::new(None, StoreIdentity::new(1))),
        Screen::TripDelete(TripDeleteScreen::new(7, "Fontaine du Mont Ventoux Loop")),
        Screen::Rides(RidesScreen::new()),
        Screen::RideDetail(RideDetailScreen::new(0)),
        Screen::RouteOverview(RouteOverviewScreen::new(0, None)),
        Screen::StartAway(StartAwayScreen::sample(false)),
        Screen::StartAway(StartAwayScreen::sample(true)),
        Screen::Arrival(ArrivalScreen::new(ArrivalView { route: 0, day: Some(1), next: Some(1) })),
        Screen::Arrival(ArrivalScreen::new(ArrivalView { route: 0, day: None, next: None })),
        Screen::RouteSwap(RouteSwapScreen::new(0)),
        Screen::RouteReceived(RouteReceivedScreen::new(0, 0, None)),
        Screen::RouteUpdated(RouteUpdatedScreen::new(0, 0)),
        Screen::TripReceived(TripReceivedScreen::new(7, 0)),
        Screen::Passkey(PasskeyScreen::new(123_456)),
        Screen::MapTransfer(MapTransferScreen::new(MapTransfer::Receiving { received_kib: 1_024, total_kib: 65_536 })),
        Screen::MapTransfer(MapTransferScreen::new(MapTransfer::Installed)),
        Screen::Settings(SettingsPage::hub()),
        Screen::Ride(SettingsPage::new(&page::RIDE)),
        Screen::Display(SettingsPage::new(&page::DISPLAY)),
        Screen::Sound(SettingsPage::new(&page::SOUND)),
        Screen::Connections(SettingsPage::new(&page::CONNECTIONS)),
        Screen::Power(SettingsPage::new(&page::POWER)),
        Screen::System(SettingsPage::new(&page::SYSTEM)),
        Screen::DateTime(SettingsPage::new(&page::DATETIME)),
        Screen::Firmware(SettingsPage::new(&page::FIRMWARE)),
        Screen::StatFields(StatFieldsScreen::new()),
        Screen::AddField(AddFieldScreen::new()),
        Screen::Sensors(SensorsScreen::new()),
        Screen::SensorScan(SensorScanScreen::new(0)),
        Screen::Language(LanguageScreen::new(language)),
        Screen::About(AboutScreen::new()),
        Screen::Reset(ResetScreen::new()),
        Screen::DfuCheck(DfuCheckScreen::new()),
        Screen::DfuConfirm(DfuConfirmScreen::new(DfuScanReport::new("1.4.0", "1.5.0", false))),
        Screen::DfuConfirm(DfuConfirmScreen::new(DfuScanReport::new("1.4.0", "1.4.0", true))),
        Screen::DfuProgress(DfuProgressScreen::new()),
        Screen::DfuInstalling(DfuInstallingScreen::new()),
        Screen::DfuUpdated(DfuUpdatedScreen::new("1.5.0")),
    ]);
    v.extend(plain(
        [
            RecoveryMode::Resumable,
            RecoveryMode::Damaged,
            RecoveryMode::RepairFailed,
            RecoveryMode::DiscardFailed,
            RecoveryMode::Unrepairable,
        ]
        .map(|mode| Screen::RideRecovery(RideRecoveryScreen::new(mode)))
        .into(),
    ));
    v.extend(plain(
        [MapTransferError::Storage, MapTransferError::Damaged, MapTransferError::NotAMap, MapTransferError::Refused]
            .map(|e| Screen::MapTransfer(MapTransferScreen::new(MapTransfer::Failed(e))))
            .into(),
    ));
    v.extend(plain(
        [
            WarningFlags::NO_GPS,
            WarningFlags::NO_ALTIMETER,
            WarningFlags::NO_COMPASS,
            WarningFlags::REC_ERROR,
            WarningFlags::SETTINGS_ERROR,
            WarningFlags::STORAGE_ERROR,
        ]
        .map(|flags| Screen::Warning(WarningScreen::new(flags)))
        .into(),
    ));
    v.extend(plain(
        [
            DfuScanError::NotFound,
            DfuScanError::Unreadable,
            DfuScanError::Damaged,
            DfuScanError::TooLarge,
            DfuScanError::TooFragmented,
            DfuScanError::Untrusted,
        ]
        .map(|e| Screen::DfuError(DfuErrorScreen::new(e)))
        .into(),
    ));
    v.extend(plain(
        [DfuFailure::NotStarted, DfuFailure::Reverted]
            .map(|why| Screen::DfuFailed(DfuFailedScreen::new(why, Some("1.5.0"))))
            .into(),
    ));
    // The About page is taller than the panel, so the lines under the fold are drawn only after it
    // scrolls. One step per line reaches every one of them; the offset clamps at the end.
    v.push((Screen::About(AboutScreen::new()), vec![Gesture::Step(1); 24]));
    // A sheet's rows hide the copy of the page each one opens: an editor's choices, a toggle's
    // other state, or the screen a door row replaces the sheet with.
    for row in 0..4 {
        v.push((Screen::QuickDrawer(QuickDrawerScreen::opening()), press_row(row, CHOICES)));
    }
    let mut power = press_row(3, 0);
    power.push(Gesture::Hold); // the completed hold, which is the powering-off frame
    v.push((Screen::QuickDrawer(QuickDrawerScreen::opening()), power));
    for menu in CONTEXT_MENUS {
        v.push((Screen::ContextDrawer(ContextDrawerScreen::opening(menu, language)), Vec::new()));
        for (row, declared) in menu.rows.iter().enumerate() {
            // Only a value row opens an editor with choices to step through. A toggle draws one
            // more state, and a door row replaces the sheet with a screen seeded in its own right.
            let steps = if matches!(declared.action, ContextAction::Edit(_)) { CHOICES } else { 1 };
            v.push((Screen::ContextDrawer(ContextDrawerScreen::opening(menu, language)), press_row(row, steps)));
        }
    }
    v
}

/// Every declared context sheet. The chord opens one of these and no other.
const CONTEXT_MENUS: &[&ContextMenu] = &[
    &ctx_menu::RIDE,
    &ctx_menu::MAP,
    &ctx_menu::MAP_DISPLAY,
    &ctx_menu::MAP_ICONS,
    &ctx_menu::MAP_POI_CATEGORIES,
    &ctx_menu::UP_AHEAD,
    &ctx_menu::ROUTE_PLAN,
    &ctx_menu::FIND_PLACE,
    &ctx_menu::ASSISTANT_RESUME,
    &ctx_menu::ASSISTANT_VISIT,
    &ctx_menu::LANDMARK_CONTENT,
];

/// Seeds with no walk: the screen draws all its copy on its first frame.
fn plain(screens: Vec<Screen>) -> Vec<Seed> {
    screens.into_iter().map(|s| (s, Vec::new())).collect()
}

/// Walk `seed` in `language` and return every string its frames asked to draw, with the screen the
/// frame was on. `stage` runs once the seed is on the stack, for a screen whose page needs domain
/// state under it. The clock steps past a sheet's slide between gestures, because a sliding page
/// owns its input.
fn walk(
    seed: Seed,
    language: Language,
    bytes: &[u8],
    stage: impl FnOnce(&mut App),
) -> Vec<(&'static str, obc_render::text_tap::TextDraw)> {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid fixture");
    let reader = Reader::new(&src, &tables, &cache);
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_settings(Settings { language, ..Default::default() });
    // A panel light adds the quick drawer's brightness control, which is the row its editor hangs
    // off, so the walk below reaches that page.
    app.set_backlight_available(true);
    let (screen, gestures) = seed;
    assert!(app.ui.stack.push(screen).is_ok(), "the seed fits the screen stack");
    stage(&mut app);

    let mut scratch = Box::new(obc_render::RenderScratch::new());
    let mut buf = Buf::new(PANEL.width as i32, PANEL.height as i32);
    let mut drawn = Vec::new();
    let mut now = SLIDE_STEP_MS;
    for step in 0..=gestures.len() {
        if step > 0 {
            app.advance_animations(InputClock(now));
            app.apply_gesture(gestures[step - 1]);
            now += SLIDE_STEP_MS;
            app.advance_animations(InputClock(now));
        }
        let name = app.top_screen().name();
        let frame = obc_render::text_tap::record(|| {
            app.render_frame(
                Some(&mut scratch),
                &mut buf,
                &reader,
                None,
                PANEL.width as f32,
                PANEL.height as f32,
                |c| {
                    let (r, g, b) = rgb565_to_rgb888(c);
                    Rgb888::new(r, g, b)
                },
            );
        });
        drawn.extend(frame.into_iter().map(|d| (name, d)));
    }
    drawn
}

/// Longer than any sheet slide, so the page a gesture opened has landed before the next one.
const SLIDE_STEP_MS: u32 = 400;

/// Why `drawn` does not fit, if it does not.
fn complaint(name: &str, language: Language, drawn: &obc_render::text_tap::TextDraw) -> Option<String> {
    let (at, size) = (drawn.area.top_left, drawn.area.size);
    let (right, bottom) = (at.x + size.width as i32, at.y + size.height as i32);
    if at.x < 0 || right > PANEL.width as i32 || at.y < 0 || bottom > PANEL.height as i32 {
        return Some(format!(
            "  {name} {language:?}: {:?} draws off the panel, at x {}..{}, y {}..{}",
            drawn.text, at.x, right, at.y, bottom
        ));
    }
    // A line the screen centres on the panel midline is laid out across the whole panel.
    let centred = at.x + right == PANEL.width as i32;
    (centred && size.width as i32 > COPY_W).then(|| {
        format!(
            "  {name} {language:?}: centred {:?} is {} px over the {COPY_W} px copy width",
            drawn.text,
            size.width as i32 - COPY_W
        )
    })
}

#[test]
fn every_screen_is_seeded() {
    let seeded: Vec<&str> = seeds(Language::En).iter().map(|(s, _)| s.name()).collect();
    let missing: Vec<&&str> = Screen::NAMES.iter().filter(|n| !seeded.contains(n) && !NO_SEED.contains(n)).collect();
    assert!(missing.is_empty(), "a new screen needs a seed in the copy-fit gate: {missing:?}");
}

#[test]
fn every_string_fits_the_panel_in_every_language() {
    let bytes = build_min_obcm(0xF800);
    let mut offenders: Vec<String> = Vec::new();
    for language in Language::ALL {
        for seed in seeds(language) {
            for (name, drawn) in walk(seed, language, &bytes, |_| {}) {
                offenders.extend(complaint(name, language, &drawn));
            }
        }
    }
    report(offenders);
}

/// The reading page, which `Landmarks`, `LandmarkSources` and `PeakArticle` all draw. It needs a
/// map with a landmark section under it, so it is its own sweep: the minimal fixture leaves the
/// page on its status line, which draws none of the article's copy.
#[test]
fn the_article_page_fits_the_panel_in_every_language() {
    // The first article line the fixture carries. A page that is not ready draws its status line
    // and none of the article, which is a hole rather than a pass, so the sweep proves it read one.
    const FIRST_LINE: &str = "First source page.";

    let bytes = crate::landmarks::tests::map();
    let mut offenders: Vec<String> = Vec::new();
    let mut read_an_article = false;
    for language in Language::ALL {
        // Press opens the selected card's article; each step turns a page of it, or of the sources.
        for seed in [
            (Screen::Landmarks(LandmarksScreen), vec![Gesture::Press, Gesture::Step(1), Gesture::Step(1)]),
            (Screen::LandmarkSources(LandmarkSourcesScreen), vec![Gesture::Step(1), Gesture::Step(1)]),
        ] {
            for (name, drawn) in walk(seed, language, &bytes, |app| app.ui.landmarks.restart(false)) {
                read_an_article |= drawn.text == FIRST_LINE;
                offenders.extend(complaint(name, language, &drawn));
            }
        }
    }
    assert!(read_an_article, "no frame drew {FIRST_LINE:?}: the walk never reached the article page");
    report(offenders);
}

/// Fail on everything that did not fit, once, naming each one.
fn report(mut offenders: Vec<String>) {
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "{} string(s) do not fit the {}x{} panel — they draw clipped on-glass:\n{}",
        offenders.len(),
        PANEL.width,
        PANEL.height,
        offenders.join("\n"),
    );
}
