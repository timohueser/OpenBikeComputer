//! Assistant action symbols in a 24 × 24 pixel box.

use embedded_graphics::prelude::Point;
use obc_render::Surface;

use crate::{screen::vocab::chrome::stroke2, Msg};

pub(super) fn draw(cv: &mut impl Surface, question: Msg, at: Point, ink: u16) {
    let paths: &[&[(i32, i32)]] = match question {
        Msg::AssistantFind => &[
            &[(10, 22), (4, 14), (2, 9), (2, 5), (5, 1), (10, 0), (15, 1), (18, 5), (18, 9), (16, 14), (10, 22)],
            &[(8, 6), (12, 6), (13, 8), (12, 10), (8, 10), (7, 8), (8, 6)],
        ],
        Msg::AssistantNext => {
            &[&[(3, 22), (3, 17), (5, 14), (14, 12), (17, 10), (17, 2)], &[(12, 7), (17, 2), (22, 7)]]
        }
        Msg::AssistantEasier => {
            &[&[(0, 19), (7, 10), (12, 16), (17, 12), (22, 19)], &[(1, 4), (20, 4)], &[(16, 1), (20, 4), (16, 7)]]
        }
        Msg::AssistantLandmarks => &[
            &[(1, 7), (11, 1), (21, 7), (1, 7)],
            &[(3, 9), (3, 20)],
            &[(8, 9), (8, 20)],
            &[(14, 9), (14, 20)],
            &[(19, 9), (19, 20)],
            &[(1, 22), (21, 22)],
        ],
        Msg::AssistantBlocked => &[
            &[(1, 6), (21, 6), (21, 14), (1, 14), (1, 6)],
            &[(4, 14), (4, 22)],
            &[(18, 14), (18, 22)],
            &[(3, 13), (10, 6)],
            &[(13, 14), (20, 7)],
        ],
        Msg::AssistantBackRoute => {
            &[&[(16, 22), (16, 2)], &[(11, 7), (16, 2), (21, 7)], &[(1, 21), (1, 16), (3, 13), (12, 11), (16, 7)]]
        }
        Msg::AssistantDetour => &[
            &[(2, 22), (2, 15), (4, 12), (10, 11), (13, 7), (13, 2)],
            &[(9, 6), (13, 2), (17, 6)],
            &[(20, 11), (20, 19)],
            &[(16, 15), (23, 15)],
        ],
        _ => return,
    };
    for path in paths {
        for pair in path.windows(2) {
            let point = |(x, y)| at + Point::new(x, y);
            stroke2(cv, point(pair[0]), point(pair[1]), ink);
        }
    }
}
