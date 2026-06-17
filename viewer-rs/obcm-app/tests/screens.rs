//! Screen-stack tests: navigation [`Transition`]s per gesture, the guarded-action
//! "needs a completed hold" rule, the stack discipline ([`apply`]), and a render
//! snapshot proving Ride control composites over the map. Mirrors the style of
//! `obcm-render/tests/priority.rs` (feed inputs, assert the outcome) and
//! `obcm-app/tests/marker.rs` (render into a tiny `DrawTarget`).

use std::collections::VecDeque;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obcm_app::activity::Activity;
use obcm_app::screen::{apply, Ctx, HomeScreen, MapScreen, MenuScreen, RideControl, Screen, Stack, Transition};
use obcm_app::{App, AppState, Button, ButtonEvent, Fix, Gesture, InputEvent, InputSource, LocationSource, Mode};
use obcm_reader::{rgb565_to_rgb888, Reader};

/// A handle [`Ctx`] over freshly-made state/activity for a one-gesture test.
fn ctx<'a>(state: &'a mut AppState, activity: &'a mut Activity) -> Ctx<'a> {
    Ctx { state, activity, now_ms: 0 }
}

// ---------------------------------------------------------------------------
// Per-gesture navigation transitions.
// ---------------------------------------------------------------------------

#[test]
fn map_press_pauses_into_ride_control() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MapScreen::new().handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Push(Screen::RideControl(_))));
    assert_eq!(act.mode, Mode::Paused, "pausing stops tracking immediately");
}

#[test]
fn map_turn_zooms_in_place() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let z0 = st.zoom;
    let t = MapScreen::new().handle(Gesture::Turn(2), &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None));
    assert!(st.zoom > z0, "clockwise turn zooms in");
}

#[test]
fn map_back_hold_opens_the_menu() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MapScreen::new().handle(Gesture::BackHold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Push(Screen::Menu(_))));
}

#[test]
fn ride_control_resume_is_a_press_that_pops() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let mut rc = RideControl::new(); // starts on Resume
    assert!(!rc.selection_is_guarded());
    let t = rc.handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Pop), "Resume returns to the caller");
    assert_eq!(act.mode, Mode::Riding);
}

#[test]
fn ride_control_back_resumes() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let t = RideControl::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Pop));
    assert_eq!(act.mode, Mode::Riding, "back cancels the pause");
}

#[test]
fn guarded_action_needs_a_completed_hold_not_a_press() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let mut rc = RideControl::new();
    rc.handle(Gesture::Turn(1), &mut ctx(&mut st, &mut act)); // move to Finish (guarded)
    assert!(rc.selection_is_guarded());

    // A press must NOT commit an irreversible action.
    let t = rc.handle(Gesture::Press, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::None));
    assert_eq!(act.mode, Mode::Paused, "a stray press can't finish the ride");

    // A completed hold (the recognizer only emits `Hold` once the threshold is
    // crossed) is what confirms it.
    let t = rc.handle(Gesture::Hold, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Home), "Finish clears back to Home");
    assert_eq!(act.mode, Mode::Idle);
}

#[test]
fn hold_on_a_non_guarded_item_does_nothing() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Paused));
    let t = RideControl::new().handle(Gesture::Hold, &mut ctx(&mut st, &mut act)); // on Resume
    assert!(matches!(t, Transition::None));
    assert_eq!(act.mode, Mode::Paused);
}

#[test]
fn menu_back_returns_to_caller() {
    let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Riding));
    let t = MenuScreen::new().handle(Gesture::Back, &mut ctx(&mut st, &mut act));
    assert!(matches!(t, Transition::Pop));
}

// ---------------------------------------------------------------------------
// Stack discipline.
// ---------------------------------------------------------------------------

#[test]
fn apply_pushes_pops_replaces_and_returns_home() {
    let mut stack: Stack = Stack::new();
    let _ = stack.push(Screen::Home(HomeScreen::new()));
    let _ = stack.push(Screen::Map(MapScreen::new()));

    // Overlay an Menu, then back out to the caller (Map).
    apply(&mut stack, Transition::Push(Screen::Menu(MenuScreen::new())));
    assert_eq!(stack.len(), 3);
    assert!(matches!(stack.last(), Some(Screen::Menu(_))));
    apply(&mut stack, Transition::Pop);
    assert!(matches!(stack.last(), Some(Screen::Map(_))), "Pop returns to caller");

    // Replace swaps the top without growing the stack.
    apply(&mut stack, Transition::Replace(Screen::Menu(MenuScreen::new())));
    assert_eq!(stack.len(), 2);
    assert!(matches!(stack.last(), Some(Screen::Menu(_))));

    // Home clears every overlay back to the root.
    apply(&mut stack, Transition::Home);
    assert_eq!(stack.len(), 1);
    assert!(matches!(stack.last(), Some(Screen::Home(_))));

    // The root is the guaranteed floor — Pop can't empty the stack.
    apply(&mut stack, Transition::Pop);
    assert_eq!(stack.len(), 1, "the Home root is never popped");
}

// ---------------------------------------------------------------------------
// Render snapshot: the map, and Ride control composited over it.
// ---------------------------------------------------------------------------

/// One scripted raw input event per `poll` — drives [`App::handle_input`].
struct Script(VecDeque<InputEvent>);
impl InputSource for Script {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// A no-fix location source (the marker stays off so it can't confuse the snapshot).
struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<Fix> {
        None
    }
}

#[test]
fn ride_control_composites_over_the_map() {
    let bytes = build_min_obcm(0xF800);
    let mut app = App::new(AppState::new(0, 0, 0.05));

    // Riding: the center is the (blue sea) backdrop.
    let map = render(&mut app, &bytes);
    let backdrop = map.get(60, 60);

    // A press (Down+Up within the threshold) pauses into Ride control.
    let mut press = Script(VecDeque::from(vec![
        InputEvent::Button(ButtonEvent::Down(Button::Encoder)),
        InputEvent::Button(ButtonEvent::Up(Button::Encoder)),
    ]));
    app.handle_input(0, &mut press);
    assert_eq!(app.mode(), Mode::Paused, "press paused the ride");

    // Now the center carries the parchment Ride-control panel, not the backdrop.
    let paused = render(&mut app, &bytes);
    let panel = paused.get(60, 60);
    assert_ne!(panel, backdrop, "the overlay changed the center");
    assert!(panel.r() > backdrop.r(), "parchment panel is lighter than the sea backdrop");
}

// --- tiny render harness (mirrors marker.rs) ---

fn render(app: &mut App, bytes: &[u8]) -> Buf {
    app.tick(&mut NoFix);
    let reader = Reader::new(bytes).expect("valid v5 file");
    let mut buf = Buf::new(120, 120);
    app.render_frame(&mut buf, &reader, 120.0, 120.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

struct Buf {
    w: i32,
    h: i32,
    px: Vec<Rgb888>,
}
impl Buf {
    fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    fn get(&self, x: i32, y: i32) -> Rgb888 {
        self.px[(y * self.w + x) as usize]
    }
    fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
}
impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}
impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }
}

/// A minimal valid v5 file: one sea-backdrop style, one empty LOD leaf, no chunks.
/// The map renders as a flat backdrop, so the only non-backdrop pixels come from a
/// screen drawn on top. (Same builder as `marker.rs`.)
fn build_min_obcm(marker: u16) -> Vec<u8> {
    let style_off: u32 = 32;
    let mut styles = vec![1u8];
    styles.push(1);
    styles.push(0);
    styles.extend_from_slice(&0x001Fu16.to_le_bytes());
    styles.push(1);
    styles.push(0);

    let lod_tab_off = style_off as usize + styles.len();
    let index_off = lod_tab_off + 18;

    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&(index_off as u32).to_le_bytes());
    table.extend_from_slice(&1u32.to_le_bytes());
    table.extend_from_slice(&16u16.to_le_bytes());
    table.extend_from_slice(&0u32.to_le_bytes());

    let index = 0x7FFF_FFFFu32.to_le_bytes();

    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(5);
    for v in [-1000i32, -1000, 1000, 1000] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&style_off.to_le_bytes());
    f.push(1);
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    f.extend_from_slice(&styles);
    f.extend_from_slice(&table);
    f.extend_from_slice(&index);
    f
}
