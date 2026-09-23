//! The four UI languages' flags, as 18 × 12 pixel marks. A flag is the one glyph a rider can read
//! in a language they cannot, which is why the language rows carry one.

use obc_render::{rect, Surface};

use crate::screen::palette;
use crate::settings::Language;

pub(crate) const FLAG_W: i32 = 18;
pub(crate) const FLAG_H: i32 = 12;

/// Draw `lang`'s flag with its top-left at `(x, y)`, inside a one-pixel grey keyline so the white
/// fields read against the parchment.
pub(crate) fn draw_flag(cv: &mut impl Surface, x: i32, y: i32, lang: Language) {
    use palette::*;
    let (w, h) = (FLAG_W, FLAG_H);
    match lang {
        Language::De => {
            cv.fill(rect(x, y, w, 4), HUD);
            cv.fill(rect(x, y + 4, w, 4), RED);
            cv.fill(rect(x, y + 8, w, 4), YELLOW);
        }
        Language::Fr => {
            cv.fill(rect(x, y, 6, h), BREADCRUMB);
            cv.fill(rect(x + 6, y, 6, h), ART_WHITE);
            cv.fill(rect(x + 12, y, 6, h), RED);
        }
        Language::Es => {
            cv.fill(rect(x, y, w, 3), RED);
            cv.fill(rect(x, y + 3, w, 6), YELLOW);
            cv.fill(rect(x, y + 9, w, 3), RED);
        }
        Language::En => {
            cv.fill(rect(x, y, w, h), BREADCRUMB);
            // The saltire: two 3 px diagonals.
            for i in 0..w {
                let yy = y + i * h / w;
                for dy in -1..=1 {
                    let py = yy + dy;
                    if py >= y && py < y + h {
                        cv.fill(rect(x + i, py, 1, 1), ART_WHITE);
                        cv.fill(rect(x + w - 1 - i, py, 1, 1), ART_WHITE);
                    }
                }
            }
            // The cross: white border, red core.
            cv.fill(rect(x, y + 4, w, 4), ART_WHITE);
            cv.fill(rect(x + 7, y, 4, h), ART_WHITE);
            cv.fill(rect(x, y + 5, w, 2), RED);
            cv.fill(rect(x + 8, y, 2, h), RED);
        }
    }
    cv.round_outline(rect(x - 1, y - 1, w + 2, h + 2), 2, CONTOUR);
}
