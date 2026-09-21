//! i18n catalog guards. The one failure mode a rendered PNG cannot assert cheaply is a translation
//! carrying a char outside the device font's repertoire, which the text path renders as a silent
//! `?`. [`every_string_is_renderable`] walks the whole catalog ([`obc_app::i18n::TABLE`], every
//! `Msg` × every `Language`) plus the endonyms that live outside it, and checks each char against
//! [`obc_render::glyph_supported`]. [`render_smoke`] drives each language to the text-heavy Menu
//! and asserts the frame draws.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_app::i18n::{t, Msg};
use obc_app::settings::Language;
use obc_app::{App, AppState, Gesture, Screen, Settings};
use obc_ports::{Button, ButtonEvent, InputClock, InputEvent};

use crate::common::{build_min_obcm, build_min_obcm_profiles, keys, render_120};

/// The four shipped languages, in `Language` discriminant order — the column order of
/// [`obc_app::i18n::TABLE`].
const LANGS: [Language; 4] = [Language::En, Language::De, Language::Fr, Language::Es];

#[test]
fn every_string_is_renderable() {
    let mut offenders: Vec<String> = Vec::new();

    // The generated catalog: every `Msg` row × every `Language` column.
    for (row, cols) in obc_app::i18n::TABLE.iter().enumerate() {
        for (col, s) in cols.iter().enumerate() {
            check_str(&mut offenders, s, &format!("TABLE[msg {row}][{:?}]", LANGS[col]));
        }
    }

    // Endonyms are hardcoded in `Language::name` (a language must name *itself* even before its
    // column exists), so they sit outside TABLE — check them explicitly.
    for lang in LANGS {
        check_str(&mut offenders, lang.name(), &format!("Language::{lang:?}.name()"));
    }

    assert!(
        offenders.is_empty(),
        "i18n: {} string(s) contain a char outside the device font repertoire (Latin-1 + Latin \
         Extended-A, per obc-render #489/#601) — they would render as a silent `?` on-glass:\n{}",
        offenders.len(),
        offenders.join("\n"),
    );
}

/// Append a located complaint for every unrenderable char in `s`.
fn check_str(offenders: &mut Vec<String>, s: &str, where_: &str) {
    for c in s.chars() {
        // `\n` is layout glue (a couple of two-line strings), not a printed glyph; the text path
        // treats it as a line break, so it needs no font slot.
        if c == '\n' {
            continue;
        }
        if !obc_render::glyph_supported(c) {
            offenders.push(format!("  {where_}: char U+{:04X} {c:?} in {s:?} is not covered", c as u32));
        }
    }
}

#[test]
fn guard_rejects_out_of_repertoire_chars() {
    assert!(!obc_render::glyph_supported('\u{2019}'), "curly ' must be rejected");
    assert!(!obc_render::glyph_supported('\u{2014}'), "em-dash must be rejected");
    assert!(obc_render::glyph_supported('\''), "ASCII ' is covered");
    assert!(obc_render::glyph_supported('ß'), "German ß is covered (Latin-1)");
    assert!(obc_render::glyph_supported('œ'), "French œ is covered (Latin Extended-A)");
    assert!(obc_render::glyph_supported('?'), "'?' itself is a real glyph");
}

/// Catalog values that flow into a fixed `heapless` buffer whose `push_str`/`write!` result is
/// discarded. A `heapless` overflow is an atomic no-op, so an over-length caption renders fully
/// blank on-glass — a failure the repertoire test above cannot see. This bounds each such key's byte
/// length across all four languages, at the budget its call site leaves after the glued unit or
/// number.
#[test]
fn fixed_buffer_captions_fit() {
    // (key, byte budget, where the buffer lives). Budget = buffer capacity − the largest thing
    // concatenated alongside the translation at the call site.
    let bounds: &[(Msg, usize, &str)] = &[
        // Climb tiles: `ClimbCell`'s `String<12>` caption (climb.rs). The three direct captions get
        // the whole buffer; `ClimbToGo` is prefixed with the 2-char unit label ("KM"/"MI") by
        // `cap_dist`, so it clears 10. This is the tightest screen — de's "Ø STEIG." is 9/12.
        (Msg::ClimbToClimb, 12, "climb.rs ClimbCell caption (String<12>)"),
        (Msg::ClimbGrade, 12, "climb.rs ClimbCell caption (String<12>)"),
        (Msg::ClimbAvgGrad, 12, "climb.rs ClimbCell caption (String<12>)"),
        (Msg::ClimbToGo, 10, "climb.rs cap_dist: 2-char unit label + this → String<12>"),
        // Map off-route pill: `write_distance_coarse` fills a `String<20>` with this prefix plus a
        // distance suffix up to ~7 bytes ("9999km" / "5279ft"), so the prefix must clear ≤ 13 (map.rs).
        (Msg::MapOffRoute, 13, "map.rs off-route pill (String<20>, ≤7-byte distance follows)"),
        // The quick drawer's brightness title: `draw_brightness` writes this plus " 100%" into a
        // `String<24>` and discards the result, so a caption past the budget silently loses its
        // trailing fragments. The current worst is de "HELLIGKEIT" at 10.
        (Msg::QuickBrightness, 19, "quick_drawer.rs draw_brightness title (String<24>, ' 100%' follows)"),
    ];

    let mut offenders: Vec<String> = Vec::new();
    for &(msg, budget, where_) in bounds {
        for lang in LANGS {
            let s = t(msg, lang);
            if s.len() > budget {
                offenders.push(format!("  {where_}: {lang:?} {s:?} is {} bytes > {budget}-byte budget", s.len()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "i18n: {} caption(s) would overflow a fixed heapless buffer — heapless drops the overflow, so \
         the caption renders fully blank on-glass:\n{}",
        offenders.len(),
        offenders.join("\n"),
    );
}

/// Stateless render→assert per language: seed `Settings.language`, open the Menu, whose title bar
/// is translated, render, and assert the frame drew. Guards the draw path, not just the data.
#[test]
fn render_smoke() {
    let bytes = build_min_obcm(0xF800);
    for lang in LANGS {
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        app.set_settings(Settings { language: lang, ..Default::default() });

        // Home (idle) → press opens the compass Menu, whose title bar carries translated copy.
        let mut press = keys(&[
            InputEvent::Button(ButtonEvent::Down(Button::Select)),
            InputEvent::Button(ButtonEvent::Up(Button::Select)),
        ]);
        app.handle_input(InputClock(0), &mut press);

        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "menu rendered blank in {lang:?}",);
    }
}

/// The Up-ahead timeline's own copy is translated, not four English placeholders, and every one of
/// its states renders through each catalog column: the route-less empty state, the merged list's
/// title, and the timeline's own context sheet with its nested filter editor.
#[test]
fn up_ahead_copy_is_localized_and_every_state_renders() {
    // The context row label the timeline reuses as its title, and the empty-state sentence under it.
    assert_eq!(t(Msg::RideContextUpAhead, Language::En), "Up ahead");
    assert_eq!(t(Msg::RideContextUpAhead, Language::De), "Voraus");
    assert_eq!(t(Msg::RideContextUpAhead, Language::Fr), "\u{c0} venir");
    assert_eq!(t(Msg::RideContextUpAhead, Language::Es), "Pr\u{f3}ximo");
    assert_eq!(t(Msg::UpAheadNone, Language::En), "Nothing up ahead");
    assert_eq!(t(Msg::UpAheadNone, Language::De), "Nichts voraus");
    assert_eq!(t(Msg::UpAheadEverything, Language::Fr), "Tout");
    assert_eq!(t(Msg::UpAheadEverything, Language::Es), "Todo");

    // Every key the screen can draw must differ per language — a forgotten translation that
    // silently ships the English string is exactly what this net is for.
    for (key, msg) in [
        ("up_ahead.none", Msg::UpAheadNone),
        ("up_ahead.none_sub", Msg::UpAheadNoneSub),
        ("up_ahead.none_category_sub", Msg::UpAheadNoneCategorySub),
        ("up_ahead.no_route_sub", Msg::UpAheadNoRouteSub),
        // The two filter row labels are deliberately absent: "Filter" is the German word and
        // "Sources" the French one, so both would fail this net for being right. They are covered
        // instead by the render sweep below and by `context_drawer`'s width test.
        ("poi_detail.side_left", Msg::PoiDetailSideLeft),
        ("poi_detail.side_right", Msg::PoiDetailSideRight),
    ] {
        for lang in [Language::De, Language::Fr, Language::Es] {
            assert_ne!(t(msg, lang), t(msg, Language::En), "`{key}` is still English in {lang:?}");
        }
    }

    let bytes = build_min_obcm(0xF800);
    for lang in LANGS {
        let mut app = App::new(AppState::new(0, 0, 0.05));
        app.set_settings(Settings { language: lang, ..Default::default() });
        assert!(app.apply_chord(obc_app::Chord::Context)); // Map -> the ride context sheet
        app.apply_gesture(Gesture::Press); // Assistant
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::WhatsNext(_)));
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "route-less Up-ahead state rendered blank in {lang:?}");

        app.apply_gesture(Gesture::Press); // Explore ahead owns the filter drawer.
        assert!(app.apply_chord(obc_app::Chord::Context));
        assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "the Up-ahead context sheet rendered blank in {lang:?}");

        app.apply_gesture(Gesture::Press); // -> the Filter editor
        app.advance_animations(InputClock(400)); // let the page slide land
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "the filter editor rendered blank in {lang:?}");
    }
}

/// The map sheet's own copy is translated in all four columns, and both of its states render: the
/// five-row map table over the riding Map, and the three-switch display sheet its last row swaps in.
#[test]
fn the_map_sheet_is_localized_and_every_state_renders() {
    for (key, msg) in [
        ("map_context.map_display", Msg::MapContextMapDisplay),
        ("map_context.clock", Msg::MapContextClock),
        ("map_context.scale_bar", Msg::MapContextScaleBar),
        ("map_context.contours", Msg::MapContextContours),
    ] {
        for lang in [Language::De, Language::Fr, Language::Es] {
            assert_ne!(t(msg, lang), t(msg, Language::En), "`{key}` is still English in {lang:?}");
        }
    }

    let bytes = build_min_obcm(0xF800);
    for lang in LANGS {
        let mut app = App::new(AppState::new(0, 0, 0.05));
        app.set_settings(Settings { language: lang, ..Default::default() });

        assert!(app.apply_chord(obc_app::Chord::Context), "the Map declares a context");
        assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "the map sheet rendered blank in {lang:?}");

        app.apply_gesture(Gesture::Step(-1)); // → the Map display row
        app.apply_gesture(Gesture::Press); // → the display sheet, landed at once
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "the map display sheet rendered blank in {lang:?}");
    }
}

/// The route-plan sheet's one row label is translated in all four columns, and both of its states
/// render: the one-row table over the create-route confirm card, and the nested bike-type editor.
///
/// The editor's choices are deliberately not in the copy net: they are the loaded map's profile
/// names, byte-identical in every column, and their width is pinned by `context_drawer`'s width
/// test. Two fixtures, because the walk needs two unrelated things a map carries: POIs to browse to
/// a confirm card, and a profile table with more than one entry, without which the row is inert.
#[test]
fn the_route_plan_sheet_is_localized_and_every_state_renders() {
    use obc_ports::Fix;
    use obcm_testkit::{build_poi_map, PoiSpec};

    // "Type de vélo" / "Tipo de bici" are 12 monospace characters, 168 px in `Font::Body`, which
    // the centred row's 172 px label budget holds (`context_drawer`'s width test pins both numbers).
    assert_eq!(t(Msg::RouteContextBikeType, Language::Fr), "Type de v\u{e9}lo");
    assert_eq!(t(Msg::RouteContextBikeType, Language::Es), "Tipo de bici");
    for lang in [Language::De, Language::Fr, Language::Es] {
        assert_ne!(
            t(Msg::RouteContextBikeType, lang),
            t(Msg::RouteContextBikeType, Language::En),
            "`route_context.bike_type` is still English in {lang:?}"
        );
    }

    const BBOX: (i32, i32, i32, i32) = (7_000_000, 43_000_000, 8_000_000, 44_000_000);
    const POS: (i32, i32) = (7_500_000, 43_500_000);
    let water = vec![PoiSpec { lat: 43_500_500, lon: 7_500_000, subtype: 1, name: "Fontaine".into(), payload: 0xFFFF }];
    let bytes = build_poi_map(BBOX, 512, &[(1, water)]);

    // The profile names a host mirrors on map load, the set both snapshot fixtures carry.
    let profile_map = build_min_obcm_profiles(0, &["Road", "Gravel", "MTB", "Touring"]);
    let src = obc_reader::SliceSource(&profile_map);
    let tables = obc_reader::MapTables::parse(&src).expect("valid fixture");

    for lang in LANGS {
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        app.set_settings(Settings { language: lang, ..Default::default() });
        app.set_nav_profiles(tables.nav_profiles());
        app.state.user_fix = Some(Fix::at(POS.1, POS.0));

        // Assistant → Find → Water → shared place detail and its route-profile context.
        assert!(app.apply_chord(obc_app::Chord::Assistant));
        app.apply_gesture(Gesture::Press); // Find a place
        app.apply_gesture(Gesture::Press); // Water
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press); // More places
        render_120(&mut app, &bytes); // the lazy POI snapshot fills on a render
        app.apply_gesture(Gesture::Press); // → the detail
        render_120(&mut app, &bytes); // resolve current opening hours before enabling the action
        assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "the shared place detail owns the visit profile");

        assert!(app.apply_chord(obc_app::Chord::Context), "the place detail declares a route-profile context");
        assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
        let root = render_120(&mut app, &bytes);
        assert!(root.px.iter().any(|&p| p != Rgb888::BLACK), "the route-plan sheet rendered blank in {lang:?}");

        app.apply_gesture(Gesture::Press); // → the bike-type editor
        app.advance_animations(InputClock(400)); // let the page slide land
        let editor = render_120(&mut app, &bytes);
        assert!(editor.px.iter().any(|&p| p != Rgb888::BLACK), "the bike-type editor rendered blank in {lang:?}");
        assert!(editor.px != root.px, "the press must land on the editor, not re-render the sheet root, in {lang:?}");
    }
}
