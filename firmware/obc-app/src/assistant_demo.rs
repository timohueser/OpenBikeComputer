//! Explicitly enabled shop-visit UI study. The simulator supplies the route fixtures and costs.

use crate::{screen, App, CameraMode, Mode, RecorderIntent};

mod candidates;
pub use candidates::{Candidates, MAX_RESULTS, ON_WAY_EXTRA_M};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Map,
    Questions,
    Categories,
    Choices,
    Preview,
    ToStop,
    Visit,
    Arrived,
    Returning,
    Rejoined,
    Skip,
}

impl Stage {
    pub const ALL: [(Self, &'static str); 11] = [
        (Self::Map, "map"),
        (Self::Questions, "questions"),
        (Self::Categories, "categories"),
        (Self::Choices, "choices"),
        (Self::Preview, "preview"),
        (Self::ToStop, "to-stop"),
        (Self::Visit, "visit"),
        (Self::Arrived, "arrival"),
        (Self::Returning, "returning"),
        (Self::Rejoined, "rejoined"),
        (Self::Skip, "remove-stop"),
    ];

    pub fn needs_stop(self) -> bool {
        !matches!(self, Self::Map | Self::Questions | Self::Categories | Self::Choices)
    }
}

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
    pub fn on_way(&self) -> bool {
        self.extra_m <= ON_WAY_EXTRA_M
    }
    pub fn position(&self) -> (i32, i32) {
        *self.approach.last().expect("demo stop has an approach")
    }
}

#[derive(Debug, PartialEq)]
pub struct Fixture {
    pub original: usize,
    pub start: (i32, i32),
    pub stops: &'static [Stop],
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
    pub candidates: Candidates,
    awaiting_start: bool,
}

impl Demo {
    pub fn stop(self) -> &'static Stop {
        &self.fixture.stops[self.candidates.indices[self.selected as usize] as usize]
    }

    pub fn stops(self) -> impl Iterator<Item = &'static Stop> {
        self.candidates
            .indices
            .into_iter()
            .take(self.candidates.len as usize)
            .map(move |i| &self.fixture.stops[i as usize])
    }
}

impl App {
    /// Opt into synthetic search results and preloaded route legs; ordinary startup leaves it off.
    pub fn enable_assistant_demo(&mut self, fixture: &'static Fixture) {
        self.state.assistant_demo = Some(Demo {
            fixture,
            selected: 0,
            phase: Phase::Riding,
            awaiting_start: true,
            candidates: candidates::select(fixture.stops, 0..fixture.stops.len()),
        });
        self.activate_route(fixture.original);
        self.activity.mode = Mode::Riding;
        self.assistant_demo_position(fixture.start);
        screen::apply(&mut self.ui.stack, screen::Transition::Root(screen::Screen::Map(screen::MapScreen::new())));
        self.ui.map_dirty = true;
    }

    /// Supply a candidate set, then return to its comparison without starting a new recording.
    pub fn set_assistant_candidates(&mut self, available: &[usize]) {
        let Some(mut demo) = self.state.assistant_demo else { return };
        demo.candidates = candidates::select(demo.fixture.stops, available.iter().copied());
        self.state.assistant_demo = Some(demo);
        self.show_assistant_demo(Stage::Choices, 0);
    }

    /// Jump to a study stage. Empty results admit only the menu and comparison stages.
    pub fn show_assistant_demo(&mut self, stage: Stage, selected: usize) -> bool {
        let Some(mut demo) = self.state.assistant_demo else { return false };
        if selected >= (demo.candidates.len as usize).max(1) || (stage.needs_stop() && demo.candidates.len == 0) {
            return false;
        }
        demo.selected = selected as u8;
        demo.phase = match stage {
            Stage::ToStop | Stage::Visit | Stage::Skip => Phase::ToStop,
            Stage::Arrived | Stage::Returning => Phase::Returning,
            _ => Phase::Riding,
        };
        let route = match demo.phase {
            Phase::Riding => demo.fixture.original,
            Phase::ToStop => demo.stop().outbound,
            Phase::Returning => demo.stop().continuation,
        };
        let position = match stage {
            Stage::Arrived | Stage::Returning => demo.stop().position(),
            Stage::Rejoined => demo.stop().approach[0],
            _ => demo.fixture.start,
        };
        self.state.assistant_demo = Some(demo);
        self.activate_route(route);
        self.assistant_demo_position(position);
        screen::apply(&mut self.ui.stack, screen::Transition::Root(screen::Screen::Map(screen::MapScreen::new())));
        if let Some(page) = screen::AssistantScreen::for_stage(stage, selected) {
            screen::apply(&mut self.ui.stack, screen::Transition::Push(screen::Screen::Assistant(page)));
        }
        self.ui.map_dirty = true;
        true
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
    static FIXTURE: Fixture = Fixture { original: 0, start: (0, 0), stops: &[STOP, STOP] };

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
    fn stage_jumps_preserve_the_recording_and_empty_results_cannot_start_a_visit() {
        static FOUR: Fixture = Fixture { original: 0, start: (0, 0), stops: &[STOP, STOP, STOP, STOP] };
        let mut app = app();
        app.enable_assistant_demo(&FOUR);
        let session = app.recorder.session();
        for (stage, _) in Stage::ALL {
            assert!(app.show_assistant_demo(stage, 3));
            let demo = app.state.assistant_demo.unwrap();
            let expected = match demo.phase {
                Phase::Riding => 0,
                Phase::ToStop => 1,
                Phase::Returning => 2,
            };
            assert_eq!(app.active_route_index(), Some(expected));
            assert_eq!(demo.selected, 3);
            assert_eq!(app.recorder.session(), session);
            assert!(app.recorder.recording());
        }
        assert!(!app.show_assistant_demo(Stage::Preview, 4));
        app.set_assistant_candidates(&[]);
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.active_route_index(), Some(0));
        assert_eq!(app.state.assistant_demo.unwrap().candidates.len, 0);
        assert!(!app.show_assistant_demo(Stage::ToStop, 0));
        app.set_assistant_candidates(&[3, 2, 1, 0]);
        assert!(app.show_assistant_demo(Stage::Choices, 3));
        app.apply_gesture(Gesture::Press);
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.state.assistant_demo.unwrap().selected, 3);
        assert_eq!(app.active_route_index(), Some(1));
        assert_eq!(app.recorder.session(), session);
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
