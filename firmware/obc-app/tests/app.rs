//! Behavior tests for the shared app layer: the HAL boundary and the
//! follow/free camera logic. These run on the host (std test harness) against the
//! same `no_std` code the firmware links.

use obc_app::{AppState, CameraMode, Fix};

mod common;
use common::ReplayFix;

const BERLIN: (i32, i32) = (52_520_000, 13_405_000); // (lat, lon) microdegrees

fn berlin_fix() -> Fix {
    Fix { lat: BERLIN.0, lon: BERLIN.1, course: Some(90.0), speed_mps: Some(3.0) }
}

#[test]
fn follow_mode_recenters_camera_on_each_fix() {
    let mut app = AppState::new(0, 0, 1.0); // defaults to Follow
    assert_eq!(app.mode, CameraMode::Follow);

    let mut loc = ReplayFix(Some(berlin_fix()));
    app.update(&mut loc);

    assert_eq!(app.cam_lat, BERLIN.0);
    assert_eq!(app.cam_lon, BERLIN.1);
}

#[test]
fn free_mode_records_fix_but_leaves_camera_put() {
    let mut app = AppState::new(1_000, 2_000, 1.0);
    app.mode = CameraMode::Free;

    let mut loc = ReplayFix(Some(berlin_fix()));
    app.update(&mut loc);

    // Camera stays where the host's pan/zoom left it...
    assert_eq!(app.cam_lon, 1_000);
    assert_eq!(app.cam_lat, 2_000);
    // ...but the fix is still recorded for the marker.
    assert_eq!(app.user_fix, Some(berlin_fix()));
}

#[test]
fn fix_starts_none_and_then_tracks_the_source() {
    let mut app = AppState::new(0, 0, 1.0);
    assert_eq!(app.user_fix, None);

    let mut loc = ReplayFix(Some(berlin_fix()));
    app.update(&mut loc);

    let fix = app.user_fix.expect("fix recorded after update");
    assert_eq!(fix.course, Some(90.0));
    assert_eq!(fix.speed_mps, Some(3.0));
}

#[test]
fn no_fix_holds_the_last_camera_position() {
    let mut app = AppState::new(5, 6, 1.0);
    let mut loc = ReplayFix(None); // no satellite lock

    app.update(&mut loc);

    assert_eq!((app.cam_lon, app.cam_lat), (5, 6));
    assert_eq!(app.user_fix, None);
}

#[test]
fn viewport_carries_the_camera_and_display_size() {
    let app = AppState::new(13_405_000, 52_520_000, 0.5);
    let vp = app.viewport(240.0, 320.0);

    assert_eq!((vp.w, vp.h), (240.0, 320.0));
    assert_eq!(vp.cam_lon, 13_405_000);
    assert_eq!(vp.cam_lat, 52_520_000);
    assert_eq!(vp.zoom, 0.5);
}

#[test]
fn fix_at_helper_is_stationary() {
    let f = Fix::at(BERLIN.0, BERLIN.1);
    assert_eq!(f.course, None);
    assert_eq!(f.speed_mps, None);
}

#[test]
fn north_up_is_the_default_orientation() {
    let app = AppState::new(0, 0, 1.0);
    assert!(!app.heading_up);

    let vp = app.viewport(200.0, 200.0);
    assert_eq!(vp.course_rad, 0.0);
    // A point due north of the camera projects straight up: centered in x, above
    // center in y (screen y grows downward).
    let (x, y) = vp.to_screen(0, 1_000);
    assert_eq!(x, 100);
    assert!(y < 100);
}

#[test]
fn heading_up_rotates_course_to_screen_top() {
    let mut app = AppState::new(0, 0, 1.0);
    app.heading_up = true;
    // Heading-up with no fix yet is still north-up — there's no course to face.
    assert_eq!(app.viewport(200.0, 200.0).course_rad, 0.0);

    // Now record a fix heading due east (course 90°).
    let mut loc = ReplayFix(Some(Fix { lat: 0, lon: 0, course: Some(90.0), speed_mps: Some(5.0) }));
    app.update(&mut loc);

    let vp = app.viewport(200.0, 200.0);
    assert!((vp.course_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-6);

    // Facing east: east is now up, and north swings to the left.
    let (ex, ey) = vp.to_screen(1_000, 0); // 1000 µdeg east
    assert!((ex - 100).abs() <= 1); // centered horizontally
    assert!(ey < 100); // up

    let (nx, ny) = vp.to_screen(0, 1_000); // 1000 µdeg north
    assert!(nx < 100); // to the left
    assert!((ny - 100).abs() <= 1); // centered vertically
}

#[test]
fn projection_round_trips_under_rotation() {
    let mut app = AppState::new(13_405_000, 52_520_000, 0.5);
    app.heading_up = true;
    let mut loc = ReplayFix(Some(Fix { lat: BERLIN.0, lon: BERLIN.1, course: Some(37.0), speed_mps: Some(4.0) }));
    app.update(&mut loc);
    let vp = app.viewport(240.0, 320.0);

    // Project map → screen → map; the result is within a few microdegrees (screen
    // is integer pixels at 0.5 px/µdeg, and aspect divides longitude back out).
    for &(lon, lat) in &[(13_405_000, 52_520_000), (13_410_000, 52_525_000), (13_400_000, 52_515_000)] {
        let (sx, sy) = vp.to_screen(lon, lat);
        let (rlon, rlat) = vp.to_map(sx as f32, sy as f32);
        assert!((rlon - lon).abs() < 6, "lon {lon} -> {rlon}");
        assert!((rlat - lat).abs() < 6, "lat {lat} -> {rlat}");
    }
}

#[test]
fn rotation_widens_the_cull_box() {
    let north = AppState::new(0, 0, 1.0).viewport(200.0, 100.0);

    let mut app = AppState::new(0, 0, 1.0);
    app.heading_up = true;
    let mut loc = ReplayFix(Some(Fix { lat: 0, lon: 0, course: Some(45.0), speed_mps: Some(1.0) }));
    app.update(&mut loc);
    let rot = app.viewport(200.0, 100.0);

    // A 45°-tilted 200×100 view covers more latitude than the axis-aligned one, so
    // the quadtree cull box must grow or corner features would be dropped.
    let nb = north.visible_bbox();
    let rb = rot.visible_bbox();
    assert!((rb.max_lat - rb.min_lat) > (nb.max_lat - nb.min_lat), "rotated lat span should exceed north-up");
}
