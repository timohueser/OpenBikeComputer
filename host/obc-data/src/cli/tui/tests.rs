use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::{json, Value};

use super::*;
use crate::cli::build_cli::EnvPlan;
use crate::cli::status_cli::{Attention, AttentionKind, LayerStatus, ProductStatus};
use crate::cli::Stored;
use crate::sources::parse_sources;
use crate::store::gc::Kept;

const SOURCES: &str = r#"
    [[source]]
    id = "osm"
    kind = "data"
    licence = "ODbL-1.0"
    attribution = "© OpenStreetMap contributors"
    fetch = { kind = "http", url = "https://planet.openstreetmap.org/pbf/planet-{yymmdd}.osm.pbf" }
    version = "date"
    refresh = 7
    redistribute = true

    [[source]]
    id = "planetiler"
    kind = "tool"
    fetch = { kind = "github", url = "https://api.github.com/repos/onthegomap/planetiler" }
    version = "release"
    refresh = "manual"
    redistribute = true
"#;

const REGION: &str = "europe/germany/baden-wuerttemberg";

/// What `status --json` writes: `maps` is unknown, and `planner` has the optional layer `sun`
/// on, which live lacks.
fn status() -> Status {
    let layer = |layer: &str, state, reason: &str| LayerStatus {
        layer: layer.into(),
        state,
        reason: Some(reason.into()).filter(|reason: &String| !reason.is_empty()),
    };
    let product =
        |product: &str, release: &str, applied: Option<&str>, bytes, optional: &[&str], layers| ProductStatus {
            product: product.into(),
            release: Some(release.repeat(8)),
            applied: applied.map(str::to_string),
            bytes: Some(bytes),
            optional: optional.iter().map(|layer| layer.to_string()).collect(),
            layers,
        };
    let planner = vec![
        layer("planner/basemap", State::Stale, "osm: 120 d > 90 d"),
        layer("planner/routing", State::CodeChanged, "host/route-build/src/main.rs"),
        layer("planner/overlays", State::InputChanged, "planner/routing"),
        layer("planner/sun", State::NotApplied, "missing in live"),
    ];
    let attention = |kind, about: &str, reason: &str| Attention { kind, about: about.into(), reason: reason.into() };
    Status {
        from: "https://maps.openbikecomputer.com".into(),
        products: vec![
            product("maps", "3f9a2c1e", None, 980_000_000, &[], None),
            product(
                "planner",
                "8b0d47a5",
                Some("2026-10-02T09:14:05Z"),
                2_370_000_000,
                &["climate", "sun"],
                Some(planner),
            ),
        ],
        attention: vec![
            attention(AttentionKind::Stale, "osm", "120 d > 90 d"),
            attention(AttentionKind::OldCache, "/home/rider/obc-bake", "12 files, 1.2 GB"),
            attention(AttentionKind::Unreachable, "maps", "a fetch that the step list needs failed"),
        ],
        check: None,
    }
}

/// What `plan live --json` writes: the edit of `sun`, two stale sources and a change of code.
fn plan() -> EnvPlan {
    let build = |step: &str, minutes: u64, bytes_out: u64| {
        let estimate = json!({"wall_ms": minutes * 60_000, "bytes_out": bytes_out, "peak_rss_bytes": null});
        json!({"step": step, "recipe": "recipe", "key": null, "estimate": estimate})
    };
    let fetch = |source: &str, version: &str, bytes: u64| json!([{"source": source, "version": version, "params": [], "files": [], "bytes": bytes}]);
    let group = |id: &str, cause: Value, fetches: Value, builds: Vec<Value>| json!({"id": id, "cause": cause, "layers": builds, "drops": [], "fetches": fetches, "builds": builds});
    let (basemap, routing) = (build("planner/basemap", 41, 324_000_000), build("planner/routing", 37, 1_040_000_000));
    let groups = [
        group("layers", json!({"kind": "layers"}), json!([]), vec![build("planner/sun", 55, 30_000_000)]),
        group(
            "move:land",
            json!({"kind": "move", "source": "land", "from": ["2024-01-01"], "to": "2024-01-03"}),
            fetch("land", "2024-01-03", 950_000_000),
            vec![basemap.clone()],
        ),
        group(
            "move:osm",
            json!({"kind": "move", "source": "osm", "from": ["2024-01-02"], "to": "2024-01-09"}),
            fetch("osm", "2024-01-09", 710_000_000),
            vec![basemap, routing.clone()],
        ),
        group(
            "code:planner/routing",
            json!({"kind": "code", "paths": ["host/route-build"], "crates": []}),
            json!([]),
            vec![routing, build("planner/overlays", 7, 63_000_000)],
        ),
    ];
    serde_json::from_value(json!({
        "env": "live",
        "region": REGION,
        "layers": ["sun"],
        "moves": {"land": "2024-01-03", "osm": "2024-01-09"},
        "versions": [],
        "live": [{"product": "planner", "release": "8b0d47a5".repeat(8)}],
        "edits": [{"kind": "layers", "product": "planner", "on": ["sun"], "off": []}],
        "only": [],
        "groups": groups,
        "blocked": [{"product": "maps", "reason": "source `wikidata` is blocked", "layers": []}],
        "remove": [{"key": "planner/objects/aa", "bytes": 1_100_000_000}, {"key": "planner/objects/bb", "bytes": 5_000_000}],
        "listed": false,
        "needs_prepare": false,
    }))
    .unwrap()
}

fn app() -> App {
    let sources = parse_sources(SOURCES).unwrap().into_iter().map(|source| SourceRow {
        live: Some(vec![if source.kind == Kind::Tool { "0.10.2" } else { "2024-01-02" }.into()]),
        upstream: (source.kind == Kind::Data).then(|| "2024-01-09".into()),
        age_days: None,
        state: State::Ok,
        reason: None,
        snapshots: vec![Stored { version: "2024-01-02".into(), bytes: 1_000_000 }],
        requests: Vec::new(),
        credential_missing: false,
        source,
    });
    let run = |id: &str| Details {
        summary: Summary {
            id: id.into(),
            command: "build test".into(),
            started: "2024-01-10T12:00:00Z".into(),
            outcome: Outcome::Ok,
            wall_ms: Some(1000),
            bytes_fetched: 0,
            bytes_built: 10,
        },
        error: None,
        phase: None,
        published: Vec::new(),
        fetches: Vec::new(),
        steps: Vec::new(),
    };
    let mut app = App::new(sources.collect(), vec![run("2024-01-10-120000"), run("2024-01-09-120000")]);
    let store = gc::Plan {
        kept: vec![Kept { entry: "osm@2024-01-02".into(), bytes: 1_000_000, because: vec!["live maps".into()] }],
        snapshots: vec!["osm@2023-12-01".into()],
        objects: vec![("ab".repeat(32), 1_000_000)],
        remove_bytes: 1_000_000,
        ..gc::Plan::default()
    };
    app.store = Some(CleanPlan { store, ..CleanPlan::default() });
    app.status = Some(status());
    app.env = Some(Edited { env: LIVE.into(), region: REGION.into(), layers: vec!["sun".into()] });
    app.edited = true;
    app.regions = Ok(["europe/andorra", REGION, "monaco"]
        .map(|id| crate::regions::parse_region(id, "name='Test region'\nkind='box'\nbox=[7,47,8,48]\n").unwrap())
        .to_vec());
    app
}

/// Plan over an empty screen, with group `group` selected.
fn planned(group: usize, steps: bool, skipped: &[&str]) -> App {
    let skipped = skipped.iter().map(|id| id.to_string()).collect();
    let plan = PlanView { group, steps, skipped, ..PlanView::new(plan()) };
    App { screen: Screen::Runs, runs: Vec::new(), overlay: Some(Overlay::Plan), plan: Some(plan), ..app() }
}

fn view(app: &App) -> String {
    let plan = app.plan.as_ref().map(|plan| (plan.group, plan.steps, plan.skipped.clone()));
    let selected = [app.row, app.source, app.kept, app.run, app.choice];
    format!(
        "{:?}",
        (
            app.screen,
            app.overlay,
            selected,
            app.asking,
            (&app.filter, app.filtering),
            plan,
            (app.source_view.scope, &app.source_view.filter, app.source_view.typing),
            &app.region_editor,
            &app.policy_days,
            app.notice.as_ref().map(|error| (&error.message, &error.fix))
        )
    )
}

fn opened(overlay: Overlay, source: usize, choice: usize) -> App {
    let mut app = App { screen: Screen::Sources, source, ..app() };
    app.act(Action::Open(overlay));
    App { choice, ..app }
}

/// The text of each line of the screen.
fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let lines = buffer.content.chunks(width as usize);
    lines.map(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>().trim_end().to_string()).collect()
}

#[test]
fn no_key_does_two_things_and_each_key_in_the_bar_acts() {
    let mut states = vec![app(), App::new(Vec::new(), Vec::new())];
    states.extend((0..app().live_rows().len()).map(|row| App { row, ..app() }));
    for screen in [Screen::Sources, Screen::Store, Screen::Runs] {
        states.push(App { screen, ..app() });
    }
    states.push(App { screen: Screen::Sources, source: 1, ..app() });
    for overlay in [
        Overlay::Help,
        Overlay::Attribution,
        Overlay::Source,
        Overlay::Error,
        Overlay::Policy,
        Overlay::Clean,
        Overlay::Region,
    ] {
        states.push(opened(overlay, 0, 0));
        states.push(opened(overlay, 1, 1));
    }
    states.push(App { asking: true, ..opened(Overlay::Clean, 0, 0) });
    states.push(App { store: Some(CleanPlan::default()), ..opened(Overlay::Clean, 0, 0) });
    states.push(App { screen: Screen::Store, store: Some(CleanPlan::default()), ..app() });
    states.push(App { filtering: true, ..opened(Overlay::Region, 0, 0) });
    states.push(App { filtering: true, filter: "mon".into(), ..opened(Overlay::Region, 0, 0) });
    for mode in [regions::Mode::Areas, regions::Mode::Box] {
        let mut editing = opened(Overlay::Region, 0, 0);
        editing.region_editor.open(mode);
        states.push(editing);
    }
    let mut custom = opened(Overlay::Policy, 0, 0);
    custom.act(Action::CustomPolicy);
    states.push(custom);
    for group in 0..4 {
        states.extend([planned(group, false, &[]), planned(group, true, &[]), planned(group, false, &["move:land"])]);
    }
    for app in states {
        let keys = app.bindings();
        for binding in &keys {
            assert_eq!(keys.iter().filter(|other| other.key == binding.key).count(), 1, "{:?}", binding.key);
        }
        for binding in keys.iter().filter(|binding| binding.bar.is_some()) {
            let mut after = app.clone();
            let effect = after.act(binding.action);
            assert!(effect != Effect::None || view(&after) != view(&app), "{:?} in {}", binding.key, view(&app));
        }
    }
}

#[test]
fn live_shows_each_product_the_optional_layers_and_what_needs_attention() {
    let mut app = app();
    let drawn = screen(&mut app, 80, 18);
    let live = [
        " 1 Live   3 Sources   4 Store   5 Runs",
        "",
        "LIVE from https://maps.openbikecomputer.com",
        "region  europe/germany/baden-wuerttemberg",
        "",
        "PRODUCT  RELEASE           APPLIED     SIZE      STATE",
        "maps     release 3f9a2c1e  —           980.0 MB  unknown",
        "planner  release 8b0d47a5  2026-10-02  2.37 GB   not applied",
        "",
        "OPTIONAL LAYERS",
        "[ ] climate",
        "[x] sun      not applied",
        "",
        "NEEDS ATTENTION",
        "stale        osm                   120 d > 90 d",
        "old cache    /home/rider/obc-bake  12 files, 1.2 GB",
        "unreachable  maps                  a fetch that the step list needs failed",
        "r region   R check R2   p plan   u undo environment   ? help",
    ];
    assert_eq!(drawn, live, "{drawn:#?}");
    assert_eq!(app.key(KeyCode::Char('R')), Effect::Status { check: true }, "only `R` lists R2");
    // 0 region, 1 maps, 2 planner, 3 climate, 4 sun, 5 stale, 6 old cache, 7 unreachable.
    app.row = 3;
    assert_eq!(app.key(KeyCode::Char(' ')), Effect::Layer("climate".into(), Switch::On));
    app.row = 4;
    assert_eq!(app.key(KeyCode::Char(' ')), Effect::Layer("sun".into(), Switch::Off));
    assert_eq!(app.key(KeyCode::Char('u')), Effect::Undo);
    app.row = 7;
    assert_eq!(app.key(KeyCode::Enter), Effect::None, "an unreachable product has no fix");
    (app.row, app.source) = (5, 1);
    app.key(KeyCode::Enter);
    assert_eq!((app.screen, app.source), (Screen::Sources, 0), "the stale source");
}

#[test]
fn the_region_picker_filters_and_sets_the_region_of_live() {
    let mut app = app();
    app.key(KeyCode::Char('r'));
    assert_eq!((app.overlay, app.choice), (Some(Overlay::Region), 1), "the region of live");
    assert_eq!(app.key(KeyCode::Enter), Effect::None, "live has the region already");
    app.key(KeyCode::Char('/'));
    assert_eq!(app.key(KeyCode::Char('q')), Effect::None);
    assert!(app.shown_regions().is_empty(), "`q` types: it does not quit");
    app.key(KeyCode::Backspace);
    "and".chars().for_each(|c| drop(app.key(KeyCode::Char(c))));
    assert_eq!(app.shown_regions(), ["europe/andorra"]);
    assert_eq!(app.key(KeyCode::Enter), Effect::Region("europe/andorra".into()));
    assert_eq!(app.overlay, None);

    let mut broken = App { regions: Err("data/regions/monaco.toml: no `kind`".into()), ..app };
    broken.key(KeyCode::Char('r'));
    assert_eq!(broken.region_lines(), [Line::styled("data/regions/monaco.toml: no `kind`", Color::Red)]);
    assert!(broken.bindings().iter().all(|binding| binding.key != KeyCode::Char('/')), "nothing to filter");
}

#[test]
fn plan_takes_or_leaves_only_a_move_and_always_shows_what_r2_loses() {
    let mut app = App { screen: Screen::Runs, runs: Vec::new(), ..app() };
    assert_eq!(app.key(KeyCode::Char('p')), Effect::Plan);
    app.plan = Some(PlanView::new(plan()));
    let drawn = screen(&mut app, 100, 18);
    let changes = [
        "          ┌ PLAN · live ─────────────────────────────────────────────────────────────────┐",
        "          │     CHANGE                             FETCH     TIME     OUTPUT             │",
        "          │[x]  planner +sun                                 55m 00s  30.0 MB            │",
        "          │[x]  move land 2024-01-01 → 2024-01-03  950.0 MB  41m 00s  324.0 MB           │",
        "          │[x]  move osm 2024-01-02 → 2024-01-09   710.0 MB  1h 18m   1.36 GB            │",
        "          │[x]  code of host/route-build                     44m 00s  1.10 GB            │",
    ];
    let footer = [
        "          │                                                                              │",
        "          │REMOVE FROM R2  2 keys, 1.10 GB                                               │",
        "          │⚠ blocked maps: source `wikidata` is blocked                                  │",
        "          │⚠ R2 was not listed, so leftovers are unknown                                 │",
        "          │TOTAL  fetch 1.66 GB · build 4 layers, 2h 20m · output 1.46 GB                │",
        "          └──────────────────────────────────────────────────────────────────────────────┘",
    ];
    assert_eq!(drawn[4..16], [&changes[..], &footer[..]].concat(), "{drawn:#?}");
    assert_eq!(drawn[17], "d steps   esc close", "only keys");
    assert_eq!(app.key(KeyCode::Char(' ')), Effect::None, "an edit always goes");
    app.key(KeyCode::Down);
    assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(vec!["move:osm".into()]));
    app.key(KeyCode::Down);
    assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(vec!["none".into()]), "the last move too");
    app.key(KeyCode::Up);
    assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(vec!["move:land".into()]));
    app.key(KeyCode::Down);
    assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(Vec::new()));
    for key in [KeyCode::Enter, KeyCode::Char('a')] {
        assert_eq!(app.key(key), Effect::None);
    }
    app.key(KeyCode::Char('d'));
    let drawn = screen(&mut app, 100, 22);
    let steps = [
        "          │FETCH                                                                         │",
        "          │  land  2024-01-03  950.0 MB                                                  │",
        "          │  osm   2024-01-09  710.0 MB                                                  │",
        "          │BUILD                                                                         │",
        "          │  planner/sun       55m 00s                                                   │",
        "          │  planner/basemap   41m 00s                                                   │",
        "          │  planner/routing   37m 00s                                                   │",
        "          │  planner/overlays  7m 00s                                                    │",
    ];
    assert_eq!(drawn[5..19], [&steps[..], &footer[..]].concat(), "{drawn:#?}");
}

#[test]
fn enter_in_policy_sets_another_policy_of_a_date_source() {
    let mut app = App { screen: Screen::Sources, ..app() };
    assert_eq!(app.key(KeyCode::Char('e')), Effect::None);
    assert_eq!((app.overlay, app.choice), (Some(Overlay::Policy), 0));
    app.key(KeyCode::Down);
    assert_eq!(app.key(KeyCode::Enter), Effect::Policy("osm".into(), Refresh::Days(30)));
    let mut tool = App { source: 1, ..app };
    tool.key(KeyCode::Char('e'));
    assert_eq!(tool.overlay, None, "a release version has no age");
}

#[test]
fn a_clean_needs_a_then_y() {
    let mut app = App { screen: Screen::Store, ..app() };
    assert_eq!(app.key(KeyCode::Char('c')), Effect::PlanClean, "the plan of now");
    assert_eq!(app.overlay, Some(Overlay::Clean));
    for key in [KeyCode::Enter, KeyCode::Char('y')] {
        assert_eq!(app.key(key), Effect::None);
    }
    app.key(KeyCode::Char('a'));
    assert!(app.asking);
    app.key(KeyCode::Esc);
    assert_eq!((app.overlay, app.asking), (Some(Overlay::Clean), false));
    app.key(KeyCode::Char('a'));
    assert_eq!(app.key(KeyCode::Char('y')), Effect::Clean);
    assert_eq!(app.overlay, None);
    let mut empty = App { store: Some(CleanPlan::default()), ..app };
    empty.act(Action::Open(Overlay::Clean));
    assert_eq!(empty.key(KeyCode::Char('a')), Effect::None, "an empty plan has nothing to clean");
}

#[test]
fn a_click_selects_a_row_or_shows_a_screen() {
    let mut app = App { screen: Screen::Sources, ..app() };
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut click = |app: &mut App, hit: Hit| {
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let (area, _) = *app.hits.iter().find(|(_, drawn)| *drawn == hit).unwrap();
        app.click(area.x, area.y)
    };
    click(&mut app, Hit::Row(1));
    assert_eq!((app.screen, app.overlay, app.source), (Screen::Sources, None, 1));
    click(&mut app, Hit::Screen(Screen::Store));
    assert_eq!((app.screen, app.overlay), (Screen::Store, None));
}

#[test]
fn an_area_draft_keeps_selected_paths_and_typed_fields_across_search_escape_and_completion() {
    let mut app = app();
    app.key(KeyCode::Char('r'));
    assert_eq!(app.key(KeyCode::Char('n')), Effect::Areas, "read cached suggestions only");
    let mut loaded = app.clone();
    loaded.region_editor.areas = Some(super::super::regions_cli::Suggestions {
        version: "1".into(),
        areas: ["europe/alpha", "europe/beta"]
            .map(|id| super::super::regions_cli::Suggestion {
                id: id.into(),
                name: id.into(),
                parent: None,
                countries: vec!["CH".into()],
                bounds: crate::regions::Bbox { west: 7., south: 47., east: 8., north: 48. },
            })
            .into(),
    });
    app.complete(Effect::Areas, loaded);
    for query in ["alpha", "beta"] {
        app.key(KeyCode::Delete);
        for c in query.chars() {
            assert_eq!(app.key(KeyCode::Char(c)), Effect::None);
        }
        app.key(KeyCode::Char(' '));
    }
    for field in ["qrf", "Selected areas", "Europe/Zurich"] {
        app.key(KeyCode::Tab);
        for c in field.chars() {
            app.key(KeyCode::Char(c));
        }
    }
    let Effect::CreateRegion(args) = app.key(KeyCode::F(2)) else { panic!("save must use the typed API") };
    assert_eq!(
        (args.id.as_str(), args.name.as_str(), args.time_zone.as_str()),
        ("qrf", "Selected areas", "Europe/Zurich")
    );
    assert_eq!(args.areas, ["europe/alpha", "europe/beta"]);
    assert!(args.countries.is_empty(), "the shared API derives country codes");
    app.busy = true;
    assert_eq!(app.key(KeyCode::F(2)), Effect::None, "only one edit can run");
    app.key(KeyCode::Char('x'));
    let mut completed = app.clone();
    completed.region_editor.mode = None;
    app.complete(Effect::CreateRegion(args), completed);
    assert_eq!(app.region_editor.mode, Some(regions::Mode::Areas), "a late save cannot close a changed draft");
    app.key(KeyCode::Esc);
    app.key(KeyCode::Char('n'));
    assert_eq!(app.region_editor.draft().unwrap().areas.len(), 2, "Escape never drops selected areas");
}

#[test]
fn source_scope_and_filter_keep_identity_when_background_rows_change_order() {
    let mut app = App { screen: Screen::Sources, source: 1, ..app() };
    let selected = app.sources[1].source.id.clone();
    app.key(KeyCode::Char('f'));
    app.key(KeyCode::Char('/'));
    for c in "qrf".chars() {
        assert_eq!(app.key(KeyCode::Char(c)), Effect::None);
    }
    app.key(KeyCode::Esc);
    assert_eq!(
        (app.screen, app.source_view.scope, app.source_view.filter.as_str()),
        (Screen::Sources, sources::Scope::All, "qrf")
    );
    assert!(!app.source_visible());
    assert_eq!(app.key(KeyCode::Char('e')), Effect::None, "a hidden source cannot be edited");
    let mut completed = app.clone();
    completed.sources.reverse();
    app.complete(Effect::CheckNow, completed);
    assert_eq!(app.sources[app.source].source.id, selected);
    assert_eq!(app.source_view.filter, "qrf");
    app.key(KeyCode::Char('/'));
    app.key(KeyCode::Delete);
    app.key(KeyCode::Enter);
    let drawn = screen(&mut app, 80, 24).join("\n");
    assert!(drawn.contains("STATE") && drawn.contains("POLICY") && drawn.contains("All sources"));
}

#[test]
fn custom_policy_validation_keeps_the_error_until_recovery_and_does_not_dispatch_text() {
    let mut app = App { screen: Screen::Sources, ..app() };
    app.key(KeyCode::Char('e'));
    app.key(KeyCode::Char('c'));
    app.key(KeyCode::Delete);
    app.key(KeyCode::Char('0'));
    assert_eq!(app.key(KeyCode::Enter), Effect::None);
    let error = app.notice.clone().unwrap();
    app.key(KeyCode::Tab);
    assert_eq!(app.notice.as_ref().unwrap().message, error.message);
    let completed = app.clone();
    app.complete(Effect::Areas, completed);
    assert_eq!(app.notice.as_ref().unwrap().message, error.message);
    let drawn = screen(&mut app, 80, 24).join("\n");
    assert!(drawn.contains("Error:") && drawn.contains("Fix:") && drawn.contains("1..65535"));
    app.key(KeyCode::Delete);
    for c in "14".chars() {
        app.key(KeyCode::Char(c));
    }
    assert_eq!(app.key(KeyCode::Enter), Effect::Policy("osm".into(), Refresh::Days(14)));
}

#[test]
fn box_fields_and_area_buttons_share_the_keyboard_action_without_global_shortcuts() {
    let mut app = app();
    app.key(KeyCode::Char('r'));
    assert_eq!(app.key(KeyCode::F(5)), Effect::LoadAreas, "the area list is an explicit fetch");
    app.key(KeyCode::Char('b'));
    for value in ["qrf", "Box qrf", "Europe/Zurich", "CH", "7,46,8,47"] {
        for c in value.chars() {
            assert_eq!(app.key(KeyCode::Char(c)), Effect::None);
        }
        app.key(KeyCode::Tab);
    }
    let Effect::CreateRegion(args) = app.key(KeyCode::F(2)) else { panic!("typed Box save") };
    assert_eq!(args.id, "qrf");
    assert_eq!(args.bbox.as_deref(), Some("7,46,8,47"));
    assert_eq!(args.countries, ["CH"]);
    assert!(args.areas.is_empty());
    app.key(KeyCode::Esc);
    app.key(KeyCode::Char('n'));
    app.region_editor.areas = Some(super::super::regions_cli::Suggestions {
        version: "1".into(),
        areas: vec![super::super::regions_cli::Suggestion {
            id: "europe/a-long-fully-qualified-selected-area".into(),
            name: "Area".into(),
            parent: None,
            countries: vec!["CH".into()],
            bounds: crate::regions::Bbox { west: 7., south: 46., east: 8., north: 47. },
        }],
    });
    app.key(KeyCode::Down);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let (hit, _) = *app.hits.iter().find(|(_, hit)| *hit == Hit::Region(regions::Target::Area(0))).unwrap();
    assert_eq!(app.click(hit.x, hit.y), Effect::None);
    assert_eq!(app.region_editor.draft().unwrap().areas, ["europe/a-long-fully-qualified-selected-area"]);
    app.region_editor.areas.as_mut().unwrap().areas.clear();
    app.key(KeyCode::F(3));
    let drawn = screen(&mut app, 80, 24).join("\n");
    let text = drawn.replace('│', "");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        text.contains("[x] europe/a-long-fully-qualified-selected-area · not in this index; deselect before saving"),
        "missing selections remain visible: {drawn}"
    );
    app.key(KeyCode::Char(' '));
    assert!(app.region_editor.draft().unwrap().areas.is_empty(), "a removed suggestion can be explicitly deselected");
}

#[test]
fn deletion_completion_preserves_a_newer_draft_and_reference_review_blocks_confirmation() {
    let mut app = opened(Overlay::Region, 0, 0);
    app.region_editor.open(regions::Mode::Delete);
    let mut plan = super::super::regions_cli::Deletion {
        region: "unused".into(),
        sha256: "a".repeat(64),
        used_by: vec!["environment:live".into()],
    };
    app.region_editor.deletion = Some(plan.clone());
    assert_eq!(app.key(KeyCode::Char('y')), Effect::None, "a referenced definition cannot be deleted");
    plan.used_by.clear();
    app.region_editor.deletion = Some(plan.clone());
    assert_eq!(app.key(KeyCode::Char('y')), Effect::DeleteRegion(plan.clone()));
    app.busy = true;
    app.key(KeyCode::Esc);
    app.key(KeyCode::Char('b'));
    app.key(KeyCode::Char('q'));
    let mut completed = app.clone();
    completed.region_editor.mode = None;
    app.complete(Effect::DeleteRegion(plan), completed);
    assert_eq!(app.region_editor.mode, Some(regions::Mode::Box));
    assert_eq!(app.region_editor.draft().unwrap().id, "q");
    assert_eq!(app.screen, Screen::Sources);
}

#[test]
fn a_busy_task_keeps_the_previous_plan_and_refuses_plan_keys_or_clicks_without_blocking_navigation() {
    let mut app = planned(1, false, &[]);
    app.overlay = None;
    app.screen = Screen::Live;
    app.busy = true;
    assert_eq!(app.key(KeyCode::Char('p')), Effect::None);
    assert_eq!(app.act(Action::Open(Overlay::Plan)), Effect::None, "mouse actions use the same admission");
    app.status.as_mut().unwrap().attention[2].kind = AttentionKind::Drift;
    app.row = app.live_rows().iter().position(|row| *row == LiveRow::Attention(2)).unwrap();
    assert_eq!(app.key(KeyCode::Enter), Effect::None, "a repair cannot open Plan while another task runs");
    assert_eq!(app.act(Action::Fix), Effect::None);
    assert_eq!(app.overlay, None);
    assert_eq!(app.plan.as_ref().unwrap().group, 1, "refused actions never clear the existing plan");
    app.key(KeyCode::Char('3'));
    assert_eq!(app.screen, Screen::Sources);
    app.key(KeyCode::Char('/'));
    app.key(KeyCode::Char('q'));
    assert_eq!(app.source_view.filter, "q", "navigation and input remain available");
    app.key(KeyCode::Esc);
    app.busy = false;
    assert_eq!(app.key(KeyCode::Char('p')), Effect::Plan);
    assert_eq!(app.overlay, Some(Overlay::Plan));
}

#[test]
fn region_validation_uses_working_form_actions_without_relabeling_tool_or_file_failures() {
    let missing = super::super::Code::Usage.error("no Geofabrik area `europe/removed` in the cached index");
    let areas = regions::creation_error(missing.clone(), true);
    assert_eq!((areas.code, areas.message.as_str()), (missing.code, missing.message.as_str()));
    for action in ["F2", "F5", "F3", "Space"] {
        assert!(areas.fix.contains(action));
    }
    assert!(!areas.fix.contains("obc data"));
    let box_error = regions::creation_error(missing, false);
    assert!(box_error.fix.contains("Tab") && box_error.fix.contains("F2"));
    assert!(!box_error.fix.contains("F5"), "the Box form has no area-list action");
    let index = regions::area_list_error(super::super::Code::Blocked.error("the index is not in the store"));
    assert!(index.fix.contains("F5"));
    let metadata = regions::creation_error(super::super::Code::Blocked.error("no country metadata"), true);
    assert!(metadata.fix.contains("Countries"));
    for original in [
        super::super::Code::Failed.error("cannot read the file").fix("Check filesystem permissions."),
        super::super::Code::InvalidData.error("index digest differs").fix("Restore verified input bytes."),
        super::super::Code::Usage
            .error("cannot validate IANA time zone")
            .fix("Prepare an offline Python runtime with zoneinfo data."),
    ] {
        let adapted = regions::creation_error(original.clone(), true);
        assert_eq!((adapted.code, adapted.message, adapted.fix), (original.code, original.message, original.fix));
    }
}
