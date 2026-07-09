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
use obc_app::settings::Language;
use obc_app::{App, AppState, Button, ButtonEvent, InputClock, InputEvent, RideClock, Sensors, Settings};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};

mod common;
use common::{build_min_obcm, keys, Buf, NoFix};

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
            InputEvent::Button(ButtonEvent::Down(Button::Encoder)),
            InputEvent::Button(ButtonEvent::Up(Button::Encoder)),
        ]);
        app.handle_input(InputClock(0), &mut press);

        let buf = render(&mut app, &bytes);
        assert!(buf.px.iter().any(|&p| p != Rgb888::BLACK), "menu rendered blank in {lang:?}",);
    }
}

/// Tick once (no fix) and composite one frame into a `120×120` recording buffer.
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    app.tick(
        RideClock(0),
        Sensors {
            loc: &mut NoFix,
            altimeter: None,
            temperature: None,
            clock: None,
            compass: None,
            track: None,
            fuel: None,
        },
        None,
    );
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid obcm");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(120, 120);
    app.render_frame(&mut buf, &reader, None, 120.0, 120.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}
