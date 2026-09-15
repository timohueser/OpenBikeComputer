//! The marquee at the runtime seam: a frame's draw names the one long text to scroll, the render
//! adopts it and arms its first step, the pass steps it on the clock inside the text row's region,
//! and a frame that stops asking stops it.

use embedded_graphics::{pixelcolor::Rgb888, prelude::Point};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};

use crate::harness::support::{build_min_obcm, Buf, Frames};
use crate::screen::vocab::chrome::TITLE_BAR_H;
use crate::screen::vocab::marquee::{Marquee, HEAD_REST_MS, STEP_MS};
use crate::screen::{Screen, TripDeleteScreen};
use crate::{App, AppState};

/// Render one full 240×320 frame into `buf` without a tick, so the pass clock stays the test's.
fn render(app: &mut App, bytes: &[u8], buf: &mut Buf) {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid fixture");
    let reader = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame(Some(&mut scratch), buf, &reader, None, 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
}

#[test]
fn a_long_name_scrolls_inside_its_row_and_stops_when_the_screen_leaves() {
    let bytes = build_min_obcm(1);
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    let mut frames = Frames::new();
    // 29 characters over the confirm card's 15-character field.
    let _ = app.ui.stack.push(Screen::TripDelete(TripDeleteScreen::new(7, "Fontaine du Mont Ventoux Loop")));
    frames.idle(&mut app, 0);
    let planned = app.ui.next_wake_ms;

    let mut head = Buf::new(240, 320);
    render(&mut app, &bytes, &mut head);
    assert_eq!(
        app.ui.next_wake_ms,
        Some(planned.map_or(HEAD_REST_MS, |w| w.min(HEAD_REST_MS))),
        "the render that names the scroll arms its first step"
    );

    let dirty = frames.idle(&mut app, HEAD_REST_MS);
    let name_row = obc_render::rect(12, TITLE_BAR_H + 12, 240 - 24, 28);
    assert!(dirty.map, "the first step is a repaint");
    assert_eq!(dirty.region, Some(name_row), "clipped to the name's text row");
    assert_eq!(app.ui.next_wake_ms, Some(STEP_MS), "the next step is armed");

    let mut stepped = Buf::new(240, 320);
    render(&mut app, &bytes, &mut stepped);
    let mut moved = 0;
    for y in 0..320 {
        for x in 0..240 {
            if head.get(x, y) != stepped.get(x, y) {
                assert!(name_row.contains(Point::new(x, y)), "a pixel changed outside the promised row at ({x}, {y})");
                moved += 1;
            }
        }
    }
    assert!(moved > 0, "the name moved by one character");

    let _ = app.ui.stack.pop();
    frames.idle(&mut app, HEAD_REST_MS + STEP_MS);
    render(&mut app, &bytes, &mut stepped);
    assert_eq!(app.ui.marquee, Marquee::default(), "a frame that asks for nothing stops the marquee");
    frames.idle(&mut app, HEAD_REST_MS + 2 * STEP_MS);
    assert_ne!(app.ui.next_wake_ms, Some(STEP_MS), "and nothing wakes for it");
}
