//! i18n catalog guards (epic #602, L5).
//!
//! `ui-snapshots.sh` renders every screen in de/fr/es to eyeball translations, but it is a manual
//! dev tool — not wired into CI. This binary is the CI-green net for the one failure mode a PNG
//! can't assert cheaply: a translation carrying a char **outside the device font's repertoire**,
//! which the text path renders as a silent `?` (no panic, no error — see `obc-render::text`).
//!
//! The [`every_string_is_renderable`] guard walks the *whole* catalog
//! ([`obc_app::i18n::TABLE`] — every `Msg` × every `Language`) plus the handful of endonyms that
//! live outside the table, and asserts each `char` maps to a real glyph via
//! [`obc_render::glyph_supported`] — i.e. against the **actual** Latin-1 + Latin Extended-A strip
//! shipped by #489/#601, not a hand-copied range. Any future translation that reaches for a curly
//! quote, em-dash, ellipsis, or a non-Latin letter fails the build here, before it can ship as a
//! `?` on-glass.
//!
//! [`render_smoke`] adds a couple of stateless render→assert cases: construct `Settings { language,
//! .. }`, drive to the text-heavy Menu, and assert the frame draws without panicking.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_app::i18n::{t, Msg};
use obc_app::settings::Language;
use obc_app::{App, AppState, Gesture, Screen, Settings};
use obc_ports::{Button, ButtonEvent, InputClock, InputEvent};

mod common;
use common::{build_min_obcm, keys, render_120};

/// The four shipped languages, in `Language` discriminant order — the column order of
/// [`obc_app::i18n::TABLE`].
const LANGS: [Language; 4] = [Language::En, Language::De, Language::Fr, Language::Es];

/// Every string the UI can show must be renderable by the device font — no char that falls back to
/// the silent `?` slot. Walks the full catalog plus the out-of-table endonyms.
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

/// A tiny sanity check that the repertoire assertion is real: a known bad char (a curly
/// apostrophe, which #601's Latin strip deliberately omits) is *not* renderable, while the ASCII
/// apostrophe the catalog is authored with is.
#[test]
fn guard_rejects_out_of_repertoire_chars() {
    assert!(!obc_render::glyph_supported('\u{2019}'), "curly ' must be rejected");
    assert!(!obc_render::glyph_supported('\u{2014}'), "em-dash must be rejected");
    assert!(obc_render::glyph_supported('\''), "ASCII ' is covered");
    assert!(obc_render::glyph_supported('ß'), "German ß is covered (Latin-1)");
    assert!(obc_render::glyph_supported('œ'), "French œ is covered (Latin Extended-A)");
    assert!(obc_render::glyph_supported('?'), "'?' itself is a real glyph");
}

/// Catalog values that flow into a **fixed `heapless` buffer** whose `push_str`/`write!` result is
/// discarded. A `heapless` overflow is an atomic no-op, so an over-length caption renders *fully
/// blank* on-glass — a failure the repertoire test above can't see (every glyph is fine; there are
/// just too many). This bounds each such key's byte length across all four languages, at the budget
/// its call site leaves after the glued unit/number. No current value overflows; the guard is for
/// the future accented edit that would (#614).
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
        // Map off-route pill: `write_off_route`'s `String<20>` = this prefix + a distance suffix up
        // to ~7 bytes ("9999km" / "5279ft"), so the prefix must clear ≤ 13 (map.rs).
        (Msg::MapOffRoute, 13, "map.rs off-route pill (String<20>, ≤7-byte distance follows)"),
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

/// Stateless render→assert per language: seed `Settings.language`, open the Menu (its title bar is
/// translated — `MENU` / `MENÜ` / …), render, and assert the frame drew without panicking. Guards
/// the draw path against a translated string blowing up, not just the catalog data.
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

/// The Up-ahead timeline's own copy (epic #946, U3) is translated — not four English placeholders
/// — and every one of its states renders through each catalog column without a font/buffer failure:
/// the route-less empty state, the merged list's title, and the timeline's own context sheet with
/// its nested filter editor (#1515 D4a).
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
        // The two D4a row labels are **deliberately absent**: "Filter" is the German word and
        // "Sources" the French one, so both would fail this net for being right. They are covered
        // instead by the four-language render sweep below and by `context_drawer`'s width test,
        // which reads every label and every choice out of all four columns.
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
        app.apply_gesture(Gesture::Press); // its first row -> Up ahead
        assert!(matches!(app.top_screen(), Screen::UpAhead(_)));
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "route-less Up-ahead state rendered blank in {lang:?}");

        // The timeline declares its own context (#1515 D4a): the sheet's two value rows, then the
        // nested filter editor a press opens.
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

/// The **weather sheet's** own copy (#1515 D4b) is translated in all four columns, and both of its
/// states render through each of them: the two-row table over the dashboard, and the nested Interval
/// editor whose five choices are the `[weather_refresh]` names the deleted settings screen used to
/// draw.
#[test]
fn the_weather_sheet_is_localized_and_every_state_renders() {
    // Both row labels differ from English everywhere — unlike D4a's *Filter* / *Sources*, which are
    // words their own languages already spell that way, these join the net rather than excepting
    // themselves from it.
    for (key, msg) in [
        ("weather_context.refresh_now", Msg::WeatherContextRefreshNow),
        ("weather_context.interval", Msg::WeatherContextInterval),
    ] {
        for lang in [Language::De, Language::Fr, Language::Es] {
            assert_ne!(t(msg, lang), t(msg, Language::En), "`{key}` is still English in {lang:?}");
        }
    }

    let bytes = build_min_obcm(0xF800);
    for lang in LANGS {
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        app.set_settings(Settings { language: lang, ..Default::default() });
        // Home → Menu → the Weather station → the dashboard.
        app.apply_gesture(Gesture::Press);
        for _ in 0..4 {
            app.apply_gesture(Gesture::Step(1));
        }
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::Weather(_)));

        assert!(app.apply_chord(obc_app::Chord::Context), "the dashboard declares a context");
        assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "the weather sheet rendered blank in {lang:?}");

        app.apply_gesture(Gesture::Step(1)); // → the Interval row
        app.apply_gesture(Gesture::Press); // → its editor
        app.advance_animations(InputClock(400)); // let the page slide land
        let buf = render_120(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "the interval editor rendered blank in {lang:?}");
    }
}
