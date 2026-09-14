//! Explicitly enabled shop-visit UI study. The simulator supplies the route fixtures and costs.

use crate::{screen, App, CameraMode, Mode, RecorderIntent};

#[derive(Debug, PartialEq)]
pub struct Stop {
    pub name: &'static str,
    pub approach: &'static [(i32, i32)],
    pub distance_m: u32,
    pub climb_m: u32,
    pub extra_m: u32,
    pub extra_climb_m: u32,
    pub return_m: u32,
    pub return_climb_m: u32,
    pub outbound: usize,
    pub continuation: usize,
}

impl Stop {
    pub fn position(&self) -> (i32, i32) {
        *self.approach.last().expect("demo stop has an approach")
    }
}

#[derive(Debug, PartialEq)]
pub struct Fixture {
    pub original: usize,
    pub destination: &'static str,
    pub start: (i32, i32),
    pub stops: [Stop; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Riding,
    ToStop,
    Returning,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Demo {
    pub fixture: &'static Fixture,
    pub selected: u8,
    pub phase: Phase,
    awaiting_start: bool,
}

impl Demo {
    pub fn stop(self) -> &'static Stop {
        &self.fixture.stops[self.selected as usize]
    }
}

impl App {
    /// Opt into synthetic search results and preloaded route legs; ordinary startup leaves it off.
    pub fn enable_assistant_demo(&mut self, fixture: &'static Fixture) {
        self.state.assistant_demo = Some(Demo { fixture, selected: 0, phase: Phase::Riding, awaiting_start: true });
        self.activate_route(fixture.original);
        self.activity.mode = Mode::Riding;
        self.assistant_demo_position(fixture.start);
        screen::apply(&mut self.ui.stack, screen::Transition::Root(screen::Screen::Map(screen::MapScreen::new())));
        self.ui.map_dirty = true;
    }

    /// The host calls this after startup can admit a recording, before its next device pass.
    pub fn start_assistant_demo_if_ready(&mut self) {
        let Some(mut demo) = self.state.assistant_demo else { return };
        if demo.awaiting_start && self.can_record() {
            self.recorder.request(RecorderIntent::Start);
            demo.awaiting_start = false;
            self.state.assistant_demo = Some(demo);
        }
    }

    /// Simulate reaching the selected stop, or rejoining after the accepted return leg.
    pub fn advance_assistant_demo(&mut self) {
        let Some(mut demo) = self.state.assistant_demo else { return };
        if demo.phase == Phase::Riding {
            return;
        }
        self.ui.stack.retain(|screen| !matches!(screen, screen::Screen::Assistant(_)));
        match demo.phase {
            Phase::ToStop => {
                demo.phase = Phase::Returning;
                self.assistant_demo_position(demo.stop().position());
                self.activate_route(demo.stop().continuation);
                screen::apply(
                    &mut self.ui.stack,
                    screen::Transition::Push(screen::Screen::Assistant(screen::AssistantScreen::arrival())),
                );
            }
            Phase::Returning => {
                demo.phase = Phase::Riding;
                self.assistant_demo_position(demo.stop().approach[0]);
                self.activate_route(demo.fixture.original);
            }
            Phase::Riding => return,
        }
        self.state.assistant_demo = Some(demo);
        self.ui.map_dirty = true;
    }

    fn assistant_demo_position(&mut self, (lon, lat): (i32, i32)) {
        self.state.user_fix = Some(obc_ports::Fix { lon, lat, course: None, speed_mps: Some(0.0) });
        self.state.cam_lon = lon;
        self.state.cam_lat = lat;
        self.state.zoom = 0.045;
        self.state.mode = CameraMode::Follow;
        self.state.heading_up = false;
        self.state.pan = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppState, Chord, Gesture, RouteSummary};

    const STOP: Stop = Stop {
        name: "Shop",
        approach: &[(100, 100), (200, 200)],
        distance_m: 800,
        climb_m: 90,
        extra_m: 1000,
        extra_climb_m: 80,
        return_m: 500,
        return_climb_m: 0,
        outbound: 1,
        continuation: 2,
    };
    static FIXTURE: Fixture = Fixture { original: 0, destination: "Pass", start: (0, 0), stops: [STOP, STOP] };

    fn app() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_backlight_available(true);
        let routes = core::array::from_fn::<_, 3, _>(|_| RouteSummary {
            name: heapless::String::try_from("Route").unwrap(),
            distance_km: 10,
            climb_m: 100,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 },
            start_lon: 0,
            start_lat: 0,
        });
        app.set_routes_with_ids(&routes, &[10, 11, 12]);
        app.enable_assistant_demo(&FIXTURE);
        app.recorder.test_open();
        app
    }

    fn open(app: &mut App) {
        app.apply_chord(Chord::Quick);
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), screen::Screen::Assistant(_)));
    }

    fn preview(app: &mut App) {
        open(app);
        for _ in 0..3 {
            app.apply_gesture(Gesture::Press);
        }
    }

    #[test]
    fn visit_keeps_the_recording_and_restores_the_route_after_rejoining() {
        for dismiss in [Some(Gesture::Press), Some(Gesture::Back), None] {
            let mut app = app();
            let session = app.recorder.session();
            preview(&mut app);
            assert_eq!(app.active_route_index(), Some(0), "preview does not replace navigation");
            app.apply_gesture(Gesture::Press);
            assert_eq!(app.active_route_index(), Some(1));
            assert!(matches!(app.top_screen(), screen::Screen::Map(_)));

            app.apply_gesture(Gesture::Press);
            assert!(matches!(app.top_screen(), screen::Screen::RideControl(_)), "Map Select still pauses");
            app.apply_gesture(Gesture::Back);
            if dismiss.is_none() {
                open(&mut app);
            }
            app.advance_assistant_demo();
            assert!(matches!(app.top_screen(), screen::Screen::Assistant(_)));
            assert_eq!(app.active_route_index(), Some(2), "return guidance is active before any arrival input");
            assert_eq!(app.state.assistant_demo.unwrap().phase, Phase::Returning);
            if let Some(gesture) = dismiss {
                app.apply_gesture(gesture);
                assert!(matches!(app.top_screen(), screen::Screen::Map(_)));
                assert_eq!(app.active_route_index(), Some(2), "dismissal does not change navigation");
                open(&mut app);
                app.apply_gesture(Gesture::Press);
                assert!(matches!(app.top_screen(), screen::Screen::Map(_)));
                assert_eq!(app.active_route_index(), Some(2));
            }
            app.advance_assistant_demo();
            assert_eq!(app.active_route_index(), Some(0));
            assert!(matches!(app.top_screen(), screen::Screen::Map(_)), "an ignored arrival card clears on rejoin");
            assert_eq!(app.state.assistant_demo.unwrap().phase, Phase::Riding);
            assert_eq!(app.recorder.session(), session);
            assert!(app.recorder.recording());
            assert_eq!(app.activity.mode, Mode::Riding);
        }
    }

    #[test]
    fn backing_out_and_removing_a_stop_do_not_start_a_return_from_the_shop() {
        let mut app = app();
        let session = app.recorder.session();
        preview(&mut app);
        for _ in 0..4 {
            app.apply_gesture(Gesture::Back);
        }
        assert_eq!(app.active_route_index(), Some(0));
        assert!(matches!(app.top_screen(), screen::Screen::Map(_)));
        preview(&mut app);
        app.apply_gesture(Gesture::Press);
        open(&mut app);
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.active_route_index(), Some(1), "removal requires its own confirmation");
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.active_route_index(), Some(0));
        assert_eq!(app.recorder.session(), session);
        assert_eq!(app.state.user_fix.map(|fix| (fix.lon, fix.lat)), Some(FIXTURE.start));
    }
}
