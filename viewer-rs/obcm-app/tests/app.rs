//! Behavior tests for the shared app layer: the HAL boundary and the
//! follow/free camera logic. These run on the host (std test harness) against the
//! same `no_std` code the firmware links.

use obcm_app::{AppState, CameraMode, Fix, LocationSource};

/// A `LocationSource` that always replays the same scripted fix — stands in for
/// the simulator's control-panel override.
struct Fixed(Option<Fix>);

impl LocationSource for Fixed {
    fn poll(&mut self) -> Option<Fix> {
        self.0
    }
}

const BERLIN: (i32, i32) = (52_520_000, 13_405_000); // (lat, lon) microdegrees

fn berlin_fix() -> Fix {
    Fix { lat: BERLIN.0, lon: BERLIN.1, course: Some(90.0), speed_mps: Some(3.0) }
}

#[test]
fn follow_mode_recenters_camera_on_each_fix() {
    let mut app = AppState::new(0.0, 0.0, 1.0); // defaults to Follow
    assert_eq!(app.mode, CameraMode::Follow);

    let mut loc = Fixed(Some(berlin_fix()));
    app.update(&mut loc);

    assert_eq!(app.cam_lat, BERLIN.0 as f64);
    assert_eq!(app.cam_lon, BERLIN.1 as f64);
}

#[test]
fn free_mode_records_fix_but_leaves_camera_put() {
    let mut app = AppState::new(1_000.0, 2_000.0, 1.0);
    app.mode = CameraMode::Free;

    let mut loc = Fixed(Some(berlin_fix()));
    app.update(&mut loc);

    // Camera stays where the host's pan/zoom left it...
    assert_eq!(app.cam_lon, 1_000.0);
    assert_eq!(app.cam_lat, 2_000.0);
    // ...but the fix is still recorded for the marker.
    assert_eq!(app.user_fix, Some(berlin_fix()));
}

#[test]
fn fix_starts_none_and_then_tracks_the_source() {
    let mut app = AppState::new(0.0, 0.0, 1.0);
    assert_eq!(app.user_fix, None);

    let mut loc = Fixed(Some(berlin_fix()));
    app.update(&mut loc);

    let fix = app.user_fix.expect("fix recorded after update");
    assert_eq!(fix.course, Some(90.0));
    assert_eq!(fix.speed_mps, Some(3.0));
}

#[test]
fn no_fix_holds_the_last_camera_position() {
    let mut app = AppState::new(5.0, 6.0, 1.0);
    let mut loc = Fixed(None); // no satellite lock

    app.update(&mut loc);

    assert_eq!((app.cam_lon, app.cam_lat), (5.0, 6.0));
    assert_eq!(app.user_fix, None);
}

#[test]
fn viewport_carries_the_camera_and_display_size() {
    let app = AppState::new(13_405_000.0, 52_520_000.0, 0.5);
    let vp = app.viewport(240.0, 320.0);

    assert_eq!((vp.w, vp.h), (240.0, 320.0));
    assert_eq!(vp.cam_lon, 13_405_000.0);
    assert_eq!(vp.cam_lat, 52_520_000.0);
    assert_eq!(vp.zoom, 0.5);
}

#[test]
fn fix_at_helper_is_stationary() {
    let f = Fix::at(BERLIN.0, BERLIN.1);
    assert_eq!(f.course, None);
    assert_eq!(f.speed_mps, None);
}
