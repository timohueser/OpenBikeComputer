//! The screen chrome: the framed page header every screen draws through, the card glyphs, the
//! Recalculating banner, and the shared text and stroke helpers.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Canvas, Surface,
};

use crate::screen::palette;

pub(crate) const TITLE_BAR_H: i32 = 34;

pub(crate) const LIST_TOP: i32 = TITLE_BAR_H + 8;

/// Draw the shared screen chrome: a background, a thin rounded outline, and a wood title bar with
/// `title` on the left and `right` (a counter, a grade readout) on the right. The caller fills the
/// body below [`LIST_TOP`]. For the BLE indicator in the right slot, call [`title_frame_ble`].
pub(crate) fn title_frame(cv: &mut impl Surface, w: i32, h: i32, title: &str, right: &str) {
    title_frame_ble(cv, w, h, title, right, false)
}

/// [`title_frame`] plus the BLE connected indicator: a small Bluetooth rune in the title bar's
/// right slot. The `right` readout is inset left of it, so the two never overlap. The rune does not
/// animate, so it repaints only on a link change.
pub(crate) fn title_frame_ble(cv: &mut impl Surface, w: i32, h: i32, title: &str, right: &str, ble_connected: bool) {
    use palette::*;
    cv.clear(PARCHMENT);
    title_chrome(cv, w, h, title);
    let right_x = if ble_connected {
        ble_glyph(cv, w - 14 - BLE_GLYPH_W, TITLE_BAR_H / 2 + 4, PARCHMENT);
        w - 14 - BLE_GLYPH_W - 8
    } else {
        w - 14
    };
    // The two y values differ because the Body and Label glyphs have different baselines.
    cv.text(right, Point::new(right_x, 10), Font::Label, TextAlign::Right, PARCHMENT);
}

/// The outline and the titled wood bar of [`title_frame`], without its clear: for a page whose
/// map band has already painted the background.
pub(crate) fn title_chrome(cv: &mut impl Surface, w: i32, h: i32, title: &str) {
    use palette::*;
    cv.round_outline(rect(4, 4, w - 8, h - 8), 8, WOOD_LIGHT);
    cv.round(rect(4, 4, w - 8, TITLE_BAR_H), 6, WOOD);
    cv.text(title, Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
}

/// Total width (px) the [`ble_glyph`] rune occupies, so callers can reserve its slot.
pub(crate) const BLE_GLYPH_W: i32 = 11;

/// Draw the Bluetooth bind-rune (ᛒ) centred vertically on `cy`, its left edge at `x`. It is
/// plotted as lines, not a font glyph, so it reads at the device's pixel scale.
pub(crate) fn ble_glyph(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    let half = 8; // half-height, so a 16 px stem
    let (top, mid, bot) = (cy - half, cy, cy + half);
    let stem_x = x + 3; // inset, to leave room for the left back-strokes
    let tip_x = x + BLE_GLYPH_W - 1; // the rightmost point of each triangle
    let left_x = x; // the two left corners the diagonals reach
    let quarter = half / 2;
    let (t, b, c) = (Point::new(stem_x, top), Point::new(stem_x, bot), Point::new(stem_x, mid));
    let up_tip = Point::new(tip_x, top + quarter);
    let lo_tip = Point::new(tip_x, bot - quarter);
    cv.line(t, b, color);
    cv.line(t, up_tip, color);
    cv.line(up_tip, c, color);
    cv.line(b, lo_tip, color);
    cv.line(lo_tip, c, color);
    // The crossing diagonals to the opposite left corner. Without them the glyph reads as two
    // stacked chevrons.
    cv.line(up_tip, Point::new(left_x, bot - quarter), color);
    cv.line(lo_tip, Point::new(left_x, top + quarter), color);
}

/// Draw the shared card warning glyph, an amber triangle with an ink exclamation, centred at
/// `center`. `k` is the triangle's half-height.
pub(crate) fn card_triangle(cv: &mut impl Surface, center: Point, k: i32) {
    use palette::*;
    let (cx, cy) = (center.x, center.y);
    cv.triangle(Point::new(cx, cy - k), Point::new(cx - k, cy + k), Point::new(cx + k, cy + k), AMBER);
    // Exclamation: a bar over a dot.
    cv.vline(cx, cy - k / 4, k / 2, 3, INK);
    cv.disc(Point::new(cx, cy + k / 2 + 1), 2, INK);
}

/// Draw the shared card check glyph near `center`, `k` its half-width. The two strokes are stepped
/// out of discs, because the canvas has no diagonal thick-line primitive.
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
    seg(cv, (cx - k, cy), (cx - k / 3, cy + k * 2 / 3));
    seg(cv, (cx - k / 3, cy + k * 2 / 3), (cx + k, cy - k * 2 / 3));
}

/// The half-width of [`row_check`].
pub(crate) const ROW_CHECK_HALF: i32 = 5;

/// The two-stroke check at list-row scale, centred at `c` on the cap of a row's name line.
pub(crate) fn row_check(cv: &mut impl Surface, c: Point, color: u16) {
    fn seg(cv: &mut impl Surface, a: (i32, i32), b: (i32, i32), color: u16) {
        const N: i32 = 8;
        for s in 0..=N {
            let x = a.0 + (b.0 - a.0) * s / N;
            let y = a.1 + (b.1 - a.1) * s / N;
            cv.disc(Point::new(x, y), 1, color);
        }
    }
    let k = ROW_CHECK_HALF;
    seg(cv, (c.x - k, c.y), (c.x - k / 3, c.y + k * 2 / 3), color);
    seg(cv, (c.x - k / 3, c.y + k * 2 / 3), (c.x + k, c.y - k * 2 / 3), color);
}

pub(crate) fn wrapped_line_pitch(font: Font) -> i32 {
    font.line_height() as i32 - 5
}

/// The wrap budget of a line laid out across the whole panel: the frame width less the rounded
/// outline and its clearance either side.
pub(crate) const fn copy_w(w: i32) -> i32 {
    w - 12
}

/// Greedy word wrap over the monospace cell, one call of `emit` per line. The budget counts
/// characters, not bytes: the face renders every char of its repertoire in one cell, so a byte
/// count breaks the accented languages early. A single word wider than the budget breaks after
/// its last slash or hyphen that fits, else at the budget.
pub(crate) fn wrap(text: &str, width_px: i32, font: Font, mut emit: impl FnMut(&str)) {
    let budget = (width_px / font.char_width() as i32).max(1) as usize;
    let mut line: heapless::String<64> = heapless::String::new();
    for mut word in text.split(' ') {
        while let Some((limit, _)) = word.char_indices().nth(budget) {
            let cut = word[..limit].rfind(['/', '-']).map_or(limit, |at| at + 1);
            if !line.is_empty() {
                emit(&line);
                line.clear();
            }
            emit(&word[..cut]);
            word = &word[cut..];
        }
        let used = line.chars().count();
        if used != 0 && used + 1 + word.chars().count() > budget {
            emit(&line);
            line.clear();
        }
        if !line.is_empty() {
            let _ = line.push(' ');
        }
        let _ = line.push_str(word);
    }
    if !line.is_empty() {
        emit(&line);
    }
}

/// Draw `text` word-wrapped into centred `font` lines within `width_px`, the first line at
/// `top_y`. Returns the `y` just past the last line, so a caller can stack more below it.
pub(crate) fn wrapped(
    cv: &mut impl Surface,
    text: &str,
    cx: i32,
    top_y: i32,
    width_px: i32,
    font: Font,
    color: u16,
) -> i32 {
    wrapped_aligned(cv, text, cx, top_y, width_px, font, TextAlign::Center, color)
}

/// [`wrapped`] with the alignment spelled out, for copy that sits in a row instead of on the
/// panel's centreline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wrapped_aligned(
    cv: &mut impl Surface,
    text: &str,
    x: i32,
    top_y: i32,
    width_px: i32,
    font: Font,
    align: TextAlign,
    color: u16,
) -> i32 {
    let lh = wrapped_line_pitch(font);
    let mut y = top_y;
    wrap(text, width_px, font, |line| {
        cv.text(line, Point::new(x, y), font, align, color);
        y += lh;
    });
    y
}

/// The panel's 2 px line: the segment plus a twin offset 1 px across its dominant axis.
pub(crate) fn stroke2(cv: &mut impl Surface, a: Point, b: Point, color: u16) {
    cv.line(a, b, color);
    let off = if (b.x - a.x).abs() > (b.y - a.y).abs() { Point::new(0, 1) } else { Point::new(1, 0) };
    cv.line(a + off, b + off, color);
}

// The Recalculating banner is painted by [`App::render_overlay`](crate::App::render_overlay), not
// on the map plane: drawing it on the map plane would mean rendering the map, which the freeze
// forbids. Whether it is up at all is
// [`CoreMode`](crate::device_core::core_mode::CoreMode)'s answer; this is only how it looks.

/// Banner height, including the small compass below the label.
const BANNER_H: i32 = 56;
/// Horizontal padding (px) around the copy, split either side. It is tight, because the copy is
/// one long word and a wider pad makes the pill read as a full-width bar at 240 px.
const BANNER_PAD_X: i32 = 20;
const BANNER_RADIUS: u32 = 9;
/// Where the banner's top sits, as a fraction of frame height: clear of the top-centre clock, and
/// above the centred rider marker. The map below is frozen, not gone, so the marker must stay
/// visible.
const BANNER_Y_FRAC: f32 = 0.275;

/// The banner's bounding rows `[y0, y0 + rows)` in a `h`-high frame. The board pushes these rows,
/// not whole frames.
pub(crate) fn recalculating_banner_rows(h: f32) -> (u16, u16) {
    let y0 = (h * BANNER_Y_FRAC) as i32;
    let y0 = y0.clamp(0, (h as i32 - BANNER_H).max(0));
    (y0 as u16, BANNER_H.min(h as i32).max(0) as u16)
}

/// Draw the "Recalculating..." banner: a centred parchment pill with an ink outline and ink copy.
pub(crate) fn recalculating_banner<D, F>(target: &mut D, color_fn: &F, w: f32, h: f32, text: &str, phase: u8)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let (w, h) = (w as i32, h as i32);
    // Label, not Body: the copy is one long word in every language, and the pill must keep a
    // margin at 240 px.
    let font = Font::Label;
    let (y0, _) = recalculating_banner_rows(h as f32);
    let pw = (text_width(text, font) as i32 + BANNER_PAD_X).min(w - 8);
    let px = (w - pw) / 2;
    let py = y0 as i32;
    let mut cv = Canvas::new(target, color_fn);
    cv.round(rect(px, py, pw, BANNER_H), BANNER_RADIUS, palette::PARCHMENT);
    cv.round_outline(rect(px, py, pw, BANNER_H), BANNER_RADIUS, palette::INK);
    cv.text(text, Point::new(w / 2, py + 5), font, TextAlign::Center, palette::INK);
    crate::screen::menu::draw_needle(
        &mut cv,
        Point::new(w / 2, py + 40),
        f32::from(phase.saturating_sub(1)) * 120.0,
        10.0,
        3.0,
    );
}

/// Draw a centred two-line empty state: a `title` over a muted `hint`.
pub(crate) fn empty_state(cv: &mut impl Surface, w: i32, h: i32, title: &str, hint: &str) {
    cv.text(title, Point::new(w / 2, h / 2 - 28), Font::Body, TextAlign::Center, palette::INK);
    cv.text(hint, Point::new(w / 2, h / 2 + 8), Font::Label, TextAlign::Center, palette::SUBTEXT);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The face draws one cell per char, so the wrap budget counts chars.
    #[test]
    fn the_wrap_budget_counts_glyph_cells_not_bytes() {
        let copy = "Réessayez plus tôt"; // 18 chars, 20 bytes
        let lines = |width_px| {
            let mut n = 0;
            wrap(copy, width_px, Font::Label, |_| n += 1);
            n
        };
        assert_eq!(lines(18 * Font::Label.char_width() as i32), 1);
        assert_eq!(lines(17 * Font::Label.char_width() as i32), 2);
    }

    #[test]
    fn a_word_wider_than_the_budget_breaks_after_a_hyphen_or_at_the_budget() {
        let mut lines = std::vec::Vec::new();
        let copy = "2019-07-30-Dunlough Castle 12345678901234567890";
        wrap(copy, 18 * Font::Label.char_width() as i32, Font::Label, |line| {
            lines.push(std::string::String::from(line))
        });
        assert_eq!(lines, ["2019-07-30-", "Dunlough Castle", "123456789012345678", "90"]);
    }

    #[test]
    fn the_recalculating_banner_band_stays_on_panel_and_clear_of_the_marker() {
        let (y0, rows) = recalculating_banner_rows(320.0);
        assert_eq!((y0, rows), (88, 56));
        assert!(y0 as i32 + rows as i32 <= 320);
        assert!(y0 + rows <= 160 - 12, "clear of the full rider chevron, including its north tip");

        let mut previous = None;
        for phase in 1..=3 {
            let mut frame = crate::harness::support::Buf::new(240, 320);
            recalculating_banner(
                &mut frame,
                &|c| {
                    let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
                    embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
                },
                240.0,
                320.0,
                "Finding",
                phase,
            );
            assert!(
                frame.px[..y0 as usize * 240]
                    .iter()
                    .chain(&frame.px[(y0 + rows) as usize * 240..])
                    .all(|p| *p == embedded_graphics::pixelcolor::Rgb888::new(0, 0, 0)),
                "the full spinner stays inside the board band"
            );
            if let Some(previous) = previous {
                assert_ne!(frame.px, previous, "each one-second phase rotates the compass");
            }
            previous = Some(frame.px);
        }

        let (y0, rows) = recalculating_banner_rows(20.0); // a frame shorter than the banner
        assert_eq!(y0, 0, "clamped to the top rather than drawn off-panel");
        assert_eq!(rows, 20);
    }
}
