//! Semantic UI colours and their device theme mapping.
//!
//! Screen code selects a role from this module. The frame policy resolves that role once per
//! primitive. Authored map and photo colours bypass this mapping.

use crate::settings::Theme;

/// Pack 8-bit RGB into RGB565.
pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
}

// Device-64 has no warm off-white. The trailing comments name the quantized panel colour and are
// checked by the palette integration test.
pub const PARCHMENT: u16 = rgb565(245, 243, 238); // → (255,255,255) white
pub const PARCHMENT_SHADE: u16 = rgb565(180, 170, 105); // → (170,170,85) tan
pub const HUD: u16 = rgb565(46, 37, 26); // → (0,0,0) near-black frame
pub const WOOD: u16 = rgb565(150, 100, 40); // → (170,85,0) wood brown
pub const WOOD_LIGHT: u16 = rgb565(180, 168, 100); // → (170,170,85) tan
pub const INK: u16 = rgb565(44, 33, 20); // → (0,0,0) text black
pub const BAR_TEXT: u16 = rgb565(255, 255, 255); // → (255,255,255) white
pub const ON_ACCENT: u16 = rgb565(0, 0, 0); // → (0,0,0) black
pub const SUBTEXT_ON_ACCENT: u16 = rgb565(110, 95, 58); // → (85,85,0) olive
pub const GUARD_BASE: u16 = rgb565(186, 174, 112); // → (170,170,85) tan
pub const ON_GUARD: u16 = rgb565(8, 4, 1); // → (0,0,0) black
pub const SUBTEXT_ON_GUARD: u16 = rgb565(16, 8, 2); // → (0,0,0) black
pub const SUBTEXT: u16 = rgb565(110, 90, 58); // → (85,85,0) olive
pub const RULE: u16 = rgb565(180, 170, 100); // → (170,170,85) tan
pub const AMBER: u16 = rgb565(227, 165, 43); // → (255,170,0) accent
pub const WARNING: u16 = rgb565(192, 73, 46); // → (255,85,0) warning
pub const CONTOUR: u16 = rgb565(96, 96, 96); // → (85,85,85) grey
pub const ON: u16 = rgb565(0, 170, 0); // → (0,170,0) green
pub const YELLOW: u16 = rgb565(255, 255, 0); // → (255,255,0) yellow
pub const RED: u16 = rgb565(255, 0, 0); // → (255,0,0) red
pub const CLIMB_TILE: u16 = rgb565(255, 170, 85); // → (255,170,85) apricot
pub const ROUTE: u16 = rgb565(255, 0, 255); // → (255,0,255) magenta
pub const DETOUR: u16 = rgb565(0, 90, 255); // → (0,85,255) blue
pub const BREADCRUMB: u16 = rgb565(0, 0, 170); // → (0,0,170) navy
/// Stable white for authored symbols such as national flags.
pub const ART_WHITE: u16 = rgb565(250, 250, 250); // → (255,255,255) white

/// Dark counterparts for roles that change. Accent, warning, map-overlay, and grade colours stay
/// fixed. The common Light path is one branch and adds no allocation or state.
#[inline]
pub(crate) fn resolve(theme: Theme, color: u16) -> u16 {
    if theme == Theme::Light {
        return color;
    }
    match color {
        PARCHMENT => rgb565(0, 0, 0),
        PARCHMENT_SHADE | WOOD_LIGHT => rgb565(85, 85, 0),
        INK | BAR_TEXT => rgb565(255, 255, 255),
        SUBTEXT => rgb565(170, 170, 85),
        CONTOUR => rgb565(170, 170, 170),
        CLIMB_TILE => rgb565(85, 0, 0),
        color if color == rgb565(170, 170, 170) => rgb565(85, 85, 85),
        color if color == rgb565(85, 85, 85) => rgb565(170, 170, 170),
        _ => color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_is_identity_and_dark_keeps_fixed_accents() {
        for color in [PARCHMENT, INK, SUBTEXT, WOOD, AMBER, WARNING, ROUTE, DETOUR, BREADCRUMB] {
            assert_eq!(resolve(Theme::Light, color), color);
        }
        for color in [WOOD, AMBER, WARNING, ROUTE, DETOUR, BREADCRUMB, ON_ACCENT, SUBTEXT_ON_ACCENT] {
            assert_eq!(resolve(Theme::Dark, color), color);
        }
        assert_eq!(resolve(Theme::Dark, PARCHMENT), rgb565(0, 0, 0));
        assert_eq!(resolve(Theme::Dark, INK), rgb565(255, 255, 255));
    }
}
