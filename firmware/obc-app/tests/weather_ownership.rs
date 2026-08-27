//! **Who decides what the rider is told about weather** (#1549, epic #1433 §2).
//!
//! Every fact on the weather surfaces is `WeatherDomain`'s: whether an update is running, when a
//! weather screen repaints, how many rain steps exist, when the alert engine runs, and whether
//! opening the dashboard is worth a radio trip. Before this slice each of those lived in a host —
//! two of them in *three* hosts — and the domain had no production writer at all.
//!
//! Each test names the mutant it fails against, because a test that passes against the mistake it
//! was written for is not evidence.

use obc_app::device_core::{
    DataIdentity, DerivedInputs, DerivedTargets, ExternalFacts, OutcomeSlots, PassClock, PassInputs, PassPlan,
    Revision, WeatherData,
};
use obc_app::weather::{WeatherEffect, WeatherOutcome};
use obc_app::{App, AppState, BleLink, BleStatus, Gesture, Screen, WeatherSnapshot};
use obc_ports::{InputClock, RideClock, Sensors};

mod common;
use common::{build_min_obcm, weather_snapshot, Buf, OnceFix, EVERY_CAPABILITY};

/// A miniature runtime: one pass, then the one thing a weather executor does — **raise** the
/// request and answer that, which is all `RequestRefresh` means. Without it the domain's operation
/// never terminates and the UPDATING cue would stay up for the rest of the test, which is exactly
/// the behaviour a real executor's answer prevents.
#[derive(Default)]
struct Host {
    outcomes: OutcomeSlots,
    /// How many requests actually reached the radio.
    raised: usize,
}

impl Host {
    fn pass(
        &mut self,
        app: &mut App,
        ms: u32,
        snapshot: Option<&WeatherSnapshot>,
        note: impl FnOnce(&mut ExternalFacts),
    ) -> PassPlan {
        let mut facts = ExternalFacts::NONE;
        note(&mut facts);
        let mut loc = OnceFix(None);
        let mut plan = app.run_pass(PassInputs {
            now: PassClock { ride: RideClock(ms), ui: InputClock(ms) },
            gestures: &[],
            sensors: Sensors::new(&mut loc),
            route: None,
            weather: snapshot,
            support: EVERY_CAPABILITY,
            outcomes: &mut self.outcomes,
            facts: &mut facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
        if let Some(WeatherEffect::RequestRefresh { token }) = plan.effects.weather.take() {
            self.raised += 1;
            let _ = self.outcomes.weather.try_put(WeatherOutcome::Raised { token });
        }
        plan
    }
}

/// The one product these tests install.
const PRODUCT: u64 = 4;

fn installed(revision: u64) -> WeatherData {
    WeatherData { data: DataIdentity::new(PRODUCT), revision: Revision::new(revision) }
}

/// A device with a companion connected, so `WeatherCapabilities::refresh` is up — without one the
/// domain refuses to start a refresh at all, which is its own test below.
fn linked() -> App {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_ble_status(BleStatus { link: BleLink::Connected, paired: true, passkey: None });
    app
}

/// A five-frame bundle at the app's own wall instant: four frames lie ahead of NOW.
fn bundle(app: &App, frames: usize) -> WeatherSnapshot {
    weather_snapshot(app.wall_unix_now() as i64, &vec![0u8; frames], None)
}

/// A bundle whose frames are all in the past — nothing is current, so nothing may be claimed and
/// (by the engine's own law) nothing may alert.
fn expired_bundle(app: &App) -> WeatherSnapshot {
    let now = app.wall_unix_now() as i64;
    let mut snap = weather_snapshot(now - 100_000, &[12; 9], None);
    snap.valid_until = now - 90_000;
    snap
}

/// Back out of the dashboard and open it again — the dial is still on the Weather station, so this
/// is the same push site a second time.
fn reopen_dashboard(app: &mut App) {
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Menu(_)));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Weather(_)));
}

/// Walk the Menu to the Weather station — the *only* push site of `Screen::Weather`, and therefore
/// the only place the refresh intent is named.
fn open_dashboard(app: &mut App) {
    app.apply_gesture(Gesture::Press); // Home → Menu
    for _ in 0..4 {
        app.apply_gesture(Gesture::Step(1));
    }
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Weather(_)));
}

/// Run quiet passes until the device stops asking for a repaint, so what a later pass dirties is
/// this test's own change and not the screen push that preceded it.
fn quiesce(host: &mut Host, app: &mut App, snapshot: Option<&WeatherSnapshot>) {
    for i in 0..8 {
        if !host.pass(app, 1_000 + i * 10, snapshot, |_| {}).render.map {
            return;
        }
    }
    panic!("the device never stopped repainting");
}

/// How many requests the executor raised over `passes` quiet frames. More than one pass, because
/// the companion capability is the level stage 12 calculated on the *previous* one.
fn refreshes(host: &mut Host, app: &mut App, passes: u32) -> usize {
    let before = host.raised;
    for i in 0..passes {
        host.pass(app, 100 + i * 10, None, |_| {});
    }
    host.raised - before
}

// ==================== the domain has production writers ====================

/// **An installed bundle reaches the domain from a real host.**
///
/// Mutant: drop the host's `note_weather_data` report — `installed()` stays `None` on glass, which
/// is exactly the tree this slice started from: the fact existed, stage 2 consumed it, and no
/// production host ever filled it.
#[test]
fn an_installed_bundle_reaches_the_domain_from_a_real_host() {
    let (mut host, mut app) = (Host::default(), linked());
    assert_eq!(app.weather().installed(), None, "nothing is installed at boot");

    host.pass(&mut app, 100, None, |facts| facts.note_weather_data(installed(1)));
    assert_eq!(app.weather().installed(), Some(installed(1)), "the platform's report is the domain's level");
    assert_eq!(app.weather().last_refresh(), None, "a first sighting is the card's own bundle, not a fetch");

    // A *move* of that level is what a completed refresh looks like from here.
    host.pass(&mut app, 110, None, |facts| facts.note_weather_data(installed(2)));
    assert_eq!(app.weather().last_refresh(), Some(obc_app::weather::RefreshResult::Installed));
}

// ==================== the resample is the repaint edge ====================

/// **A resample repaints an open dashboard without a dirty flag.**
///
/// Mutant: delete the sample revision but keep the key — the dashboard freezes on a stale card.
/// This is what the deleted between-pass seam did by hand, by sniffing the top screen, and the
/// reason it survived S4: a resample changes the card's contents with every other fact unmoved, so
/// no stack-local key could see it until the domain held one.
#[test]
fn a_resample_repaints_an_open_dashboard_without_a_dirty_flag() {
    let (mut host, mut app) = (Host::default(), linked());
    open_dashboard(&mut app);
    let snap = bundle(&app, 5);
    host.pass(&mut app, 100, Some(&snap), |facts| facts.note_weather_sample(Revision::new(1)));
    quiesce(&mut host, &mut app, Some(&snap));

    let quiet = host.pass(&mut app, 1_100, Some(&snap), |_| {});
    assert!(!quiet.render.map, "an unchanged sample repaints nothing");

    let resampled = host.pass(&mut app, 1_110, Some(&snap), |facts| facts.note_weather_sample(Revision::new(2)));
    assert!(resampled.render.map, "a resample repaints the card the rider is looking at");
}

/// **A resample does not repaint Home.**
///
/// Mutant: dirty unconditionally instead of through the key — a 1 Hz full-chrome redraw at GPS
/// cadence, which is the exact cost the deleted screen sniff was written to avoid.
#[test]
fn a_resample_does_not_repaint_home() {
    let (mut host, mut app) = (Host::default(), linked());
    assert!(matches!(app.top_screen(), Screen::Home(_)), "Home draws no weather");
    let snap = bundle(&app, 5);
    host.pass(&mut app, 100, Some(&snap), |facts| facts.note_weather_sample(Revision::new(1)));
    quiesce(&mut host, &mut app, Some(&snap));

    let plan = host.pass(&mut app, 1_100, Some(&snap), |facts| facts.note_weather_sample(Revision::new(2)));
    assert!(!plan.render.map, "a background resample must not cost Home a full-chrome redraw");
}

// ==================== the cue is the domain's answer ====================

/// Render the dashboard and report whether the UPDATING cue is up, by comparing the title band
/// against a frame rendered with the cue known to be down.
fn dashboard_band(app: &mut App, map: &[u8], snap: &WeatherSnapshot) -> Vec<embedded_graphics::pixelcolor::Rgb888> {
    use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
    let src = SliceSource(map);
    let tables = MapTables::parse(&src).expect("valid map");
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame_with_rain(Some(&mut scratch), &mut buf, &reader, None, None, Some(snap), 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
    });
    buf.px[..40 * 240].to_vec()
}

/// **The UPDATING cue follows the domain, not the platform flag.**
///
/// Mutant: keep `Render::weather_refreshing` fed from the executor's own argument — the cue and
/// `WeatherDomain::refreshing()` can then disagree, which is the two-sources-of-truth this slice
/// exists to end. The render entries no longer *have* a place to put a second answer.
#[test]
fn the_updating_cue_follows_the_domain_not_the_platform_flag() {
    let map = build_min_obcm(0x07E0);
    let (mut host, mut app) = (Host::default(), linked());
    open_dashboard(&mut app);
    let snap = bundle(&app, 5);
    quiesce(&mut host, &mut app, Some(&snap));
    assert!(!app.weather().refreshing());
    let quiet = dashboard_band(&mut app, &map, &snap);

    host.pass(&mut app, 1_100, Some(&snap), |facts| facts.note_weather_refreshing(true));
    assert!(app.weather().refreshing(), "the domain is the one answer");
    let cue = dashboard_band(&mut app, &map, &snap);
    assert_ne!(quiet, cue, "the title band carries the domain's answer");

    host.pass(&mut app, 1_110, Some(&snap), |facts| facts.note_weather_refreshing(false));
    assert!(!app.weather().refreshing());
    assert_eq!(dashboard_band(&mut app, &map, &snap), quiet, "and it falls with it");
}

/// **A periodic fetch nobody ordered still shows the cue.**
///
/// Mutant: make `refreshing()` token-only — the `weather_refresh` cadence's own fetches carry no
/// token, so the cue would go dark during exactly the fetches today's board shows it for. This is
/// the case the owner decision turns on: "a fetch is running" is an external fact precisely because
/// nobody asked for it.
#[test]
fn a_periodic_fetch_nobody_ordered_still_shows_the_cue() {
    let (mut host, mut app) = (Host::default(), linked());
    // The rider never opened the dashboard, so no intent was named, no operation is in flight and
    // no token exists — and the plane is fetching anyway, on its own cadence.
    host.pass(&mut app, 100, None, |facts| facts.note_weather_refreshing(true));
    assert!(app.weather().refreshing(), "a cadence fetch is still a fetch the rider can see");
    assert!(!app.weather().refresh_pending(), "…and nothing was requested to produce it");
}

// ==================== the intent has one producer ====================

/// **Two taps of the dashboard row are one request.**
///
/// Mutant: drop `apply_intent`'s coalesce — two radio trips on a metered link. The second tap's
/// answer is the fetch already in the air, whoever started it.
#[test]
fn two_taps_of_the_dashboard_row_are_one_request() {
    let (mut host, mut app) = (Host::default(), linked());
    // Two entries inside one gesture batch: Weather → Back → Weather. The board's deleted screen
    // sniff called this one entry edge on purpose, and the coalesce is what preserves that.
    open_dashboard(&mut app);
    reopen_dashboard(&mut app);
    assert_eq!(refreshes(&mut host, &mut app, 3), 1, "one question, not two");

    // …and a tap while the provider plane is already fetching is the same question again.
    host.pass(&mut app, 200, None, |facts| facts.note_weather_refreshing(true));
    reopen_dashboard(&mut app);
    assert_eq!(refreshes(&mut host, &mut app, 3), 0, "a fetch already in the air answers this tap too");
}

/// **A refresh asked for without a companion is not started.**
///
/// Mutant: drop the `WeatherCapabilities::refresh` gate — an effect is emitted that no link can
/// serve, and the rider is told a fetch failed rather than never being told a fetch happened.
#[test]
fn a_refresh_asked_for_without_a_companion_is_not_started() {
    let (mut host, mut app) = (Host::default(), App::new_idle(AppState::new(0, 0, 1.0))); // never connected
    open_dashboard(&mut app);
    assert_eq!(refreshes(&mut host, &mut app, 3), 0, "no link, no radio trip");
    assert!(app.weather().refresh_pending(), "and no invented failure either — the rider still asked");

    // The link returns and the question the rider asked goes out.
    app.set_ble_status(BleStatus { link: BleLink::Connected, paired: true, passkey: None });
    assert_eq!(refreshes(&mut host, &mut app, 3), 1);
}

/// **Opening Hourly and coming back asks nothing.**
///
/// Mutant: name the intent on the Weather screen's own `handle` instead of on the row that pushes
/// it — Back from Hourly then manufactures a second urgent request, which is the bug the board's
/// `was_on_weather` comparison existed to avoid.
#[test]
fn opening_hourly_and_coming_back_asks_nothing() {
    let (mut host, mut app) = (Host::default(), linked());
    open_dashboard(&mut app);
    assert_eq!(refreshes(&mut host, &mut app, 3), 1, "the entry raises one");

    app.apply_gesture(Gesture::Press); // → Hourly
    assert!(matches!(app.top_screen(), Screen::WeatherHourly(_)));
    app.apply_gesture(Gesture::Back); // → back to the dashboard
    assert!(matches!(app.top_screen(), Screen::Weather(_)));
    assert_eq!(refreshes(&mut host, &mut app, 3), 0, "returning to the dashboard is not a new question");
}

// ==================== the view state has one interpreter ====================

/// **The rain step clamps to the domain's step count.**
///
/// Mutant: leave `rain_steps_ahead` in `AppState` — two copies of the same figure, and the rider's
/// cursor clamps against whichever one the host last remembered to refresh.
#[test]
fn the_rain_step_clamps_to_the_domains_step_count() {
    let (mut host, mut app) = (Host::default(), linked());
    let five = bundle(&app, 5);
    host.pass(&mut app, 100, Some(&five), |_| {});
    assert_eq!(app.weather().steps_ahead(), 4, "five frames, four of them ahead");

    open_dashboard(&mut app);
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press); // → the rain map
    assert!(matches!(app.top_screen(), Screen::WeatherRainMap(_)));
    for _ in 0..9 {
        app.apply_gesture(Gesture::Step(1));
    }
    assert_eq!(app.state.rain_step, 4, "the cursor stops at the last frame that exists");

    // The bundle shrinks under the rider: the cursor comes back with it, at stage 10 and nowhere
    // else — no host re-clamps anything.
    let two = bundle(&app, 2);
    host.pass(&mut app, 200, Some(&two), |_| {});
    assert_eq!(app.weather().steps_ahead(), 1);
    assert_eq!(app.state.rain_step, 1, "the cursor clamps against the domain's new count");
}

// ==================== the alert decision runs at stage 10 ====================

/// **An alert is evaluated once per pass, not once per resample.**
///
/// Mutant: leave the tick in the executor — a host that resamples twice between passes evaluates
/// twice, and one that never resamples never evaluates at all. *When* the honesty law runs stops
/// being the executor's choice only when the stage owns it.
#[test]
fn an_alert_is_evaluated_once_per_pass_not_once_per_resample() {
    let (mut host, mut app) = (Host::default(), linked());
    let storm = weather_snapshot(app.wall_unix_now() as i64, &[12; 9], None);

    // No resample at all this pass — and the decision still runs.
    host.pass(&mut app, 100, Some(&storm), |_| {});
    assert!(matches!(app.top_screen(), Screen::WeatherAlert(_)), "the card fires from the stage, not from a resample");
    let depth = app.debug_stack_len();

    // Three more passes over the same storm: the card is updated in place, never stacked, and the
    // persisted mark is written once.
    for i in 0..3 {
        host.pass(&mut app, 110 + i * 10, Some(&storm), |_| {});
    }
    assert_eq!(app.debug_stack_len(), depth, "one card, however many passes evaluate it");
}

/// **A stale bundle alerts nothing from stage ten.**
///
/// Mutant: drop the validity gate on the way into the stage — the engine's own law ("no snapshot
/// never alerts, and neither does expired data") re-proved at its new call site, because a law
/// that moves is a law that has to be re-proved.
#[test]
fn a_stale_bundle_alerts_nothing_from_stage_ten() {
    let (mut host, mut app) = (Host::default(), linked());
    let expired = expired_bundle(&app);
    for i in 0..4 {
        host.pass(&mut app, 100 + i * 10, Some(&expired), |_| {});
    }
    assert!(!matches!(app.top_screen(), Screen::WeatherAlert(_)), "expired data claims nothing");

    // …and neither does no bundle at all.
    for i in 0..4 {
        host.pass(&mut app, 200 + i * 10, None, |_| {});
    }
    assert!(!matches!(app.top_screen(), Screen::WeatherAlert(_)), "no snapshot never alerts");
}

/// A facts batch is only ever built through the `note_*` doors, so this suite's helper closures
/// stay honest about what a host can actually report.
#[test]
fn the_two_new_weather_levels_merge_like_every_other_level() {
    let mut facts = ExternalFacts::NONE;
    facts.note_weather_sample(Revision::new(7));
    facts.note_weather_sample(Revision::new(4));
    assert_eq!(facts.weather_sample(), Some(Revision::new(7)), "a reordered report cannot walk the edge backwards");

    facts.note_weather_refreshing(true);
    facts.note_weather_refreshing(false);
    assert_eq!(facts.weather_refreshing(), Some(false), "the newest level is the truth");
}
