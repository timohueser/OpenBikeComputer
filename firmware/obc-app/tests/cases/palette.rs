//! Lock the `screen::palette` device-64 annotations to reality.
//!
//! Each palette color is an RGB565 value chosen for how it lands on the
//! LS021B7DD02's 64-color (RGB222) gamut, with the resulting device-64 RGB written
//! in a trailing comment next to it in `screen/palette.rs`. Those comments are
//! hand-maintained, so this test quantizes every color through the real
//! `rgb565_to_device64` and asserts the documented value — retune a color without
//! updating its comment (or vice versa) and this fails.

use obc_app::screen::palette::*;
use obc_reader::rgb565_to_device64;

#[test]
fn palette_quantizes_to_documented_device64() {
    // (name, RGB565 constant, documented device-64 RGB) — kept in lock-step with the
    // trailing comments in `screen/palette.rs`.
    let cases: &[(&str, u16, (u8, u8, u8))] = &[
        ("PARCHMENT", PARCHMENT, (255, 255, 255)),
        ("PARCHMENT_SHADE", PARCHMENT_SHADE, (170, 170, 85)),
        ("RECESSED_BASE", RECESSED_BASE, (170, 170, 170)),
        ("HUD", HUD, (0, 0, 0)),
        ("WOOD", WOOD, (170, 85, 0)),
        ("WOOD_LIGHT", WOOD_LIGHT, (170, 170, 85)),
        ("INK", INK, (0, 0, 0)),
        ("BAR_TEXT", BAR_TEXT, (255, 255, 255)),
        ("ON_ACCENT", ON_ACCENT, (0, 0, 0)),
        ("SUBTEXT_ON_ACCENT", SUBTEXT_ON_ACCENT, (85, 85, 0)),
        ("GUARD_BASE", GUARD_BASE, (170, 170, 85)),
        ("ON_GUARD", ON_GUARD, (0, 0, 0)),
        ("SUBTEXT_ON_GUARD", SUBTEXT_ON_GUARD, (0, 0, 0)),
        ("SUBTEXT", SUBTEXT, (85, 85, 0)),
        ("RULE", RULE, (170, 170, 85)),
        ("AMBER", AMBER, (255, 170, 0)),
        ("WARNING", WARNING, (255, 85, 0)),
        ("ON", ON, (0, 170, 0)),
        ("YELLOW", YELLOW, (255, 255, 0)),
        ("RED", RED, (255, 0, 0)),
        ("CLIMB_TILE", CLIMB_TILE, (255, 170, 85)),
        ("ROUTE", ROUTE, (255, 0, 255)),
        ("BREADCRUMB", BREADCRUMB, (0, 0, 170)),
        ("ART_WHITE", ART_WHITE, (255, 255, 255)),
        ("TRACK_START", TRACK_START, (0, 85, 0)),
        ("TRACK_END", TRACK_END, (170, 0, 0)),
        ("TRAIL", TRAIL, (170, 0, 0)),
        ("CONTOUR", CONTOUR, (85, 85, 85)),
    ];
    for &(name, c, want) in cases {
        assert_eq!(rgb565_to_device64(c), want, "{name}: device-64 result drifted from screen/palette.rs");
    }
}
