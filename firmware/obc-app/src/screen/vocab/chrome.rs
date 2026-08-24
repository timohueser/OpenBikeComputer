//! The screen **chrome** — the framed page header every screen draws through, the card glyphs, the
//! Recalculating banner, and the small shared text/stroke helpers the bodies below it are assembled
//! from.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Canvas, Surface,
};

use crate::screen::palette;

/// Height of the wood title bar. Sized for the Body-tier title with even ≈8 px padding.
pub(crate) const TITLE_BAR_H: i32 = 34;

/// Top of the list area (just below the title bar) shared by list screens.
pub(crate) const LIST_TOP: i32 = TITLE_BAR_H + 8;

/// Draw the shared screen chrome: a near-white background, a thin rounded outline, and a rounded
/// wood title bar with `title` left-aligned and `right` (a counter, a grade readout, …) right-
/// justified. `title` is left-aligned so a long right-hand readout never collides with it. Every
/// framed screen draws its header through this; the caller fills the body below [`LIST_TOP`].
///
/// This is the plain header; framed screens that want the BLE connected indicator in the right slot
/// (the menus) call [`title_frame_ble`] instead, threading the app's link state.
pub(crate) fn title_frame(cv: &mut impl Surface, w: i32, h: i32, title: &str, right: &str) {
    title_frame_ble(cv, w, h, title, right, false)
}

/// [`title_frame`] plus the BLE **connected indicator** (epic #447): when `ble_connected`, a small
/// static Bluetooth rune sits in the title bar's right slot, on the parchment glyph colour of the
/// bar text. The `right` readout is inset left of it so the two never overlap (in practice the
/// menus that show the indicator carry no right readout). Static — no animation — so it stays
/// dirty-row-cheap: it only appears/disappears on a link change, which
/// [`App::set_ble_status`](crate::App::set_ble_status) gates a repaint on.
pub(crate) fn title_frame_ble(cv: &mut impl Surface, w: i32, h: i32, title: &str, right: &str, ble_connected: bool) {
    use palette::*;
    cv.clear(PARCHMENT);
    cv.round_outline(rect(4, 4, w - 8, h - 8), 8, WOOD_LIGHT);
    cv.round(rect(4, 4, w - 8, TITLE_BAR_H), 6, WOOD);
    // Both rows vertically centered in the bar; the two y's account for the different glyph baselines.
    cv.text(title, Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
    // The rune occupies the far-right slot; any `right` readout is pushed left of it so they can't
    // collide. `BLE_GLYPH_W` + a small gap is the reserved band.
    let right_x = if ble_connected {
        ble_glyph(cv, w - 14 - BLE_GLYPH_W, TITLE_BAR_H / 2 + 4, PARCHMENT);
        w - 14 - BLE_GLYPH_W - 8
    } else {
        w - 14
    };
    cv.text(right, Point::new(right_x, 10), Font::Label, TextAlign::Right, PARCHMENT);
}

/// Total width (px) the [`ble_glyph`] rune occupies, so callers can reserve its slot.
pub(crate) const BLE_GLYPH_W: i32 = 11;

/// Draw the Bluetooth "connected" rune centred vertically on `cy`, its left edge at `x`, in `color`.
///
/// The classic Bluetooth bind-rune (ᛒ): a vertical stem; from the stem's top a stroke runs to the
/// upper-right tip and back to the centre notch, mirrored from the bottom; and two crossing
/// back-strokes run from each tip to the opposite left corner — the diagonals that close the rune.
/// Hand-plotted as `line`s in the panel's own glyph idiom (like the climb triangles and POI bearing
/// arrows) rather than a font glyph, so it quantizes and reads at the device's pixel scale. Static
/// and tiny (~11×16), so painting it is cheap and it composites into a single dirty row-band.
pub(crate) fn ble_glyph(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    let half = 8; // half-height → a 16 px-tall stem
    let (top, mid, bot) = (cy - half, cy, cy + half);
    let stem_x = x + 3; // the vertical bar, inset so the left back-strokes have room on either side
    let tip_x = x + BLE_GLYPH_W - 1; // the rightmost point of each triangle
    let left_x = x; // the two left corners the diagonals reach
    let quarter = half / 2;
    let (t, b, c) = (Point::new(stem_x, top), Point::new(stem_x, bot), Point::new(stem_x, mid));
    let up_tip = Point::new(tip_x, top + quarter);
    let lo_tip = Point::new(tip_x, bot - quarter);
    // The vertical stem.
    cv.line(t, b, color);
    // Right-hand strokes: top → upper-tip → centre, and bottom → lower-tip → centre.
    cv.line(t, up_tip, color);
    cv.line(up_tip, c, color);
    cv.line(b, lo_tip, color);
    cv.line(lo_tip, c, color);
    // The crossing diagonals to the opposite left corner — what makes it read as the ᛒ rune, not two
    // stacked chevrons. Upper tip → lower-left, lower tip → upper-left.
    cv.line(up_tip, Point::new(left_x, bot - quarter), color);
    cv.line(lo_tip, Point::new(left_x, top + quarter), color);
}

/// Draw the shared card **warning glyph** — an amber triangle with an ink exclamation — centred at
/// `center`, `k` the triangle's half-height (epic #678 T1's dialog anatomy kit). Drawn in the
/// "glyph slot": horizontally centred, vertically in the band between the title bar and the card's
/// text block. Pixel-for-pixel the glyph the DFU error cards established (the reference
/// composition); the factory-Reset screen and the routing-failure / sensor-warning cards draw the
/// identical sign through this one helper.
pub(crate) fn card_triangle(cv: &mut impl Surface, center: Point, k: i32) {
    use palette::*;
    let (cx, cy) = (center.x, center.y);
    cv.triangle(Point::new(cx, cy - k), Point::new(cx - k, cy + k), Point::new(cx + k, cy + k), AMBER);
    // Exclamation: a bar over a dot.
    cv.vline(cx, cy - k / 4, k / 2, 3, INK);
    cv.disc(Point::new(cx, cy + k / 2 + 1), 2, INK);
}

/// Draw the shared card **check glyph** — an amber check mark, two strokes stepped out of discs
/// (the canvas has no diagonal thick-line primitive) — centred near `center`, `k` its half-width.
/// The success twin of [`card_triangle`], factored from the DFU "UPDATED" toast (the reference)
/// and the Reset done state; the "ROUTE UPDATED" card draws the same mark.
pub(crate) fn card_check(cv: &mut impl Surface, center: Point, k: i32) {
    fn seg(cv: &mut impl Surface, a: (i32, i32), b: (i32, i32)) {
        const N: i32 = 14;
        for s in 0..=N {
            let x = a.0 + (b.0 - a.0) * s / N;
            let y = a.1 + (b.1 - a.1) * s / N;
            cv.disc(Point::new(x, y), 3, palette::AMBER);
        }
    }
    let (cx, cy) = (center.x, center.y);
    // Down-stroke to the low point, then up-stroke to the top-right.
    seg(cv, (cx - k, cy), (cx - k / 3, cy + k * 2 / 3));
    seg(cv, (cx - k / 3, cy + k * 2 / 3), (cx + k, cy - k * 2 / 3));
}

/// Draw `text` word-wrapped into centred `font` lines within `width_px`, the first line at
/// `top_y`, in `color` — the shared multi-line card body (author each catalog string on one line;
/// wrap at draw time). Greedy over the monospace cell width; returns the `y` just past the last
/// line so a caller can stack more below it. A single word wider than the budget is left to clip
/// (versions and the like are short). The line advance is the font's cap height plus a hair of
/// lead. Shared by the DFU cards (which established it) and the routing-failure card.
pub(crate) fn wrapped(
    cv: &mut impl Surface,
    text: &str,
    cx: i32,
    top_y: i32,
    width_px: i32,
    font: Font,
    color: u16,
) -> i32 {
    let lh = font.cap_height() as i32 + 1; // cap + a hair of lead (Label: the 19 px the DFU cards pinned)
    let char_w = font.char_width() as i32;
    let budget = (width_px / char_w).max(1) as usize;
    let mut y = top_y;
    let mut line: heapless::String<48> = heapless::String::new();
    for word in text.split(' ') {
        let extra = if line.is_empty() { word.len() } else { line.len() + 1 + word.len() };
        if extra > budget && !line.is_empty() {
            cv.text(&line, Point::new(cx, y), font, TextAlign::Center, color);
            y += lh;
            line.clear();
        }
        if !line.is_empty() {
            let _ = line.push(' ');
        }
        let _ = line.push_str(word);
    }
    if !line.is_empty() {
        cv.text(&line, Point::new(cx, y), font, TextAlign::Center, color);
        y += lh;
    }
    y
}

/// Doubled-1-px stroke: the segment plus a twin offset 1 px across its dominant axis — the
/// panel's 2 px line idiom (the menu bezel ticks / passkey phone established it; the POI bearing
/// arrows and the computed-route shape preview draw through this one helper).
pub(crate) fn stroke2(cv: &mut impl Surface, a: Point, b: Point, color: u16) {
    cv.line(a, b, color);
    let off = if (b.x - a.x).abs() > (b.y - a.y).abs() { Point::new(0, 1) } else { Point::new(1, 0) };
    cv.line(a + off, b + off, color);
}

// ==================== the Recalculating banner (issue #1146, P2) ====================
//
// The overlay chrome the freeze raises. Drawing it on the map plane would mean rendering the map —
// the exact thing the freeze forbids — so it is painted by
// [`App::render_overlay`](crate::App::render_overlay) instead, the cheap half that composites over
// the still-visible frame, beside the long-press bulge. Whether it is up at all is
// [`CoreMode`](crate::device_core::core_mode::CoreMode)'s answer; this is only how it looks.

/// Banner height (px) — the map's status-chip height, so the two chrome pills read as one family.
const BANNER_H: i32 = 36;
/// Horizontal padding (px) around the copy, split either side — tighter than the status chip's 28,
/// because the copy is one long word: at 240 px the longest catalogued string ("Neuberechnung...")
/// would otherwise leave under 10 px of frame either side and read as a full-width bar.
const BANNER_PAD_X: i32 = 20;
/// Corner radius (px) — the shared pill radius.
const BANNER_RADIUS: u32 = 9;
/// Where the banner's top sits, as a fraction of frame height. A third of the way down: clear of
/// the top-centre clock, and well above the centred rider marker the rider is looking at (the map
/// under the banner is frozen, not gone — covering the marker would read as "lost").
const BANNER_Y_FRAC: f32 = 0.3;

/// The banner's bounding rows `[y0, y0 + rows)` in a `h`-high frame — what a partial-overlay host
/// re-presents (the board pushes overlay rows, not whole frames).
pub(crate) fn recalculating_banner_rows(h: f32) -> (u16, u16) {
    let y0 = (h * BANNER_Y_FRAC) as i32;
    let y0 = y0.clamp(0, (h as i32 - BANNER_H).max(0));
    (y0 as u16, BANNER_H.min(h as i32).max(0) as u16)
}

/// Draw the "Recalculating..." banner: a centred parchment pill with an ink outline and the copy in
/// ink — the calm chip idiom (the alert orange stays reserved for the No-GPS / off-route chip, which
/// is *below* on the frozen map plane and never collides with this band).
pub(crate) fn recalculating_banner<D, F>(target: &mut D, color_fn: &F, w: f32, h: f32, text: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let (w, h) = (w as i32, h as i32);
    // [`Font::Label`], not the status chip's Body: the copy is a long word in every language
    // ("Neuberechnung...", "Recalculando..."), and the pill must keep a margin at 240 px.
    let font = Font::Label;
    let (y0, _) = recalculating_banner_rows(h as f32);
    let pw = (text_width(text, font) as i32 + BANNER_PAD_X).min(w - 8);
    let px = (w - pw) / 2;
    let py = y0 as i32;
    let mut cv = Canvas::new(target, color_fn);
    cv.round(rect(px, py, pw, BANNER_H), BANNER_RADIUS, palette::PARCHMENT);
    cv.round_outline(rect(px, py, pw, BANNER_H), BANNER_RADIUS, palette::INK);
    cv.text(text, Point::new(w / 2, py + 5), font, TextAlign::Center, palette::INK);
}

/// Draw a centered two-line empty state — a bold `title` over a muted `hint` — the shared
/// "nothing to show yet" body the Route menu and Statistics draw under their header.
pub(crate) fn empty_state(cv: &mut impl Surface, w: i32, h: i32, title: &str, hint: &str) {
    cv.text(title, Point::new(w / 2, h / 2 - 28), Font::Body, TextAlign::Center, palette::INK);
    cv.text(hint, Point::new(w / 2, h / 2 + 8), Font::Label, TextAlign::Center, palette::SUBTEXT);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The banner sits in its own band: below the top-centre clock, above the centred rider marker,
    /// and always fully on-panel.
    #[test]
    fn the_recalculating_banner_band_stays_on_panel_and_clear_of_the_marker() {
        let (y0, rows) = recalculating_banner_rows(320.0);
        assert_eq!((y0, rows), (96, 36));
        assert!(y0 as i32 + rows as i32 <= 320);
        assert!((y0 + rows) < 160, "clear of the centred user marker");

        let (y0, rows) = recalculating_banner_rows(20.0); // a frame shorter than the banner (the test harnesses')
        assert_eq!(y0, 0, "clamped to the top rather than drawn off-panel");
        assert_eq!(rows, 20);
    }
}
